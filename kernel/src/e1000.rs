// Intel e1000 / e1000e NIC driver for bare-metal Rusty Penguin kernel.
//
// Covers Intel 82540EM (QEMU's -device e1000), and the modern i217/i218/i219
// laptop NIC family. These all share the same descriptor and register model;
// the i219 has slightly different PHY/MDIO paths but the data-plane is identical.
//
// PCI IDs handled:
//   0x8086:0x100E  82540EM  (QEMU default e1000)
//   0x8086:0x100F  82545EM
//   0x8086:0x153A  i217-LM
//   0x8086:0x153B  i217-V
//   0x8086:0x1539  i211-AT
//   0x8086:0x156F  i219-LM
//   0x8086:0x1570  i219-V
//   0x8086:0x15B7  i219-LM
//   0x8086:0x15B8  i219-V
//   0x8086:0x15BE  i219-LM
//   0x8086:0x15D6  i219-V
//   0x8086:0x15D7  i219-LM
//   0x8086:0x15D8  i219-V
//   0x8086:0x15E3  i219-LM
//   0x8086:0x15EB  i219-LM
//
// DMA buffers at fixed addresses in the 64 MiB identity map (virt == phys):
//   0x0340_0000  RX descriptor ring  (32 descriptors × 16 B = 512 B)
//   0x0340_0200  TX descriptor ring  (32 descriptors × 16 B = 512 B)
//   0x0340_0400  RX packet buffers   (32 × 2 KiB = 64 KiB)
//   0x0350_0400  TX packet buffers   (32 × 2 KiB = 64 KiB)
// (Same 34 MiB arena as RTL8139 for consistency; we never use both at once.)

// ── MMIO helpers ─────────────────────────────────────────────────────────────
unsafe fn r32(base: u64, off: usize) -> u32 { ((base + off as u64) as *const u32).read_volatile() }
unsafe fn w32(base: u64, off: usize, v: u32) { ((base + off as u64) as *mut u32).write_volatile(v) }
fn spin(n: u64) { for _ in 0..n { unsafe { core::arch::asm!("pause", options(nomem,nostack)); } } }

// ── Register offsets ─────────────────────────────────────────────────────────
const CTRL:   usize = 0x0000;
const STATUS: usize = 0x0008;
const RCTL:   usize = 0x0100;
const TCTL:   usize = 0x0400;
const TIPG:   usize = 0x0410; // TX inter-packet gap
const RDBAL:  usize = 0x2800;
const RDBAH:  usize = 0x2804;
const RDLEN:  usize = 0x2808;
const RDH:    usize = 0x2810;
const RDT:    usize = 0x2818;
const TDBAL:  usize = 0x3800;
const TDBAH:  usize = 0x3804;
const TDLEN:  usize = 0x3808;
const TDH:    usize = 0x3810;
const TDT:    usize = 0x3818;
const RAL0:   usize = 0x5400; // Receive Address Low  (MAC[31:0])
const RAH0:   usize = 0x5404; // Receive Address High (MAC[47:32] + AV bit)
const MTA:    usize = 0x5200; // Multicast Table Array (128 × u32)
const IMC:    usize = 0x00D8; // Interrupt Mask Clear (disable all IRQs)

// CTRL bits
const CTRL_SLU:  u32 = 1 << 6;  // Set Link Up
const CTRL_RST:  u32 = 1 << 26; // Reset

// RCTL bits
const RCTL_EN:   u32 = 1 << 1;
const RCTL_UPE:  u32 = 1 << 3;  // Unicast Promiscuous Enable
const RCTL_MPE:  u32 = 1 << 4;  // Multicast Promiscuous Enable
const RCTL_BAM:  u32 = 1 << 15; // Broadcast Accept Mode
const RCTL_SZ:   u32 = 0 << 16; // Buffer Size = 2 KiB (BSIZE bits [17:16] = 00)
const RCTL_SECRC:u32 = 1 << 26; // Strip Ethernet CRC

