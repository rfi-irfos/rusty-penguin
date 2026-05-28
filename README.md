# Rusty Penguin Distribution

[![Language: Rust](https://img.shields.io/badge/Language-Rust-ce422b?logo=rust)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-green)](LICENSE)
[![Version: 1.0.0](https://img.shields.io/badge/Version-1.0.0--pre--1-blue)](https://github.com/rfi-irfos/rusty-penguin)
[![Platform: x86_64](https://img.shields.io/badge/Platform-x86__64-333)](https://en.wikipedia.org/wiki/X86-64)
[![Kernels: Dual-Boot](https://img.shields.io/badge/Kernels-Bare--metal%20%2B%20Linux-purple)](https://github.com/rfi-irfos/rusty-penguin)
[![Status: Active Development](https://img.shields.io/badge/Status-Active%20Development-brightgreen)](https://github.com/rfi-irfos/rusty-penguin/pulse)

> "Binary hardware. Ternary mind."

**A complete operating system distribution built entirely in Rust, with ternary logic as a first-class computational primitive. Ship with bare-metal kernel, Linux kernel, or bring your own.**

## Architecture: Distribution + Kernel Separation

Rusty Penguin is a **distribution layer** (userspace, desktop, package system, TIS runtime) that works with **multiple kernels**:

```
┌────────────────────────────────────────────────────────┐
│   Rusty Penguin Distribution                           │
│   (Desktop, shell, package mgmt, file system, TIS)     │
├────────────────────────────────────────────────────────┤
│  Boot: Select Your Kernel                              │
├──────────────────────────────┬──────────────────────────┤
│ Option A:                    │ Option B:               │
│ Rusty Penguin Bare-Metal     │ Linux Kernel           │
│ (Pure Rust, no Linux)        │ (Standard, proven)     │
│ ISO: rp-bare-metal.iso       │ ISO: rp-linux.iso      │
└──────────────────────────────┴──────────────────────────┘
```

Developers choose:
- **Pure Rust**: Boot bare-metal kernel + Rusty Penguin distro (technology showcase, long-term vision)
- **Production-ready**: Boot Linux kernel + Rusty Penguin distro (use it now, swap from Ubuntu today)
- **Kernel only**: Use the bare-metal kernel with your own userspace
- **Distro only**: Run Rusty Penguin distribution on any x86_64 Linux system

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

## The Rusty Penguin Distribution

Core components (run on both bare-metal and Linux kernels):

- **Modern graphical desktop** with Ubuntu-inspired visual design, full window manager, drag/resize/minimize
- **psh shell** with 90+ commands: pipes, redirects, loops, variables, command substitution, ternary arithmetic
- **File manager** with directory browsing and file operations
- **Graphical text editor** (dedicated GUI, not terminal-based) with open/save/edit
- **System tools**: process viewer, system monitor, settings panel
- **Package manager** (in development) for installing and updating software
- **TIS runtime integration** for sparse ternary neural network inference
- **Rust-only init (PID 1)** with proper signal handling and clean shutdown

**Desktop & UI:**
- Live stats bar: clock, memory usage, ternary state indicator
- Anti-flicker rendering with three-tier dirty tracking
- Responsive window rendering at 25Hz+
- Ubuntu color palette and modern card-based design

**Under the Hood:**
- Hand-written Rust from init to window manager to terminal emulator
- No libc (syscall interface only)
- No external UI toolkits
- Pure systems programming without C dependencies

---

## Implementation Status

### Distribution Layer (Shared Across All Kernels)
| Component | Description | Status |
|---|---|---|
| **Desktop UI** | Window manager, rendering, taskbar, icons | ✅ **Working** |
| **psh shell** | 90+ commands, pipes, redirects, loops, variables | ✅ **Working** |
| **Text editor** | Graphical editor with open/save | ✅ **Working** |
| **Ternary runtime** | Balanced ternary arithmetic, trit operations | ✅ **Working** |
| **File manager** | Directory browsing, file operations | 🔄 **In Progress** |
| **System monitor** | Process viewer, memory stats | 🔄 **In Progress** |
| **Package manager** | Install/update software | 🔄 **Planned** |
| **Persistent config** | Settings, preferences, boot options | 🔄 **Planned** |
| **TIS integration** | AI runtime, sparse inference | 🔄 **Planned** |

### Kernels (Swappable)
| Component | Description | Status |
|---|---|---|
| **Bare-metal Kernel** | Pure Rust x86_64 kernel, no Linux | ✅ **Boots & Runs** |
| `kernel/` | Boot, memory map, interrupts, syscalls | ✅ **Complete** |
| `vfs` (bare-metal) | In-memory filesystem | ✅ **Complete** |
| `desktop-metal` | GUI renderer for bare-metal | ✅ **Complete** |
| **Linux Kernel** | Standard x86_64 Linux 6.17+ | ✅ **Supported** |
| Persistent storage (Linux) | Real filesystems (ext4, btrfs, etc.) | ✅ **Works** |
| Networking (Linux) | Network stack, drivers | ✅ **Works** |
| **Features Planned for Both**|
| Persistent storage (bare-metal) | Block I/O, disk filesystem | 🔄 **Phase 2** |
| Networking (bare-metal) | Network stack on custom kernel | 🔄 **Phase 3** |

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

## Boot Options

### Option 1: Linux Kernel (Recommended for Daily Use)

Default boot path. GRUB → Linux 6.17 → Rust init (PID 1) → Rusty Penguin distribution. 

**Advantages:**
- Real filesystems (ext4, btrfs, NFS)
- Network stack (Ethernet, WiFi with drivers)
- Hardware support ecosystem
- Swap to Rusty Penguin *today* without kernel development
- Use it for production inference and daily work

**Boot time:** ~3 seconds in QEMU

### Option 2: Bare-Metal Kernel (Pure Rust, Technology Showcase)

Select "Rusty Penguin (bare metal)" at GRUB menu. No Linux kernel. No libc. Pure Rust from bootloader to desktop.

**Advantages:**
- 100% pure Rust OS (no C, no dependencies)
- Full control over kernel architecture
- Proof that systems programming in Rust works
- Path to long-term vision: standalone Rusty Penguin kernel

**Current capabilities:**
- x86_64 boot (32-bit protected mode → 64-bit long mode)
- Memory management and Multiboot2 parsing
- PS/2 keyboard and framebuffer rendering
- VGA/VESA graphics output
- Custom syscall ABI (14 syscalls)
- In-memory filesystem
- Complete shell and graphical desktop

**Next phases:**
- Phase 2: Persistent storage (block I/O, real filesystems)
- Phase 3: Networking and multi-process support
- Phase 4: Hardware breadth (USB, audio, etc.)

**Boot output:**
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
psh>
```

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
