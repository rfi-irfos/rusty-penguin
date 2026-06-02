# Changelog — Rusty Penguin

All notable changes to this project will be documented here.

## [Unreleased]

### Added — Item 6: virgl 3D resource path end-to-end proved (2026-06-02)

- 6-step virgl pipeline verified against `-device virtio-gpu-gl -display egl-headless`:
  GET_CAPSET_INFO → GET_CAPSET (308-byte capability blob) → CTX_CREATE → RESOURCE_CREATE_3D
  (1 KiB vertex buffer on host GPU, cmd 0x0207) → CTX_ATTACH_RESOURCE → clean teardown.
  Every step round-trips through virglrenderer on the host.
- `iso/build-virgltest.sh` reproducible headless CI.

### Added — Item 5: ACPI S3 suspend-to-RAM — trampoline + QMP-verified (2026-06-02)

- 121-byte real-mode resume trampoline (real16 → pm32 → lm64), written to phys 0x8000
  at suspend time. Restores CR4, boot CR3 (PML4[0] identity-mapped), LME, paging, RSP,
  then jumps to acpi_s3_resumed() in the kernel image.
- parse_s3() reads \_S3_ SLP_TYP from DSDT; FACS.firmware_waking_vector = 0x8000.
- vmm::save_kernel_cr3() snapshots boot CR3 before any private address spaces are created.
- `iso/build-s3test.sh` QMP end-to-end CI: QEMU status=suspended → system_wakeup → serial
  "resumed from S3 — kernel alive". PASS.

### Added — UI palette pivot: graphite/teal/amber "premium automotive UI" (2026-06-02)