// TCTL bits
const TCTL_EN:   u32 = 1 << 1;
const TCTL_PSP:  u32 = 1 << 3;  // Pad Short Packets
const TCTL_CT:   u32 = 0x0F << 4; // Collision Threshold (15)
const TCTL_COLD: u32 = 0x3F << 12; // Collision Distance (63 for full-duplex)

// ── DMA layout ───────────────────────────────────────────────────────────────
const RX_RING_PHYS: u64 = 0x0340_0000;
const TX_RING_PHYS: u64 = 0x0340_0200;
const RX_BUF_PHYS:  u64 = 0x0340_0400;
const TX_BUF_PHYS:  u64 = 0x0350_0400;
const N_DESC:       usize = 32;
const BUF_SZ:       usize = 2048;

// RX descriptor (legacy, 16 bytes)
#[repr(C, packed)]
struct RxDesc {
    addr:   u64, // physical address of packet buffer
    length: u16,
    csum:   u16,
    status: u8,  // bit0=DD (descriptor done), bit1=EOP
    errors: u8,
    vlan:   u16,
}

// TX descriptor (legacy, 16 bytes)
#[repr(C, packed)]
struct TxDesc {
    addr:    u64, // physical address of packet data
    length:  u16,
    cso:     u8,
    cmd:     u8, // EOP|IFCS|RS
    status:  u8, // bit0=DD
    css:     u8,
    vlan:    u16,
}

// ── Driver state ─────────────────────────────────────────────────────────────
static mut BASE:   u64    = 0;
static mut MAC:    [u8;6] = [0;6];
static mut RX_IDX: usize  = 0; // next descriptor to check for DD
static mut TX_IDX: usize  = 0; // next descriptor to write for TX
static mut READY:  bool   = false;

pub struct Nic { pub mac: [u8; 6] }

pub fn nic() -> Option<Nic> {
    unsafe { if READY { Some(Nic { mac: MAC }) } else { None } }
}


// PCI device IDs for Intel e1000 family.
const DEVICES: &[(u16, u16)] = &[
    (0x8086, 0x100E), // 82540EM (QEMU)
    (0x8086, 0x100F), // 82545EM
    (0x8086, 0x153A), // i217-LM
    (0x8086, 0x153B), // i217-V
    (0x8086, 0x1539), // i211-AT
    (0x8086, 0x156F), // i219-LM
    (0x8086, 0x1570), // i219-V
    (0x8086, 0x15B7), // i219-LM (common laptop)
    (0x8086, 0x15B8), // i219-V  (common laptop)
    (0x8086, 0x15BE), // i219-LM
    (0x8086, 0x15D6), // i219-V
    (0x8086, 0x15D7), // i219-LM
    (0x8086, 0x15D8), // i219-V
    (0x8086, 0x15E3), // i219-LM
    (0x8086, 0x15EB), // i219-LM
    (0x8086, 0x0D4F), // i219-LM (10th gen)
    (0x8086, 0x0D4C), // i219-V  (10th gen)
    (0x8086, 0x0DC5), // i219-V  (12th gen)
    (0x8086, 0x0DC6), // i219-LM (12th gen)
];

