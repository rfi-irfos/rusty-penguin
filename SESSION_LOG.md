# Rusty Penguin — Session Log

A running, honest record of build sessions: what shipped, how it was verified, and
what's genuinely still open. Brutal honesty over polish — nothing here is claimed
"done" without a QEMU/host proof.

---

## 2026-06-01 → 06-02 — Daily-driver gap closing (Ubuntu-replacement goal)

Goal for the session: push the six remaining "replace Ubuntu" gaps as far as is
*verifiable*, brick by brick, each behind a flag, each proven in QEMU or on the
host, each committed + pushed. Items 1 (real FS) and 2 (TLS CA trust store) were
already closed earlier; this session worked items 3–6.

### Shipped + verified this session

| Brick | Item | What | Proof | Commit |
|---|---|---|---|---|
| Per-task syscall stack | 4 — preemption maturity | The SYSCALL trampoline shared ONE kernel stack + ONE `_user_rsp` global; when the desktop blocked in a syscall and a 2nd app syscalled in that window they clobbered each other's saved frame → kernel #GP. Fixed with a per-task syscall stack (`_cur_syscall_stack`, defaults to the shared stack so the normal boot is unchanged; `preempt_tick` retargets it per CR3-switch). | `schedesktop2`: real desktop + a 2nd real app, both preemptively scheduled & isolated, **no #GP**, desktop renders 16046 cells. Normal-boot fault profile identical to baseline. `docs/multiproc-desktop-plus-app-scheduled.png` | `3dd0e39` |
| Software brightness | 5 — power | Present-time dimming LUT (`v*b/100`), fast-path at 100%; Quick Settings brightness slider; `brightness=N` boot arg (default/kiosk/accessibility knob + headless verification idiom). Works on any panel incl. those with no hardware backlight. | `brightness=40` → whole desktop at **0.380× baseline luminance** (expected ~0.40); normal boot unchanged. `docs/brightness-40pct.png` | `0d29059` |
| WPA2 auth core | 3 — WiFi | The hardware-independent half of WiFi: `wpa2.rs` — SHA-1, HMAC-SHA1, PBKDF2, `wpa_passphrase_to_psk`→PMK, IEEE 802.11i PRF→PTK. | Verified vs **published vectors** (FIPS 180-1, RFC 2202, RFC 6070, IEEE 802.11i §H.4 PMK `f42c…a12e`). `tools/wpa2_test.rs` (host) + boot serial `[wifi: WPA2 auth core OK]`. | `a70c4af` |
| VIRGL 3D detection | 6 — GPU accel | Kernel reads `VIRTIO_GPU_F_VIRGL` + `num_capsets` (device cfg); `has_3d()`. Read-only — 2D path untouched. | Two-sided: `-device virtio-gpu-gl -display egl-headless` → "VIRGL 3D offered, host capsets 2"; plain virtio-gpu → "no VIRGL, 2D only". Both still pass the 2D scanout self-test. | `1014f4e` |
| Windowed app | 4 — multi-app | The desktop (a scheduled process) composites a SECOND real app process's live surface into a titled on-screen window: kernel maps the surface read-only into the desktop AS (0x3000000), desktop reads it via `sys_app_surface` (#41) and blits it 4× scaled. The full windowed multi-app model, the path to windowed DOOM. | `schedesktop2`: orange app window on the live desktop, 128×128 (16384) px, 0 faults; normal-boot regression 0 faults/clean. `docs/multiproc-windowed-app-on-desktop.png` | `66cbf45` |
| virgl 3D control path | 6 — GPU accel | Past detection: negotiate `F_VIRGL`, `GET_CAPSET_INFO`, `CTX_CREATE`/`CTX_DESTROY` — the 3D command channel round-tripping through the host virglrenderer. Flag-gated (`virgltest`); 2D path untouched. | `virgltest` + `-device virtio-gpu-gl -display egl-headless`: capset id 1 / ver 1 / size 308, CTX_CREATE OK; 2D self-test still passes; default boot does NOT negotiate. Serial log. | `e9f9849` |
| Per-task ABI mode (windowed-DOOM brick 1) | 4 — multi-app | A Linux-ABI process and the native desktop scheduled together, each routing syscalls to the right table (`TASK_LINUX[]` + `linux::set_linux` per switch). Also fixed a real GOT-indirect `_lx_a4/5/6` #PF that hit any Linux process in a private AS. | `linuxroute`: native routed 11× + Linux routed (`LINUXROUTE`), 0 faults; `enter()` Linux path regression-passes (hello write+exit). Serial log. | `9707242` |

Also confirmed item 5's **real battery** readout is already wired: `sys_battery_pct`
(#20) reads the ACPI EC, and Quick Settings shows "AC" gracefully when no battery
is present (as under QEMU).

### Honest status of the six gaps after this session

- **1. Real filesystem** — ✅ done (RPFS v2, earlier).
- **2. TLS CA trust store** — ✅ done (cert-chain validation, earlier).
- **3. Bare-metal WiFi** — 🟡 brick 1 (iwlwifi detection + firmware parser) + brick
  2a (WPA2 auth core) done & verified. Remaining: radio MMIO/firmware bring-up +
  the EAPOL 4-way handshake wired to the driver — **needs real Intel hardware**
  QEMU can't emulate.
- **4. Preemptive multitasking + isolation maturity** — ✅ the concurrency #GP is
  fixed; the real desktop + a 2nd real app run isolated & preemptively at once,
  and the desktop composites that 2nd app's surface into an on-screen window.
  Windowed multi-app is proven end to end with a *native* synthetic app.
  **Windowed DOOM is a separate multi-brick build** (DOOM is a Linux-ABI process,
  not a native one — an earlier note calling it a "mechanical swap" was wrong):
  - brick 1 ✅ per-task ABI mode — a Linux process + the native desktop scheduled
    together, each routing syscalls correctly (`linuxroute`, `9707242`);
  - brick 2 ▢ load a dynamic Linux ELF (ld.so+libc) into a private AS as a
    *scheduled* task (today DOOM runs only via the one-way `enter()` path);
  - brick 3 ▢ redirect that process's `/dev/fb0` to a private surface;
  - brick 4 ▢ composite the surface into a desktop window (reuses `sys_app_surface`).
- **5. Power management** — 🟡 ACPI S5 shutdown+reboot ✅; software brightness ✅;
  real battery ✅ wired. Remaining: hardware backlight control + S3 suspend/resume
  (large, and largely **hardware-bound** / not meaningfully verifiable under QEMU).
- **6. 3D GPU accel (virgl)** — 🟡 capability detection + the **3D control path**
  (negotiate `F_VIRGL`, capset query, `CTX_CREATE`/`CTX_DESTROY`) done & verified
  via egl-headless. The remaining `RESOURCE_CREATE_3D` + `SUBMIT_3D` GL/TGSI
  command stream — and routing the desktop through a 3D surface — is the genuine
  multi-year part (the user's own framing), but it IS incrementally verifiable here.

Method note: every claim above is backed by a committed proof (screenshot, serial
log, or host test). Where something can't be verified in this sandbox (real WiFi
radio, hardware backlight, S3), it's marked hardware-bound rather than "done".
