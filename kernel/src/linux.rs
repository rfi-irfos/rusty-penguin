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
// Set when a Linux app puts the console keyboard in K_RAW/K_MEDIUMRAW mode
// (fbDOOM does). The PS/2 IRQ then delivers raw scancodes instead of ASCII.
static mut RAW_KBD: bool = false;
pub fn raw_kbd() -> bool { unsafe { RAW_KBD } }

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
// The initrd is relocated to 128 MiB (main.rs INITRD_RELOC). A dynamic binary's
// ld.so mmaps libc/the PIE into this arena, so it MUST start above the initrd or
// it overwrites the very files the program is reading (this clobbered DOOM1.WAD,
// breaking real fbDOOM). The Linux-track initrd is small (~8 MiB → ≤136 MiB), so
// 160 MiB clears it with margin while staying under the 256 MiB identity map.
const MMAP_BASE: u64 = 0x0A00_0000; // 160 MiB
const MMAP_CAP:  u64 = 0x1000_0000; // 256 MiB
static mut BRK_CUR:  u64 = BRK_BASE;
static mut MMAP_CUR: u64 = MMAP_BASE;

// ── Linux stdin key ring (populated by the PS/2 IRQ; served by read(0,…)) ────
const LKEYS_SZ: usize = 64;
static mut LKEYS:   [u8; LKEYS_SZ] = [0u8; LKEYS_SZ];
static mut LKEYS_H: usize = 0;  // write index
static mut LKEYS_T: usize = 0;  // read index

/// Push an ASCII byte into the Linux-process stdin buffer (called from IRQ).
pub fn push_linux_key(b: u8) {
    if b == 0 { return; }
    unsafe {
        let next = (LKEYS_H + 1) % LKEYS_SZ;
        if next != LKEYS_T { LKEYS[LKEYS_H] = b; LKEYS_H = next; }
    }
}

fn pop_linux_key() -> Option<u8> {
    unsafe {
        if LKEYS_H == LKEYS_T { return None; }
        let b = LKEYS[LKEYS_T];
        LKEYS_T = (LKEYS_T + 1) % LKEYS_SZ;
        Some(b)
    }
}

/// When true, exit_group restarts desktop-metal instead of halting.
pub static mut RESTART_DESKTOP: bool = false;

/// Reset all per-process Linux ABI state (called before restarting desktop).
pub fn reset() {
    unsafe {
        LINUX_ABI = false;
        RAW_KBD   = false;
        BRK_CUR   = BRK_BASE;
        MMAP_CUR  = MMAP_BASE;
        LKEYS_H   = 0;
        LKEYS_T   = 0;
        RESTART_DESKTOP = false;
        for fd in FDS.iter_mut() { fd.used = false; }
    }
}

/// Virtual fd sentinel for /dev/fb0 (not a real FDS slot — handled specially).
const DEV_FB_FD: u64 = 200;

const PAGE: u64 = 4096;
fn page_up(n: u64) -> u64 { (n + PAGE - 1) & !(PAGE - 1) }

// ── Thread TID allocator (minimal, no real scheduling) ───────────────────────
static mut NEXT_TID: u32 = 2;  // 1 is the main thread; children get 2,3,…
unsafe fn alloc_tid() -> u32 { let t = NEXT_TID; NEXT_TID = NEXT_TID.wrapping_add(1); t }

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
    // Special device nodes
    if p == b"dev/fb0" || p == b"dev/tty" || p == b"dev/tty0" { return DEV_FB_FD; }
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

/// Write a byte string into a fixed-size field at `base+off` (for utsname etc.).
unsafe fn wstr(base: *mut u8, off: usize, s: &[u8]) {
    for (i, &c) in s.iter().enumerate() { *base.add(off + i) = c; }
}

