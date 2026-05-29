// Realtek RTL8139 Fast Ethernet driver for the bare-metal Rusty Penguin kernel.
//
// The RTL8139 (PCI 0x10EC:0x8139) is the classic teaching NIC: a flat
// port-I/O register interface, a single linear RX ring, and four TX
// descriptors — no virtqueues. QEMU emulates it with `-device rtl8139`.
//
// DMA buffers live at fixed addresses in the 64 MiB identity map (virt == phys),
// same arena strategy as the HDA driver. We POLL the RX ring (no IRQ wiring yet);
// that is enough to prove a full TX→RX round-trip (see net::arp_probe).

use crate::port::{inb, outb, outl, outw};

// ── Register offsets (from the I/O BAR base) ────────────────────────────────
const IDR0:    u16 = 0x00; // MAC bytes 0..5
const TSD0:    u16 = 0x10; // transmit status/command (4 × u32)
const TSAD0:   u16 = 0x20; // transmit start address  (4 × u32, physical)
const RBSTART: u16 = 0x30; // RX buffer physical address
const CR:      u16 = 0x37; // command register
const CAPR:    u16 = 0x38; // current addr of packet read (RX read ptr − 16)
const IMR:     u16 = 0x3C; // interrupt mask
const ISR:     u16 = 0x3E; // interrupt status
const RCR:     u16 = 0x44; // receive config
const CONFIG1: u16 = 0x52;

// CR bits
const CR_RST:  u8 = 0x10;  // soft reset
const CR_RE:   u8 = 0x04;  // receiver enable
const CR_TE:   u8 = 0x08;  // transmitter enable
const CR_BUFE: u8 = 0x01;  // RX buffer empty (1 = nothing to read)

// RCR: accept broadcast | physical-match | all (promisc) | WRAP, 8K+16 buffer.
const RCR_CFG: u32 = (1 << 7) | 0x0F; // WRAP + AB|AM|APM|AAP

// ── DMA buffers in the identity-mapped arena (52 MiB) ───────────────────────
// 64 MiB identity map: heap ≤24 MiB, HDA at 50 MiB, ring-3 stack at 63 MiB.
// 52 MiB is free. RX ring is 8K+16 with WRAP, so reserve 8K+16+1500 → 12 KiB.
const RX_BUF_PHYS: u32 = 0x0340_0000; // 52 MiB
const RX_BUF_LEN:  usize = 8192 + 16 + 1536;
const TX_BUF_PHYS: u32 = 0x0340_4000; // 52 MiB + 16 KiB, four 2 KiB slots
const TX_SLOT:     u32 = 0x800;       // 2 KiB per TX descriptor

pub struct Rtl8139 {
    io: u16,
    pub mac: [u8; 6],
    rx_off: usize,  // software RX read offset into the ring
    tx_cur: u8,     // next TX descriptor (0..3)
}

static mut NIC: Option<Rtl8139> = None;

pub fn nic() -> Option<&'static mut Rtl8139> {
    unsafe { (&mut *core::ptr::addr_of_mut!(NIC)).as_mut() }
}

/// Probe PCI, reset and bring up the RTL8139. Returns true on success.
pub fn init() -> bool {
    let (bus, dev, func) = match crate::pci::find(0x10EC, 0x8139) {
        Some(b) => b,
        None => { crate::serial::write_str("  [rtl8139] no NIC found\n"); return false; }
    };
    crate::pci::enable_bus_master(bus, dev, func);

    // BAR0 is the I/O space base for the RTL8139.
    let io = (crate::pci::bar(bus, dev, func, 0) & !0x3) as u16;
    if io == 0 { crate::serial::write_str("  [rtl8139] BAR0 (I/O) not set\n"); return false; }

    unsafe {
        // Power on, then soft reset and wait for it to clear.
        outb(io + CONFIG1, 0x00);
        outb(io + CR, CR_RST);
        let mut spins = 0u32;
        while inb(io + CR) & CR_RST != 0 {
            spins += 1;
            if spins > 1_000_000 { crate::serial::write_str("  [rtl8139] reset timeout\n"); return false; }
        }

        // RX ring, receive config, then enable RX + TX.
        outl(io + RBSTART, RX_BUF_PHYS);
        outw(io + IMR, 0x0000);       // we poll; mask all IRQs
        outl(io + RCR, RCR_CFG);
        outb(io + CR, CR_RE | CR_TE);

        let mut mac = [0u8; 6];
        for i in 0..6 { mac[i] = inb(io + IDR0 + i as u16); }

        let nic = Rtl8139 { io, mac, rx_off: 0, tx_cur: 0 };
        NIC = Some(nic);
    }
    true
}

impl Rtl8139 {
    /// Transmit one raw Ethernet frame (≤ ~1792 bytes).
    pub fn send(&mut self, frame: &[u8]) {
        let len = frame.len().min(TX_SLOT as usize - 4);
        let slot = self.tx_cur as u32;
        let buf_phys = TX_BUF_PHYS + slot * TX_SLOT;
        unsafe {
            // Identity-mapped: physical addr is usable as a pointer directly.
            let dst = buf_phys as *mut u8;
            for i in 0..len { dst.add(i).write_volatile(frame[i]); }
            // Pad runt frames to the 60-byte Ethernet minimum.
            let mut tx_len = len;
            while tx_len < 60 { dst.add(tx_len).write_volatile(0); tx_len += 1; }
            outl(self.io + TSAD0 + (slot * 4) as u16, buf_phys);
            outl(self.io + TSD0  + (slot * 4) as u16, tx_len as u32); // OWN clears → start DMA
        }
        self.tx_cur = (self.tx_cur + 1) & 3;
    }

    /// Poll one received frame into `out`; returns its length, or None if empty.
    pub fn poll_rx(&mut self, out: &mut [u8]) -> Option<usize> {
        unsafe {
            if inb(self.io + CR) & CR_BUFE != 0 { return None; } // ring empty
            let base = RX_BUF_PHYS as *const u8;
            let hdr = base.add(self.rx_off) as *const u16;
            let status = core::ptr::read_volatile(hdr);
            let frame_len = core::ptr::read_volatile(hdr.add(1)) as usize; // incl. 4-byte CRC
            if frame_len < 4 || frame_len > RX_BUF_LEN { // desync → resync ring
                self.rx_off = 0;
                outw(self.io + CAPR, (0u16).wrapping_sub(16));
                return None;
            }
            let payload_len = frame_len - 4;
            let n = payload_len.min(out.len());
            if status & 0x01 != 0 { // ROK
                let src = base.add(self.rx_off + 4);
                for i in 0..n { out[i] = core::ptr::read_volatile(src.add(i)); }
            }
            // Advance past [status u16][len u16][payload+crc], 4-byte aligned.
            self.rx_off = (self.rx_off + frame_len + 4 + 3) & !3;
            if self.rx_off >= 8192 { self.rx_off -= 8192; }
            outw(self.io + CAPR, (self.rx_off as u16).wrapping_sub(16));
            // Acknowledge RX.
            outw(self.io + ISR, 0x0001);
            if status & 0x01 != 0 { Some(n) } else { None }
        }
    }
}
