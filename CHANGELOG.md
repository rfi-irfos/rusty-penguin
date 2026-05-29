# Changelog — Rusty Penguin

All notable changes to this project will be documented here.

## [Unreleased]

### Added — Linux ABI layer brick 1: the bare-metal Rust kernel runs a real Linux binary (2026-05-29)

- **The from-scratch pure-Rust Rusty Penguin kernel executes an *unmodified*
  static Linux x86-64 ELF.** This is the first brick of the Linux ABI
  compatibility layer — the bridge that makes "the bare-metal OS is the total
  Ubuntu replacement" technically reachable: real third-party software runs
  *natively on our own kernel*, not a Linux kernel. Proof:
  `docs/linux-abi-brick1-serial.txt` (the ELF prints via Linux `write(2)` and
  ends via `exit_group(2)`).
- New `kernel/src/linux.rs`: a **per-process ABI mode** (Native vs Linux —
  needed because Linux syscall numbers collide with the native table, e.g.
  Linux `9=mmap` vs native `sys_ps`), the **SysV AMD64 initial stack**
  (argc/argv/envp/**auxv** with AT_PAGESZ/AT_RANDOM/AT_ENTRY), and a Linux
  syscall dispatcher (`write`, `writev`, `brk`, `mmap`, `arch_prctl`/TLS,
  `set_tid_address`, `rt_sig*`, `ioctl`, `clock_gettime`, `getrandom`,
  `exit`/`exit_group`; the rest log ENOSYS on serial for the next brick).
  Syscall outcome modelled as a ternary `Trit` (+1 ok / 0 EAGAIN / -1 errno).
- `kernel/src/syscall.rs`: the asm trampoline now stashes Linux args 4–6
  (`r10/r8/r9`) and `syscall_handler` routes Linux-mode processes to
  `linux::syscall`. Native apps are unaffected.
- `kernel/src/main.rs`: booting a bare-metal entry with `linuxtest` on the
  multiboot2 cmdline diverts to the Linux loader instead of the desktop.
- `kernel/linux-abi-test/linux-hello.c` (freestanding, `-nostdlib -static`) +
  `iso/build-linux-abi-test.sh` (build kernel + 1-entry GRUB ISO + headless
  serial capture). Roadmap + brick table: `docs/LINUX_ABI.md`.
- HONEST SCOPE: brick 1 of ~9+. Running glibc/threads/dynamic-linked GUI apps
  (let alone a browser) natively on our kernel is a multi-year road — see the
  doc. This proves the foundation, not the destination.

### Added — Mozilla Firefox renders too: "firefox and chrome gotta work" MET (2026-05-29)

- **A real Mozilla Firefox renders on Rusty Penguin's Linux track**, completing
  the "firefox and chrome gotta work" directive (both browsers now work). Full
  Firefox UI (tabs, address bar, menu) + the CSS/flexbox page render via the
  same Xorg + modesetting stack, with WebRender on its **software backend
  (swgl)** — no system GL needed. Proof: `docs/firefox-on-rusty-penguin.png`.
- Uses the **self-contained Mozilla tarball** (not the confinement-bound snap),
  cached to `iso/cache/firefox` (gitignored). `iso/build-web-rootfs.sh` gains
  an opt-in Firefox bundle (`RP_WEB_FIREFOX=1`): the 303 MB tree + its system
  ldd closure (GTK3/glib/pango/cairo — Firefox ships its own NSS + codecs).
- Launcher disables Firefox's sandboxes via env (`MOZ_DISABLE_CONTENT_SANDBOX`
  etc. — no user namespaces in the initramfs), forces software WebRender
  (`MOZ_ACCELERATED=0`), and points `FONTCONFIG_PATH=/etc/fonts`.
- `start-x.sh` now picks the browser by marker: Firefox > Chrome > xterm.
- Same honest scope as Chrome: initramfs proof, not the production install-to-disk
  layout; software rendering only.

### Added — Google Chrome renders real web content (2026-05-29)

- **A real, full Google Chrome (142) runs on Rusty Penguin's Linux track** —
  the "firefox and chrome gotta work" milestone. Full browser chrome (tabs,
  omnibox, menu) + a CSS/flexbox HTML page render correctly via the proven
  Xorg + modesetting + Mesa-swrast stack, painted by Chrome's software
  compositor (`--disable-gpu`, CPU raster → X). Proof:
  `docs/chrome-on-rusty-penguin.png`.
- `iso/build-web-rootfs.sh` gains an opt-in Chrome bundle (`RP_WEB_CHROME=1`,
  off by default so the lean rp.web stays xterm-only): copies the 374 MB
  `/opt/google/chrome` tree + its ldd closure + the NSS crypto stack
  (dlopen'd, ldd-invisible), and launches it on the X server.
- `iso/build-web-test.sh` (new): fast iteration path — builds *only* init + the
  web initrd and boots it directly via QEMU `-kernel`/`-initrd` (no bare-metal
  kernel, no grub-mkrescue, no ISO), with QMP screenshot capture.
- init mounts **`/dev/shm`** (tmpfs) — Chrome and many GUI apps require POSIX
  shared memory.
- Three Chrome-specific fixes: (1) **do not bundle `dri_gbm.so`** without its
  `libgallium`/`libLLVM` backend — a present-but-broken GBM loader makes Xorg
  modesetting fail hard ("couldn't get display device") instead of falling back
  to the shadow framebuffer; (2) **strip Chrome's `libqt{5,6}_shim.so`** — their
  closure drags in Qt6 core, but the Qt `xcb` *platform plugin* isn't shipped,
  so Qt `qFatal()`-aborted the browser; without the shims Chrome uses its
  built-in Views toolkit; (3) **gate client launch on X readiness** (poll the
  Xorg log for the DRISWRAST marker) — on a loaded host X init can take ~45 s,
  and clients launched earlier can't connect.
- HONEST SCOPE: this is Chrome in the **initramfs** (522 MB rootfs resident in
  RAM, 241 MB initrd) — a verified proof, not the production layout. Production
  belongs on the **install-to-disk RPDATA root** (too big for initramfs). Also:
  `--no-sandbox` (no user namespaces in the minimal initramfs) and software
  rendering only. Firefox is the same X stack, next.

### Added — X11 display server: real GUI apps render (2026-05-29)

- **Rusty Penguin runs a real X server and renders third-party Linux GUI apps.**
  A new `Web (X11)` GRUB entry / `rp.web` init mode starts **Xorg + the
  `modesetting` driver on virtio-gpu DRM + Mesa software GL (DRISWRAST)**, and a
  real **xterm** renders on the Linux track (proof: `docs/x11-xterm-on-rusty-penguin.png`).
  This is the foundation for running Firefox/Chrome.
- `iso/build-web-rootfs.sh` assembles the X stack (Xorg + xterm + full ldd
  closure + dlopen extras: xkb data, fonts, Mesa, xorg modules) + the DRM
  modules into `initrd-web.img`. Default desktop boot is untouched (separate
  initrd + entry).
- Five fixes were needed: bundle the *real* `/usr/lib/xorg/Xorg` (not the
  wrapper script); create `/var/lib/xkb`; add a `/bin/sh` symlink (Xorg runs
  xkbcomp via `popen()`); use **modesetting on DRM** instead of fbdev (fbdev
  never reached the visible scanout on truecolor efifb); and mount **devpts**
  (xterm needs a pseudo-terminal).
- Next: bundle Chrome (+ deps + the GBM `dri_gbm.so` EGL loader + a dbus
  session) onto the install-to-disk root — it's too big for the initramfs.

### Added — Package integrity verification (SHA-256, 2026-05-28)

- The repo index now carries a digest per package
  (`@pkg <name> <version> <url> <sha256|-> [deps]`); `rpm install <name>`
  downloads each package and **verifies its SHA-256 against the index** before
  installing — install aborts on mismatch (corruption/tamper detection).
- Unit-tested against the known `SHA-256("abc")` vector; index parses the digest
  field (9 tests pass).
- *Scope:* this is **integrity** (package matches the published digest);
  **authenticity** signing is below.

### Added — Package authenticity: ed25519-signed repo index (2026-05-28)

- `rpm update` now supports a **signed mode**: if a repo public key is
  provisioned at `/opt/rusty-penguin/repo.pub` (raw 32-byte ed25519), the index
  must ship a valid `.sig` (raw 64-byte signature) — verified with
  `verify_strict` before the index is accepted; update aborts on a bad/missing
  signature. With no key provisioned it falls back to unsigned with a clear
  UNVERIFIED warning (apt-style). The private key stays offline with the
  publisher and never ships in the OS.
- Together with the SHA-256 per-package digests, this gives end-to-end package
  trust: signed index → verified digests → verified packages. Closes the
  "package signing" gap (verification side).
- Verification unit-tested: valid sig passes; tampered message, wrong key, and
  malformed inputs all rejected (10 tests pass).

### Added — Package repository + dependency resolution (2026-05-28)

- `rpm update <url>` caches a repo **index in `.tern` format** (ternary-native):
  `@pkg <name> <version> <url> [dep …]`. `rpm install <name>` then resolves the
  transitive dependency closure (topological order, cycle-detected,
  skips already-installed) and installs each over HTTP via busybox wget.
- `install` now distinguishes a repo **name** (resolve from index) from a local
  `.rpkg` path or an `http(s)` URL (install directly).
- Dependency resolver unit-tested: topological ordering, shared-dep dedup,
  skip-installed, missing-dependency error, cycle detection (7 tests pass).
- Closes the "package manager + real repo + dependency resolution" gap.

### Added — Ternary CSS engine (brick 1) + browser strategy (2026-05-28)

- New `desktop-metal/src/css.rs`: a pure-Rust, no_std **CSS-subset styling
  engine**. Parses declarations (`background/color/border/accent/radius/
  pad-x/pad-y/border-width/shadow`) into a `Style`, and paints Apple-like
  panels (soft multi-layer shadow, rounded corners, hairline highlight +
  border). Every component carries a ternary `state` Trit:
  `+1` active (accent ring) / `0` normal / `-1` disabled (dimmed).
- First migration: the desktop's center welcome card is now rendered through
  the engine from a CSS string instead of hardcoded fill_rects — the start of
  moving the whole frontend from the "debug look" to a declarative,
  Apple-OS-grade aesthetic.
- **Brick 2:** CSS *selectors* via `StyleSheet` (`.name { … } .other { … }` →
  selector→Style lookup), and the **start menu** migrated to the engine — a
  rounded Apple-like panel (soft shadow, hairline header). Item geometry
  unchanged so hit-testing stays valid. Verified via screenshot.
- `docs/BROWSER_PLAN.md`: architecture decision + staged roadmap for running
  real browsers. Two complementary paths: (a) this native ternary CSS engine
  (pure Rust, owns the look, long road to web compat); (b) a real Linux X/Mesa
  stack + Chrome/Firefox on the install-to-disk root (pragmatic route to
  today's web; ISO grows to ~600 MB–1 GB). Honest: from-scratch web-platform
  parity with Firefox is a long-horizon effort.

### Added — Sparse "ternary" rendering: dirty-rect present (smooth dragging, 2026-05-28)

- Window dragging now uses **sparse damage tracking** — the concrete embodiment
  of ternary/sparse logic applied to rendering: on a pure drag frame the rest of
  the screen is **dormant**, so we recomposite the (always-correct) backbuffer
  but `present_rows` ONLY the window's damage band to VRAM, skipping the
  dominant full-screen MMIO copy. The drag handler computes the band as the
  union of the window's old+new vertical span; the periodic cursor-blink and
  topbar ticks provide self-correcting full presents.
- Verified in QEMU (QMP-driven drag): window renders correctly mid-drag and at
  the drop position, vacated area clean — no trails/corruption.
- This is the architecture thesis in miniature: sparsity designed in
  (dormant = skip the work), not bolted on. See `docs/BROWSER_PLAN.md`.

### Changed — Cache the static desktop background (drag perf groundwork, 2026-05-28)

- The bare-metal compositor cached the static scene (gradient + logo + icon
  dock): `recomposite` now blits the cache instead of recomputing the
  1080-row gradient and redrawing the dock every frame. Invalidated only on
  icon hover. Heap raised 24→32 MiB to hold the extra full-screen cache buffer
  (BSS still ends ~36 MiB, below the 40 MiB initrd and 63 MiB stack).
- **Honest scope:** this cuts per-frame *compute*, but the dominant cost of
  dragging a window at 1080p is the full-screen `present()` copy to VRAM (MMIO).
  Fully smooth dragging needs **dirty-rectangle present** (push only changed
  rows) — a compositor change queued for review, with this cache as its
  groundwork (`restore_bg`/`restore_bg_rect` can cheaply erase to background).
- Verified: desktop still renders correctly at 1920×1080 with the cache.

### Added — Install to disk: boot standalone, no ISO (2026-05-28)

- **`rp-install /dev/<disk>`** installs Rusty Penguin to a disk so it boots on
  its own — the biggest structural daily-driver gap, now closed. New `installer`
  workspace crate. Six stages: mount install media (isofs) → GPT partition
  (sgdisk: ESP + RPDATA) → format (mkfs.fat + busybox mke2fs) → copy bootloader
  + kernel + initrd to the ESP → sync. Requires an explicit target disk (it
  repartitions, so never guesses).
- Disk layout: `p1 RPESP` (FAT32) holds a standalone GRUB-EFI image
  (`grub-mkstandalone`, bundled) + `vmlinuz` + `initrd.img`; `p2 RPDATA` (ext4)
  is the persistent root that init mounts at /home.
- **Console / installer boot mode**: a new GRUB entry
  `Rusty Penguin -- Console / Install to disk` (kernel arg `rp.console`) boots
  to a text shell to run the installer; init skips disk auto-provisioning in
  this mode so it doesn't fight the installer for the target disk.
- Bundled into the initramfs: static busybox, `sgdisk` (+libs), `mkfs.fat`
  (+libs), `isofs.ko`, `nls_iso8859-1.ko` (vfat IO charset), and the standalone
  `BOOTX64.EFI`.
- **Verified end-to-end in QEMU/UEFI**: installed to a blank disk, then booted
  that disk with NO CD attached → OVMF → GRUB → kernel → init → desktop renders
  at 1280×800; RPDATA carries the persistent /home + /opt and a written
  `boot.tern`. Proof: `docs/installed-disk-standalone-boot.png`.

### Fixed — Partition-aware persistence (data-loss footgun + installer prereq, 2026-05-28)

- Persistence no longer blindly `mke2fs`-es the first disk it finds — which
  would have **destroyed the partition table of an installed system**. New
  resolution order: (1) mount an existing `RPDATA`-labeled partition, (2) else
  auto-provision only a genuinely *blank* whole disk (no partition table), (3)
  else ephemeral. The ext label is read straight from the superblock (no blkid).
- Verified in QEMU: a pre-partitioned disk with an `RPDATA` ext4 partition is
  mounted as-is (marker file preserved, not wiped); a blank disk still
  auto-provisions. This is the prerequisite for install-to-disk.

### Added — Network package install + persistent packages + recovery console (2026-05-28)

- **`rpm install <url>`** — the package manager now installs over the network,
  not just from local files: an `http(s)` argument is fetched with the bundled
  static busybox `wget` to /tmp, then installed. (The package manager was also
  previously an orphan module on the Linux track — now wired into the psh
  command dispatch with `mod pkg` + an `rpm` builtin; unit tests added.)
- **Installed packages persist**: init bind-mounts `/persist/opt` → `/opt`, and
  `/opt/rusty-penguin/bin` is on PATH, so `rpm`-installed software survives
  reboots. Verified `/opt` lands on the persistent disk in QEMU.
- **Recovery console**: if the desktop can't start (e.g. no `/dev/fb0`) init now
  drops to a shell instead of freezing. Fixed an infinite desktop↔psh relaunch
  loop (desktop's fallback now sets `RP_RECOVERY=1` so the shell doesn't bounce
  back into the desktop). The Linux track is no longer a dead end without a
  display.
- Network fetch path verified end-to-end in QEMU: busybox wget connects to the
  host over the DHCP'd link and completes the HTTP round-trip.

### Added — Networking userland, the ternary way (daily-driver gap, 2026-05-28)

- `init` now brings up the network: first non-loopback interface up + a DHCP
  lease via the bundled static busybox `udhcpc` (IP, netmask, default route,
  DNS → /etc/resolv.conf). virtio_net is built into the kernel, so no module
  bundling needed.
- Link state is a **ternary `Trit`** with reachability semantics:
  - `+1` Active — lease acquired **and** default gateway pings back
  - ` 0` Dormant — link up but no lease, or lease without reachable gateway
  - `-1` Suppressed — no network device
- A gateway reachability probe (busybox `ping`) means `+1` is "the network path
  actually works", not merely "got an IP".
- Network state is recorded in the `.tern` boot record (`@network +`).
- Verified in QEMU (virtio-net, user-mode): `eth0 → 10.0.2.15/24`, lease from
  10.0.2.2, gateway reachable.

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
