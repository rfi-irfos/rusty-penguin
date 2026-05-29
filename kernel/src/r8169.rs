// Realtek RTL8169/RTL8111/RTL8168 Gigabit Ethernet driver.
//
// Covers the most common laptop NIC family after Intel i219: RTL8111 (almost
// every ASUS, MSI, Gigabyte motherboard) and RTL8168 (many laptops). Also
// handles the RTL8169 (older desktop card) and RTL8125 (2.5 GbE, newer).
//
// PCI IDs:
//   0x10EC:0x8169  RTL8169 (desktop GbE)
//   0x10EC:0x8168  RTL8168B/C/D/E (mobile/laptop GbE) — very common
//   0x10EC:0x8111  RTL8111 (embedded, same as 8168)
//   0x10EC:0x8136  RTL8101E/8102E/8103E (100 Mbps, older)
//   0x10EC:0x8125  RTL8125 (2.5 GbE, Ryzen laptops)
//   0x10EC:0x3000  RTL8125B (Killer E3100X)
//
// DMA layout (34 MiB arena, same as RTL8139/e1000 — only one NIC active):
//   0x0340_0000  RX descriptor ring  (4 × 16 B = 64 B, we use 4 descriptors)
//   0x0340_1000  TX descriptor ring  (4 × 16 B)
//   0x0340_2000  RX packet buffers   (4 × 2 KiB)
//   0x0340_4000  TX packet buffers   (4 × 2 KiB)

// ── MMIO helpers ─────────────────────────────────────────────────────────────
unsafe fn r8 (b: u64, o: usize) -> u8  { ((b+o as u64) as *const u8 ).read_volatile() }
unsafe fn r16(b: u64, o: usize) -> u16 { ((b+o as u64) as *const u16).read_volatile() }
unsafe fn r32(b: u64, o: usize) -> u32 { ((b+o as u64) as *const u32).read_volatile() }
unsafe fn w8 (b: u64, o: usize, v: u8 ) { ((b+o as u64) as *mut u8 ).write_volatile(v) }
unsafe fn w16(b: u64, o: usize, v: u16) { ((b+o as u64) as *mut u16).write_volatile(v) }
unsafe fn w32(b: u64, o: usize, v: u32) { ((b+o as u64) as *mut u32).write_volatile(v) }
unsafe fn w64(b: u64, o: usize, v: u64) { ((b+o as u64) as *mut u64).write_volatile(v) }
fn spin(n: u64) { for _ in 0..n { unsafe { core::arch::asm!("pause",options(nomem,nostack)); } } }

// ── Register offsets ─────────────────────────────────────────────────────────
const MAC0:    usize = 0x00; // MAC bytes 0-5
const CR:      usize = 0x37; // Command Register
const TPPOLL:  usize = 0x38; // Transmit Priority Polling
const IMR:     usize = 0x3C; // Interrupt Mask Register
const ISR:     usize = 0x3E; // Interrupt Status Register
const TCR:     usize = 0x40; // Transmit Configuration Register
const RCR:     usize = 0x44; // Receive Configuration Register
const RCR_9346:usize = 0x50; // 9346CR (EEPROM/config)
const CONFIG1: usize = 0x52;
const PHY_ACCESS:usize=0x60; // PHY Access Register (some variants)
const RDSAR:   usize = 0xE4; // RX Descriptor Start Address
const TNPDS:   usize = 0x20; // TX Normal Priority Descriptor Start
const RMS:     usize = 0xDA; // RX packet max size

// CR bits
const CR_RST:  u8 = 1 << 4;
const CR_RE:   u8 = 1 << 3; // Receive Enable
const CR_TE:   u8 = 1 << 2; // Transmit Enable

// RCR bits — accept broadcast + multicast + directed (our MAC)
const RCR_AAP: u32 = 1 << 0;  // Accept All Packets (promiscuous)
const RCR_APM: u32 = 1 << 1;  // Accept Physical Match
const RCR_AM:  u32 = 1 << 2;  // Accept Multicast
const RCR_AB:  u32 = 1 << 3;  // Accept Broadcast
const RCR_RXFTH: u32 = 7 << 13; // Rx FIFO Threshold = no threshold
const RCR_MXDMA: u32 = 7 << 8;  // DMA burst = unlimited

