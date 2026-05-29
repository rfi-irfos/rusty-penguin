//! Linux ABI compatibility layer — the bridge that lets the from-scratch
//! pure-Rust Rusty Penguin kernel execute *unmodified Linux x86-64 binaries*.
//!
//! This is the long road (a Linux ABI is ~400 syscalls with exact semantics),
//! built brick by brick. Brick 1: run a freestanding static Linux ELF that
//! makes raw `write`/`exit_group` syscalls. Later bricks: TLS + the SysV
//! initial stack (here), brk/mmap, a static musl libc, dynamic linking, …
//!
//! Design: a process runs in one of two ABI modes (see `AbiMode`). The native
//! Rusty Penguin apps use the custom syscall numbers in `syscall.rs`; Linux
//! binaries are routed here instead, because Linux's numbers (e.g. 9=mmap,
//! 12=brk) collide with the native table. The syscall result is modelled as a
//! ternary `Trit`: +1 ok / 0 would-block (EAGAIN) / -1 error (negative errno).

use crate::vga;
use crate::serial;
use ternary_core::Trit;

// ── Per-process ABI mode ─────────────────────────────────────────────────────
static mut LINUX_ABI: bool = false;

// Args 4–6 of the *current* syscall. Linux passes them in r10/r8/r9; the asm
// trampoline in syscall.rs stashes them into these .data globals before the
// Rust call clobbers the registers. Safe because syscalls are serialised
// (FMASK clears IF on entry; single-CPU kernel).
extern "C" {
    static _lx_a4: u64;
    static _lx_a5: u64;
    static _lx_a6: u64;
}

#[inline]
pub fn is_linux() -> bool { unsafe { LINUX_ABI } }

#[inline]
fn extra_args() -> (u64, u64, u64) {
    unsafe {
        (
            core::ptr::read_volatile(&_lx_a4),
            core::ptr::read_volatile(&_lx_a5),
            core::ptr::read_volatile(&_lx_a6),
        )
    }
}

// ── Errno (Linux returns negative errno in rax) ──────────────────────────────
const ENOSYS: i64 = -38;
const EBADF:  i64 = -9;
fn errno(e: i64) -> u64 { e as u64 }

// ── brk / mmap arenas (identity-mapped; see enter()'s map extension) ──────────
// Crude bump arenas — good enough for a static musl process. A real per-process
// VMM with demand paging is a later brick.
const BRK_BASE:  u64 = 0x0700_0000; // 112 MiB
const BRK_CAP:   u64 = 0x0800_0000; // 128 MiB
const MMAP_BASE: u64 = 0x0800_0000; // 128 MiB
const MMAP_CAP:  u64 = 0x1000_0000; // 256 MiB
static mut BRK_CUR:  u64 = BRK_BASE;
static mut MMAP_CUR: u64 = MMAP_BASE;

const PAGE: u64 = 4096;
fn page_up(n: u64) -> u64 { (n + PAGE - 1) & !(PAGE - 1) }

// MSR for the FS base (TLS) — set by arch_prctl(ARCH_SET_FS).
const IA32_FS_BASE: u32 = 0xC000_0100;
const ARCH_SET_FS: u64 = 0x1002;
unsafe fn wrmsr(msr: u32, val: u64) {
    core::arch::asm!("wrmsr", in("ecx") msr,
        in("eax") val as u32, in("edx") (val >> 32) as u32, options(nostack));
}

// ── Open-file table: Linux fd → ramfs-backed file ────────────────────────────
// Read-only files served from the initrd. Foundation for dynamic linking
// (ld.so openat()s + mmap()s the interpreter and shared libraries).
#[derive(Clone, Copy)]
struct OpenFile { used: bool, data: *const u8, size: usize, off: usize }
const FD_BASE: usize = 3;          // 0/1/2 = stdio
const MAX_OPEN: usize = 32;
static mut FDS: [OpenFile; MAX_OPEN] =
    [OpenFile { used: false, data: core::ptr::null(), size: 0, off: 0 }; MAX_OPEN];

