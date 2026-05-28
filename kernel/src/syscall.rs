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
        0 => {
            // sys_read(fd, buf, len)
            let fd  = arg1;
            let buf = arg2 as *mut u8;
            let len = arg3 as usize;
            if fd == 0 {
                // stdin: block on keyboard
                let maxlen = len.min(256);
                if maxlen == 0 { return 0; }
                let mut i = 0;
                while i < maxlen {
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
            } else {
                crate::vfs::read(fd, buf, len)
            }
        }
        1 => {
            // sys_write(fd, buf, len) — fd 1/2 go to terminal
            let len = (arg3 as usize).min(4096);
            if len == 0 { return 0; }
            let ptr = arg2 as *const u8;
            let bytes = unsafe { core::slice::from_raw_parts(ptr, len) };
            for &b in bytes {
                if b == 0 { break; }
                vga::write_byte(b, vga::Color::White);
            }
            len as u64
        }
        2 => {
            // sys_open(path_ptr, path_len) → fd or MAX
            let ptr = arg1 as *const u8;
            let pathlen = (arg2 as usize).min(256);
            let path = unsafe { core::slice::from_raw_parts(ptr, pathlen) };
            // strip leading '/'
            let path = if path.starts_with(b"/") { &path[1..] } else { path };
            crate::vfs::open(path)
        }
        3 => {
            // sys_close(fd)
            crate::vfs::close(arg1)
        }
        // Kept at old numbers for backwards compat:
        // sys_clear=10, sys_reboot=11 (renumbered to avoid overlap)
        10 => {
            // sys_clear
            crate::vga::clear();
            0
        }
        11 => {
            // sys_reboot
            unsafe {
                loop { if crate::port::inb(0x64) & 0x02 == 0 { break; } }
                crate::port::outb(0x64, 0xFE);
            }
            loop {}
        }
        12 => {
            // sys_serial_debug(byte)
            crate::serial::write_byte(arg1 as u8);
            0
        }
        6 => {
            // sys_fb_query(out_ptr) → fills 24-byte struct, returns base virt addr
            // Struct layout: [u64 base][u32 width][u32 height][u32 pitch][u32 bpp]
            let base = crate::fb::base() as u64;
            if arg1 != 0 {
                let p = arg1 as *mut u8;
                unsafe {
                    p.cast::<u64>().write_unaligned(base);
                    p.add(8).cast::<u32>().write_unaligned(crate::fb::width());
                    p.add(12).cast::<u32>().write_unaligned(crate::fb::height());
                    p.add(16).cast::<u32>().write_unaligned(crate::fb::pitch());
                    p.add(20).cast::<u32>().write_unaligned(crate::fb::bpp() as u32);
                }
            }
            base
        }
        4 => {
            // sys_ticks — returns tick count (100 Hz since pit_init)
            crate::idt::ticks()
        }
        5 => {
            // sys_meminfo — returns (free_mib << 32) | total_mib
            let (free, total) = crate::pmm::stats();
            let free_mib  = (free  / 256) as u64;
            let total_mib = (total / 256) as u64;
            (free_mib << 32) | (total_mib & 0xFFFF_FFFF)
        }
        7 => {
            // sys_input_poll — non-blocking; returns event or 0 if empty
            crate::input::poll().unwrap_or(0)
        }
        13 => {
            // sys_rtc — read CMOS real-time clock
            // Returns packed u64:
            //   [63:48] year (e.g. 2026)  [47:40] month  [39:32] mday
            //   [31:24] hour              [23:16] min    [15:8]  sec
            //   [7:0]   weekday (1=Sun … 7=Sat)
            unsafe fn cmos_rd(reg: u8) -> u8 {
                crate::port::outb(0x70, reg & 0x7F);
                crate::port::inb(0x71)
            }
            unsafe {
                // Wait until RTC update-in-progress bit clears
                let mut tries = 0u32;
                loop {
                    crate::port::outb(0x70, 0x0A);
                    if crate::port::inb(0x71) & 0x80 == 0 { break; }
                    tries += 1;
                    if tries > 200_000 { break; }
                }
                let sec   = cmos_rd(0x00);
                let min   = cmos_rd(0x02);
                let hour  = cmos_rd(0x04);
                let wday  = cmos_rd(0x06);
                let mday  = cmos_rd(0x07);
                let month = cmos_rd(0x08);
                let year  = cmos_rd(0x09);
                let cent  = cmos_rd(0x32);
                // Status register B bit 2: 0=BCD, 1=binary
                crate::port::outb(0x70, 0x0B);
                let regb  = crate::port::inb(0x71);
                let bcd   = (regb & 0x04) == 0;
                let cvt   = |v: u8| -> u8 { if bcd { (v >> 4) * 10 + (v & 0x0F) } else { v } };
                let sec   = cvt(sec);
                let min   = cvt(min);
                let hour  = cvt(hour & 0x7F);
                let wday  = cvt(wday);
                let mday  = cvt(mday);
                let month = cvt(month);
                let year2 = cvt(year);
                let cent2 = cvt(cent);
                let century = if cent2 >= 19 && cent2 <= 21 { cent2 } else { 20 };
                let year4 = century as u16 * 100 + year2 as u16;
                ((year4 as u64) << 48)
                    | ((month as u64) << 40)
                    | ((mday  as u64) << 32)
                    | ((hour  as u64) << 24)
                    | ((min   as u64) << 16)
                    | ((sec   as u64) <<  8)
                    | (wday   as u64)
            }
        }
        8 => {
            // sys_input_wait — blocks until an event is available
            crate::input::wait()
        }
        9 => {
            // sys_ps(buf, max_count) → records written
            let max = (arg2 as usize).min(16);
            crate::sched::fill_ps(arg1 as *mut u8, max) as u64
        }
        14 => {
            // sys_listdir(path_ptr, path_len, out_buf, max_entries)
            // Lists files in directory. Output format: [name_len][name][size (u64)]...
            let path_ptr = arg1 as *const u8;
            let path_len = (arg2 as usize).min(256);
            let out_buf = arg3 as *mut u8;

            let path = unsafe { core::slice::from_raw_parts(path_ptr, path_len) };
            let path = if path.starts_with(b"/") { &path[1..] } else { path };

            let mut out_off = 0usize;
            let mut count = 0u64;

            unsafe {
                for i in 0..crate::ramfs::inode_count() {
                    if let Some(ino) = crate::ramfs::inode(i) {
                        let name = &ino.name[..ino.name_len];
                        let stored = if name.starts_with(b"./") { &name[2..] } else { name };

                        let in_dir = if path.is_empty() {
                            !stored.contains(&b'/')
                        } else {
                            stored.starts_with(path) &&
                            stored.get(path.len()) == Some(&b'/') &&
                            !stored[path.len() + 1..].contains(&b'/')
                        };

                        if in_dir {
                            let filename = if path.is_empty() {
                                stored
                            } else {
                                &stored[path.len() + 1..]
                            };

                            let name_len = filename.len().min(255) as u8;
                            if out_off + 1 + name_len as usize + 8 <= 4096 {
                                *out_buf.add(out_off) = name_len;
                                out_off += 1;

                                core::ptr::copy_nonoverlapping(
                                    filename.as_ptr(),
                                    out_buf.add(out_off),
                                    name_len as usize
                                );
                                out_off += name_len as usize;

                                let size_ptr = out_buf.add(out_off) as *mut u64;
                                size_ptr.write_unaligned(ino.size as u64);
                                out_off += 8;

                                count += 1;
                            }
                        }
                    }
                }
            }
            count
        }
        15 => {
            // sys_delete(path_ptr, path_len) → 0 on success, -1 on error
            let path_ptr = arg1 as *const u8;
            let path_len = (arg2 as usize).min(256);
            let path = unsafe { core::slice::from_raw_parts(path_ptr, path_len) };
            let path = if path.starts_with(b"/") { &path[1..] } else { path };

            // Find and delete from ramfs inode table
            for i in 0..crate::ramfs::inode_count() {
                if let Some(ino) = crate::ramfs::inode(i) {
                    let name = &ino.name[..ino.name_len];
                    let stored = if name.starts_with(b"./") { &name[2..] } else { name };
                    if stored == path {
                        // Actually mark the inode as deleted
                        return if crate::ramfs::delete(i) { 0 } else { u64::MAX };
                    }
                }
            }
            u64::MAX // not found
        }
        24 => {
            // sys_yield — cooperative yield (no-op: single process)
            crate::sched::yield_();
            0
        }
        39 => {
            // sys_getpid → current PID
            crate::sched::current_pid()
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
