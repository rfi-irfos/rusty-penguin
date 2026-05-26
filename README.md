# Rusty Penguin

> "Binary hardware. Ternary mind."

A ternary-first operating system written in Rust. Runs on standard Linux hardware today (userspace personality track) while building toward a native bare-metal ternary kernel.

Part of the [Ternary Intelligence Stack](https://ternlang.com) — actively researched and developed by [RFI-IRFOS](https://github.com/rfi-irfos).

---

## Core Philosophy

Every value in Rusty Penguin is one of three states:

| Trit | Value | Meaning |
|---|---|---|
| Pos | +1 | Active, running, promoted |
| Zero | 0 | Dormant, idle, neutral |
| Neg | -1 | Suppressed, terminated, rejected |

**Dormancy is sacred.** Zero is not nothing — it is the third option binary systems cannot express. Idle processes are dormant, not absent. Sparse computation skips zeros rather than multiplying through them.

---

## Status — v1.0.0

| Crate | Description | Status |
|---|---|---|
| `ternary-core` | Trit and Tryte primitives (±9841, 9 trits) | Working |
| `mathematics` | Balanced ternary mul/div/mod/consensus/scale | Working |
| `scheduler` | Ternary process states, real `/proc` scanning | Working |
| `ai-runtime` | Sparse ternary inference (Zero-dormancy skipping) | Working |
| `shell` (psh) | Interactive REPL, PID 1 capable | Working |
| `init` | PID 1 init: mounts, hostname, spawns psh | Working |
| `iso/` | Bootable ISO builder (grub-mkrescue) | Working |
| `compiler/` | ternlang-core lexer/parser/VM | Planned |
| `filesystem/` | Ternary-annotated VFS | Planned |
| `ipc/` | Actor model (TernNode, Unix sockets) | Planned |
| `memory/` | TernPage: ternary-annotated mmap pages | Planned |
| `kernel/` | Bare-metal x86_64-unknown-none kernel | Research |

---

## Try It Now

```bash
# Run the Penguin Shell (psh) directly on your machine
cargo run -p shell

# Build a bootable ISO
bash iso/build.sh

# Test in QEMU
qemu-system-x86_64 -cdrom rusty-penguin.iso -m 512M -nographic
```

### psh commands

```
psh> trit 42          # inspect balanced ternary representation
psh> mul 6 7          # ternary multiply (double-width result)
psh> div 17 5         # ternary divide with remainder
psh> scale 100 -1     # one-trit transform (negate)
psh> ps               # list processes with ternary state annotations
psh> activate 1234    # SIGCONT → ACTIVE  (+1)
psh> dormant  1234    # SIGSTOP → DORMANT  (0)
psh> suppress 1234    # SIGTERM → SUPPRESSED (-1)
psh> ai 16            # run sparse ternary inference demo
```

---

## Architecture — Two Parallel Tracks

**Track 1: Userspace personality (today)**
Stock Linux kernel + minimal initramfs. The `init` binary mounts filesystems, sets hostname `rusty-penguin`, and drops into `psh`. Bootable from ISO in QEMU or VirtualBox.

**Track 2: Bare-metal kernel (long-term)**
`x86_64-unknown-none` no_std Rust kernel. Multiboot2 boot, ternary scheduler, BET bytecode VM bare-metal. Target: replace the Linux kernel entirely.

---

## TIS Integration

Rusty Penguin is built on top of the Ternary Intelligence Stack:

| Rusty Penguin module | TIS source |
|---|---|
| `compiler/` | ternlang-core lexer/parser/BET bytecode/VM |
| `filesystem/` | ternlang-fs VFS patterns |
| `ipc/` | ternlang-runtime TernNode actor model |
| `hardware-abstraction/` | ternlang-driver HAL traits |
| `ai-runtime/` | ternlang-ml TritTensor + sparse inference |

---

## License

MIT — see [LICENSE](LICENSE) or workspace `Cargo.toml`.
