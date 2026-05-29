# Linux ABI compatibility layer

**Goal:** let the from-scratch **pure-Rust Rusty Penguin kernel** (the bare-metal
boot entry) execute *unmodified Linux x86-64 binaries* — so the one OS we ship
can eventually run real third-party software (and, the long-horizon dream,
browsers) *natively on our own kernel*, not on a Linux kernel.

This is the bridge that makes "the bare-metal OS is the total Ubuntu
replacement" technically reachable. It is a **multi-year road**, built brick by
brick, and we are honest about that: a Linux ABI is ~400 syscalls with exact
semantics, plus a dynamic linker, glibc/musl, threads, futexes, epoll, signals,
`/proc`, and DRM/GPU ioctls. Serious teams (FreeBSD Linuxulator, Google gVisor,
Microsoft WSL1 — which ultimately pivoted to shipping a real Linux kernel in a
VM) have each spent *years* here and still have gaps. No from-scratch hobby OS
runs Chrome natively. We go one verifiable brick at a time.

## Design

A process runs in one of two **ABI modes**:

- **Native** — the Rusty Penguin apps (desktop, shell, games). Custom syscall
  numbers in `kernel/src/syscall.rs`.
- **Linux** — unmodified Linux binaries, routed to `kernel/src/linux.rs`.

The split is necessary because Linux's numbers collide with the native table
(Linux `9 = mmap`, `12 = brk` vs native `9 = sys_ps`, `12 = sys_serial_debug`).
The asm syscall trampoline stashes Linux's args 4–6 (passed in `r10/r8/r9`), and
`syscall_handler` routes to `linux::syscall` when the current process is Linux.

**Ternary lens (per project mandate):** a syscall's outcome is a `Trit` —
`+1` ok / `0` would-block (EAGAIN) / `-1` error (negative errno). Process and
thread liveness will be modelled the same way as scheduling lands.

## Bricks

| # | Brick | Status |
|---|-------|--------|
| 1 | Run a **freestanding** static Linux ELF (raw `write`+`exit_group`), SysV initial stack (argc/argv/envp/auxv), per-process ABI mode, Linux syscall dispatch | ✅ **DONE 2026-05-29** — proof `docs/linux-abi-brick1-serial.txt`; harness `iso/build-linux-abi-test.sh` |
| 2 | Static **musl** libc binary (real `printf`): needs `arch_prctl`(TLS), `brk`/`mmap`, `set_tid_address`, `rt_sig*`, `writev`, `getrandom`, `clock_gettime` — all stubbed/implemented in `linux.rs`, needs verifying end-to-end | ⏳ next |
| 3 | **Real per-process VMM + memory** (demand paging, proper mmap/munmap, reclaim) — current arenas are crude bumps in the identity map | ❌ |
| 4 | **Dynamic linking** (`ld-linux`/`ld-musl`): `openat`/`read`/`fstat`/`mmap` of the loader + libs, `PT_INTERP`, relocations | ❌ |
| 5 | **glibc** dynamic binaries (busybox-glibc, coreutils) | ❌ |
| 6 | **Threads**: `clone`, `futex`, `set_robust_list`, TLS per thread, a real scheduler | ❌ |
| 7 | **epoll/poll/eventfd/signalfd/timerfd**, signals (`rt_sigaction` delivery) | ❌ |
| 8 | A **filesystem** with Linux semantics (`/proc`, `/sys`, `/dev`, real VFS) | ❌ |
| 9 | A tiny **GTK/X client** rendering to our framebuffer (the first real GUI app on our own kernel) | ❌ |
| … | … eventually the browser. Far. Honest. | ❌ |

## Test

```
iso/build-linux-abi-test.sh
```

Builds the freestanding ELF + the kernel, makes a one-entry GRUB ISO that boots
`kernel.elf linuxtest`, runs it headless, and prints the serial log. Booting any
bare-metal entry with `linuxtest` on the multiboot2 cmdline diverts to the Linux
loader instead of the desktop.
