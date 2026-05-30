//! AHCI / SATA disk driver for the bare-metal Rusty Penguin kernel.
//!
//! This is the persistence brick: it lets the OS read and write real 512-byte
//! sectors on a SATA disk, so files and settings can survive a reboot — the
//! single biggest gap between a research prototype and a daily driver.
//!
//! Polled (no IRQ) single-port driver: find the AHCI HBA on PCI (class 0x01,
//! subclass 0x06), enable it, locate the first port with a device, and issue
//! READ DMA EXT / WRITE DMA EXT (48-bit LBA) commands through one command slot.
//! Verified in QEMU (q35 has an ICH9 AHCI controller; attach with
//!   -drive file=disk.img,format=raw,if=none,id=d0 -device ide-hd,drive=d0).

use core::ptr::{read_volatile, write_volatile};

// ── DMA structures live in identity-mapped RAM at 55 MiB (free: net rings end
//    ~53 MiB, ring-3 stack starts at 63 MiB). All physical == virtual here.
const CLB_PHYS:  u64 = 0x0370_0000;  // command list   (1 KiB, 1 KiB-aligned)
const FB_PHYS:   u64 = 0x0370_1000;  // received FIS    (256 B, 256-aligned)
const CT_PHYS:   u64 = 0x0370_2000;  // command table   (128-aligned)
const DATA_PHYS: u64 = 0x0370_3000;  // sector data buffer (up to 64 KiB)
const DATA_MAX:  usize = 64 * 1024;  // 128 sectors per transfer

// HBA global registers (offsets from ABAR).
const GHC: usize = 0x04;     // global host control
const PI:  usize = 0x0C;     // ports implemented
const GHC_AE: u32 = 1 << 31; // AHCI enable

// Port register offsets (from ABAR + 0x100 + port*0x80).
const PXCLB:  usize = 0x00;
const PXCLBU: usize = 0x04;
const PXFB:   usize = 0x08;
const PXFBU:  usize = 0x0C;
const PXIS:   usize = 0x10;
const PXCMD:  usize = 0x18;
const PXTFD:  usize = 0x20;
const PXSSTS: usize = 0x28;
const PXSERR: usize = 0x30;
const PXCI:   usize = 0x38;

const PXCMD_ST:  u32 = 1 << 0;
const PXCMD_FRE: u32 = 1 << 4;
const PXCMD_FR:  u32 = 1 << 14;
const PXCMD_CR:  u32 = 1 << 15;

const ATA_READ_DMA_EX:  u8 = 0x25;
const ATA_WRITE_DMA_EX: u8 = 0x35;
const ATA_BUSY: u32 = 0x80;
const ATA_DRQ:  u32 = 0x08;

static mut ABAR: u64 = 0;
static mut PORT: usize = 0;     // ABAR offset of the active port's registers
static mut READY: bool = false;

#[inline] unsafe fn r(off: usize) -> u32 { read_volatile((ABAR + off as u64) as *const u32) }
#[inline] unsafe fn w(off: usize, v: u32) { write_volatile((ABAR + off as u64) as *mut u32, v); }
#[inline] unsafe fn pr(off: usize) -> u32 { read_volatile((ABAR + PORT as u64 + off as u64) as *const u32) }
#[inline] unsafe fn pw(off: usize, v: u32) { write_volatile((ABAR + PORT as u64 + off as u64) as *mut u32, v); }

