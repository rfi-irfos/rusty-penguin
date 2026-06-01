//! ACPI power management — clean shutdown and reboot, the daily-driver basics.
//!
//! Before this the kernel had no way to power off: closing the VM or a triple
//! fault was it. Now we parse the firmware's ACPI tables (RSDP → RSDT/XSDT →
//! FADT, and the FADT's DSDT for the `\_S5` sleep package) and drive the PM1
//! control registers to enter S5 (soft-off), with an ACPI/8042 reset path for
//! reboot. Read-only table parsing + a couple of port writes — fully verifiable
//! in QEMU (the VM powers off / resets).
//!
//! Honestly out of scope here: real battery state (`_BST`) and brightness need
//! an AML interpreter / backlight path, and S3 suspend/resume is a much larger
//! job. This brick is shutdown + reboot; those are follow-ups.

#![allow(dead_code)]

use crate::port;
use crate::vmm::phys_to_virt;

// Parsed once at init.
static mut FOUND: bool = false;
static mut PM1A_CNT: u32 = 0;
static mut PM1B_CNT: u32 = 0;
static mut SLP_TYPA: u16 = 0;
static mut SLP_TYPB: u16 = 0;
static mut RESET_SUPPORTED: bool = false;
static mut RESET_ADDR: u64 = 0;
static mut RESET_IS_IO: bool = false;
static mut RESET_VALUE: u8 = 0;

const SLP_EN: u16 = 1 << 13;

#[inline]
unsafe fn rd8(phys: u64) -> u8 {
    core::ptr::read_unaligned(phys_to_virt(phys) as *const u8)
}
#[inline]
unsafe fn rd32(phys: u64) -> u32 {
    core::ptr::read_unaligned(phys_to_virt(phys) as *const u32)
}
#[inline]
unsafe fn rd64(phys: u64) -> u64 {
    core::ptr::read_unaligned(phys_to_virt(phys) as *const u64)
}

fn checksum(phys: u64, len: usize) -> u8 {
    let mut s = 0u8;
    for i in 0..len {
        s = s.wrapping_add(unsafe { rd8(phys + i as u64) });
    }
    s
}

/// Find the Root System Description Pointer ("RSD PTR ") in the BIOS area.
fn find_rsdp() -> Option<u64> {
    let sig = b"RSD PTR ";
    let mut addr = 0xE0000u64;
    while addr < 0x10_0000 {
        let mut m = true;
        for i in 0..8u64 {
            if unsafe { rd8(addr + i) } != sig[i as usize] {
                m = false;
                break;
            }
        }
        if m && checksum(addr, 20) == 0 {
            return Some(addr);
        }
        addr += 16;
    }
    None
}

/// Find a table by 4-byte signature by walking the RSDT (32-bit entries) or
/// XSDT (64-bit entries). Returns the table's physical address.
fn find_table(sdt: u64, xsdt: bool, want: &[u8; 4]) -> Option<u64> {
    let len = unsafe { rd32(sdt + 4) } as u64;
    let entries = (len - 36) / if xsdt { 8 } else { 4 };
    for i in 0..entries {
        let ent = if xsdt {
            unsafe { rd64(sdt + 36 + i * 8) }
        } else {
            unsafe { rd32(sdt + 36 + i * 4) as u64 }
        };
        let mut sig = [0u8; 4];
        for j in 0..4 {
            sig[j] = unsafe { rd8(ent + j as u64) };
        }
        if &sig == want {
            return Some(ent);
        }
    }
    None
}

/// Decode a tiny AML integer at `phys` → (value, bytes_consumed).
unsafe fn aml_int(phys: u64) -> (u16, u64) {
    match rd8(phys) {
        0x00 => (0, 1),               // ZeroOp
        0x01 => (1, 1),               // OneOp
        0x0A => (rd8(phys + 1) as u16, 2), // BytePrefix
        0x0B => (rd32(phys + 1) as u16 & 0xffff, 3), // WordPrefix
        other => (other as u16, 1),   // bare small value
    }
}

/// Parse the FADT's DSDT for the `\_S5_` package and pull SLP_TYPa / SLP_TYPb.
fn parse_s5(dsdt: u64) {
    let len = unsafe { rd32(dsdt + 4) } as u64;
    // scan the AML body for the "_S5_" name
    let mut i = 36u64;
    while i + 5 < len {
        if unsafe { rd8(dsdt + i) } == b'_'
            && unsafe { rd8(dsdt + i + 1) } == b'S'
            && unsafe { rd8(dsdt + i + 2) } == b'5'
            && unsafe { rd8(dsdt + i + 3) } == b'_'
        {
            // expect a PackageOp (0x12) within a couple of bytes
            let mut p = i + 4;
            let mut guard = 0;
            while guard < 4 && unsafe { rd8(dsdt + p) } != 0x12 {
                p += 1;
                guard += 1;
            }
            if unsafe { rd8(dsdt + p) } != 0x12 {
                return;
            }
            p += 1; // past PackageOp
            // PkgLength: top 2 bits of first byte = # of extra length bytes
            let pl = unsafe { rd8(dsdt + p) };
            p += 1 + (pl >> 6) as u64;
            // NumElements
            p += 1;
            unsafe {
                let (a, adv) = aml_int(dsdt + p);
                SLP_TYPA = a;
                let (b, _) = aml_int(dsdt + p + adv);
                SLP_TYPB = b;
            }
            return;
        }
        i += 1;
    }
}