/// Initialise the Intel e1000/e1000e NIC. Returns true on success.
pub fn init() -> bool {
    // Scan PCI for any known Intel e1000 variant.
    let mut found = None;
    'outer: for &(vid, did) in DEVICES {
        if let Some(loc) = crate::pci::find(vid, did) { found = Some(loc); break 'outer; }
    }
    let (bus, dev, func) = match found {
        Some(l) => l,
        None => {
            crate::serial::write_str("  [e1000] no Intel NIC found\n");
            return false;
        }
    };
    crate::serial::write_str("  [e1000] Intel NIC found\n");
    crate::pci::enable_bus_master(bus, dev, func);

    // Map MMIO BAR0.
    let bar0_raw = crate::pci::bar(bus, dev, func, 0);
    let base = if (bar0_raw >> 1) & 3 == 2 {
        let lo = (bar0_raw & !0xF) as u64;
        let hi = crate::pci::bar(bus, dev, func, 1) as u64;
        lo | (hi << 32)
    } else {
        (bar0_raw & !0xF) as u64
    };
    if base == 0 { crate::serial::write_str("  [e1000] BAR0 not set\n"); return false; }
    crate::vmm::map_mmio_range(base, 0x20000); // map 128 KiB of MMIO

    unsafe {
        BASE = base;

        // Disable all interrupts, then reset.
        w32(base, IMC, 0xFFFF_FFFF);
        w32(base, CTRL, r32(base, CTRL) | CTRL_RST);
        spin(50_000);
        // Wait for reset to clear.
        let mut t = 0u32;
        while r32(base, CTRL) & CTRL_RST != 0 && t < 100_000 { spin(10); t += 1; }
        w32(base, IMC, 0xFFFF_FFFF); // disable again post-reset

        // Set link up.
        w32(base, CTRL, r32(base, CTRL) | CTRL_SLU);
        spin(20_000);

        // Read MAC from Receive Address register 0.
        let ral = r32(base, RAL0);
        let rah = r32(base, RAH0);
        MAC[0] = (ral)       as u8;
        MAC[1] = (ral >> 8)  as u8;
        MAC[2] = (ral >> 16) as u8;
        MAC[3] = (ral >> 24) as u8;
        MAC[4] = (rah)       as u8;
        MAC[5] = (rah >> 8)  as u8;
        // Ensure AV (Address Valid) bit is set in RAH so the MAC filter is live.
        if rah & (1 << 31) == 0 {
            w32(base, RAH0, rah | (1 << 31));
        }
        crate::serial::write_str("  [e1000] MAC ");
        for (i, &b) in MAC.iter().enumerate() {
            let hi = b >> 4; let lo = b & 0xF;
            crate::serial::write_byte(if hi < 10 { b'0' + hi } else { b'a' + hi - 10 });
            crate::serial::write_byte(if lo < 10 { b'0' + lo } else { b'a' + lo - 10 });
            if i < 5 { crate::serial::write_byte(b':'); }
        }
        crate::serial::write_byte(b'\n');

        // Clear MTA (multicast table).
        for i in 0..128 { w32(base, MTA + i * 4, 0); }

        // ── RX descriptor ring ────────────────────────────────────────────────
        // Clear DMA area.
        core::ptr::write_bytes(RX_RING_PHYS as *mut u8, 0,
            N_DESC * core::mem::size_of::<RxDesc>() + N_DESC * core::mem::size_of::<TxDesc>());
        let rxd = RX_RING_PHYS as *mut RxDesc;
        for i in 0..N_DESC {
            let d = &mut *rxd.add(i);
            d.addr = RX_BUF_PHYS + (i * BUF_SZ) as u64;
        }
        w32(base, RDBAL, RX_RING_PHYS as u32);
        w32(base, RDBAH, (RX_RING_PHYS >> 32) as u32);
        w32(base, RDLEN, (N_DESC * 16) as u32);
        w32(base, RDH, 0);
        // Give N_DESC-1 descriptors to hardware before enabling RX.
        // Per e1000 spec: hardware owns [RDH, RDT). With RDH=0, RDT=N_DESC-1,
        // hardware owns 0..N_DESC-2 and can receive into them immediately.
        w32(base, RDT, (N_DESC - 1) as u32);

        // ── TX descriptor ring ────────────────────────────────────────────────
        let txd = TX_RING_PHYS as *mut TxDesc;
        for i in 0..N_DESC {
            let d = &mut *txd.add(i);
            d.addr = TX_BUF_PHYS + (i * BUF_SZ) as u64;
            d.status = 1; // Mark all as done (DD=1) so we can use them
        }
        w32(base, TDBAL, TX_RING_PHYS as u32);
        w32(base, TDBAH, (TX_RING_PHYS >> 32) as u32);
        w32(base, TDLEN, (N_DESC * 16) as u32);
        w32(base, TDH, 0);
        w32(base, TDT, 0);

        // ── RX control ────────────────────────────────────────────────────────
        // EN=1, UPE=1 (unicast promisc), MPE=1, BAM=1 (broadcast), SECRC=1.
        // UPE+MPE ensures all frames destined for us arrive regardless of RAH/RAL.
        w32(base, RCTL, RCTL_EN | RCTL_UPE | RCTL_MPE | RCTL_BAM | RCTL_SZ | RCTL_SECRC);

        // ── TX control ────────────────────────────────────────────────────────
        // Standard values from Linux e1000 driver.
        w32(base, TIPG, 0x00702008); // default IPG for 82540
        w32(base, TCTL, TCTL_EN | TCTL_PSP | TCTL_CT | TCTL_COLD);

        RX_IDX = 0;
        TX_IDX = 0;
        READY = true;

        // Wait for link (STATUS.LU = bit 1) + LAN_INIT_DONE (bit 2).
        let mut t = 0u32;
        while r32(base, STATUS) & 6 != 6 && t < 2_000_000 { spin(10); t += 1; }
        let status_v = r32(base, STATUS);
        let link = if status_v & 2 != 0 { "up" } else { "down" };
        crate::serial::write_str("  [e1000] ready (link ");
        crate::serial::write_str(link);
        crate::serial::write_str(")\n");
        true
    }
}

