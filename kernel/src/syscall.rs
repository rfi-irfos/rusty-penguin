use crate::vga;
use crate::gdt;

// ── ACPI EC battery reader ────────────────────────────────────────────────────
// ACPI Embedded Controller (EC) at I/O ports 0x62 (data) / 0x66 (command).
// This is the standard ACPI EC used by virtually all laptops.
// Returns battery percentage 0-100, or 0xFF if EC not present/not responsive.
//
// ACPI EC register map (common across most laptop BIOSes):
//   0x2A = BFC high byte (full charge capacity, mAh or mWh)
//   0x2B = BFC low byte
//   0x44 = BRC high byte (remaining capacity)
//   0x45 = BRC low byte
//
// Ternary lens: battery state is a Trit + percentage:
//   +1 Charging/full, 0 Discharging (ok), -1 Critical (<15%)
const EC_DATA:  u16 = 0x62;
const EC_CMD:   u16 = 0x66;
const EC_OBF:   u8  = 1;   // output buffer full (data ready to read)
const EC_IBF:   u8  = 2;   // input buffer full (busy, not ready for cmd)
const EC_READ:  u8  = 0x80; // EC read command

unsafe fn ec_wait_ibf() -> bool {
    let mut t = 0u32;
    while crate::port::inb(EC_CMD) & EC_IBF != 0 && t < 100_000 { t += 1; }
    t < 100_000
}
unsafe fn ec_wait_obf() -> bool {
    let mut t = 0u32;
    while crate::port::inb(EC_CMD) & EC_OBF == 0 && t < 100_000 { t += 1; }
    t < 100_000
}
unsafe fn ec_read(reg: u8) -> Option<u8> {
    if !ec_wait_ibf() { return None; }
    crate::port::outb(EC_CMD, EC_READ);
    if !ec_wait_ibf() { return None; }
    crate::port::outb(EC_DATA, reg);
    if !ec_wait_obf() { return None; }
    Some(crate::port::inb(EC_DATA))
}

unsafe fn acpi_battery_pct() -> u8 {
    // Can't use ? in non-Option context; inline the chain.
    let status = crate::port::inb(EC_CMD);
    if status == 0xFF { return 0xFF; }
    let Some(brc_hi) = ec_read(0x44) else { return 0xFF; };
    let Some(brc_lo) = ec_read(0x45) else { return 0xFF; };
    let Some(bfc_hi) = ec_read(0x2A) else { return 0xFF; };
    let Some(bfc_lo) = ec_read(0x2B) else { return 0xFF; };
    let remaining = ((brc_hi as u32) << 8) | brc_lo as u32;
    let full      = ((bfc_hi as u32) << 8) | bfc_lo as u32;
    if full == 0 || remaining > full * 2 { return 0xFF; }
    ((remaining * 100) / full).min(100) as u8
}

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
    // Linux ABI args 4–6 (r10/r8/r9) stashed for the current syscall.
    ".global _lx_a4",
    ".global _lx_a5",
    ".global _lx_a6",
    "_lx_a4: .quad 0",
    "_lx_a5: .quad 0",
    "_lx_a6: .quad 0",
    // User RIP (return address from syscall) — needed by clone() to set the
    // child thread's initial PC (the instruction after the `syscall` opcode).
    ".global _user_rip_save",
    "_user_rip_save: .quad 0",

    ".section .text",
    ".global syscall_entry",
    "syscall_entry:",

    // 1. Save user RSP, switch to kernel stack
    "mov qword ptr [rip + _user_rsp], rsp",
    "lea rsp, [rip + _syscall_kstack_top]",

    // 1b. Stash Linux args 4–6 (r10/r8/r9) and user RIP (rcx) before the
    //     Rust call clobbers them. Harmless for native syscalls.
    "mov qword ptr [rip + _lx_a4], r10",
    "mov qword ptr [rip + _lx_a5], r8",
    "mov qword ptr [rip + _lx_a6], r9",
    "mov qword ptr [rip + _user_rip_save], rcx",

    // 2. Save state. The Linux syscall ABI requires the kernel to PRESERVE all
    //    registers except rax (return), rcx and r11 (clobbered by SYSCALL).
    //    The C call to syscall_handler clobbers the caller-saved regs
    //    (rdi/rsi/rdx/r8/r9/r10), so we must save+restore them too — not just
    //    the callee-saved set — or Linux binaries get garbage regs back and
    //    crash. (Native apps don't care, so this is strictly safer for both.)
    "push r11",          // user RFLAGS
    "push rcx",          // user RIP
    "push rdi",          // Linux arg1 — preserve for caller
    "push rsi",          // Linux arg2
    "push rdx",          // Linux arg3
    "push r8",           // Linux arg5
    "push r9",           // Linux arg6
    "push r10",          // Linux arg4
    "push rbx",
    "push rbp",
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
    // rax = return value (kept; everything else restored below)

    // 4. Restore (reverse order)
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop rbp",
    "pop rbx",
    "pop r10",
    "pop r9",
    "pop r8",
    "pop rdx",
    "pop rsi",
    "pop rdi",
    "pop rcx",           // user RIP → rcx (consumed by sysretq)
    "pop r11",           // user RFLAGS → r11 (consumed by sysretq)

    // 5. Restore user RSP, return to ring-3
    "mov rsp, qword ptr [rip + _user_rsp]",
    "sysretq",
);

