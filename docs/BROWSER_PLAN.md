# Running real browsers (Firefox & Chrome) on Rusty Penguin

**Decision (Simeon, 2026-05-28):** Firefox and Chrome must work. This is the
last giant piece toward Linux-Mint-parity daily-driver use.

## What this actually requires

Browsers cannot run on the bespoke Rust framebuffer compositor — they need a
real Linux graphics stack. None of this is something we *write*; like every
Linux distro, we **assemble** upstream components:

- a **display server** — X11 (Xorg, simplest via the `fbdev`/`modesetting`
  driver on the existing `/dev/fb0`, which already works under UEFI) or Wayland
  (needs a compositor — `cage`/`weston`/wlroots — none on the host yet);
- **Mesa** for OpenGL — software path (`swrast`/`llvmpipe`, present on host) so
  it runs with no GPU; later GPU accel via virtio-gpu DRM;
- the **browser + its dependency tree** — Chrome is a 374 MB tree under
  `/opt/google/chrome`; Firefox on the host is a snap (harder — prefer the
  `.tar` build or `firefox-esr` .deb); plus GTK, glib, pango, cairo, fontconfig,
  freetype, nss, dbus, libX11/xcb, libinput, xkb data, …

This is **too big for the initramfs** — it lives on the persistent **RPDATA
root** created by `rp-install`. That is exactly why install-to-disk came first.

## Architecture

Rusty Penguin becomes a **Rust-identity distro on a real Linux userland**:

- **Daily-driver session**: Xorg (fbdev/modesetting) → minimal WM → browser,
  branded as Rusty Penguin. This is what runs Firefox/Chrome.
- **Showcase/research layer** (unchanged): the pure-Rust framebuffer desktop,
  games, pure-Rust DOOM, and the bare-metal Rust kernel — RP's identity.
- **init** keeps the ternary boot (storage/network Trits, `.tern` records) and
  chooses the session: graphical browser session vs. the Rust desktop vs.
  console/installer.

## Staged roadmap

1. **Graphics substrate** — DRM `/dev/dri/card0` (virtio-gpu) for accel + the
   modesetting path; confirm Mesa software GL works headless. *(brick 1)*
2. **X server on a surface** — Xorg on `/dev/fb0` (fbdev) or modesetting; prove
   a trivial X client (xterm/xclock) renders. *Proves real X11 apps run.*
3. **Browser rootfs** — assemble Chrome (real 374 MB tree) + its deps into the
   RPDATA root (recursive `ldd` + the runtime-dlopen extras: dri drivers,
   fontconfig data, gtk modules, nss). Build tooling to produce this rootfs
   reproducibly (it won't fit in git — generate at build time).
4. **Session integration** — a "Web" launcher / boot option that starts
   Xorg + WM + the browser; RP branding; input via libinput/evdev.
5. **Firefox** — de-snap (tar build) once Chrome works; same stack.
6. **Wayland + GPU accel** — later: a wlroots compositor + virtio-gpu virgl for
   hardware acceleration.

## Honest scale

Multi-session. ISO grows to ~600 MB–1 GB. The hard parts are the runtime
dlopen dependencies (not shown by `ldd`) and getting Xorg input + config right.
Tooling on host to assemble from: Xorg, Xwayland, Mesa (swrast/llvmpipe),
libwayland, google-chrome, firefox(snap). DRM modules: virtio-gpu, bochs.