/// Read a NUL-terminated user C string (bounded).
unsafe fn cstr(ptr: u64) -> &'static [u8] {
    if ptr == 0 { return &[]; }
    let p = ptr as *const u8;
    let mut n = 0usize;
    while n < 4096 && *p.add(n) != 0 { n += 1; }
    core::slice::from_raw_parts(p, n)
}

/// Open a ramfs file by path → fd, or negative errno.
fn open_path(path: &[u8]) -> u64 {
    let p = if path.starts_with(b"/") { &path[1..] } else { path };
    if TRACE {
        serial::write_str("  [open] ");
        for &b in path { serial::write_byte(b); }
    }
    match crate::ramfs::find(p) {
        Some(data) => unsafe {
            if TRACE { serial::write_str(" -> OK\n"); }
            for i in 0..MAX_OPEN {
                if !FDS[i].used {
                    FDS[i] = OpenFile { used: true, data: data.as_ptr(), size: data.len(), off: 0 };
                    return (FD_BASE + i) as u64;
                }
            }
            errno(-24) // EMFILE
        },
        None => { if TRACE { serial::write_str(" -> ENOENT\n"); } errno(-2) }
    }
}

fn fd_slot(fd: u64) -> Option<usize> {
    let fd = fd as usize;
    if fd >= FD_BASE && fd < FD_BASE + MAX_OPEN && unsafe { FDS[fd - FD_BASE].used } {
        Some(fd - FD_BASE)
    } else { None }
}

/// Fill a Linux `struct stat` (x86-64, 144 bytes). `ino` MUST be unique per
/// file: ld.so deduplicates loaded objects by (st_dev, st_ino), so identical
/// inodes make it skip mapping a library it thinks is already loaded.
unsafe fn fill_stat(sb: *mut u8, is_reg: bool, size: u64, ino: u64) {
    if sb.is_null() { return; }
    core::ptr::write_bytes(sb, 0, 144);
    const S_IFREG: u32 = 0x8000;
    const S_IFCHR: u32 = 0x2000;
    let mode = if is_reg { S_IFREG | 0o644 } else { S_IFCHR | 0o666 };
    (sb as *mut u64).write_unaligned(1);                // st_dev = 1
    (sb.add(8)  as *mut u64).write_unaligned(ino);      // st_ino (unique per file)
    (sb.add(16) as *mut u64).write_unaligned(1);        // st_nlink
    (sb.add(24) as *mut u32).write_unaligned(mode);     // st_mode
    (sb.add(48) as *mut u64).write_unaligned(size);     // st_size
    (sb.add(56) as *mut u64).write_unaligned(4096);     // st_blksize
}

// ── The Linux syscall dispatcher ─────────────────────────────────────────────
/// Returns the Linux ABI value for rax (>=0 success, <0 negated errno).
/// Per-syscall serial trace toggle (debugging the ABI layer brick by brick).
const TRACE: bool = false;
fn dbg_hex(tag: &str, v: u64) {
    serial::write_str(tag);
    let mut i = 60i32;
    serial::write_str("0x");
    let mut started = false;
    while i >= 0 {
        let nib = ((v >> i) & 0xF) as u8;
        if nib != 0 || started || i == 0 { started = true;
            serial::write_byte(if nib < 10 { b'0' + nib } else { b'a' + nib - 10 }); }
        i -= 4;
    }
}

