# Rusty Penguin

[![Language: Rust](https://img.shields.io/badge/Language-Rust-ce422b?logo=rust)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-green)](LICENSE)
[![Version: 1.0.0](https://img.shields.io/badge/Version-1.0.0--bm-blue)](https://github.com/rfi-irfos/rusty-penguin)
[![Platform: x86_64](https://img.shields.io/badge/Platform-x86__64-333)](https://en.wikipedia.org/wiki/X86-64)
[![Architecture: Bare-metal](https://img.shields.io/badge/Architecture-Bare--metal-purple)](https://en.wikipedia.org/wiki/Bare_metal)
[![Status: Active](https://img.shields.io/badge/Status-Active-brightgreen)](https://github.com/rfi-irfos/rusty-penguin/pulse)

> "Binary hardware. Ternary mind."

**The first bootable operating system in Rust built around ternary logic as a first-class computational primitive.**

Two fully working boot tracks:

- **Userspace track** — GRUB → Linux kernel → Rust init (PID 1) → modern graphical desktop + psh shell
- **Bare-metal track** — GRUB → our own x86_64 kernel (no Linux, no libc) → graphical desktop GUI + ternary arithmetic on bare metal

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

- **Modern graphical desktop** with Ubuntu-inspired visual design, rendered directly to `/dev/fb0`
- **Window manager** with drag, minimize/maximize, proper clipping, smooth ~25Hz drag rendering
- **Graphical text editor** (dedicated GUI, not terminal-based) with file open/save operations
- **Live stats bar**: clock, memory usage, ternary state indicator
- **Start menu** (Dingir 𒀭 icon) with five launchers
  - **psh** — Interactive shell with pipes, redirects, loops, variables, command substitution
  - **Files** — File browser with `ls -la` output
  - **Edit** — Graphical text editor (Ctrl+S to save, Ctrl+Q to close)
  - **ai** — Sparse ternary neural network inference demo
  - **trit** — Balanced ternary arithmetic inspector  
  - **km** — Kernel manager for bare-metal kernel staging
- **Rich shell scripting** — for/do/done, while loops, if/then/else, test/[, variable expansion, command substitution
- **Text editor** (nano) — full file editing with keyboard navigation
- **System utilities** — 50+ built-in commands (ls, cat, grep, sort, find, etc.)
- **Anti-flicker rendering** with three-tier dirty tracking (chrome, content, cursor)
- **Rust-only init (PID 1)** with proper signal handling and clean shutdown

Every component from framebuffer driver to window manager to terminal emulator is hand-written Rust. **No libc beyond syscalls. No UI toolkits. Pure bare-metal systems programming.**

---

## Status

| Component | Description | Status |
|---|---|---|
| **Userspace Desktop** | Graphical window manager, 50+ shell commands, text editor | ✅ **Production-ready** |
| `psh` (shell) | POSIX-like scripting, pipes, redirects, loops, variables | ✅ **Complete** |
| `term` | Terminal emulator with full text editing, 80×24 cells | ✅ **Complete** |
| `wm` (window mgr) | Window dragging, resizing, taskbar, modern UI design | ✅ **Complete** |
| `framebuffer` | Direct framebuffer rendering, no display server | ✅ **Complete** |
| `trit` (arithmetic) | Balanced ternary: +1/0/-1, mul/div, dormancy semantics | ✅ **Complete** |
| `ai-runtime` | Sparse ternary inference with zero-dormancy skipping | ✅ **Complete** |
| **Bare-metal Desktop** | Full graphical desktop on custom kernel (no Linux) | ✅ **Phase 1 Complete** |
| `desktop-metal` | Bare-metal ring-3 GUI with modern Ubuntu-style design | ✅ **Complete** |
| `kernel/` | x86_64 kernel: boot, VGA, interrupts, memory map, keyboard | ✅ **Complete** |
| `user-psh` | Shell compiled for bare-metal execution | ✅ **Complete** |
| `vfs` | In-memory flat filesystem with VFS files | ✅ **Complete** |
| `iso/` | ISO builder with both userspace and bare-metal tracks | ✅ **Complete** |
| `compiler/` | ternlang-core lexer/parser/VM | 🔄 **Planned** |
| `memory/` | Ternary-annotated page allocator | 🔄 **Planned** |

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

**Track 2: Bare-metal kernel (working — Phase 1)**
No Linux. No libc. No OS of any kind. GRUB loads our ELF, the boot stub transitions from 32-bit protected mode to 64-bit long mode, hands off to `kernel_main`. VGA driver writes directly to 0xB8000. 8259 PIC remapped, timer + keyboard IRQs live. PS/2 keyboard echoes to screen with full backspace and newline handling. Multiboot2 memory map parsed (511 MiB available). `ternary-core` arithmetic runs on bare metal as the first computation.

Select "Rusty Penguin (bare metal)" at the GRUB menu.

```
Rusty Penguin v1.0.0 -- bare metal kernel
Binary hardware. Ternary mind.

[interrupts: OK]
[memory map]
  0x0 + 27F KiB
  0x100000 + 7FBB0 KiB
  total available: 1FF MiB

ternary: 42 + (-7) = 35

keyboard active -- type below
> _
```

Next: physical page allocator → virtual memory → processes → psh on bare metal.

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
