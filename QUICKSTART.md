# Rusty Penguin Quick Start

Getting started with Rusty Penguin distribution.

## Boot Options

### Option 1: Linux Kernel (Recommended)

```bash
# Boot with Linux kernel (QEMU)
qemu-system-x86_64 -cdrom rusty-penguin.iso -m 512M \
  -device virtio-tablet-pci -vga std -display sdl

# Select: Ubuntu/Linux at GRUB menu
# Login: any username, any password (no actual auth yet)
# Select: "Rusty Penguin (Linux track)" or similar
```

### Option 2: Bare-Metal Kernel

```bash
# Boot with pure Rust kernel (QEMU)
qemu-system-x86_64 -cdrom rusty-penguin.iso -m 512M \
  -device virtio-tablet-pci -vga std -display sdl

# Select: "Rusty Penguin (bare metal)" at GRUB menu
# Full graphical desktop starts automatically
```

## Using the Package Manager

### List Installed Packages

```bash
psh> rpm list
```

### Install a Package

```bash
psh> rpm install hello-world-1.0.0.rpkg
Installed package: hello-world (extracted and linked)

psh> hello
Hello from Rusty Penguin Package Manager!
```

### Install TIS Stack

```bash
psh> rpm install tis-stack-1.5.0.rpkg
Installed package: tis-stack (extracted and linked)

psh> albert
albert. v1.5.0 (TIS MoE-13)
Usage: albert [model] [prompt]
Example: albert moe-13 'What is ternary logic?'

psh> trit-calc add 6 9
Balanced Ternary Calculator
Usage: trit-calc <operation> <args...>
Operations: add, mul, div, scale
```

### Get Package Info

```bash
psh> rpm info tis-stack
Package: tis-stack
[displays manifest.toml contents]
```

### Remove a Package

```bash
psh> rpm remove hello-world
Removed package: hello-world
```

## Ternary Arithmetic in Shell

Even without packages, explore ternary math:

```bash
psh> trit 42
42  (+0--)

psh> mul 6 7
  6 * 7 = 42 (lo=6 hi=0)

psh> div 17 5
  17 / 5 = 3 remainder 2

psh> scale 100 -1
  scale(100, -1) = -100
```

## Process Management

```bash
psh> ps
  PID      NAME               RSS(kb) STATE
  1        psh                4096    (+) ACTIVE
```

## System Information

```bash
psh> uname
Rusty Penguin v1.0.0 (bare-metal kernel)

psh> df
Filesystem    1024-blocks  Used Available Use%
ramfs         16384        2048  14336     12%

psh> free
       total    used    free
RAM    512M     128M    384M
```

## Available Shell Commands

```
psh> help

Commands:
  trit <n>           Convert integer to balanced ternary
  mul <a> <b>        Multiply two integers
  div <a> <b>        Divide a by b
  scale <n> <-1|0|1> Scale integer by a trit
  ps                 List processes
  activate <pid>     Signal ACTIVE
  dormant <pid>      Signal DORMANT
  suppress <pid>     Signal SUPPRESSED
  ai [n]             Sparse ternary inference demo
  rpm <cmd>          Package manager
  exit | quit        Exit
```

## File Manager (Desktop)

Click the "Files" icon to browse files (in graphical mode).

## Text Editor (Desktop)

Click the "Edit" icon to edit files graphically.

## System Shutdown

```bash
psh> exit
[init] Halting system...
```

Or in desktop: Window manager close button.

## Creating Custom Packages

See `PACKAGE_BUILD.md` for full instructions.

Quick example:

```bash
mkdir -p myapp-1.0.0/bin
echo '#!/bin/sh' > myapp-1.0.0/bin/myapp
echo 'echo "Hello from myapp"' >> myapp-1.0.0/bin/myapp
chmod +x myapp-1.0.0/bin/myapp

cat > myapp-1.0.0/manifest.toml << 'EOF'
[package]
name = "myapp"
version = "1.0.0"
description = "My app"
author = "Me"
license = "MIT"

[binaries]
myapp = "bin/myapp"
EOF

tar czf myapp-1.0.0.rpkg myapp-1.0.0/
rpm install myapp-1.0.0.rpkg
myapp
```

## Architecture

```
Rusty Penguin Distribution Layer
├── Desktop (window manager + apps)
├── psh Shell (90+ commands)
├── Package Manager (rpm)
├── File Manager
├── TIS Runtime
└── System Utilities

Boot: Choose Kernel
├── Linux Kernel (production-ready)
└── Bare-Metal Rust Kernel (pure Rust)
```

## What's Working

- ✅ Full graphical desktop with window manager
- ✅ Package manager with tar extraction
- ✅ Ternary arithmetic and AI inference
- ✅ Process management
- ✅ File manager (basic)
- ✅ Text editor (graphical)
- ✅ Shell with 90+ commands
- ✅ System utilities (ps, df, free, etc.)

## What's Coming (Phase 2-3)

- 🔄 Real filesystem support (persistent storage)
- 🔄 Settings/preferences UI
- 🔄 Network stack (Linux track first)
- 🔄 More system utilities
- 🔄 User accounts and permissions

## Troubleshooting

**Desktop doesn't start:** Boot with Linux kernel option, fall back to terminal.

**Package install fails:** Make sure .rpkg file is in current directory or use full path.

**Out of memory:** ISO is 512MB, limit window count if needed.

**Mouse/keyboard not responding:** Try clicking in window first, then typing.

## Next Steps

1. Install TIS stack: `rpm install tis-stack-1.5.0.rpkg`
2. Explore ternary arithmetic: `trit 42`, `mul 6 7`
3. Create a custom package
4. Test file manager and text editor
5. Report issues or suggestions!

---

**Goal**: Make Rusty Penguin a daily-usable OS that can replace Ubuntu.

**Status**: Phase 2 complete (package manager). Phase 3 (settings) upcoming.

**Community**: https://github.com/rfi-irfos/rusty-penguin