// ── Rust syscall dispatcher ──────────────────────────────────────────────────
#[no_mangle]
pub extern "C" fn syscall_handler(nr: u64, arg1: u64, arg2: u64, arg3: u64) -> u64 {
    // Linux-ABI processes use a different syscall table (Linux numbers collide
    // with the native ones, e.g. 9=mmap vs native sys_ps). Route them out.
    if crate::linux::is_linux() {
        return crate::linux::syscall(nr, arg1, arg2, arg3);
    }
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
        16 => {
            // sys_http_get(host_ptr, host_len | out_cap<<16, out_ptr)
            //   -> bytes written into out (0 on failure)
            // Fetches GET / over HTTP/1.0 via the kernel TCP/IP stack (brick 6).
            // host_len is the low 16 bits of arg2; the output capacity is the high
            // bits, so the kernel never writes past the caller's buffer.
            let host_ptr = arg1 as *const u8;
            let host_len = (arg2 & 0xFFFF) as usize;
            let out_cap  = (arg2 >> 16) as usize;
            let out_ptr  = arg3 as *mut u8;
            if host_len == 0 || host_len > 256 || out_cap == 0 || out_ptr.is_null() {
                return 0;
            }
            let host = unsafe { core::slice::from_raw_parts(host_ptr, host_len) };
            let host = match core::str::from_utf8(host) { Ok(s) => s, Err(_) => return 0 };
            let out = unsafe { core::slice::from_raw_parts_mut(out_ptr, out_cap) };
            crate::net::http_get(host, out).unwrap_or(0) as u64
        }
        17 => {
            // sys_audio_write(pcm_ptr, len, byte_offset) → bytes written
            // Writes raw 44.1 kHz stereo 16-bit PCM into the DMA ring buffer
            // at `byte_offset`. Use offset 0 or AUDIO_BYTES/2 to double-buffer.
            let ptr = arg1 as *const u8;
            let len = (arg2 as usize).min(0x2_0000);
            let off = arg3 as usize;
            if len == 0 || ptr.is_null() { return 0; }
            let data = unsafe { core::slice::from_raw_parts(ptr, len) };
            crate::hda::audio_write(data, off) as u64
        }
        18 => {
            // sys_audio_vol(level) → 0
            // Sets master volume 0–127.
            crate::hda::set_volume(arg1 as u8);
            0
        }
        24 => {
            // sys_yield — cooperative yield (no-op: single process)
            crate::sched::yield_();
            0
        }
        20 => {
            // sys_battery_pct → battery percentage 0-100, or 0xFF if unavailable.
            // Reads ACPI EC battery capacity via I/O port 0x66 (EC command) / 0x62 (EC data).
            // EC register 0x44 = remaining capacity (BRC), 0x2A = full charge capacity (BFC).
            // Not all ACPI ECs are at these ports — returns 0xFF if not responsive.
            unsafe { acpi_battery_pct() as u64 }
        }
        21 => {
            // sys_kbd_layout(arg1): 0 = query, 1 = set EN (QWERTY), 2 = set DE (QWERTZ).
            // Returns the current layout afterward: 0 = EN, 1 = DE.
            match arg1 {
                1 => crate::idt::set_layout_de(false),
                2 => crate::idt::set_layout_de(true),
                _ => {}
            }
            if crate::idt::layout_is_de() { 1 } else { 0 }
        }
        25 => {
            // sys_disk_write(name_ptr, name_len|(data_len<<16), data_ptr) → 0 on success, MAX on error
            // Persists a named file to the RPFS on-disk filesystem via the AHCI driver.
            let name_ptr  = arg1 as *const u8;
            let name_len  = (arg2 & 0xFF) as usize;
            let data_len  = ((arg2 >> 16) & 0xFFFF) as usize;
            let data_ptr  = arg3 as *const u8;
            if name_len == 0 || name_len > 51 || data_len > 65536 || name_ptr.is_null() || data_ptr.is_null() {
                return u64::MAX;
            }
            let name = unsafe { core::slice::from_raw_parts(name_ptr, name_len) };
            let data = unsafe { core::slice::from_raw_parts(data_ptr, data_len) };
            if crate::diskfs::write_file(name, data) { 0 } else { u64::MAX }
        }
        26 => {
            // sys_disk_read(name_ptr, name_len|(out_max<<16), out_ptr) → bytes_read or MAX on error
            // Reads a named file from the RPFS on-disk filesystem.
            let name_ptr  = arg1 as *const u8;
            let name_len  = (arg2 & 0xFF) as usize;
            let out_max   = ((arg2 >> 16) & 0xFFFF) as usize;
            let out_ptr   = arg3 as *mut u8;
            if name_len == 0 || name_len > 51 || out_max == 0 || name_ptr.is_null() || out_ptr.is_null() {
                return u64::MAX;
            }
            let name = unsafe { core::slice::from_raw_parts(name_ptr, name_len) };
            let out  = unsafe { core::slice::from_raw_parts_mut(out_ptr, out_max) };
            match crate::diskfs::read_file(name, out) {
                Some(n) => n as u64,
                None    => u64::MAX,
            }
        }
        28 => {
            // sys_exec_linux(path_ptr, path_len)
            // Suspend the native desktop and run a Linux ELF from the initrd.
            // Sets RESTART_DESKTOP so exit_group restarts the desktop when done.
            // Never returns on success (kernel IRETs into the Linux binary).
            let path_ptr = arg1 as *const u8;
            let path_len = (arg2 as usize).min(256);
            if path_ptr.is_null() || path_len == 0 { return u64::MAX; }
            let path = unsafe { core::slice::from_raw_parts(path_ptr, path_len) };
            let p = if path.starts_with(b"/") { &path[1..] } else { path };
            if let Some(elf) = crate::ramfs::find(p) {
                unsafe { crate::linux::RESTART_DESKTOP = true; }
                crate::linux::enter(elf);
            }
            u64::MAX // only reached if binary not found
        }
        29 => {
            // sys_initrd_read(path_ptr, path_len, out_ptr|(out_len<<32)) → bytes or MAX
            // arg3 packs: low 32 bits = out_ptr, high 32 bits = out_len (max 32 MiB).
            let path_ptr = arg1 as *const u8;
            let path_len = (arg2 as usize).min(256);
            let out_ptr  = (arg3 & 0xFFFF_FFFF) as *mut u8;
            let out_len  = ((arg3 >> 32) as usize).min(32 * 1024 * 1024);
            if path_ptr.is_null() || out_ptr.is_null() || path_len == 0 || out_len == 0 {
                return u64::MAX;
            }
            let path = unsafe { core::slice::from_raw_parts(path_ptr, path_len) };
            let p = if path.starts_with(b"/") { &path[1..] } else { path };
            match crate::ramfs::find(p) {
                Some(data) => {
                    let n = data.len().min(out_len);
                    unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), out_ptr, n); }
                    n as u64
                }
                None => u64::MAX,
            }
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