/// Probe for an AHCI controller + device. Returns true if a disk is ready.
pub fn init() -> bool {
    let (bus, dev, func, _) = match crate::pci::find_class(0x01, 0x06) {
        Some(t) => t,
        None => { crate::serial::write_str("  [ahci] no AHCI controller\n"); return false; }
    };
    crate::pci::enable_bus_master(bus, dev, func);
    let abar = (crate::pci::bar(bus, dev, func, 5) & !0xFFF) as u64;
    if abar == 0 { return false; }
    crate::vmm::map_mmio_range(abar, 0x2000);
    unsafe {
        ABAR = abar;
        w(GHC, r(GHC) | GHC_AE);          // enable AHCI

        // Find the first implemented port that has a device with PHY comm.
        let pi = r(PI);
        let mut found: i32 = -1;
        for p in 0..32u32 {
            if pi & (1 << p) == 0 { continue; }
            let poff = 0x100 + (p as usize) * 0x80;
            let ssts = read_volatile((ABAR + poff as u64 + PXSSTS as u64) as *const u32);
            let det = ssts & 0x0F;
            let ipm = (ssts >> 8) & 0x0F;
            if det == 3 && ipm == 1 { found = p as i32; break; }
        }
        if found < 0 { crate::serial::write_str("  [ahci] no SATA device on any port\n"); return false; }
        PORT = 0x100 + (found as usize) * 0x80;

        // Stop the port, point it at our command list + FIS area, restart.
        stop_port();
        pw(PXCLB, CLB_PHYS as u32);  pw(PXCLBU, (CLB_PHYS >> 32) as u32);
        pw(PXFB,  FB_PHYS as u32);   pw(PXFBU,  (FB_PHYS >> 32) as u32);
        // Zero the command list + FIS receive area.
        core::ptr::write_bytes(CLB_PHYS as *mut u8, 0, 1024);
        core::ptr::write_bytes(FB_PHYS as *mut u8, 0, 256);
        pw(PXSERR, 0xFFFF_FFFF);     // clear errors
        pw(PXIS, 0xFFFF_FFFF);
        start_port();

        READY = true;
        crate::serial::write_str("  [ahci] SATA disk ready on port ");
        crate::serial::write_str(if found < 10 {
            match found { 0=>"0",1=>"1",2=>"2",3=>"3",4=>"4",5=>"5",6=>"6",7=>"7",8=>"8",_=>"9" }
        } else { "?" });
        crate::serial::write_str("\n");
    }
    // Self-test: write a signature sector and read it back (LBA far out of the
    // way of any real filesystem, near the end of a small test image is risky,
    // so use a high-but-safe LBA the install layout reserves for scratch).
    true
}

unsafe fn stop_port() {
    let mut cmd = pr(PXCMD);
    cmd &= !PXCMD_ST;
    cmd &= !PXCMD_FRE;
    pw(PXCMD, cmd);
    // Wait for FR and CR to clear.
    for _ in 0..1_000_000 {
        if pr(PXCMD) & (PXCMD_FR | PXCMD_CR) == 0 { break; }
    }
}

unsafe fn start_port() {
    // Wait until CR is clear, then set FRE and ST.
    for _ in 0..1_000_000 { if pr(PXCMD) & PXCMD_CR == 0 { break; } }
    let cmd = pr(PXCMD) | PXCMD_FRE;
    pw(PXCMD, cmd);
    pw(PXCMD, pr(PXCMD) | PXCMD_ST);
}