// ── The Linux syscall dispatcher ─────────────────────────────────────────────
/// Returns the Linux ABI value for rax (>=0 success, <0 negated errno).
/// Per-syscall serial trace toggle (debugging the ABI layer brick by brick).
const TRACE: bool = true;
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
        // read(fd, buf, len) — ramfs files or stdin from the Linux key ring.
        0 => unsafe {
            if let Some(i) = fd_slot(a1) {
                let avail = FDS[i].size.saturating_sub(FDS[i].off);
                let n = (a3 as usize).min(avail);
                if n > 0 {
                    core::ptr::copy_nonoverlapping(FDS[i].data.add(FDS[i].off), a2 as *mut u8, n);
                    FDS[i].off += n;
                }
                n as u64
            } else if a1 <= 2 {
                // stdin (fd=0): pop from Linux key ring; stdout/stderr reads→ 0.
                if a1 != 0 { return 0; }
                let buf = a2 as *mut u8;
                let cap = (a3 as usize).min(256);
                let mut n = 0usize;
                while n < cap {
                    match pop_linux_key() {
                        Some(b) => { *buf.add(n) = b; n += 1; }
                        None => break,
                    }
                }
                if n == 0 { errno(-11) } else { n as u64 }  // EAGAIN if no keys
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
            // /dev/fb0 mmap: return physical framebuffer address directly.
            // The FB is identity-mapped by enter() via map_mmio_range, so
            // virt == phys and ring-3 can write pixels straight to hardware.
            if a5 == DEV_FB_FD { return crate::fb::base() as u64; }

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
        // ── common identity / time / info syscalls real programs expect ──
        39 | 110 | 186 => 1,                 // getpid / getppid / gettid
        102 | 104 | 107 | 108 => 0,          // getuid / getgid / geteuid / getegid (root)
        // uname(utsname): 6 × 65-byte fields.
        63 => unsafe {
            let b = a1 as *mut u8;
            if !b.is_null() {
                core::ptr::write_bytes(b, 0, 6 * 65);
                wstr(b, 0,   b"Linux");
                wstr(b, 65,  b"rusty-penguin");
                wstr(b, 130, b"6.0.0-rustypenguin");
                wstr(b, 195, b"#1 Rusty Penguin bare-metal ternary");
                wstr(b, 260, b"x86_64");
                wstr(b, 325, b"(none)");
            }
            0
        }
        // getcwd(buf, size) → "/" (returns length incl. NUL).
        79 => unsafe {
            let buf = a1 as *mut u8;
            if !buf.is_null() && a2 >= 2 { *buf = b'/'; *buf.add(1) = 0; }
            2
        }
        // gettimeofday(tv, tz) from the 100 Hz tick counter.
        96 => unsafe {
            let tv = a1 as *mut u64;
            if !tv.is_null() { let t = crate::idt::ticks(); *tv = t / 100; *tv.add(1) = (t % 100) * 10_000; }
            0
        }
        // nanosleep / clock_nanosleep → no-op success (no fine timer yet).
        35 | 230 => 0,
        // sched_getaffinity(pid, size, mask) → single CPU.
        204 => unsafe { let m = a3 as *mut u8; if !m.is_null() && a2 >= 1 { *m = 1; } 8 }
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
        // ioctl — handle /dev/fb0 queries; pretend success for everything else.
        16 => {
            if a1 == DEV_FB_FD {
                const FBIOGET_VSCREENINFO: u64 = 0x4600;
                const FBIOGET_FSCREENINFO: u64 = 0x4602;
                match a2 {
                    FBIOGET_VSCREENINFO => unsafe {
                        // struct fb_var_screeninfo (160 bytes, kernel abi)
                        let p = a3 as *mut u8;
                        if p.is_null() { return 0; }
                        core::ptr::write_bytes(p, 0, 160);
                        let fw = crate::fb::width();
                        let fh = crate::fb::height();
                        (p as *mut u32).write_unaligned(fw);         // xres
                        (p.add(4)  as *mut u32).write_unaligned(fh); // yres
                        (p.add(8)  as *mut u32).write_unaligned(fw); // xres_virtual
                        (p.add(12) as *mut u32).write_unaligned(fh); // yres_virtual
                        // xoffset=0, yoffset=0 already zero
                        (p.add(24) as *mut u32).write_unaligned(32); // bits_per_pixel
                        // red.offset=16 .length=8  [bytes 32–43]
                        (p.add(32) as *mut u32).write_unaligned(16);
                        (p.add(36) as *mut u32).write_unaligned(8);
                        // green.offset=8 .length=8  [bytes 44–55]
                        (p.add(44) as *mut u32).write_unaligned(8);
                        (p.add(48) as *mut u32).write_unaligned(8);
                        // blue.offset=0 .length=8  [bytes 56–67]
                        (p.add(56) as *mut u32).write_unaligned(0);
                        (p.add(60) as *mut u32).write_unaligned(8);
                        0
                    }
                    FBIOGET_FSCREENINFO => unsafe {
                        // struct fb_fix_screeninfo (68 bytes)
                        let p = a3 as *mut u8;
                        if p.is_null() { return 0; }
                        core::ptr::write_bytes(p, 0, 68);
                        let id = b"VESA VGA\0\0\0\0\0\0\0\0";
                        core::ptr::copy_nonoverlapping(id.as_ptr(), p, 16);
                        // smem_start at offset 16 (unsigned long = u64 on x86-64)
                        (p.add(16) as *mut u64).write_unaligned(crate::fb::base() as u64);
                        // smem_len at offset 24
                        let fb_len = crate::fb::pitch() as u64 * crate::fb::height() as u64;
                        (p.add(24) as *mut u32).write_unaligned(fb_len as u32);
                        // visual=FB_VISUAL_TRUECOLOR(2) at offset 36
                        (p.add(36) as *mut u32).write_unaligned(2);
                        // line_length at offset 48
                        (p.add(48) as *mut u32).write_unaligned(crate::fb::pitch());
                        0
                    }
                    _ => 0,
                }
            } else {
                // KDGKBTYPE: console apps (DOOM) probe for a keyboard by asking
                // the console keyboard type on each fd. Report KB_101 and succeed
                // so the app accepts this fd as its keyboard — we then feed its
                // read(0) from the PS/2 key ring. Without this, fbDOOM prints
                // "Unable to find a file descriptor associated with the keyboard"
                // and exits right after I_InitGraphics.
                const KDGKBTYPE: u64 = 0x4B33;
                const KB_101: u8 = 0x02;
                const KDSKBMODE: u64 = 0x4B45;
                // K_RAW=0, K_XLATE=1, K_MEDIUMRAW=2, K_UNICODE=3.
                if a2 == KDGKBTYPE && a3 != 0 {
                    unsafe { *(a3 as *mut u8) = KB_101; }
                } else if a2 == KDSKBMODE {
                    unsafe { RAW_KBD = a3 == 0 || a3 == 2; } // raw / medium-raw
                }
                0
            }
        }
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
        // ── Brick 5: threads ─────────────────────────────────────────────────
        // clone(flags, child_stack, ptid, ctid, tls)
        // CLONE_THREAD cooperative stub: assigns a tid, wires SETTID/SETTLS flags,
        // but does NOT create a real concurrent thread (single-CPU kernel, no
        // preemptive scheduler yet). Programs that only use threads for internal
        // glibc machinery (malloc, stdio) will work fine — those locks are never
        // contended when only one logical thread is running.
        56 => unsafe {
            let flags = a1;
            let ptid  = a3 as *mut u32;   // CLONE_PARENT_SETTID target
            let ctid  = a4 as *mut u32;   // CLONE_CHILD_SETTID/CLEARTID target
            let tls   = a5;               // CLONE_SETTLS: new FS base for child
            let tid   = alloc_tid();
            const CLONE_PARENT_SETTID: u64 = 0x0010_0000;
            const CLONE_CHILD_SETTID:  u64 = 0x0100_0000;
            if flags & CLONE_PARENT_SETTID != 0 && !ptid.is_null() { *ptid = tid; }
            if flags & CLONE_CHILD_SETTID  != 0 && !ctid.is_null() { *ctid = tid; }
            // CLONE_SETTLS: record the child's TLS base but do NOT apply it to the
            // parent's FS — the child thread (once we schedule it) sets its own FS.
            // For now just log it so we know when programs use it.
            if flags & 0x0008_0000 != 0 && tls != 0 {
                serial::write_str("  [clone] SETTLS tls=0x");
                serial::write_hex_u32(tls as u32);
                serial::write_byte(b'\n');
            }
            tid as u64
        }
        // futex(uaddr, op, val, timeout, uaddr2, val3)
        // Minimal brick-5 implementation: WAIT and WAKE are stubs that return
        // immediately. This is correct for single-threaded-in-practice programs
        // (glibc's internal locks are never contended when one thread runs).
        // FUTEX_WAIT: if *uaddr != val → EAGAIN (value already changed, retry)
        //             if *uaddr == val → return 0 (spurious wakeup, retry CAS)
        202 => unsafe {
            let op  = (a2 & 0x7F) as u32;  // strip FUTEX_PRIVATE_FLAG (128)
            let val = a3 as u32;
            match op {
                0 => { // FUTEX_WAIT
                    let cur = *(a1 as *const u32);
                    if cur != val { errno(-11) } else { 0 } // EAGAIN if changed
                }
                1 => 0, // FUTEX_WAKE → 0 threads woken
                _ => 0, // other ops: stub success
            }
        }
        // mprotect(addr, len, prot) → no-op success. We use a flat RW identity
        // map and don't enforce page permissions, so RELRO/GOT-hardening calls
        // from ld.so just succeed. (Returning ENOSYS makes glibc abort on RELRO.)
        10 => 0,
        // munmap(addr, len) → no-op success. The mmap arena is a bump allocator
        // that never reclaims; pretending the unmap worked is harmless.
        11 => 0,
        // ── File descriptor ops ───────────────────────────────────────────────
        // poll(fds, nfds, timeout) → 0 (no events, non-blocking stub).
        7 => 0,
        // ppoll(fds, nfds, tmo_p, sigmask, sigsetsize) → 0.
        271 => 0,
        // epoll_create / epoll_create1 → fake fd 100.
        213 | 291 => 100,
        // epoll_ctl → success.
        233 => 0,
        // epoll_wait / epoll_pwait → 0 events.
        232 | 281 => 0,
        // pipe2(fds, flags) → two fake readable/writable ends (fd 101/102).
        293 => unsafe {
            let fds = a1 as *mut u32;
            if !fds.is_null() { *fds = 101; *fds.add(1) = 102; }
            0
        }
        // dup2 / dup3 → just return newfd.
        33 | 292 => a2,
        // dup(oldfd) → return same fd (identity dup).
        32 => a1,
        // fcntl(fd, cmd, arg) → F_GETFL=1 returns O_RDWR(2), others return 0.
        72 => { if a2 == 3 { 2 } else { 0 } }
        // setsockopt / getsockopt / bind / connect / socket → ENOTSUP stubs
        // (networking not yet wired; avoids hard ENOSYS crash in glibc's resolver).
        41 | 49 | 54 | 55 | 200 => errno(-95), // EOPNOTSUPP
        // readlink(path, buf, size) — common for /proc/self/exe.
        89 => unsafe {
            let path = cstr(a1);
            let buf  = a2 as *mut u8;
            let sz   = a3 as usize;
            if !buf.is_null() && sz > 0 && path == b"/proc/self/exe" {
                let val = b"/bin/linuxtest";
                let n = val.len().min(sz);
                core::ptr::copy_nonoverlapping(val.as_ptr(), buf, n);
                return n as u64;
            }
            errno(-2) // ENOENT
        }
        // getdents64(fd, dirp, count) → 0 (empty directory).
        217 => 0,
        // sysinfo(info) — fill struct sysinfo with rough values.
        99 => unsafe {
            let p = a1 as *mut u64;
            if !p.is_null() {
                core::ptr::write_bytes(p as *mut u8, 0, 112);
                *p = 1;                                  // uptime (1 sec)
                *p.add(9)  = (512u64 * 1024 * 1024);    // totalram
                *p.add(10) = (480u64 * 1024 * 1024);    // freeram
                *p.add(18) = 1;                          // mem_unit
            }
            0
        }
        // tgkill(tgid, tid, sig) → 0 (no signal delivery yet).
        234 => 0,
        // rt_sigaction / rt_sigprocmask already stubbed via 13|14 below —
        // add rt_sigsuspend (130) and sigaltstack (131).
        130 | 131 => 0,
        // waitpid / wait4 → ECHILD (no children).
        61 | 247 => errno(-10),

        // exit / exit_group → restart desktop-metal when launched via sys_exec_linux,
        // otherwise halt (e.g. the old `linuxtest` boot mode).
        60 | 231 => {
            serial::write_str("\n  [linux] exit code=");
            serial::write_byte(b'0' + ((a1 % 10) as u8));
            serial::write_byte(b'\n');
            if unsafe { RESTART_DESKTOP } {
                reset();
                crate::restart_desktop();
            }
            vga::write_str("\n  [linux] process exited, code=", vga::Color::Green);
            vga::write_i32(a1 as i32);
            vga::write_byte(b'\n', vga::Color::White);
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

    // Map the physical framebuffer so /dev/fb0 mmap works. The FB lives above
    // the identity-mapped range (e.g. 0xFD000000 in QEMU), so we need 4 KiB
    // page mappings covering it before ring-3 tries to write pixels.
    let fb_base = crate::fb::base() as u64;
    if fb_base != 0 {
        let fb_size = crate::fb::pitch() as u64 * crate::fb::height() as u64 + 4096;
        crate::vmm::map_mmio_range(fb_base, fb_size);
    }

    // Dynamic linker base (ET_DYN). 8 MiB: above the program (~4 MiB), below the
    // relocated initrd (40 MiB). The interpreter is small (~0.25 MiB).
    const INTERP_BASE: u64 = 0x0080_0000;

    // PIE programs (ET_DYN) must NOT load at vaddr 0: that maps the null page and
    // makes glibc misparse the dynamic section (non-canonical l_info → #GP early
    // in ld.so). Real Linux always relocates a PIE to a high base. We place it in
    // the free window between ld.so (8–10 MiB) and the initrd (40 MiB). ET_EXEC
    // (static, fixed-address) keeps bias 0.
    const PROG_BASE: u64 = 0x0100_0000; // 16 MiB
    let prog_bias: u64 = if crate::elf::is_dyn(elf) { PROG_BASE } else { 0 };

    // If the program is dynamically linked, load the interpreter (ld.so) and
    // jump to IT; the program's own entry/phdrs go in the auxv (AT_ENTRY/AT_PHDR)
    // and ld.so relocates the program + maps shared libraries from the initrd.
    let (entry, at_base, at_entry) = match crate::elf::interp_path(elf) {
        Some(ipath) => {
            let prog_entry = crate::elf::load_bias(elf, prog_bias).expect("program load failed");
            let p = if ipath.starts_with(b"/") { &ipath[1..] } else { ipath };
            let interp_elf = crate::ramfs::find(p).expect("dynamic linker not in initrd");
            let interp_entry = crate::elf::load_bias(interp_elf, INTERP_BASE)
                .expect("interpreter load failed");
            serial::write_str("  [linux] dynamic: ld.so loaded; relocating program\n");
            (interp_entry, INTERP_BASE, prog_entry)
        }
        None => {
            let e = crate::elf::load_bias(elf, prog_bias).expect("linux ELF parse failed");
            (e, 0, e)
        }
    };

    // ── System V AMD64 initial stack ──
    // RSP → argc, argv[]·NULL, envp[]·NULL, auxv pairs·{AT_NULL,0}.
    const AT_NULL: u64 = 0; const AT_PHDR: u64 = 3; const AT_PHENT: u64 = 4;
    const AT_PHNUM: u64 = 5; const AT_PAGESZ: u64 = 6; const AT_BASE: u64 = 7;
    const AT_ENTRY: u64 = 9; const AT_RANDOM: u64 = 25; const AT_EXECFN: u64 = 31;
    let (phdr, phent, phnum) = match crate::elf::phdr_info(elf) {
        // AT_PHDR is the RUNTIME address of the program headers → add the bias
        // the PIE was actually loaded at (0 for ET_EXEC).
        Some((v, e, n)) => (v + prog_bias, e, n),
        None => (0, 56, 0),
    };
    if TRACE {
        dbg_hex("  [linux] auxv: AT_PHDR=", phdr); dbg_hex(" PHENT=", phent);
        dbg_hex(" PHNUM=", phnum); serial::write_byte(b'\n');
        dbg_hex("  [linux] prog_bias=", prog_bias); dbg_hex(" AT_ENTRY=", at_entry);
        dbg_hex(" AT_BASE=", at_base); dbg_hex(" ld.so_entry=", entry);
        serial::write_byte(b'\n');
    }

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

    // Ensure the PS/2 keyboard keeps delivering IRQs to the Linux process. A
    // stale byte sitting in the 8042 output buffer leaves it "full" so it never
    // raises a new IRQ1 (the keyboard goes dead for a console app like fbDOOM,
    // which then can't read its keys). Drain the buffer and re-enable the
    // keyboard interface + scanning.
    unsafe {
        let mut guard = 0;
        while crate::port::inb(0x64) & 0x01 != 0 && guard < 32 {
            let _ = crate::port::inb(0x60);
            guard += 1;
        }
        let mut w = 0;
        while crate::port::inb(0x64) & 0x02 != 0 && w < 100000 { w += 1; }
        crate::port::outb(0x64, 0xAE); // enable keyboard interface
        w = 0;
        while crate::port::inb(0x64) & 0x02 != 0 && w < 100000 { w += 1; }
        crate::port::outb(0x60, 0xF4); // keyboard: enable scanning
    }

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