// ── DMA descriptor ───────────────────────────────────────────────────────────
// RTL8169/8168 descriptor format (16 bytes):
//   [31:16] = Buffer Length (RX) / Frame Length (TX)
//   [15:0]  = DW0 flags: OWN=bit31, EOR=bit30, FS=bit29, LS=bit28 (TX/RX)
//   [63:32] = VLAN tag (unused for us)
//   [127:64]= Buffer Address (64-bit physical)
#[repr(C, align(16))]
struct Desc {
    flags:  u32,
    vlan:   u32,
    buf_lo: u32,
    buf_hi: u32,
}
const OWN:  u32 = 1 << 31; // NIC owns this descriptor
const EOR:  u32 = 1 << 30; // End Of Ring
const FS:   u32 = 1 << 29; // First Segment
const LS:   u32 = 1 << 28; // Last Segment
const RX_BUF_SZ: u32 = 1536; // receiver buffer size per descriptor

const RX_RING_PHYS: u64 = 0x0340_0000;
const TX_RING_PHYS: u64 = 0x0340_1000;
const RX_BUF_PHYS:  u64 = 0x0340_2000;
const TX_BUF_PHYS:  u64 = 0x0340_4000;
const N_RX: usize = 4;
const N_TX: usize = 4;

static mut BASE:   u64    = 0;
static mut MAC:    [u8;6] = [0;6];
static mut RX_IDX: usize  = 0;
static mut TX_IDX: usize  = 0;
static mut READY:  bool   = false;

pub struct Nic { pub mac: [u8;6] }
pub fn nic() -> Option<Nic> {
    unsafe { if READY { Some(Nic { mac: MAC }) } else { None } }
}

const DEVICES: &[(u16,u16)] = &[
    (0x10EC, 0x8169), // RTL8169
    (0x10EC, 0x8168), // RTL8168 (very common laptop NIC)
    (0x10EC, 0x8111), // RTL8111
    (0x10EC, 0x8136), // RTL8101E
    (0x10EC, 0x8125), // RTL8125 (2.5GbE)
    (0x10EC, 0x3000), // RTL8125B
];