pub fn syscall(nr: u64, a1: u64, a2: u64, a3: u64) -> u64 {
    let (a4, a5, a6) = extra_args();
    let _ = (a5, a6);
    if TRACE {
        dbg_hex("  [sc] nr=", nr); dbg_hex(" a1=", a1); dbg_hex(" a2=", a2);
        dbg_hex(" a3=", a3); dbg_hex(" a4=", a4); serial::write_byte(b'\n');
    }
    let ret: u64 = match nr {
        // read(fd, buf, len) — ramfs-backed files; stdin → EOF.
        0 => unsafe {
            if let Some(i) = fd_slot(a1) {
                let avail = FDS[i].size.saturating_sub(FDS[i].off);
                let n = (a3 as usize).min(avail);
                if n > 0 {
                    core::ptr::copy_nonoverlapping(FDS[i].data.add(FDS[i].off), a2 as *mut u8, n);
                    FDS[i].off += n;
                }
                n as u64
            } else { 0 }
        }
        // openat(dirfd, path, flags, mode) / open(path, flags, mode).
        257 => open_path(unsafe { cstr(a2) }),
        2   => open_path(unsafe { cstr(a1) }),
        // access(path, mode) / faccessat(dirfd, path, mode, flags): exists?
        21 => { let pp = unsafe { cstr(a1) }; let p = if pp.starts_with(b"/") { &pp[1..] } else { pp };
                if crate::ramfs::find(p).is_some() { 0 } else { errno(-2) } }
        269 => { let pp = unsafe { cstr(a2) }; let p = if pp.starts_with(b"/") { &pp[1..] } else { pp };
                if crate::ramfs::find(p).is_some() { 0 } else { errno(-2) } }
        // close(fd).
        3 => { if let Some(i) = fd_slot(a1) { unsafe { FDS[i].used = false; } } 0 }
        // lseek(fd, off, whence): 0=SET 1=CUR 2=END.
        8 => unsafe {
            if let Some(i) = fd_slot(a1) {
                let no = match a3 {
                    0 => a2 as usize,
                    1 => FDS[i].off.wrapping_add(a2 as usize),
                    2 => FDS[i].size.wrapping_add(a2 as usize),
                    _ => FDS[i].off,
                };
                FDS[i].off = no.min(FDS[i].size);
                FDS[i].off as u64
            } else { errno(EBADF) }
        }
        // pread64(fd, buf, count, pos).
        17 => unsafe {
            if let Some(i) = fd_slot(a1) {
                let pos = a4 as usize;
                let n = (a3 as usize).min(FDS[i].size.saturating_sub(pos));
                if n > 0 { core::ptr::copy_nonoverlapping(FDS[i].data.add(pos), a2 as *mut u8, n); }
                n as u64
            } else { errno(EBADF) }
        }
        // newfstatat(dirfd, path, statbuf, flags): path-based or AT_EMPTY_PATH.
        262 => unsafe {
            let path = cstr(a2);
            if (a4 & 0x1000) != 0 || path.is_empty() {
                match fd_slot(a1) {
                    Some(i) => { fill_stat(a3 as *mut u8, true, FDS[i].size as u64, FDS[i].data as u64); 0 }
                    None => { fill_stat(a3 as *mut u8, false, 0, 0); 0 }
                }
            } else {
                let p = if path.starts_with(b"/") { &path[1..] } else { path };
                match crate::ramfs::find(p) {
                    Some(d) => { fill_stat(a3 as *mut u8, true, d.len() as u64, d.as_ptr() as u64); 0 }
                    None => errno(-2),
                }
            }
        }
        // write(fd, buf, len) → console + serial (so headless boots are captured).
        1 => {
            let len = (a3 as usize).min(1 << 20);
            let ptr = a2 as *const u8;
            let bytes = unsafe { core::slice::from_raw_parts(ptr, len) };
            // vga::write_byte already mirrors to serial — don't double-emit.
            for &b in bytes { vga::write_byte(b, vga::Color::White); }
            len as u64
        }
        // writev(fd, iov, iovcnt) — array of {base, len}.
        20 => {
            let iov = a2 as *const u64; // pairs: [base, len]
            let cnt = (a3 as usize).min(1024);
            let mut total = 0u64;
            for i in 0..cnt {
                let base = unsafe { *iov.add(i * 2) } as *const u8;
                let len  = unsafe { *iov.add(i * 2 + 1) } as usize;
                if len == 0 { continue; }
                let bytes = unsafe { core::slice::from_raw_parts(base, len.min(1 << 20)) };
                for &b in bytes { vga::write_byte(b, vga::Color::White); }
                total += len as u64;
            }
            total
        }
        // brk(addr): bump program break within [BRK_BASE, BRK_CAP]. Linux
        // guarantees freshly-broken pages read as zero, and glibc's heap/TLS
        // structures (exit-handler list, tls_dtor_list) depend on it — so zero
        // any newly-exposed region or the exit path calls a garbage pointer.
        12 => unsafe {
            if a1 == 0 { return BRK_CUR; }
            if a1 >= BRK_BASE && a1 <= BRK_CAP {
                if a1 > BRK_CUR {
                    core::ptr::write_bytes(BRK_CUR as *mut u8, 0, (a1 - BRK_CUR) as usize);
                }
                BRK_CUR = a1; a1
            } else { BRK_CUR }
        }
        // mmap(addr, len, prot, flags, fd, off): bump-allocate; file-backed if
        // a real fd is given (a5=fd, a6=offset) — needed for dynamic linking,
        // where ld.so mmaps segments of the interpreter and shared libraries.
        9 => unsafe {
            let len = page_up(a2);
            let fixed = (a4 & 0x10) != 0;                       // MAP_FIXED
            let anon  = (a4 & 0x20) != 0 || a5 == u64::MAX;     // MAP_ANONYMOUS | fd==-1
            // Honor MAP_FIXED — ld.so reserves the span then maps each library
            // segment at base+offset; ignoring the address scatters the segments
            // and breaks symbol tables. Otherwise bump-allocate from the arena.
            let p = if fixed && a1 != 0 {
                a1
            } else if MMAP_CUR + len <= MMAP_CAP {
                let q = MMAP_CUR; MMAP_CUR += len; q
            } else {
                return errno(-12); // ENOMEM
            };
            if anon {
                core::ptr::write_bytes(p as *mut u8, 0, len as usize);
            } else if let Some(i) = fd_slot(a5) {
                let off  = a6 as usize;
                let copy = FDS[i].size.saturating_sub(off).min(a2 as usize);
                if copy > 0 {
                    core::ptr::copy_nonoverlapping(FDS[i].data.add(off), p as *mut u8, copy);
                }
                // zero the bss tail within this (page-aligned, non-overlapping) segment
                if (len as usize) > copy {
                    core::ptr::write_bytes((p + copy as u64) as *mut u8, 0, len as usize - copy);
                }
            }
            p
        }
        // fstat(fd, statbuf): regular file (real fd) or char device (stdio).
        5 => unsafe {
            match fd_slot(a1) {
                Some(i) => fill_stat(a2 as *mut u8, true, FDS[i].size as u64, FDS[i].data as u64),
                None    => fill_stat(a2 as *mut u8, false, 0, 0),
            }
            0
        }
        // prlimit64(pid, res, new, old): report no limits (RLIM_INFINITY).
        302 => {
            let old = a4 as *mut u64;
            if !old.is_null() {
                unsafe { *old = u64::MAX; *old.add(1) = u64::MAX; } // rlim_cur, rlim_max
            }
            0
        }
        // mprotect / munmap — no-op (bump arena never frees).
        10 | 11 => 0,
        // ioctl — pretend success (e.g. isatty/TCGETS probes).
        16 => 0,
        // arch_prctl(code, addr): set the FS base for TLS.
        158 => unsafe {
            if a1 == ARCH_SET_FS { wrmsr(IA32_FS_BASE, a2); 0 } else { errno(ENOSYS) }
        }
        // set_tid_address → fake tid; set_robust_list / rt_sig* → success stubs.
        218 => 1,
        273 | 13 | 14 => 0,
        // clock_gettime(clk, *timespec) from the 100 Hz tick counter.
        228 => {
            let ts = a2 as *mut u64;
            if !ts.is_null() {
                let ticks = crate::idt::ticks();
                unsafe {
                    *ts = ticks / 100;                       // tv_sec
                    *ts.add(1) = (ticks % 100) * 10_000_000; // tv_nsec (10 ms/tick)
                }
            }
            0
        }
        // getrandom(buf, len, flags): cheap tick-seeded LCG (not crypto).
        318 => {
            let buf = a1 as *mut u8;
            let len = a2 as usize;
            let mut x = crate::idt::ticks().wrapping_mul(6364136223846793005).wrapping_add(1);
            for i in 0..len {
                x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                unsafe { *buf.add(i) = (x >> 33) as u8; }
            }
            len as u64
        }
        // exit / exit_group → end the process.
        60 | 231 => {
            vga::write_str("\n  [linux] process exited, code=", vga::Color::Green);
            vga::write_i32(a1 as i32);
            vga::write_byte(b'\n', vga::Color::White);
            serial::write_str("\n  [linux] exit code=");
            serial::write_byte(b'0' + ((a1 % 10) as u8));
            serial::write_byte(b'\n');
            loop { unsafe { core::arch::asm!("hlt", options(nostack)); } }
        }
        // Unimplemented — report it on serial so the next brick knows what's missing.
        _ => {
            serial::write_str("  [linux] ENOSYS nr=");
            let mut n = nr; let mut b = [0u8; 20]; let mut i = 0;
            if n == 0 { serial::write_byte(b'0'); }
            while n > 0 { b[i] = b'0' + (n % 10) as u8; n /= 10; i += 1; }
            while i > 0 { i -= 1; serial::write_byte(b[i]); }
            serial::write_byte(b'\n');
            let _ = (a4, EBADF);
            errno(ENOSYS)
        }
    };
    // Ternary model of the syscall outcome — the lens the whole OS is built on,
    // applied at the syscall boundary: +1 ok / 0 empty-or-EAGAIN / -1 errno.
    let _outcome = if (ret as i64) < 0 { Trit::Neg }
                   else if ret == 0 { Trit::Zero }
                   else { Trit::Pos };
    let _ = _outcome;
    ret
}