/// Probe ACPI and cache the power-off / reset parameters.
pub fn init() -> bool {
    let rsdp = match find_rsdp() {
        Some(r) => r,
        None => {
            crate::serial::write_str("  [acpi] no RSDP found\n");
            return false;
        }
    };
    let revision = unsafe { rd8(rsdp + 15) };
    let (sdt, xsdt) = if revision >= 2 {
        (unsafe { rd64(rsdp + 24) }, true) // XSDT
    } else {
        (unsafe { rd32(rsdp + 16) as u64 }, false) // RSDT
    };

    let fadt = match find_table(sdt, xsdt, b"FACP") {
        Some(f) => f,
        None => {
            crate::serial::write_str("  [acpi] no FADT\n");
            return false;
        }
    };

    unsafe {
        PM1A_CNT = rd32(fadt + 64);
        PM1B_CNT = rd32(fadt + 68);

        // DSDT: prefer the 64-bit X_DSDT (offset 140) when present.
        let mut dsdt = rd32(fadt + 40) as u64;
        let fadt_len = rd32(fadt + 4) as u64;
        if fadt_len > 148 {
            let xdsdt = rd64(fadt + 140);
            if xdsdt != 0 {
                dsdt = xdsdt;
            }
        }
        if dsdt != 0 {
            parse_s5(dsdt);
        }

        // Reset register (FADT flags bit 10 = RESET_REG_SUP).
        let flags = rd32(fadt + 112);
        if fadt_len > 129 && (flags & (1 << 10)) != 0 {
            // ResetReg is a 12-byte Generic Address Structure at offset 116.
            let addr_space = rd8(fadt + 116); // 0 = system memory, 1 = system I/O
            let addr = rd64(fadt + 116 + 4);
            let val = rd8(fadt + 128);
            if addr != 0 {
                RESET_SUPPORTED = true;
                RESET_ADDR = addr;
                RESET_IS_IO = addr_space == 1;
                RESET_VALUE = val;
            }
        }
        FOUND = true;
    }

    crate::serial::write_str("  [acpi] ready: PM1a=");
    log_hex(unsafe { PM1A_CNT });
    crate::serial::write_str(" S5 type a=");
    log_dec(unsafe { SLP_TYPA } as u64);
    crate::serial::write_str(" b=");
    log_dec(unsafe { SLP_TYPB } as u64);
    crate::serial::write_str(if unsafe { RESET_SUPPORTED } {
        " (shutdown + ACPI reset)\n"
    } else {
        " (shutdown; reset via 8042)\n"
    });
    true
}

/// Enter ACPI S5 (soft off). Does not return on success.
pub fn poweroff() -> ! {
    crate::serial::write_str("  [acpi] powering off (entering S5)\n");
    unsafe {
        if FOUND && PM1A_CNT != 0 {
            port::outw(PM1A_CNT as u16, (SLP_TYPA << 10) | SLP_EN);
            if PM1B_CNT != 0 {
                port::outw(PM1B_CNT as u16, (SLP_TYPB << 10) | SLP_EN);
            }
        }
        // If still alive, QEMU/Bochs also honour this legacy port.
        port::outw(0xB004, 0x2000);
        port::outw(0x604, 0x2000);
    }
    // Last resort: stop the CPU.
    loop {
        unsafe { core::arch::asm!("hlt", options(nostack)); }
    }
}

/// Reboot via the ACPI reset register, falling back to the 8042 controller and
/// the PCI reset port. Does not return on success.
pub fn reboot() -> ! {
    crate::serial::write_str("  [acpi] rebooting\n");
    unsafe {
        if FOUND && RESET_SUPPORTED {
            if RESET_IS_IO {
                port::outb(RESET_ADDR as u16, RESET_VALUE);
            } else {
                core::ptr::write_volatile(phys_to_virt(RESET_ADDR) as *mut u8, RESET_VALUE);
            }
        }
        // 8042 keyboard controller: pulse the CPU reset line.
        let mut guard = 0;
        while port::inb(0x64) & 0x02 != 0 && guard < 100000 {
            guard += 1;
        }
        port::outb(0x64, 0xFE);
        // PCI reset control port.
        port::outb(0xCF9, 0x06);
    }
    loop {
        unsafe { core::arch::asm!("hlt", options(nostack)); }
    }
}

pub fn is_ready() -> bool {
    unsafe { FOUND }
}

fn log_hex(v: u32) {
    let d = |n: u8| if n < 10 { b'0' + n } else { b'a' + n - 10 };
    crate::serial::write_str("0x");
    for i in (0..8).rev() {
        crate::serial::write_byte(d(((v >> (i * 4)) & 0xf) as u8));
    }
}
fn log_dec(mut v: u64) {
    if v == 0 {
        crate::serial::write_byte(b'0');
        return;
    }
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    while v > 0 {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    for &b in &buf[i..] {
        crate::serial::write_byte(b);
    }
}