pub fn init() -> bool {
    let mut found = None;
    'scan: for &(vid, did) in DEVICES {
        if let Some(loc) = crate::pci::find(vid, did) { found = Some(loc); break 'scan; }
    }
    let (bus, dev, func) = match found {
        Some(l) => l,
        None => { crate::serial::write_str("  [r8169] no Realtek NIC found\n"); return false; }
    };
    crate::serial::write_str("  [r8169] Realtek NIC found\n");
    crate::pci::enable_bus_master(bus, dev, func);

    // Map MMIO BAR2 (memory BAR). BAR0 is usually I/O, BAR2 is MMIO.
    // Some variants have BAR1 as MMIO — try both.
    let base = {
        let bar2 = crate::pci::bar(bus, dev, func, 2) & !0xF;
        let bar1 = crate::pci::bar(bus, dev, func, 1) & !0xF;
        if bar2 != 0 { bar2 as u64 }
        else if bar1 != 0 { bar1 as u64 }
        else {
            crate::serial::write_str("  [r8169] no MMIO BAR\n");
            return false;
        }
    };
    crate::vmm::map_mmio_range(base, 0x1000);

    unsafe {
        BASE = base;

        // Unlock config registers.
        w8(base, RCR_9346, 0xC0); // 9346 unlock
        spin(10_000);

        // Software reset.
        w8(base, CR, CR_RST);
        let mut t = 0u32;
        while r8(base, CR) & CR_RST != 0 && t < 100_000 { spin(10); t += 1; }
        spin(20_000);

        // Read MAC address from first 6 bytes.
        for i in 0..6 { MAC[i] = r8(base, MAC0 + i); }
        crate::serial::write_str("  [r8169] MAC ");
        for (i, &b) in MAC.iter().enumerate() {
            let h = b >> 4; let l = b & 0xF;
            crate::serial::write_byte(if h < 10 { b'0'+h } else { b'a'+h-10 });
            crate::serial::write_byte(if l < 10 { b'0'+l } else { b'a'+l-10 });
            if i < 5 { crate::serial::write_byte(b':'); }
        }
        crate::serial::write_byte(b'\n');

        // Clear DMA area.
        core::ptr::write_bytes(RX_RING_PHYS as *mut u8, 0,
            N_RX * 16 + N_TX * 16 + N_RX * 2048 + N_TX * 2048);

        // Build RX descriptor ring.
        let rxd = RX_RING_PHYS as *mut Desc;
        for i in 0..N_RX {
            let d = &mut *rxd.add(i);
            d.flags  = OWN | RX_BUF_SZ | if i == N_RX-1 { EOR } else { 0 };
            d.vlan   = 0;
            d.buf_lo = (RX_BUF_PHYS + (i * 2048) as u64) as u32;
            d.buf_hi = ((RX_BUF_PHYS + (i * 2048) as u64) >> 32) as u32;
        }

        // Build TX descriptor ring (all free initially — OWN=0).
        let txd = TX_RING_PHYS as *mut Desc;
        for i in 0..N_TX {
            let d = &mut *txd.add(i);
            d.flags  = if i == N_TX-1 { EOR } else { 0 };
            d.vlan   = 0;
            d.buf_lo = (TX_BUF_PHYS + (i * 2048) as u64) as u32;
            d.buf_hi = ((TX_BUF_PHYS + (i * 2048) as u64) >> 32) as u32;
        }

        // Program descriptor ring addresses.
        w64(base, RDSAR, RX_RING_PHYS);
        w64(base, TNPDS, TX_RING_PHYS);

        // RX max packet size.
        w16(base, RMS, 1536);

        // Disable interrupts.
        w16(base, IMR, 0);
        w16(base, ISR, 0xFFFF);

        // RX configuration: accept broadcast + physical match + all (promiscuous).
        w32(base, RCR, RCR_AAP | RCR_APM | RCR_AM | RCR_AB | RCR_RXFTH | RCR_MXDMA);

        // TX configuration: IFG = standard, DMA = unlimited.
        w32(base, TCR, (3 << 24) | (7 << 8)); // IFG standard, MXDMA unlimited

        // Lock config.
        w8(base, RCR_9346, 0x00);

        // Enable RX + TX.
        w8(base, CR, CR_RE | CR_TE);

        RX_IDX = 0;
        TX_IDX = 0;
        READY = true;
        crate::serial::write_str("  [r8169] ready\n");
        true
    }
}

impl Nic {
    pub fn send(&self, data: &[u8]) {
        unsafe {
            let i = TX_IDX;
            let txd = (TX_RING_PHYS as *mut Desc).add(i);
            let d = &mut *txd;

            // Wait for NIC to release descriptor (OWN should be 0).
            let mut t = 0u32;
            while d.flags & OWN != 0 && t < 500_000 { spin(10); t += 1; }

            let len = data.len().min(1536);
            let buf = TX_BUF_PHYS + (i * 2048) as u64;
            core::ptr::copy_nonoverlapping(data.as_ptr(), buf as *mut u8, len);
            d.buf_lo = buf as u32;
            d.buf_hi = (buf >> 32) as u32;
            let eor = if i == N_TX-1 { EOR } else { 0 };
            d.flags = OWN | FS | LS | eor | len as u32;

            TX_IDX = (i + 1) % N_TX;
            // Trigger TX by writing TPPOLL.NPQ (bit 6) or any write.
            w8(BASE, TPPOLL, 0x40);
        }
    }

    pub fn poll_rx(&self, buf: &mut [u8]) -> Option<usize> {
        unsafe {
            let i = RX_IDX;
            let rxd = (RX_RING_PHYS as *const Desc).add(i);
            let d = &*rxd;
            // If NIC still owns this descriptor, no packet yet.
            if d.flags & OWN != 0 { return None; }

            let len = (d.flags & 0x1FFF) as usize;
            let len = len.saturating_sub(4); // strip FCS
            let take = len.min(buf.len()).min(1536);
            let src = (RX_BUF_PHYS + (i * 2048) as u64) as *const u8;
            core::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), take);

            // Re-arm: give descriptor back to NIC.
            let eor = if i == N_RX-1 { EOR } else { 0 };
            let rxd_mut = (RX_RING_PHYS as *mut Desc).add(i);
            (*rxd_mut).flags = OWN | RX_BUF_SZ | eor;
            RX_IDX = (i + 1) % N_RX;
            Some(take)
        }
    }
}
