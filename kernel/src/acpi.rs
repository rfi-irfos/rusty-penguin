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
static mut SLP_TYPA_S3: u16 = 0;
static mut SLP_TYPB_S3: u16 = 0;
static mut FACS_PHYS: u64 = 0;
static mut S3_AVAIL: bool = false;
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

/// Parse the FADT's DSDT for the `\_S3_` package (S3 suspend-to-RAM SLP_TYP values).
fn parse_s3(dsdt: u64) {
    let len = unsafe { rd32(dsdt + 4) } as u64;
    let mut i = 36u64;
    while i + 5 < len {
        if unsafe { rd8(dsdt + i) } == b'_'
            && unsafe { rd8(dsdt + i + 1) } == b'S'
            && unsafe { rd8(dsdt + i + 2) } == b'3'
            && unsafe { rd8(dsdt + i + 3) } == b'_'
        {
            let mut p = i + 4;
            let mut guard = 0;
            while guard < 4 && unsafe { rd8(dsdt + p) } != 0x12 { p += 1; guard += 1; }
            if unsafe { rd8(dsdt + p) } != 0x12 { return; }
            p += 1;
            let pl = unsafe { rd8(dsdt + p) };
            p += 1 + (pl >> 6) as u64;
            p += 1; // NumElements
            unsafe {
                let (a, adv) = aml_int(dsdt + p);
                SLP_TYPA_S3 = a;
                let (b, _) = aml_int(dsdt + p + adv);
                SLP_TYPB_S3 = b;
                S3_AVAIL = true;
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

        let fadt_len = rd32(fadt + 4) as u64;

        // FACS (firmware_waking_vector lives here; needed for S3 resume).
        // FIRMWARE_CTRL at FADT+36 (u32); X_FIRMWARE_CTRL at FADT+132 (u64, ACPI 2+).
        let mut facs = rd32(fadt + 36) as u64;
        if fadt_len > 140 {
            let xfacs = rd64(fadt + 132);
            if xfacs != 0 { facs = xfacs; }
        }
        FACS_PHYS = facs;

        // DSDT: prefer the 64-bit X_DSDT (offset 140) when present.
        let mut dsdt = rd32(fadt + 40) as u64;
        if fadt_len > 148 {
            let xdsdt = rd64(fadt + 140);
            if xdsdt != 0 { dsdt = xdsdt; }
        }
        if dsdt != 0 {
            parse_s5(dsdt);
            parse_s3(dsdt);
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
    crate::serial::write_str(" S5a=");
    log_dec(unsafe { SLP_TYPA } as u64);
    if unsafe { S3_AVAIL } {
        crate::serial::write_str(" S3a=");
        log_dec(unsafe { SLP_TYPA_S3 } as u64);
        crate::serial::write_str(" FACS=");
        log_hex(unsafe { FACS_PHYS } as u32);
    } else {
        crate::serial::write_str(" S3=none");
    }
    crate::serial::write_str(if unsafe { RESET_SUPPORTED } {
        " (shutdown+reset+S3)\n"
    } else {
        " (shutdown+S3; reset via 8042)\n"
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

pub fn is_ready() -> bool { unsafe { FOUND } }
pub fn s3_available() -> bool { unsafe { S3_AVAIL } }

// ── ACPI S3 Suspend-to-RAM ────────────────────────────────────────────────────
//
// Physical memory layout (all within the first 1 MiB — never touched by the PMM):
//   0x6000  resume state struct  (magic + cr3 + cr4 + rsp + rip)
//   0x7000  trampoline GDT       (6-byte descriptor + 5×8-byte entries)
//   0x8000  resume trampoline    (real16 → pm32 → lm64, 121 bytes)
//
// Suspend path:
//   1. Write GDT and trampoline bytes to 0x7000/0x8000 via physmap.
//   2. Save CR3 (kernel boot), CR4, RSP, RIP (= &acpi_s3_resumed) to 0x6000.
//   3. Set FACS.firmware_waking_vector = 0x8000 (real-mode linear addr).
//   4. wbinvd  +  write (SLP_TYPA_S3 << 10) | SLP_EN to PM1A_CNT.
//   QEMU transitions to "suspended"; system_wakeup via QMP resumes SeaBIOS
//   which jumps (real mode) to 0x8000.
//
// Resume path (all machine code in S3_TRAMPOLINE_CODE):
//   real mode  → sets DS=0, lgdt [0x7000], PE=1, jmp far 0x08:0x8020
//   32-bit PM  → load CR4 (PAE), CR3 (boot kernel), LME in EFER, PG=1,
//                jmp far 0x18:0x8060
//   64-bit LM  → restore RSP from [0x6018], jmp [0x6020] (acpi_s3_resumed)

const RESUME_STATE:   u64 = 0x6000;
const S3_GDT_PHYS:    u64 = 0x7000;
const S3_TRAMP_PHYS:  u64 = 0x8000;
const RESUME_MAGIC:   u64 = 0xDEAD_BEEF_CAFE_BABE;

// GDT descriptor (6 bytes at 0x7000): limit=39, base=0x7008.
const S3_GDT_DESC: [u8; 6] = [0x27, 0x00, 0x08, 0x70, 0x00, 0x00];

// Five 8-byte GDT entries at 0x7008: null / code32(0x08) / data32(0x10) /
// code64(0x18) / data64(0x20). All flat base=0 — the trampoline is
// position-independent relative to these descriptors.
const S3_GDT_ENTRIES: [u8; 40] = [
    0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,  // null
    0xFF,0xFF,0x00,0x00,0x00,0x9A,0xCF,0x00,  // code32  (0x08)
    0xFF,0xFF,0x00,0x00,0x00,0x92,0xCF,0x00,  // data32  (0x10)
    0xFF,0xFF,0x00,0x00,0x00,0x9A,0xAF,0x00,  // code64  (0x18)  L=1 D=0
    0xFF,0xFF,0x00,0x00,0x00,0x92,0xAF,0x00,  // data64  (0x20)
];

// Pre-assembled S3 resume trampoline (121 bytes).
// Wakes in real mode: SeaBIOS sets CS=0x0800, IP=0x0000  (phys 0x8000).
// DS is set to 0 immediately so all absolute data accesses use linear == phys.
//
// Resume-state offsets:  +0 magic  +8 cr3  +16 cr4  +24 rsp  +32 rip
const S3_TRAMPOLINE_CODE: [u8; 121] = [
    // ── real mode (0x00 – 0x1F) ──────────────────────────────────────────────
    0xFA,                                            // cli
    0x31,0xC0,                                       // xor ax, ax
    0x8E,0xD8,                                       // mov ds, ax
    0x8E,0xC0,                                       // mov es, ax
    0x0F,0x01,0x16,0x00,0x70,                        // lgdt [0x7000]
    0x66,0x0F,0x20,0xC0,                             // mov eax, cr0
    0x0C,0x01,                                       // or al, 1 (PE)
    0x66,0x0F,0x22,0xC0,                             // mov cr0, eax
    0x66,0xEA,0x20,0x80,0x00,0x00,0x08,0x00,         // jmp far 0x0008:0x8020
    0x90,0x90,                                       // padding → 0x20
    // ── 32-bit PM (0x20 – 0x5F) ──────────────────────────────────────────────
    0xB8,0x10,0x00,0x00,0x00,                        // mov eax, 0x10
    0x8E,0xD8,                                       // mov ds, ax
    0x8E,0xD0,                                       // mov ss, ax
    0xA1,0x10,0x60,0x00,0x00,                        // mov eax, [0x6010]  (cr4)
    0x0F,0x22,0xE0,                                  // mov cr4, eax       (PAE restored)
    0xA1,0x08,0x60,0x00,0x00,                        // mov eax, [0x6008]  (cr3)
    0x0F,0x22,0xD8,                                  // mov cr3, eax
    0xB9,0x80,0x00,0x00,0xC0,                        // mov ecx, 0xC0000080 (EFER MSR)
    0x0F,0x32,                                       // rdmsr
    0x0D,0x00,0x01,0x00,0x00,                        // or eax, 0x100      (LME)
    0x0F,0x30,                                       // wrmsr
    0x0F,0x20,0xC0,                                  // mov eax, cr0
    0x0D,0x01,0x00,0x00,0x80,                        // or eax, 0x80000001 (PG+PE)
    0x0F,0x22,0xC0,                                  // mov cr0, eax
    0xEA,0x60,0x80,0x00,0x00,0x18,0x00,              // jmp far 0x0018:0x8060
    0x90,0x90,0x90,0x90,0x90,0x90,0x90,              // padding → 0x60
    // ── 64-bit long mode (0x60 – 0x78) ───────────────────────────────────────
    // Paging is ON; 0x8060 is identity-mapped (kernel boot CR3 covers 0–64 MiB).
    0xB8,0x20,0x00,0x00,0x00,                        // mov eax, 0x20
    0x8E,0xD0,                                       // mov ss, ax
    0x48,0x8B,0x24,0x25,0x18,0x60,0x00,0x00,         // mov rsp, [abs 0x6018]
    0x48,0x8B,0x04,0x25,0x20,0x60,0x00,0x00,         // mov rax, [abs 0x6020]
    0xFF,0xE0,                                       // jmp rax → acpi_s3_resumed
];

#[inline]
unsafe fn write_phys_bytes(phys: u64, src: &[u8]) {
    for (i, &b) in src.iter().enumerate() {
        core::ptr::write_volatile(
            crate::vmm::phys_to_virt(phys + i as u64) as *mut u8,
            b,
        );
    }
}

#[inline]
unsafe fn write_phys_u32(phys: u64, val: u32) {
    core::ptr::write_volatile(crate::vmm::phys_to_virt(phys) as *mut u32, val);
}

#[inline]
unsafe fn write_phys_u64(phys: u64, val: u64) {
    core::ptr::write_volatile(crate::vmm::phys_to_virt(phys) as *mut u64, val);
}

fn setup_s3_low_memory() {
    unsafe {
        write_phys_bytes(S3_GDT_PHYS,         &S3_GDT_DESC);
        write_phys_bytes(S3_GDT_PHYS + 8,     &S3_GDT_ENTRIES);
        write_phys_bytes(S3_TRAMP_PHYS,        &S3_TRAMPOLINE_CODE);
    }
}

/// Called by the trampoline (via `jmp rax`) after returning from S3.
/// RSP is restored to the saved kernel stack; we just log and idle.
/// Interrupts may be re-enabled by the caller chain if a scheduler
/// integration is wired up later.
#[no_mangle]
pub extern "C" fn acpi_s3_resumed() -> ! {
    crate::serial::write_str("[acpi] resumed from S3 — kernel alive\n");
    loop { unsafe { core::arch::asm!("hlt", options(nostack)); } }
}

/// Enter ACPI S3 (suspend-to-RAM). Sets up the resume trampoline, writes
/// firmware_waking_vector to FACS, flushes caches, and writes the sleep
/// registers. Does not return on success (CPU is frozen by the hardware).
/// Returns false if S3 is not available or FACS is missing.
pub fn suspend() -> bool {
    unsafe {
        if !FOUND || !S3_AVAIL || FACS_PHYS == 0 || PM1A_CNT == 0 {
            crate::serial::write_str("[acpi] S3 suspend: not available\n");
            return false;
        }
    }

    // Write trampoline GDT and code to low memory (physmap-mapped, in first 1 MiB).
    setup_s3_low_memory();

    // Snapshot CPU state that the trampoline must restore.
    let cr3_val = crate::vmm::kernel_boot_cr3();
    let cr4_val: u64;
    let rsp_val: u64;
    unsafe {
        core::arch::asm!("mov {}, cr4", out(reg) cr4_val, options(nostack, readonly));
        core::arch::asm!("mov {}, rsp", out(reg) rsp_val, options(nostack, readonly));
    }
    let rip_val = acpi_s3_resumed as u64;

    // Write resume state struct to phys 0x6000.
    unsafe {
        write_phys_u64(RESUME_STATE,      RESUME_MAGIC);
        write_phys_u64(RESUME_STATE +  8, cr3_val);
        write_phys_u64(RESUME_STATE + 16, cr4_val);
        write_phys_u64(RESUME_STATE + 24, rsp_val);
        write_phys_u64(RESUME_STATE + 32, rip_val);
    }

    // Point FACS.firmware_waking_vector to physical 0x8000 (real-mode linear addr).
    // Clear x_firmware_waking_vector so SeaBIOS uses the 16-bit real-mode path.
    unsafe {
        // FACS layout: [0] sig(4) [4] len(4) [8] hw_sig(4) [12] waking_vec(4)
        //              [16] global_lock(4) [20] flags(4) [24] x_waking_vec(8)
        write_phys_u32(FACS_PHYS + 12, 0x8000u32);  // firmware_waking_vector
        write_phys_u64(FACS_PHYS + 24, 0u64);       // x_firmware_waking_vector = 0
    }

    // Write-back-invalidate all caches so FACS and the trampoline are in RAM
    // before the hardware removes power from the CPU.
    unsafe { core::arch::asm!("wbinvd", options(nostack)); }

    crate::serial::write_str("[acpi] entering S3 (suspend-to-RAM)\n");

    // Write S3 sleep values — CPU freezes here until system_wakeup.
    unsafe {
        port::outw(PM1A_CNT as u16, (SLP_TYPA_S3 << 10) | SLP_EN);
        if PM1B_CNT != 0 {
            port::outw(PM1B_CNT as u16, (SLP_TYPB_S3 << 10) | SLP_EN);
        }
    }

    // Only reached if S3 didn't take (shouldn't happen on compliant hardware).
    crate::serial::write_str("[acpi] S3 PM1_CNT write returned (S3 not supported by hardware)\n");
    false
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