- CSS + bare-metal window chrome + dock rethemed: deep graphite, teal accent (#6dd3c6),
  amber (#d8b26e), frosted glass panels, 4-layer atmospheric shadow, Aero spatial depth.
- cargo check: 0 errors.

### Added — Windowed DOOM is PLAYABLE: keyboard routed to the focused scheduled app (2026-06-02)

- The capstone. Keys reached a Linux process only when it was the current task at
  IRQ time (racy). Now, when a scheduled Linux app exists, the keyboard IRQ routes
  every raw scancode to it regardless of which task was running. QEMU-verified:
  with windowed DOOM up, sending keys moves the player through E1M1 inside the
  desktop window (11616 px changed, 0 faults). The `enter()` path and the normal
  desktop boot are unaffected. Proof: docs/doom-windowed-playable.png.

### Added — WINDOWED DOOM: real id Software DOOM runs in a desktop window (2026-06-02)

- **The headline goal.** id Software DOOM (fbdoom, a dynamic PIE) runs as an
  isolated, preemptively-scheduled Linux process in its own address space (ld.so +
  glibc), renders into a private 640×400 `/dev/fb0` surface, and the real desktop
  composites that surface into a titled on-screen window — E1M1, the marine view,
  the full HUD, in a window on the bare-metal pure-Rust desktop. 0 faults.
  Proof: docs/doom-windowed-on-desktop.png.
- DOOM-specific shakeout: `FBIOGET` reports the 640×400 surface for scheduled
  processes; a scheduled app's stdout goes to serial only (not the fb console);
  and `write()`/`lseek` to `/dev/fb0` (fbdoom's frame blit) now copies pixels into
  the surface. `selftest_linuxwin` uses the dynamic loader, so `linuxwin` runs the
  static fb-test or the real fbdoom. Fullscreen DOOM (enter) regression-checked.

### Added — Windowed-DOOM brick 4: desktop composites a scheduled Linux app into a window (2026-06-02)

- The whole windowed-Linux-app pipeline, end to end and **visible**. The real
  desktop runs as a scheduled process; a scheduled Linux process renders into its
  private `/dev/fb0` surface (brick 3); the kernel maps that 640×400 surface
  read-only into the desktop's AS; the desktop reads it via `sys_app_surface` (#41)
  + `sys_app_surface_dims` (#42) and blits it into a titled on-screen window.
- QEMU screendump: the desktop with a "Linux app (scheduled)" window full of the
  app's orange, 0 faults (docs/multiproc-linux-app-windowed.png). Normal boot
  unchanged (flag-gated behind `linuxwin`).
- Only step left for windowed DOOM: swap the synthetic fb-test for the real fbdoom
  binary (a dynamic PIE that loads via the brick-2b path) and shake out its
  DOOM-specific private-AS syscalls.

### Added — Windowed-DOOM brick 3: a scheduled process gets a private framebuffer surface (2026-06-02)

- The virtual `/dev/fb0` windowed DOOM needs. A scheduled Linux process that mmaps
  `/dev/fb0` gets a private, physically-contiguous 640×400 surface (not the real
  hardware framebuffer, which it can't see in its AS and would draw over the whole
  screen); the desktop composites that surface into a window (brick 4). The
  `enter()` single-process path still gets the real FB.
- `pmm::alloc_frames(n)`: contiguous multi-frame allocator (skips the low 1 MiB so
  a base of phys 0 can't collide with a null sentinel). `set_fb_surface` lets the
  kernel pre-allocate the surface (a value a process writes into a kernel global
  isn't reliably visible back on the boot thread; a boot-written one is).
- New `linuxfb` self-test (static `fbtest.c`): QEMU-verified, the process renders
  into the private surface and the kernel reads it back fully orange, 0 faults.

### Added — Windowed-DOOM brick 2b: a DYNAMIC Linux ELF (ld.so + libc) as a scheduled process (2026-06-02)

- The hard merge that was blocking windowed DOOM (DOOM is a dynamic PIE).
  `spawn_linux_dyn_elf` loads the PIE at a bias + the interpreter (ld.so) into a
  private address space, builds the System V auxv (AT_BASE/AT_ENTRY), and jumps to
  ld.so, which relocates the program and mmaps libc from the initrd — all in the
  task's own preemptively-scheduled AS, not the one-way `enter()` path.
- `brk` now also backs its arena with real frames in a private AS (tracking
  `BRK_MAPPED` so a partial heap page is never re-allocated). With the earlier
  `mmap` fix, the two private-AS gaps that `enter()`'s identity map hid are closed.
  Both gated on `is_preemptive_linux()`; the `enter()` path is unchanged.
- New `linuxdyn` self-test: brick4-dyn (dynamically-linked glibc) as a scheduled
  process. QEMU-verified: ld.so relocates + runs it, its glibc `printf` reaches
  serial, clean reap, scheduler healthy, 0 faults. Regression: the `enter()`
  dynamic path still runs it (printf + exit 0).
- Remaining for windowed DOOM: brick 3 redirect the process's `/dev/fb0` to a
  private surface, brick 4 composite it into a desktop window.

### Added — Windowed-DOOM brick 2b (foundation): mmap into a private address space (2026-06-02)

- The precise blocker for any *dynamic* Linux process (ld.so mmapping libc, DOOM)
  running scheduled. Linux `mmap` wrote straight to the arena VA — fine under the
  identity-mapped `enter()` path, but a scheduled task lives in its own private AS
  where the arena is unmapped. Now, when `is_preemptive_linux()`, `mmap` backs
  `[p, p+len)` with real frames mapped into the current AS. `enter()` unchanged.
- New `linuxmmap` self-test (ELF built in-kernel): a scheduled Linux process
  `mmap(…ANON…)`, writes "MMAPOK" into the buffer, `write`s it back, `exit(0)`.
  QEMU-verified: MMAPOK reaches serial, clean reap, scheduler healthy, 0 faults.
- Remaining for full ld.so: MAP_FIXED-over-reservation refinement + verifying
  ld.so/libc's other syscalls (openat/mprotect/read) from a private AS.

### Added — Windowed-DOOM brick 2a: a real static Linux ELF as a SCHEDULED process (2026-06-02)

- A real Linux binary now runs as one of several preemptively-scheduled processes
  in its own private address space — not via the one-way `enter()` path.
  `spawn_linux_static_elf` loads an ET_EXEC into a private AS and builds the System
  V AMD64 initial stack (argc/argv/envp + auxv: AT_PHDR/PHENT/PHNUM/PAGESZ/BASE/
  ENTRY/RANDOM) the C runtime expects, staged into the top stack frame via physmap.
- Scheduled Linux exit: a Linux task under the scheduler that calls exit/exit_group
  is now *reaped* (`sched::exit_current_scheduled` marks it dead; `next_alive` skips
  it) so the desktop + others keep running, instead of halting the CPU.
- New `linuxsched` self-test, QEMU-verified: the real static linux-hello ELF runs
  scheduled, prints its stdout, exits + is reaped, boot thread survives, 0 faults.
  Regression: the `enter()` Linux path still runs the same ELF (hello + exit 0).
- Remaining for windowed DOOM: brick 2b dynamic ELF (ld.so+libc, DOOM is a PIE),
  brick 3 redirect `/dev/fb0` to a private surface, brick 4 composite into a window.

### Added — Windowed-DOOM brick 1: per-task ABI mode (Linux process + native desktop scheduled together) (2026-06-02)

- The architectural foundation for windowed DOOM. DOOM is a Linux-ABI process; the
  desktop is native — to window DOOM they must run concurrently, each routing
  syscalls to the right table. ABI mode was a global one-way switch (`enter()`
  never returns); now it is per-task (`TASK_LINUX[]` + `linux::set_linux` pushed by
  `preempt_tick` on every switch; `mark_task_linux()`).
- Fixes a real latent bug: `extra_args()` took `&_lx_a4` → a GOT-indirect load, and
  the kernel's GOT sits in low memory only mapped in the boot context — so any Linux
  process running in a private address space #PF'd at syscall entry. Now read direct
  rip-relative (no GOT).
- New `linuxroute` self-test: a native task (0x1337) + a Linux task (`write` →
  "LINUXROUTE") scheduled in separate private address spaces. QEMU-verified: native
  routed 11×, Linux routed, 0 faults; the `enter()` Linux path still runs an
  unmodified Linux ELF (regression). Remaining for windowed DOOM: load a dynamic
  Linux ELF (ld.so+libc) into a private AS as a scheduled task, redirect its
  `/dev/fb0` to a private surface, composite into a desktop window.

### Added — Multiproc brick 6: the desktop composites a 2nd real app into an on-screen window (2026-06-02)

- Item 4's visible payoff — windowed multi-app, real and visible. The real desktop
  runs as an isolated, preemptively-scheduled process; a SECOND real app process
  renders into its private surface; the kernel maps that surface read-only into the
  desktop's address space (0x3000000, its free VA gap); the desktop reads it via
  `sys_app_surface` (#41) and blits it into a titled on-screen window (4× scaled).
  The desktop — itself a scheduled process — hosting another isolated process's
  output in a window, end to end. The same model scales to windowed DOOM.
- Now safe because the per-task syscall stack makes concurrent syscalls #GP-free.
  Flag-gated behind `schedesktop2`; normal boot path unchanged.
- Verified in QEMU: orange app window composited on the live desktop, exactly
  128×128 surface px, 0 faults; normal-boot regression 0 faults / clean desktop.
  Proof: docs/multiproc-windowed-app-on-desktop.png.

### Added — GPU: virgl 3D control-path probe (negotiate F_VIRGL, capset, CTX_CREATE) (2026-06-02)

- The next virgl brick past detection: prove the 3D command channel round-trips
  through the host virglrenderer (the transport every later virgl brick rides on).
  Behind a `virgltest` boot flag: negotiate `VIRTIO_GPU_F_VIRGL`, `GET_CAPSET_INFO`
  (→ capset id 1 / max-version 1 / max-size 308), `CTX_CREATE` a 3D context, then
  `CTX_DESTROY`. Verified with `-device virtio-gpu-gl -display egl-headless`; the
  2D scanout self-test still passes. Fully flag-gated — a default boot does NOT
  negotiate VIRGL and the 2D path is unchanged. Remaining (multi-year):
  `RESOURCE_CREATE_3D` + `SUBMIT_3D` with a real GL/TGSI command stream.

### Added — GPU: detect virtio-gpu VIRGL 3D capability (item-6 foundation) (2026-06-01)

- The first real brick toward 3D acceleration: the kernel reads the GPU's full
  feature set, recognises `VIRTIO_GPU_F_VIRGL`, and reads `num_capsets` from the
  device config (non-zero only when the host's virglrenderer is live). `has_3d()`
  accessor added; the previously-ignored DEVICE_CFG cap is now captured.
- Read-only detection — VIRGL is NOT negotiated yet, so the 2D scanout path is
  unchanged. Two-sided QEMU verification: `-device virtio-gpu-gl -display
  egl-headless` → "VIRGL 3D offered — host capsets 2"; `-device virtio-gpu` →
  "no VIRGL — 2D only"; both still pass the 2D scanout self-test. The virgl 3D
  command stream itself remains the multi-year part.

### Added — WiFi: WPA2 authentication core (PSK/PMK/PTK), host + bare-metal verified (2026-06-01)

- The hardware-independent half of bare-metal WiFi. `kernel/src/wpa2.rs` (pure
  `core`, no alloc, host-testable): SHA-1, HMAC-SHA1, PBKDF2-HMAC-SHA1,
  `wpa_passphrase_to_psk` (PMK), and the IEEE 802.11i PRF + `ptk()` pairwise key
  expansion (KCK/KEK/TK).
- Verified against published vectors, no self-graded numbers: SHA-1 (FIPS 180-1),
  HMAC-SHA1 (RFC 2202), PBKDF2 (RFC 6070 c=1/2/4096), the WPA PMK from IEEE
  802.11i §H.4 (`password`/`IEEE` → `f42c…a12e`), and PTK determinism + nonce-
  dependence. Host: `rustc -O tools/wpa2_test.rs`; boot: QEMU serial
  `[wifi: WPA2 auth core OK (PMK/PTK vectors verified)]`.
- Remaining for a real association: the radio MMIO/firmware bring-up + the EAPOL
  4-way handshake state machine wired to the driver (needs real Intel hardware).

### Added — Power: software brightness control (Quick Settings slider + boot default) (2026-06-01)

- A real, usable brightness control that works on ANY panel — including those with
  no ACPI/hardware backlight (and QEMU). `present()` maps every byte to the real
  screen through a precomputed `v*b/100` LUT when brightness < 100; brightness == 100
  fast-paths to a plain block copy (zero overhead). Clamped to a 15% floor so the
  screen never goes fully black.
- Quick Settings gains a brightness slider (sun icon) below the volume slider.
- `brightness=N` boot arg (`sys_boot_brightness` #40) boots the desktop pre-dimmed
  — a default/kiosk/accessibility knob and the headless verification idiom.
- Verified in QEMU: `brightness=40` → whole desktop at 0.380× baseline luminance
  (expected ~0.40); normal boot unchanged. Proof: docs/brightness-40pct.png.

### Fixed — Multiproc brick 5: per-task syscall stack → desktop + a 2nd real app concurrently, no #GP (2026-06-01)

- The genuine scheduler-isolation maturity fix. The SYSCALL trampoline switched
  to ONE shared kernel stack and stashed the user RSP in ONE global. Under
  preemption that is unsound: when the desktop blocked in a syscall
  (`sys_read` → `sti`+`hlt`) and a second app entered a syscall in that window,
  the second entry clobbered the first's saved frame + user RSP → on resume the
  trampoline `iretq`/`sysretq`'d with a garbage CS/RSP → kernel **#GP**.
- Fix: a **per-task syscall stack** (`_cur_syscall_stack`, defaults to the shared
  stack so the normal single-process boot is byte-for-byte unchanged). The
  trampoline switches to `[_cur_syscall_stack]` and pushes the user RSP onto that
  per-task stack (restored via `pop rsp`); `preempt_tick` retargets it to the next
  task's own kernel stack on every context switch, alongside `TSS.rsp0`. Written
  via inline asm (direct rip-relative — a plain Rust store to a `global_asm`
  symbol compiles to a GOT-indirect access whose slot is unmapped in the
  higher-half kernel → #PF). An 8-byte pad keeps the C call site 16-aligned.
- `schedesktop2` boot flag (checked before `schedesktop`, a substring): runs the
  REAL desktop AND a second real ELF app as two independent, preemptively-
  scheduled, address-space-isolated processes. Verified by QEMU screendump: no
  #GP, the full desktop renders (16046 non-black cells) while the 2nd app is
  scheduled. Normal-boot fault profile is identical to baseline (regression-safe).
  Proof: docs/multiproc-desktop-plus-app-scheduled.png.
- Remaining toward windowed DOOM: the desktop compositing the 2nd app's surface
  into a visible on-screen window (a desktop-code change, the easy part now that
  two real processes coexist safely under the scheduler).

### Added — Multiproc brick 4: the REAL desktop runs as a scheduled process (2026-06-01)

- The bridge from synthetic test apps (3a–3e) to the real multi-process desktop:
  load the actual 11 MB desktop-metal ELF through `spawn_ring3_elf` and run it as
  a preemptively-scheduled ring-3 process in its own private address space.
- `spawn_ring3_elf_cfg` generalizes the loader with a configurable multi-page
  user stack at the program's expected VA (the desktop wants its 16-page stack at
  USER_STACK_TOP).
- `schedesktop` boot flag → load bin/desktop into a private AS (24 MiB heap
  mapped + zeroed via the physmap), schedule it, enable preemption; the boot
  thread idles while the desktop gets the CPU and renders.
- Verified by QEMU screendump: the full desktop UI renders (wallpaper, hero card,
  dock — 11773 non-black cells, same as the normal boot), zero faults. Proof:
  docs/multiproc-real-desktop-scheduled.png.
- So the real complex desktop runs correctly under the scheduler in an isolated
  address space. Remaining toward windowed DOOM: a SECOND real app alongside it +
  the desktop compositing that app's surface (a desktop-code change).

### Added — Multiproc brick 3e: hung windowed app force-quit, others keep running (2026-06-01)

- Item 4's entire goal in one scene, combining isolation (1/2) + watchdog (2) +
  windowed compositing (3c/3d).
- `recoverwin` boot flag → a healthy app and a wedging app, each its own isolated
  process + window. The boot thread runs the compositor *and* a watchdog: it sees
  the wedged app make no syscall progress, force-quits it, and closes its window
  with a "Not Responding" overlay — while the healthy app keeps rendering.
- Verified by QEMU screendump: healthy window stays up (1024 px), hung window is
  gone (0 px) and replaced by the closed overlay, zero faults → "RECOVER PROVEN".
  Proof: docs/multiproc-hung-app-recovered.png.
- This is item 4 as written ("a hung app can't freeze the desktop, and you can
  run several real apps at once") proven end to end and on screen. The
  kernel/compositor model is complete; remaining is swapping the test apps for
  the real desktop + real apps (fbDOOM) through this same pipeline.

### Added — Multiproc brick 3d: two real app processes in two windows at once (2026-06-01)

- Closes the second half of item 4's goal ("run several real apps at once"; the
  first half — a hung app can't freeze the desktop — is bricks 1/2), visibly.
- `multiwin` boot flag → two independent ELF processes, each in its OWN private
  address space rendering its OWN colour into its OWN surface (orange + blue, same
  VA but fully isolated); the compositor blits BOTH into separate windows.
- Verified by QEMU screendump: an orange window and a blue window appear at once,
  zero faults. Proof: docs/multiproc-two-windows.png.
- With 3a (real ELF processes), 3b (offscreen surfaces) and 3c (compositor →
  screen), the whole windowed multi-app model is proven end to end and on screen,
  behind flags. Remaining: wire the real desktop + real apps (fbDOOM) through it.

### Added — Multiproc brick 3c: compositor blits a process's surface to screen — VISIBLE (2026-06-01)

- The full windowed-app pipeline, end to end and on screen: an isolated,
  preemptively-scheduled ELF process renders into its OWN offscreen surface, and
  the kernel compositor blits that surface into a bordered window on the real
  framebuffer — the exact model a window manager uses.
- `FILL_STUB` ring-3 program fills its private 32×32 surface with orange;
  `composite` boot flag → the compositor reads the surface (physmap) and blits it
  (titlebar + frame) into a window region.
- Verified by QEMU screendump: exactly 1024 (32×32) orange pixels at the window
  position, zero faults. Proof: docs/multiproc-compositor-window.png.
- This closes the conceptual gap to windowed DOOM (isolated process → offscreen
  render → compositor → on-screen window all work together). Remaining: wire the
  real desktop + a real app (fbDOOM) through this proven pipeline.

### Added — Multiproc brick 3b: isolated offscreen rendering (compositing foundation) (2026-06-01)

- The piece between "isolated processes" and "windowed apps": a process renders
  into an offscreen framebuffer private to it, and the compositor reads it back.
- `spawn_ring3_elf` gained an optional fb_frame — a kernel-owned frame mapped
  into the process at FB_VA (0x800000); the process renders there, the kernel
  reads it via the physmap (no shared writable surface).
- `offscreen` boot flag → a render program writes a 0xDEADBEEF marker into its
  private buffer; the boot thread (compositor stand-in) reads it back.
- Verified in QEMU: the render process runs under preemption with the boot
  thread, which reads 0xDEADBEEF out of the process's offscreen frame →
  "OFFSCREEN RENDER PROVEN", zero faults. This is the model a real compositor
  uses (each app renders to its own surface; the WM blits it into a window).

### Added — Multiproc brick 3a: real ELF programs as isolated scheduled processes (2026-06-01)

- Bricks 1/2 used synthetic hand-written stubs; this adds the loader path a
  multi-process desktop needs: load a genuine ELF into its own private address
  space and schedule it as a ring-3 process.
- `spawn_ring3_elf` walks an ELF's program headers and loads each PT_LOAD
  segment into freshly-allocated frames mapped into a new private address space
  (written through the higher-half physmap, no CR3 switch), then schedules a
  ring-3 task at `e_entry`. Reusable for any ELF (the desktop, fbDOOM).
- `realelf` boot flag runs two real ELF programs at the same virtual address in
  separate private spaces under preemption with the boot thread.
- Verified in QEMU: both ELF tags interleave with the boot thread (37× / 37× /
  26×), zero faults → "REAL-ELF MULTIPROCESS PROVEN". Next: load the real desktop
  + an app this way and arbitrate the framebuffer (virtual /dev/fb0 + compositing).

### Fixed — Real DOOM was broken by an mmap/initrd overlap; in-OS Doom faced the wrong way (2026-06-01)

- Real fbDOOM on the bare-metal kernel opened DOOM1.WAD and bailed with "doesn't
  have IWAD or PWAD id". Root cause: the Linux `MMAP_BASE` arena started at the
  exact address the initrd is relocated to (128 MiB), so ld.so's library mmaps
  **overwrote the WAD in place**. Moved the arena to 160 MiB. Real fbDOOM now
  loads the WAD, runs W_Init/R_Init, and reaches I_InitGraphics with the real
  1920×1080 framebuffer (serial-verified). Also added `KDGKBTYPE`/`KDSKBMODE`
  ioctls + raw-scancode delivery so its keyboard initializes, and re-enable the
  PS/2 keyboard in `enter()` so IRQs keep flowing to a console app. (A headless
  QMP input nuance still blocks the final pre-game calibration; needs a real
  keyboard to confirm fully playable.)
- The in-desktop "Doom" app (WadDoom, real E1M1 geometry) rendered a flat color:
  the world Y axis is negated but the facing angle wasn't, pointing the camera
  into the wall behind the spawn. Negated the angle to match — it now faces and
  navigates correctly. (Still a basic flat-shaded wall renderer, not fbDOOM.)

### Added — Multiproc brick 2: watchdog force-quits a hung process (2026-06-01)

- Brick 1 proved a wedged process can't *freeze* the others; this adds
  *recovery* — detect a not-responding process and force-quit it.
- Per-task syscall counter as a liveness signal; `kill_task` drops a task from
  the schedule (the scheduler already skips dead tasks). `watchdog` boot flag →
  healthy process A + wedged process B; the kernel watchdog notices B makes no
  progress and terminates it.
- Verified in QEMU: "process B NOT RESPONDING — force-quitting", "B terminated",
  "B syscalls (frozen): 0  A syscalls (still climbing): 11", A keeps running →
  "RECOVERY PROVEN". (Heuristic is a stand-in; frame reclamation is a follow-up.)

### Added — Multiproc brick 1: a hung app can't freeze the system (2026-06-01)

- First step toward a multi-process desktop, incremental and behind a flag so
  the working single-process desktop is untouched. The kernel already had timer
  preemption, per-process private address spaces, ring-3, and per-task CR3; this
  demonstrates the property a multi-process desktop needs — *a wedged process
  must not stall the others*.
- `multiproc` boot flag → `selftest_multiproc` spawns two ring-3 processes in
  separate private address spaces: process A healthy (syscalls periodically),
  process B HUNG (a pure `jmp $` infinite loop that never yields, so only the
  preemption timer can take the CPU back).
- Verified in QEMU: while B spins forever, A's syscall keeps arriving (18×) and
  the kernel keeps running (12×) → "ISOLATION PROVEN".
- Next (bigger): spawn a real app (fbDOOM) as a scheduled process alongside the
  desktop, then migrate desktop apps to the multi-process model one at a time.

### Added — ACPI power management: clean shutdown + reboot (2026-06-01)

- The kernel had no power management — the only way out was closing the VM or a
  triple fault, and the desktop "Shut Down" button was a halt-only stub.
- `acpi.rs` parses the firmware tables (RSDP → RSDT/XSDT → FADT, and the FADT's
  DSDT for the `\_S5` package), read through the higher-half RAM physmap, then:
  - `poweroff()` — real ACPI S5 soft-off via the PM1a/PM1b control ports;
  - `reboot()` — FADT reset register with 8042 (`0x64←0xFE`) and PCI (`0xCF9`)
    fallbacks.
- Syscalls 37 (`sys_poweroff`) / 38 (`sys_reboot`); the start-menu "Shut Down"
  button now actually powers the machine off.
- Verified in QEMU: probe reads `PM1a=0x604`, S5 type 0, reset supported;
  `acpipoweroff` enters S5 and the VM powers off cleanly (qemu exits rc=0);
  `acpireboot` resets.
- Scope: shutdown + reboot. Battery (`_BST`, needs an AML interpreter),
  brightness, and S3 suspend/resume are larger follow-ups.

### Changed — RPFS v2: a real filesystem (reclamation, thousands of files, directories) (2026-06-01)

- RPFS v1 was a demo — 16 files, one flat directory, append-only data that
  leaked blocks forever (delete/overwrite never reclaimed space). Replaced with
  a real filesystem.
- `rpfs.rs` — block-bitmap FS core, generic over a `BlockDev` trait so it is
  host-testable. First-fit contiguous-extent allocation over a free-block
  bitmap: deleting/overwriting a file frees its blocks for reuse (no more leak).
  2048-entry directory (128× v1's 16). Hierarchical `/`-separated paths with
  real directory entries (`mkdir`, `list_dir`), parents auto-created on write.
- Heap-disciplined for the kernel's bump-only allocator: directory, bitmap, and
  one I/O scratch buffer are allocated once at mount; file reads/writes stage
  through the scratch, so steady state never touches the heap. Kernel heap grown
  512 KiB → 1 MiB (still under the 4 MiB user-load line; `.bss` ends at 3.79 MiB).
- `tools/rpfs_test.rs` — host test over a RAM disk: 1800 files across 50 nested
  dirs, `list_dir` correctness, **reclamation** (delete 900 → blocks freed →
  rewrite 900 reuses them), overwrite-shrink reclaim, and persistence across a
  remount. ALL CHECKS PASSED.
- Verified in QEMU on real AHCI across a reboot: boot 1 formats (515457 blocks),
  reclaims on overwrite, lists a nested directory; boot 2 reads the persisted
  marker back with 5 files intact.
- Note: new superblock magic (RPFS2027) — an existing v1 disk is reformatted on
  first boot.

### Added — WiFi brick 1: Intel iwlwifi card detection + firmware parser (2026-06-01)

- Started the hardest brick in the OS, honestly. Native WiFi is firmware-driven
  and per-chip, and QEMU does not emulate iwlwifi — so device bring-up can only
  be verified on a real Intel-WiFi laptop. We lead with the host-verifiable parts.
- `iwlwifi_fw.rs` — parser for Intel's TLV `.ucode` firmware format (magic,
  version, full walk of instruction/data/section TLVs, truncation guards). Pure
  `core`, no kernel deps, so it host-tests like `bignum.rs`.
- `tools/iwlwifi_fw_test.rs` — parses a genuine Intel image. Verified against
  `iwlwifi-9260-th-b0-jf-b0-34.ucode` (2.6 MB): magic OK, 61 TLVs, 37 loadable
  sections, 898 KB of instructions, largest section 241 KB (sizes the DMA
  staging), api v34 matching the `-34` suffix; corrupt magic rejected, truncated
  image flagged.
- `iwlwifi.rs` — PCI detection (class 02:80, vendor 8086) + a device-id table
  mapping common laptop parts (7260 → AX211) to chip + firmware family; boot
  probe logs the find and the firmware it needs. Under QEMU it honestly reports
  "no Intel WiFi card".
- Next (brick 2, needs real hardware): MMIO/prph access, firmware DMA upload +
  the ALIVE handshake, RX/TX rings, then 802.11 + WPA2.

### Added — Browser build-out: security indicator, back/forward history, page titles (2026-06-01)

- Security indicator wired to the new cert validation: the kernel tracks the
  last fetch's trust state (`net.rs LAST_FETCH_TRUST`, exposed via
  `sys_fetch_trust` #36); the address bar draws a green closed padlock for a
  validated HTTPS page, amber open padlock for plain HTTP. Because `https_get`
  only returns bytes when the chain validated, "https delivered a page" ==
  "verified secure". Verified in QEMU (amber lock for google's plain-HTTP
  redirect target, matching reality).
- Real back/forward history stack (replaces back-to-home-only); forward button
  added, both gray out when empty; reload re-fetches without a history entry.
- `<title>` parsed from the page and shown in the window chrome.

### Added — TLS certificate-chain trust: the handshake now authenticates *whose* key (2026-06-01)

- The TLS 1.3 handshake did ECDHE + AES/ChaCha correctly but accepted *any*
  certificate — the Certificate message was folded into the transcript and
  thrown away. A man-in-the-middle with any valid-looking cert was invisible.
  Now the server's certificate chain is parsed and validated to an embedded CA
  root before any application data flows; on failure the connection is aborted.
- `bignum.rs` — Montgomery (CIOS) modular exponentiation for RSA signature
  verification (`sig^e mod n`). Host-fuzzed against OpenSSL at 2048 and 4096
  bits. The half of a trust store that P-256 doesn't cover.
- `x509.rs` — from-scratch DER/ASN.1 parser → certificate fields; RSA-PKCS#1
  v1.5 verify (EMSA padding + SHA-256 DigestInfo) via `bignum`; ECDSA-P256 via
  `p256.rs`; chain walk leaf → intermediate(s) → a trusted root; SAN hostname
  match (incl. single-label wildcard) and notBefore/notAfter expiry against the
  CMOS real-time clock. An anchor is matched by subject + public key, so a root
  the server sends in-band is trusted only if it equals our embedded copy.
- `ca_roots.rs` — real embedded trust anchors: GTS Root R1 (Google) and ISRG
  Root X1 (Let's Encrypt).
- `tls.rs` — splits the Certificate message's `certificate_list` into DER blobs
  and calls `x509::validate_chain` after the handshake completes; rejects with a
  human-readable reason (expired / bad hostname / bad signature / untrusted /
  malformed). This is the exact path the browser's HTTPS fetches use.
- Boot self-test fetches **www.google.com** end to end: leaf → WR2 → GTS Root
  R1 (all `sha256WithRSAEncryption`), validated, then `HTTP/1.1 200 OK` over the
  trusted channel. Verified in QEMU:
  `[tls] cert chain TRUSTED (www.google.com)` and
  `[x509] chain self-test OK (valid chain trusted, tampered rejected)`.
- Scope, honestly: signature + chain + validity + hostname. Not yet:
  basicConstraints CA flags, keyUsage, name constraints, the leaf
  CertificateVerify signature, or revocation (CRL/OCSP). Real, and next.

### Changed — Desktop v2 visual redesign from Simeon's mockup (2026-05-29)

- Adopted the warm-stone-green palette from `docs/design/rusty-penguin-os-mockup.html`
  (bg `#1B211E`, green `#6FE18B`, warm text `#ECEDE5`, ternary triad
  `neg/zero/pos = #EF7575/#909A92/#6FE18B`), replacing the cool graphite +
  system-blue. Warm-stone wallpaper gradient with a soft green glow up top.
  Clock kept in the topbar (per Simeon). Proof `docs/desktop-v2-palette.png`.
- **Frosted-glass panels** (`fb.fill_rounded_rect_glass`): panels alpha-blend
  their color over the wallpaper behind them — the mockup's translucent
  `--panel` look without a full backdrop blur. The sparse-rendering thesis
  applied to chrome: read what's dormant behind the panel and only tint it.
  Proof `docs/desktop-v2-glass.png`. Next slices: dock restyle, TIS visuals.

### Added — Linux ABI layer brick 4: dynamic linking — a dynamically-linked glibc binary runs (2026-05-29)

- **The bare-metal Rust kernel runs a *dynamically-linked* glibc binary** (the
  common case — almost no Linux software ships static). ld.so loads, maps
  `libc.so.6`, relocates the program, and runs it. Proof:
  `docs/linux-abi-brick4-dynamic-serial.txt`.
- ELF loader: `load_bias` (load the ET_DYN interpreter at a chosen base) +
  `interp_path` (read `PT_INTERP`). `linux::enter` loads `ld-linux-x86-64.so.2`
  at `AT_BASE`, builds the full auxv (`AT_BASE`/`AT_ENTRY`/`AT_PHDR`) + argv, and
  jumps to ld.so.
- File syscalls feeding ld.so: `openat`/`open`/`read`/`pread64`/`lseek`/`close`/
  `fstat`/`newfstatat`/`access`/`faccessat`, plus **file-backed mmap** (brick 3,
  same day).
- Three fixes that cracked it: (1) honor **MAP_FIXED** (ld.so reserves a span
  then maps each library segment at base+offset; ignoring the address scatters
  segments and breaks symbol tables); (2) **unique `st_dev`/`st_ino`** per file
  (ld.so dedups loaded objects by inode — 0/0 for every file made it think libc
  was already loaded and skip mapping it → "no version information available");
  (3) ship ld.so + libc in the initrd at the standard paths.
- HONEST SCOPE: single dynamically-linked program, one libc. Threads
  (`clone`/`futex`), a real per-process VMM, more libraries, `/proc` are still
  ahead. But the hardest conceptual barrier — the dynamic loader running on our
  own kernel — is crossed.

### Added — Linux ABI layer brick 2: a real glibc binary runs natively (2026-05-29)

- **The bare-metal Rust kernel runs an unmodified static-glibc binary** — real
  `printf` output and working **thread-local storage** (`__thread`, verified
  read+write), through glibc's full startup syscall sequence, exiting cleanly
  via `exit_group`. Proof: `docs/linux-abi-brick2-serial.txt`. This is a real
  C library running on our own kernel, not a Linux kernel.
- Three foundational kernel fixes made it work:
  - **SSE/SSE2 enabled at boot** (`enable_sse`: CR0.EM=0/MP=1, CR4.OSFXSR/OSXMMEXCPT)
    — the kernel is soft-float, but every Linux x86-64 binary uses SSE2 (it's
    the baseline); glibc's `init_cpu_features` `movd %xmm` faulted without it.
  - **Linux register-preservation in the syscall trampoline** — the Linux ABI
    requires the kernel to preserve all regs except `rax/rcx/r11`; the trampoline
    now saves/restores `rdi/rsi/rdx/r8/r9/r10` (not just callee-saved), or glibc
    gets garbage registers back across every syscall and crashes. (A subtle
    heisenbug: verbose tracing masked it by shifting the garbage.)
  - **`AT_PHDR/PHENT/PHNUM` in the auxv** + **zeroed `brk`** memory (Linux
    guarantees fresh break pages read as zero; glibc heap/TLS structures rely
    on it).
- New Linux syscalls: `fstat` (stdout as a char device), `prlimit64`
  (RLIM_INFINITY). Per-syscall serial trace behind a `TRACE` flag in `linux.rs`.
- Faster iteration: the kernel now loads the Linux test binary from the initrd
  (`bin/linuxtest`) so swapping test programs doesn't require a kernel rebuild;
  `iso/build-linux-abi-test.sh [binary]` + `RP_NO_KERNEL=1`.
- **Brick 2.5 (same day): full return-from-`main` / `exit(3)` / `atexit` works.**
  A complete standard C program now runs start-to-finish (glibc startup → main →
  atexit handlers → glibc cleanup → `exit_group`, exit code 0). The fix: **zero
  the general-purpose registers at process entry** to match Linux exec semantics
  — in particular RDX must be 0 (it conventionally carries an atexit function
  pointer; leaving kernel garbage there made glibc register a bogus `rtld_fini`
  and jump to it at exit). Verified with an explicit `atexit` handler too.

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
