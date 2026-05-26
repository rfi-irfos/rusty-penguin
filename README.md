# Rusty Penguin

> "Binary hardware. Ternary mind."

**The first bootable operating system in Rust built around ternary logic as a first-class computational primitive.**

Boots to a graphical desktop today. Runs on standard Linux hardware (userspace track). Building toward a native bare-metal ternary kernel.

Built by [RFI-IRFOS](https://github.com/rfi-irfos) as part of the [Ternary Intelligence Stack](https://ternlang.com).

---

## What this is

Binary computers have two states: on and off. Every value, every decision, every process is either 1 or 0.

Rusty Penguin adds a third: **dormant**. Not running, not stopped — *resting*. A process that hasn't been asked for anything yet is not the same as a process that failed. A memory page that hasn't been touched is not dead.

Every primitive in this system — from the lowest Trit to the process scheduler to the sparse AI runtime — expresses three states:

| Trit | Value | Meaning |
|---|---|---|
| Pos | +1 | Active, running, promoted |
| Zero | 0 | Dormant, idle, neutral |
| Neg | -1 | Suppressed, terminated, rejected |

Dormancy is sacred. Zero is not nothing.

---

## What it does right now

Boot the ISO in QEMU or VirtualBox. You get:

- A graphical desktop rendered directly to `/dev/fb0` — no X11, no Wayland, no display server
- A window manager: drag, minimize to taskbar, maximize to fullscreen
- A top stats bar: live clock + CPU%, MEM%, SWAP%, net rx/tx from `/proc`
- A start menu on the Dingir (𒀭) icon
- Four built-in launchers: terminal (psh), process viewer (ps), ternary AI inference (ai), ternary arithmetic inspector (trit)
- PTY-backed terminals: real pseudoterminals via `/dev/pts`, keyboard input fully forwarded
- Anti-flicker rendering: three-tier dirty tracking — chrome, content, cursor — redraws only what changed
- A Rust-only init (PID 1): mounts proc/sys/dev/devpts/tmp, loads VirtIO input module, launches the desktop, halts cleanly on exit

Everything from the framebuffer driver to the font renderer to the PTY multiplexer is hand-written Rust. No libc wrappers beyond the syscall layer. No UI toolkit.

---

## Status

| Crate | Description | Status |
|---|---|---|
| `ternary-core` | Trit and Tryte primitives (±9841, 9 trits) | Working |
| `mathematics` | Balanced ternary mul/div/mod/consensus/scale | Working |
| `scheduler` | Ternary process states, real `/proc` scanning | Working |
| `ai-runtime` | Sparse ternary inference (Zero-dormancy skipping) | Working |
| `shell` (psh) | Interactive REPL, PID 1 capable | Working |
| `init` | PID 1 init: mounts, VirtIO, hostname, spawns desktop | Working |
| `desktop` | Graphical WM, framebuffer renderer, PTY terminals | Working |
| `iso/` | Bootable ISO builder (grub-mkrescue) | Working |
| `compiler/` | ternlang-core lexer/parser/VM | Planned |
| `filesystem/` | Ternary-annotated VFS | Planned |
| `ipc/` | Actor model (TernNode, Unix sockets) | Planned |
| `memory/` | TernPage: ternary-annotated mmap pages | Planned |
| `kernel/` | Bare-metal x86_64-unknown-none kernel | Research |

---

## Boot it

```bash
# Build the ISO
bash iso/build.sh

# Run in QEMU
qemu-system-x86_64 -cdrom rusty-penguin.iso -m 512M \
  -device virtio-tablet-pci \
  -vga std -display sdl

# Or in VirtualBox
VBoxManage createvm --name "Rusty Penguin" --register
VBoxManage modifyvm "Rusty Penguin" --memory 512 --vram 16 --cpus 1
VBoxManage storagectl "Rusty Penguin" --name "IDE" --add ide
VBoxManage storageattach "Rusty Penguin" --storagectl "IDE" \
  --port 0 --device 0 --type dvddrive --medium rusty-penguin.iso
VBoxManage startvm "Rusty Penguin"
```

### Shell without booting

```bash
# Run the Penguin Shell directly
cargo run -p shell
```

### psh commands

```
psh> trit 42          # balanced ternary representation of 42
psh> mul 6 7          # ternary multiply (double-width)
psh> div 17 5         # ternary divide with remainder
psh> ps               # list processes with ternary state annotations
psh> activate 1234    # SIGCONT → ACTIVE  (+1)
psh> dormant  1234    # SIGSTOP → DORMANT  (0)
psh> suppress 1234    # SIGTERM → SUPPRESSED (-1)
psh> ai 16            # sparse ternary inference demo
psh> scale 100 -1     # one-trit transform (negate)
```

---

## Architecture

**Track 1: Userspace personality (today)**
Stock Linux kernel + minimal initramfs containing the desktop, psh, and Rust-only init. Boots in under 3 seconds in QEMU. The entire OS stack — init, WM, terminal, AI runtime — is compiled Rust.

**Track 2: Bare-metal kernel (long-term)**
`x86_64-unknown-none` no_std Rust kernel with Multiboot2 boot, ternary scheduler, BET bytecode VM. Target: replace the Linux kernel entirely with a ternary-first execution model.

---

## The computational case for ternary

Standard balanced ternary arithmetic uses fewer digits to represent the same range:
- 9 trits → ±9841 (vs 9 bits → ±255 for unsigned)
- Multiplication maps to shift-and-add on a ternary number line
- Neural networks quantized to {-1, 0, +1} can skip all zero-weight multiplications — this is the entire basis of the `ai-runtime` sparse inference engine

The ternary AI runtime in this repo achieves real sparsity savings: zero-weighted edges are dormant, not computed. This is the same insight behind BitNet and ternary quantization in large language models — implemented here from first principles in Rust, running bare-metal in a bootable OS.

---

## TIS Integration

Rusty Penguin is part of the Ternary Intelligence Stack:

| Module | Source |
|---|---|
| `compiler/` | ternlang-core lexer/parser/BET bytecode/VM |
| `filesystem/` | ternlang-fs VFS patterns |
| `ipc/` | ternlang-runtime TernNode actor model |
| `hardware-abstraction/` | ternlang-driver HAL traits |
| `ai-runtime/` | ternlang-ml TritTensor + sparse inference |

---

## License

MIT — see workspace `Cargo.toml`.
