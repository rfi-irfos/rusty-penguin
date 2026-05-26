use crate::vga;
use crate::gdt;

// ── MSR addresses ────────────────────────────────────────────────────────────
const IA32_EFER:  u32 = 0xC000_0080;
const IA32_STAR:  u32 = 0xC000_0081;
const IA32_LSTAR: u32 = 0xC000_0082;
const IA32_FMASK: u32 = 0xC000_0084;

unsafe fn wrmsr(msr: u32, val: u64) {
    core::arch::asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") val as u32,
        in("edx") (val >> 32) as u32,
        options(nostack),
    );
}

unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32; let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, readonly),
    );
    lo as u64 | ((hi as u64) << 32)
}

// ── Syscall entry trampoline (AT&T syntax, global_asm) ───────────────────────
//
// SYSCALL saves:  RCX ← user RIP,  R11 ← user RFLAGS
// SYSCALL does NOT switch the stack — we do it manually via _user_rsp.
//
// Calling convention into syscall_handler:
//   rdi = syscall number (was rax)
//   rsi = arg1 (was rdi)
//   rdx = arg2 (was rsi)
//   rcx = arg3 (was rdx)
//   Return value in rax.
//
core::arch::global_asm!(
    ".section .bss",
    ".align 16",
    "_syscall_kstack:",
    ".skip 8192",
    "_syscall_kstack_top:",

    ".section .data",
    ".align 8",
    "_user_rsp: .quad 0",

    ".section .text",
    ".global syscall_entry",
    "syscall_entry:",

    // 1. Save user RSP, switch to kernel stack
    "mov qword ptr [rip + _user_rsp], rsp",
    "lea rsp, [rip + _syscall_kstack_top]",

    // 2. Save non-scratch regs (rcx/r11 hold user rip/rflags from SYSCALL)
    "push r11",          // user RFLAGS
    "push rcx",          // user RIP
    "push rbp",
    "push rbx",
    "push r12",
    "push r13",
    "push r14",
    "push r15",

    // 3. Marshal args:  rax=nr rdi=a1 rsi=a2 rdx=a3  →  rdi=nr rsi=a1 rdx=a2 rcx=a3
    "mov rcx, rdx",      // a3 → 4th param
    "mov rdx, rsi",      // a2 → 3rd param
    "mov rsi, rdi",      // a1 → 2nd param
    "mov rdi, rax",      // nr → 1st param

    "call syscall_handler",
    // rax = return value

    // 4. Restore non-scratch regs
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop rbx",
    "pop rbp",
    "pop rcx",           // user RIP → rcx (consumed by sysretq)
    "pop r11",           // user RFLAGS → r11 (consumed by sysretq)

    // 5. Restore user RSP, return to ring-3
    "mov rsp, qword ptr [rip + _user_rsp]",
    "sysretq",
);

// ── Rust syscall dispatcher ──────────────────────────────────────────────────
#[no_mangle]
pub extern "C" fn syscall_handler(nr: u64, arg1: u64, arg2: u64, arg3: u64) -> u64 {
    match nr {
        2 => {
            // sys_clear — clear VGA screen
            crate::vga::clear();
            0
        }
        3 => {
            // sys_reboot — pulse keyboard controller reset line
            unsafe {
                loop { if crate::port::inb(0x64) & 0x02 == 0 { break; } }
                crate::port::outb(0x64, 0xFE);
            }
            loop {}
        }
        0 => {
            // sys_read(fd, buf_virt, len) — blocks with sti+hlt until '\n'
            let len = (arg3 as usize).min(256);
            if len == 0 { return 0; }
            let buf = arg2 as *mut u8;
            let mut i = 0;
            while i < len {
                let ch = loop {
                    unsafe {
                        core::arch::asm!("sti", options(nostack));
                        core::arch::asm!("hlt", options(nostack));
                    }
                    if let Some(c) = crate::idt::kbd_get() { break c; }
                };
                unsafe { *buf.add(i) = ch; }
                i += 1;
                if ch == b'\n' { break; }
            }
            i as u64
        }
        1 => {
            // sys_write(fd, buf_virt, len)
            let len = (arg3 as usize).min(256);
            if len == 0 { return 0; }
            let ptr = arg2 as *const u8;
            let bytes = unsafe { core::slice::from_raw_parts(ptr, len) };
            for &b in bytes {
                if b == 0 { break; }
                vga::write_byte(b, vga::Color::White);
            }
            len as u64
        }
        60 => {
            // sys_exit(code)
            vga::write_str("\n  [psh exited]\n", vga::Color::Green);
            loop { unsafe { core::arch::asm!("hlt", options(nostack)); } }
        }
        _ => u64::MAX,
    }
}

// ── Init: wire SYSCALL MSRs ──────────────────────────────────────────────────
pub fn init() {
    unsafe {
        // Enable SCE (System Call Extensions) in EFER bit 0
        wrmsr(IA32_EFER, rdmsr(IA32_EFER) | 1);

        // STAR: bits[47:32] = SYSCALL CS, bits[63:48] = SYSRET base
        let star = ((gdt::STAR_SYSRET  as u64) << 48)
                 | ((gdt::STAR_SYSCALL as u64) << 32);
        wrmsr(IA32_STAR, star);

        // LSTAR: syscall entry point
        extern "C" { fn syscall_entry(); }
        wrmsr(IA32_LSTAR, syscall_entry as *const () as usize as u64);

        // FMASK: clear IF (bit 9) on entry so syscall handler runs with interrupts off
        wrmsr(IA32_FMASK, 1 << 9);
    }
}