/// Build + issue one READ/WRITE DMA EXT command on slot 0, polling to completion.
unsafe fn issue(lba: u64, count: u16, write: bool) -> bool {
    // Wait for the port to be idle (not BUSY/DRQ).
    for _ in 0..1_000_000 { if pr(PXTFD) & (ATA_BUSY | ATA_DRQ) == 0 { break; } }
    pw(PXIS, 0xFFFF_FFFF);

    let bytes = count as usize * 512;

    // Command header (slot 0) in the command list.
    let chdr = CLB_PHYS as *mut u32;
    // DW0: CFL=5 (FIS DWORDs), W bit (6) if write, PRDTL=1 (one PRDT entry).
    let mut dw0 = 5u32;                  // command FIS length in DWORDs
    if write { dw0 |= 1 << 6; }
    dw0 |= 1 << 16;                      // PRDTL = 1
    write_volatile(chdr, dw0);
    write_volatile(chdr.add(1), 0);      // PRDBC
    write_volatile(chdr.add(2), CT_PHYS as u32);
    write_volatile(chdr.add(3), (CT_PHYS >> 32) as u32);

    // Command table: clear, then build the H2D FIS + PRDT.
    core::ptr::write_bytes(CT_PHYS as *mut u8, 0, 256);
    let cfis = CT_PHYS as *mut u8;
    write_volatile(cfis.add(0), 0x27);              // FIS type: H2D
    write_volatile(cfis.add(1), 0x80);              // C=1 (command)
    write_volatile(cfis.add(2), if write { ATA_WRITE_DMA_EX } else { ATA_READ_DMA_EX });
    write_volatile(cfis.add(3), 0);                 // features
    write_volatile(cfis.add(4), lba as u8);
    write_volatile(cfis.add(5), (lba >> 8) as u8);
    write_volatile(cfis.add(6), (lba >> 16) as u8);
    write_volatile(cfis.add(7), 0x40);              // device: LBA mode
    write_volatile(cfis.add(8), (lba >> 24) as u8);
    write_volatile(cfis.add(9), (lba >> 32) as u8);
    write_volatile(cfis.add(10), (lba >> 40) as u8);
    write_volatile(cfis.add(11), 0);                // features high
    write_volatile(cfis.add(12), count as u8);
    write_volatile(cfis.add(13), (count >> 8) as u8);

    // PRDT entry at offset 0x80 in the command table.
    let prdt = (CT_PHYS + 0x80) as *mut u32;
    write_volatile(prdt.add(0), DATA_PHYS as u32);
    write_volatile(prdt.add(1), (DATA_PHYS >> 32) as u32);
    write_volatile(prdt.add(2), 0);
    write_volatile(prdt.add(3), ((bytes - 1) as u32) & 0x003F_FFFF); // byte count - 1 (no IRQ)

    // Issue on slot 0.
    pw(PXCI, 1);
    // Poll for completion.
    let mut ok = false;
    for _ in 0..20_000_000u32 {
        if pr(PXCI) & 1 == 0 { ok = true; break; }
        if pr(PXIS) & (1 << 30) != 0 { break; } // TFES — task file error
    }
    if !ok { return false; }
    if pr(PXTFD) & 0x01 != 0 { return false; } // ERR bit in task file
    true
}

/// Read `count` sectors (max 128) starting at `lba` into `buf`.
pub fn read_sectors(lba: u64, count: u16, buf: &mut [u8]) -> bool {
    unsafe {
        if !READY || count == 0 { return false; }
        let bytes = count as usize * 512;
        if bytes > DATA_MAX || bytes > buf.len() { return false; }
        if !issue(lba, count, false) { return false; }
        core::ptr::copy_nonoverlapping(DATA_PHYS as *const u8, buf.as_mut_ptr(), bytes);
        true
    }
}

/// Write `count` sectors (max 128) starting at `lba` from `buf`.
pub fn write_sectors(lba: u64, count: u16, buf: &[u8]) -> bool {
    unsafe {
        if !READY || count == 0 { return false; }
        let bytes = count as usize * 512;
        if bytes > DATA_MAX || bytes > buf.len() { return false; }
        core::ptr::copy_nonoverlapping(buf.as_ptr(), DATA_PHYS as *mut u8, bytes);
        issue(lba, count, true)
    }
}

pub fn is_ready() -> bool { unsafe { READY } }

/// Boot self-test: write a signature to a reserved scratch sector and read it
/// back, proving the full DMA round-trip works. Uses LBA 2048 (1 MiB in) — far
/// from sector 0 so we never clobber a partition table or boot sector.
pub fn selftest() -> bool {
    if !is_ready() { return false; }
    const SCRATCH_LBA: u64 = 2048;
    let mut wbuf = [0u8; 512];
    let sig = b"RUSTY-PENGUIN-AHCI-OK";
    wbuf[..sig.len()].copy_from_slice(sig);
    // tag with a value derived from the timestamp so a stale match can't fake it
    let tsc = unsafe { core::arch::x86_64::_rdtsc() } as u32;
    wbuf[64..68].copy_from_slice(&tsc.to_le_bytes());
    if !write_sectors(SCRATCH_LBA, 1, &wbuf) {
        crate::serial::write_str("  [ahci] self-test WRITE failed\n");
        return false;
    }
    let mut rbuf = [0u8; 512];
    if !read_sectors(SCRATCH_LBA, 1, &mut rbuf) {
        crate::serial::write_str("  [ahci] self-test READ failed\n");
        return false;
    }
    if &rbuf[..sig.len()] == sig && rbuf[64..68] == tsc.to_le_bytes() {
        crate::serial::write_str("  [ahci] self-test OK — wrote + read back a sector\n");
        true
    } else {
        crate::serial::write_str("  [ahci] self-test MISMATCH (read-back differs)\n");
        false
    }
}