// ── Load + enter an unmodified Linux ELF in ring-3 ───────────────────────────
/// Build the System V AMD64 initial process stack and IRETQ into the binary.
/// Never returns — the process runs until it calls exit/exit_group.
pub fn enter(elf: &[u8]) -> ! {
    // Linux brk/mmap arenas live up to 256 MiB — make sure they're mapped.
    crate::vmm::extend_identity_map(256);

    // Dynamic linker base (ET_DYN). 8 MiB: above the program (~4 MiB), below the
    // relocated initrd (40 MiB). The interpreter is small (~0.25 MiB).
    const INTERP_BASE: u64 = 0x0080_0000;

    // If the program is dynamically linked, load the interpreter (ld.so) and
    // jump to IT; the program's own entry/phdrs go in the auxv (AT_ENTRY/AT_PHDR)
    // and ld.so relocates the program + maps shared libraries from the initrd.
    let (entry, at_base, at_entry) = match crate::elf::interp_path(elf) {
        Some(ipath) => {
            let prog_entry = crate::elf::load(elf).expect("program load failed");
            let p = if ipath.starts_with(b"/") { &ipath[1..] } else { ipath };
            let interp_elf = crate::ramfs::find(p).expect("dynamic linker not in initrd");
            let interp_entry = crate::elf::load_bias(interp_elf, INTERP_BASE)
                .expect("interpreter load failed");
            serial::write_str("  [linux] dynamic: ld.so loaded; relocating program\n");
            (interp_entry, INTERP_BASE, prog_entry)
        }
        None => {
            let e = crate::elf::load(elf).expect("linux ELF parse failed");
            (e, 0, e)
        }
    };

    // ── System V AMD64 initial stack ──
    // RSP → argc, argv[]·NULL, envp[]·NULL, auxv pairs·{AT_NULL,0}.
    const AT_NULL: u64 = 0; const AT_PHDR: u64 = 3; const AT_PHENT: u64 = 4;
    const AT_PHNUM: u64 = 5; const AT_PAGESZ: u64 = 6; const AT_BASE: u64 = 7;
    const AT_ENTRY: u64 = 9; const AT_RANDOM: u64 = 25; const AT_EXECFN: u64 = 31;
    let (phdr, phent, phnum) = crate::elf::phdr_info(elf).unwrap_or((0, 56, 0));

    let top = crate::vmm::USER_STACK_TOP;
    // argv[0] string, then 16 random bytes (AT_RANDOM canary source).
    let argv0 = b"/bin/linuxtest\0";
    let mut sp = top - argv0.len() as u64;
    let argv0_ptr = sp;
    unsafe { for (i, &b) in argv0.iter().enumerate() { *((argv0_ptr + i as u64) as *mut u8) = b; } }
    sp = (sp - 16) & !0xF;
    let rand_ptr = sp;
    unsafe {
        let mut x = crate::idt::ticks().wrapping_mul(2862933555777941757).wrapping_add(3037000493);
        for i in 0..16u64 { x = x.wrapping_mul(6364136223846793005).wrapping_add(1); *((rand_ptr + i) as *mut u8) = (x >> 40) as u8; }
    }

    let words: [u64; 22] = [
        1,                       // argc
        argv0_ptr,               // argv[0]
        0,                       // argv[1] = NULL
        0,                       // envp[0] = NULL
        AT_PHDR,   phdr,
        AT_PHENT,  phent,
        AT_PHNUM,  phnum,
        AT_PAGESZ, 4096,
        AT_BASE,   at_base,      // interpreter base (0 if static)
        AT_ENTRY,  at_entry,     // the PROGRAM's entry (not ld.so's)
        AT_RANDOM, rand_ptr,
        AT_EXECFN, argv0_ptr,
        AT_NULL,   0,
    ];
    let bytes = (words.len() * 8) as u64;
    let mut asp = (rand_ptr - bytes) & !0xF; // 16-byte align the argc slot
    let user_rsp = asp;
    unsafe {
        for w in words.iter() { *(asp as *mut u64) = *w; asp += 8; }
    }

    unsafe { LINUX_ABI = true; }

    vga::write_str("  [linux] entering ring-3 @ 0x", vga::Color::Cyan);
    vga::write_hex(entry, vga::Color::Cyan);
    vga::write_byte(b'\n', vga::Color::White);
    serial::write_str("  [linux] IRETQ into unmodified Linux ELF\n");

    // IRETQ into ring-3 (same selectors as the native launch in main.rs).
    // CRITICAL: Linux zeroes the general-purpose registers at exec — in
    // particular RDX must be 0 (it conventionally carries a function pointer the
    // program registers with atexit; for a static binary there is none). If we
    // leave garbage kernel registers, glibc atexit-registers a bogus rtld_fini
    // and jumps to it at exit (a #GP to a junk address). So zero all GP regs
    // after building the iretq frame.
    unsafe {
        core::arch::asm!(
            "push 0x1B",      // SS = UDATA | 3
            "push r9",        // user RSP (→ argc)
            "pushfq",
            "pop rax",
            "or  rax, 0x202", // IF=1
            "push rax",
            "push 0x23",      // CS = UCODE | 3
            "push r8",        // user RIP = entry
            // iretq frame is fully on the stack now — clear all GP registers.
            "xor rax, rax", "xor rbx, rbx", "xor rcx, rcx", "xor rdx, rdx",
            "xor rsi, rsi", "xor rdi, rdi", "xor rbp, rbp",
            "xor r8, r8",   "xor r9, r9",   "xor r10, r10", "xor r11, r11",
            "xor r12, r12", "xor r13, r13", "xor r14, r14", "xor r15, r15",
            "iretq",
            in("r8") entry,
            in("r9") user_rsp,
            options(noreturn),
        );
    }
}
