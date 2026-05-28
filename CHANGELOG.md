# Changelog — Rusty Penguin

All notable changes to this project will be documented here.

## [Unreleased]

### Added — Persistent storage, the ternary way (daily-driver gap #1, 2026-05-28)

- The Linux track is no longer ephemeral: `init` brings up a **persistent
  writable disk mounted at /home**, so user files and config survive reboots —
  the first real step toward daily-driving Rusty Penguin off a USB stick.
- Storage is modeled as a **ternary state** (`ternary-core::Trit`), not a binary
  present/absent flag:
  - `+1` Active — disk present & formatted → mounted, persistent
  - ` 0` Dormant — disk present but blank → auto-provisioned (busybox `mke2fs`,
    bundled static in the initramfs), then activated
  - `-1` Suppressed — no writable disk / mount failed → ephemeral boot
- The boot record is a first-class **`.tern` file** (`~/.rusty/boot.tern`) with
  balanced-ternary encoding: the boot counter is stored both decimal and as a
  9-trit `Tryte` (the OS's native numeric form), e.g. `@boots 3 0000000+0`.
- `init` now mounts the kernel pseudo-filesystems (devtmpfs/proc/sys/tmp) and
  `sync(2)`s after writing so data is durable across power-off.
- Verified in QEMU across 3 reboots on a shared virtio disk: blank→provisioned
  on boot 1, then boot counter advances 1→2→3 on the persistent fs.

### Added — Doom (pure-Rust raycaster FPS) in the bare-metal desktop (2026-05-28)

- A **first-person 3D shooter** runs natively as a bare-metal desktop app,
  launchable from the dock and start menu next to Snake and Minesweeper.
  - Lode-style DDA raycaster, **100% pure Rust / no_std**: only f32
    `+ - * /` and casts — no trig or sqrt at runtime (turning uses a constant
    rotation matrix), no per-frame heap allocation (zbuffer + sprite ordering
    are stack arrays). Colored walls with side/distance shading, billboarded
    enemy sprites with z-testing, gun + muzzle flash + crosshair, kill counter.
  - Controls: W/S move, A/D turn, Q/E strafe, SPACE fire. PS/2 typematic
    repeat gives smooth held-key movement.
  - NOT id Software's DOOM — that C engine runs on the **Linux track** via the
    ISO's `Rusty Penguin -- DOOM (demoable)` boot entry. This is a from-scratch
    Rust tribute that fits the pure-Rust bare-metal ethos.
  - Proof: `docs/doom-raycaster-baremetal.png`.

### Added — Preinstalled games + 1080p bare-metal desktop (2026-05-28)

- **Snake and Minesweeper** ship as native desktop apps (pure Rust, no_std, no
  per-frame heap churn). Launchable from the dock icons and the start menu.
  - New `App::tick(ticks) -> bool` hook drives animation (Snake); input-only
    apps keep the default no-op. A tiny xorshift `Rng` (seeded from the PIT
    tick) places food/mines.
  - Snake: arrow keys / WASD, SPACE restarts, waits for first steer before
    moving. Minesweeper: 12×10 / 18 mines, first click always safe, flood-fill
    reveal, mouse (L reveal / R flag) and keyboard. Proof: `docs/snake-on-rusty-penguin.png`.

- **Bare-metal desktop now boots at 1920×1080×32** (was capped at 800×600).
  Three coordinated fixes:
  - `boot.s`: the multiboot2 framebuffer request tag now asks for 1920×1080×32
    (GRUB `gfxpayload` does *not* drive multiboot2 — width/height of 0 gave the
    800×600×24 default).
  - kernel relocates GRUB's initrd module to 40 MiB before loading the ring-3
    desktop, so the desktop's in-place `.bss` zero-fill (now a 24 MiB heap)
    can't clobber the module it's loaded from (was: `entry @ 0x0` + #PF).
  - ring-3 stack moved to ~63 MiB (`vmm::USER_STACK_TOP`), out of the heap's
    `.bss` region; desktop heap raised 8→24 MiB for the 8.3 MiB 1080p backbuffer.
  - Proof: `docs/bare-metal-1080p-desktop.png`.

- **Pointer acceleration** in the PS/2 mouse driver (2× baseline, 3× on fast
  flicks) — raw 1-count-per-pixel felt half-speed, especially at high res.
- **Whole "Menu" button is clickable** now, not just the icon glyph
  (`dingir_hit` widened to the full 4..66 px button).

### Added — "It runs DOOM." (demoable milestone, 2026-05-28)

- **DOOM live-boot entry**: a third GRUB menu entry, `Rusty Penguin -- DOOM
  (demoable)`, boots the Linux track straight into id Software's DOOM
  (shareware) rendering on the raw framebuffer — no X, no Wayland, no SDL.
  - `iso/doom-assets/`: vendored fbDOOM binary, shareware `doom1.wad`
    (md5 `f0cefca49926d00903cf57551d901abe`), and `doom-init.c` (a ~900 KiB
    static PID 1 that mounts devtmpfs/proc/sys and execs fbdoom on the WAD).
  - `iso/build.sh` assembles `initrd-doom.img` (rebuilding `doom-init` from
    source when gcc is present) and stages it into the ISO automatically.
  - Verified end-to-end in QEMU: boots from the ISO and reaches the E1M1
    attract-mode demo at 1280×800. Proof shot: `docs/doom-on-rusty-penguin.png`.

### Fixed / Learned — framebuffer requires UEFI on the Linux track

- The Linux track's framebuffer (`/dev/fb0`) only materialises under **UEFI**
  (OVMF / real UEFI firmware), where the kernel inherits the GOP framebuffer
  via built-in `efifb`/`simpledrm`. Under legacy BIOS + GRUB VESA, the stock
  Ubuntu kernel binds no DRM driver from our module-less initramfs, so
  `/dev/fb0` never appears (`/sys/class/drm` shows no `card0`). Boot the ISO
  in UEFI mode. This affects the graphical *desktop* track too, not just DOOM.

## [1.0.0] — 2026-05-26

### Initial release — "Binary hardware. Ternary mind."

This is the founding commit of Rusty Penguin, a ternary-first operating system
initiative written entirely in Rust.

#### What works today (userspace personality track)

- **psh** — Penguin Shell (v0.2): interactive REPL that runs as PID 1
  - `trit <n>` — convert any integer to balanced ternary Tryte representation
  - `mul <a> <b>` — double-width balanced ternary multiply
  - `div <a> <b>` — ternary integer division with remainder
  - `scale <n> <trit>` — one-trit conditional transform (Pos/Zero/Neg)
  - `ps` — list real Linux processes annotated with ternary state (+1/0/-1)
  - `activate / dormant / suppress <pid>` — send SIGCONT / SIGSTOP / SIGTERM
  - `ai [n]` — sparse ternary inference layer demo (skip-Zero efficiency)
  - `help / exit`

- **ternary-core** — Trit and Tryte primitives (range ±9841, 9 trits)

- **mathematics** — balanced ternary arithmetic: mul, div, mod, abs, consensus, any, scale

- **scheduler** — ternary process state model; real `/proc` scanning;
  heuristic: R/high-mem-S = Active (+1), idle-S/I = Dormant (0), Z/X = Suppressed (-1)

- **ai-runtime** — TernaryLinear sparse dot-product layer; Zero-dormancy skipping

- **init** — PID 1 process: mounts /proc /sys /dev /tmp, sets hostname, spawns psh

- **iso/build.sh** — grub-mkrescue pipeline to produce a bootable ISO using the
  host Linux kernel + a minimal initramfs containing only the init binary

#### Architecture — two parallel tracks

| Track | Status | Description |
|---|---|---|
| Userspace personality | Active | psh as /sbin/init on stock Linux kernel |
| Bare-metal kernel | Planned | x86_64-unknown-none no_std Rust kernel |

#### What comes next (Phase 2)

- `compiler/` — import ternlang-core lexer/parser/BET bytecode/VM
- `filesystem/` — ternary-annotated VFS layer
- `ipc/` — actor model (adapt ternlang-runtime TernNode to Unix sockets)
- `memory/` — TernPage: ternary-annotated memory pages via mmap/mprotect

---

Active research and development is ongoing. This repository tracks the full
Ternary Intelligence Stack (TIS) OS initiative.

Links:
- Ternary Intelligence Stack: https://ternlang.com
- TIS monorepo: https://github.com/rfi-irfos/ternary-intelligence-stack
