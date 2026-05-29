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
    // Ternary view of the call's outcome, for the findings log / future telemetry.
    let _outcome: Trit;
    match nr {
        // read(fd, buf, len) — no input source yet → EOF.
        0 => { _outcome = Trit::Zero; 0 }
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
        // mmap(addr, len, prot, flags, fd, off): anonymous bump only.
        9 => unsafe {
            let len = page_up(a2);
            if MMAP_CUR + len > MMAP_CAP { return errno(-12 /*ENOMEM*/); }
            let p = MMAP_CUR; MMAP_CUR += len;
            // anonymous memory is expected zeroed
            core::ptr::write_bytes(p as *mut u8, 0, len as usize);
            p
        }
        // fstat(fd, statbuf): report fd 1/2 as a character device so glibc
        // treats stdout sensibly (line-buffered, BUFSIZ from st_blksize).
        5 => {
            let sb = a2 as *mut u8;
            if !sb.is_null() {
                unsafe {
                    core::ptr::write_bytes(sb, 0, 144); // sizeof(struct stat)
                    const S_IFCHR: u32 = 0x2000;
                    (sb.add(24) as *mut u32).write_unaligned(S_IFCHR | 0o666); // st_mode
                    (sb.add(56) as *mut u64).write_unaligned(4096);            // st_blksize
                }
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
    }
}

// ── Load + enter an unmodified Linux ELF in ring-3 ───────────────────────────
/// Build the System V AMD64 initial process stack and IRETQ into the binary.
/// Never returns — the process runs until it calls exit/exit_group.
pub fn enter(elf: &[u8]) -> ! {
    // Linux brk/mmap arenas live up to 256 MiB — make sure they're mapped.
    crate::vmm::extend_identity_map(256);

    let entry = crate::elf::load(elf).expect("linux ELF parse failed");

    // ── System V AMD64 initial stack ──
    // At _start, RSP must point to argc, then: argv[]·NULL, envp[]·NULL,
    // auxv pairs·{AT_NULL,0}. We run argc=0, no env, a minimal auxv.
    const AT_NULL: u64 = 0; const AT_PHDR: u64 = 3; const AT_PHENT: u64 = 4;
    const AT_PHNUM: u64 = 5; const AT_PAGESZ: u64 = 6; const AT_ENTRY: u64 = 9;
    const AT_RANDOM: u64 = 25;
    // Program-header info — glibc needs it to find PT_TLS / size the TCB.
    let (phdr, phent, phnum) = crate::elf::phdr_info(elf).unwrap_or((0, 56, 0));

    let top = crate::vmm::USER_STACK_TOP;
    // 16 random bytes for AT_RANDOM (glibc/musl stack canary source).
    let rand_ptr = top - 16;
    unsafe {
        let mut x = crate::idt::ticks().wrapping_mul(2862933555777941757).wrapping_add(3037000493);
        for i in 0..16u64 { x = x.wrapping_mul(6364136223846793005).wrapping_add(1); *((rand_ptr + i) as *mut u8) = (x >> 40) as u8; }
    }

    // Lay the control block out as consecutive u64s ending in AT_NULL.
    let words: [u64; 17] = [
        0,                       // argc
        0,                       // argv[0] = NULL
        0,                       // envp[0] = NULL
        AT_PHDR,   phdr,
        AT_PHENT,  phent,
        AT_PHNUM,  phnum,
        AT_PAGESZ, 4096,
        AT_RANDOM, rand_ptr,
        AT_ENTRY,  entry,
        AT_NULL,   0,
    ];
    let bytes = (words.len() * 8) as u64;
    let mut sp = (rand_ptr - bytes) & !0xF; // 16-byte align the argc slot
    let user_rsp = sp;
    unsafe {
        for w in words.iter() { *(sp as *mut u64) = *w; sp += 8; }
    }

    unsafe { LINUX_ABI = true; }

    vga::write_str("  [linux] entering ring-3 @ 0x", vga::Color::Cyan);
    vga::write_hex(entry, vga::Color::Cyan);
    vga::write_byte(b'\n', vga::Color::White);
    serial::write_str("  [linux] IRETQ into unmodified Linux ELF\n");

    // IRETQ into ring-3 (same selectors as the native launch in main.rs).
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
            "iretq",
            in("r8") entry,
            in("r9") user_rsp,
            options(noreturn),
        );
    }
}