impl Nic {
    /// Send a frame. Copies `data` into the next TX buffer and bumps TDT.
    pub fn send(&self, data: &[u8]) {
        unsafe {
            let idx = TX_IDX;
            let next = (idx + 1) % N_DESC;
            let txd = (TX_RING_PHYS as *mut TxDesc).add(idx);
            let d = &mut *txd;

            // Wait for DD bit (descriptor done = hardware released it).
            let mut t = 0u32;
            while d.status & 1 == 0 && t < 500_000 { spin(10); t += 1; }

            let len = data.len().min(BUF_SZ);
            let buf_phys = TX_BUF_PHYS + (idx * BUF_SZ) as u64;
            core::ptr::copy_nonoverlapping(data.as_ptr(), buf_phys as *mut u8, len);
            d.addr   = buf_phys;
            d.length = len as u16;
            d.cmd    = 0x0B; // EOP | IFCS | RS (report status)
            d.status = 0;    // clear DD
            d.cso    = 0;
            d.css    = 0;

            TX_IDX = next;
            w32(BASE, TDT, next as u32); // advance tail → hardware sends
            // Wait for TX completion (DD) to confirm the packet was sent.
            let mut tw = 0u32;
            while d.status & 1 == 0 && tw < 100_000 { spin(10); tw += 1; }
        }
    }

    /// Non-blocking RX poll. Returns Some(len) if a frame arrived, copying
    /// it into `buf`. On descriptor exhaustion, re-arms the ring.
    pub fn poll_rx(&self, buf: &mut [u8]) -> Option<usize> {
        unsafe {
            let rxd = (RX_RING_PHYS as *const RxDesc).add(RX_IDX);
            let d = &*rxd;
            if d.status & 1 == 0 { return None; } // DD not set

            let len = d.length as usize;
            let take = len.min(buf.len()).min(BUF_SZ);
            let src = (RX_BUF_PHYS + (RX_IDX * BUF_SZ) as u64) as *const u8;
            core::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), take);

            // Re-arm the descriptor: clear status, update tail.
            let rxd_mut = (RX_RING_PHYS as *mut RxDesc).add(RX_IDX);
            (*rxd_mut).status = 0;
            w32(BASE, RDT, RX_IDX as u32); // give descriptor back to hardware
            RX_IDX = (RX_IDX + 1) % N_DESC;
            Some(take)
        }
    }
}

/// Read STATUS register (for link check) -- exposed for diagnostics.
pub fn read_status() -> u32 { unsafe { if BASE == 0 { 0 } else { r32(BASE, STATUS) } } }
/// Read RDBAL (for ring address verify).
pub fn read_rdbal() -> u32 { unsafe { if BASE == 0 { 0 } else { r32(BASE, RDBAL) } } }
