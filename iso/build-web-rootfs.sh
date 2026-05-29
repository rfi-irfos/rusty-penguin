#!/usr/bin/env bash
# Assemble the X11 web-rootfs (Stage B/C of docs/BROWSER_PLAN.md): a real Linux
# graphics stack so Rusty Penguin can run third-party GUI apps (xterm now;
# Chrome later). Collects each binary + its full ldd closure + the dlopen
# extras ldd can't see (Xorg modules, dri swrast, xkb data, fonts), plus an
# Xorg fbdev config and a launcher. Output: a staging tree printed with size.
#
#   build-web-rootfs.sh [STAGE_DIR]        (default /tmp/rp-web)
# Best-effort: missing pieces warn but don't abort (no set -e/pipefail).
set -u
STAGE="${1:-/tmp/rp-web}"
rm -rf "$STAGE"; mkdir -p "$STAGE"

copy() {  # copy a file preserving its absolute path under STAGE
    local f="$1"; [ -e "$f" ] || return 0
    local d="$STAGE$(dirname "$f")"; mkdir -p "$d"; cp -aL "$f" "$d/" 2>/dev/null || true
}
bundle_bin() {  # binary + full transitive shared-lib closure
    local bin; bin="$(command -v "$1" || echo "$1")"
    [ -x "$bin" ] || { echo "  MISSING: $1"; return 0; }
    copy "$bin"
    ldd "$bin" 2>/dev/null | awk '/=>/{print $3} /ld-linux/{print $1}' \
        | while read -r lib; do copy "$lib"; done
}

echo "[web-rootfs] bundling binaries + closures..."
# NOTE: /usr/bin/Xorg is a wrapper script — bundle the REAL server binary.
for b in /usr/lib/xorg/Xorg xterm xkbcomp fc-cache dbus-daemon dbus-launch sh; do
    bundle_bin "$b"
done
# Xorg runs xkbcomp via popen()/system() → it needs /bin/sh. The initramfs ships
# busybox but no /bin/sh, so the keymap compile silently no-op'd. Point sh at it.
mkdir -p "$STAGE/bin"
ln -sf /bin/busybox "$STAGE/bin/sh"

echo "[web-rootfs] dlopen extras (ldd-invisible)..."
# Xorg server modules (drivers, input, extensions, glx) — whole tree.
mkdir -p "$STAGE/usr/lib/xorg"
cp -aL /usr/lib/xorg/modules "$STAGE/usr/lib/xorg/" 2>/dev/null
# Each module also links libs ldd shows — collect those too.
find /usr/lib/xorg/modules -name '*.so' 2>/dev/null | while read -r m; do
    ldd "$m" 2>/dev/null | awk '/=>/{print $3}' | while read -r lib; do copy "$lib"; done
done
# Mesa software GL.
for so in swrast_dri.so kms_swrast_dri.so; do copy "/usr/lib/x86_64-linux-gnu/dri/$so"; done
# xkb keymap data, a font, fontconfig.
mkdir -p "$STAGE/usr/share/X11"
cp -aL /usr/share/X11/xkb "$STAGE/usr/share/X11/" 2>/dev/null
mkdir -p "$STAGE/usr/share/fonts/truetype/dejavu"
cp -aL /usr/share/fonts/truetype/dejavu/DejaVuSans*.ttf "$STAGE/usr/share/fonts/truetype/dejavu/" 2>/dev/null
cp -aL /etc/fonts "$STAGE/etc/" 2>/dev/null

# DRM modules for the `modesetting` X driver (drm core is built-in; these bind
# the QEMU display device → /dev/dri/card0). fbdev didn't render on truecolor
# efifb; modesetting on DRM is the reliable path. init loads these in rp.web.
KVER=$(uname -r)
mkdir -p "$STAGE/lib/modules"
for m in virtio_dma_buf virtio-gpu bochs; do
    src=$(find "/lib/modules/$KVER" -name "$m.ko*" 2>/dev/null | head -1)
    [ -z "$src" ] && { echo "  WARNING: $m.ko not found"; continue; }
    base=$(echo "$m" | tr - _)
    if [ "${src##*.}" = "zst" ]; then
        zstd -d "$src" -o "$STAGE/lib/modules/$base.ko" --force >/dev/null 2>&1
    else
        cp "$src" "$STAGE/lib/modules/$base.ko"
    fi
    echo "  bundled DRM module $base.ko"
done

# xkbcomp shim: Xorg's invocation (source on stdin + -em1/-emp/-eml error-format
# flags) fails to produce the .xkm when Xorg forks it in this minimal
# environment — yet the equivalent FILE-input form compiles fine. So capture the
# keymap source Xorg pipes in and recompile it the working way, writing to the
# output path Xorg asked for (its last argument).
if [ -f "$STAGE/usr/bin/xkbcomp" ]; then
    mv "$STAGE/usr/bin/xkbcomp" "$STAGE/usr/bin/xkbcomp.real"
    cat > "$STAGE/usr/bin/xkbcomp" <<'WRAP'
#!/bin/busybox sh
/bin/busybox cat > /tmp/xkb-src.txt
out=""
for a in "$@"; do out="$a"; done   # Xorg passes the .xkm output path last
echo "shim called: out=[$out] args=[$*]" >> /tmp/shim.log
/usr/bin/xkbcomp.real -w 1 -R/usr/share/X11/xkb -xkm /tmp/xkb-src.txt "$out" >> /tmp/shim.log 2>&1
rc=$?
echo "shim rc=$rc wrote=[$(/bin/busybox ls -l "$out" 2>&1)]" >> /tmp/shim.log
exit $rc
WRAP
    chmod +x "$STAGE/usr/bin/xkbcomp"
fi

echo "[web-rootfs] Xorg fbdev config + launcher..."
mkdir -p "$STAGE/etc/X11"
cat > "$STAGE/etc/X11/xorg.conf" <<'XCONF'
Section "ServerFlags"
    Option "AutoAddDevices" "true"
    Option "DontVTSwitch"   "true"
EndSection
Section "Device"
    Identifier "gpu"
    Driver     "modesetting"
EndSection
Section "Screen"
    Identifier "scr"
    Device     "gpu"
EndSection
Section "InputClass"
    Identifier      "kbd"
    MatchIsKeyboard "on"
    Option "XkbRules"  "evdev"
    Option "XkbModel"  "pc105"
    Option "XkbLayout" "us"
EndSection
XCONF
cat > "$STAGE/start-x.sh" <<'XSH'
#!/bin/busybox sh
# Launch X on the framebuffer and a terminal, from inside the web-rootfs.
# Calls the REAL Xorg server directly (the /usr/bin/Xorg wrapper does suid/vt
# setup we don't need as PID-1's child running as root).
export LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe
export XKB_BINDIR=/usr/bin XKB_CONFIG_ROOT=/usr/share/X11/xkb
mkdir -p /tmp/.X11-unix; chmod 1777 /tmp /tmp/.X11-unix
mkdir -p /var/lib/xkb   # Xorg's compiled-in XKB output dir; xkbcomp writes here
# Self-test: confirm xkbcomp can compile a keymap in THIS environment.
echo 'xkb_keymap { xkb_keycodes { include "evdev+aliases(qwerty)" }; xkb_types { include "complete" }; xkb_compat { include "complete" }; xkb_symbols { include "pc+us" }; xkb_geometry { include "pc(pc105)" }; };' \
  | /usr/bin/xkbcomp -w 1 -R/usr/share/X11/xkb -xkm - /var/lib/xkb/selftest.xkm \
  && echo "[start-x] xkbcomp self-test: OK" || echo "[start-x] xkbcomp self-test: FAILED"
/usr/lib/xorg/Xorg :0 -config /etc/X11/xorg.conf -logfile /tmp/Xorg.0.log -noreset &
# Wait for X to FINISH initialising before launching clients — on a loaded host
# X init can take ~45s (faulting the rootfs in from swap). The DRISWRAST GL
# line is logged near the very end of bring-up, so it's a reliable readiness
# marker; clients launched before it can't connect to the display.
xready=0
for i in $(seq 1 150); do
    if /bin/busybox grep -q "GL provider for screen 0" /tmp/Xorg.0.log 2>/dev/null; then
        xready=1; echo "[start-x] X server ready after ${i}s"; break
    fi
    sleep 1
done
[ "$xready" = 1 ] || echo "[start-x] WARNING: X readiness marker not seen; launching client anyway"
if [ -f /.rp-web-chrome ] && [ -x /start-chrome.sh ]; then
    # Browser session: launch Chrome on the now-ready X server.
    /start-chrome.sh
else
    # White bg / black fg, positioned, with a visible banner — a black-on-black
    # xterm on a black X root would otherwise be invisible.
    DISPLAY=:0 /usr/bin/xterm -geometry 90x30+80+80 -bg white -fg black -fa DejaVuSans -fs 16 \
        -e /bin/busybox sh -c 'echo "  RUSTY PENGUIN -- X11 + real GUI apps"; echo; /bin/busybox uname -a; echo; exec /bin/busybox sh' &
fi
sleep 1
# Surface the XKB-related Xorg log lines so failures are visible headlessly.
echo "=== /var/lib/xkb contents (did xkbcomp write server-0.xkm?) ==="; /bin/busybox ls -la /var/lib/xkb/ 2>/dev/null
echo "=== FULL Xorg.0.log (for serial capture / diagnosis) ==="
/bin/busybox cat /tmp/Xorg.0.log 2>/dev/null
echo "=== X clients / sockets ==="; /bin/busybox ls -la /tmp/.X11-unix/ 2>/dev/null
wait
XSH
chmod +x "$STAGE/start-x.sh"

# ---------------------------------------------------------------------------
# Optional: bundle Google Chrome (RP_WEB_CHROME=1). The 374MB tree dwarfs the
# X stack, so it's opt-in — the default rp.web ships lean (xterm only). Chrome
# is a real X client: it runs on the proven Xorg + modesetting + Mesa-swrast
# stack. It needs system libs (ldd of chrome + its bundled .so + helpers), the
# GBM dri loader (MESA-LOADER dlopens /usr/lib/.../gbm/dri_gbm.so for EGL), the
# NSS crypto stack (dlopen'd, ldd-invisible), and /dev/shm (init mounts it).
# ---------------------------------------------------------------------------
if [ "${RP_WEB_CHROME:-0}" = "1" ] && [ -d /opt/google/chrome ]; then
    echo "[web-rootfs] bundling Google Chrome (RP_WEB_CHROME=1)..."
    mkdir -p "$STAGE/opt/google"
    cp -aL /opt/google/chrome "$STAGE/opt/google/" 2>/dev/null
    # Drop Chrome's Qt desktop-integration shims. Their ldd closure drags in the
    # Qt6 core libs, but the Qt `xcb` PLATFORM PLUGIN lives in a dlopen'd plugins
    # dir we don't ship — so Qt inits, fails to find its platform plugin, and
    # qFatal()-aborts the whole browser ("no Qt platform plugin could be
    # initialized"). Without the shims Chrome falls back to its built-in Views
    # toolkit (no native theme), which renders fine. GTK shim stays harmless.
    rm -f "$STAGE/opt/google/chrome/libqt5_shim.so" "$STAGE/opt/google/chrome/libqt6_shim.so"
    # System-lib closure for the chrome binary, its helpers, and bundled .so's.
    # (chrome's own libs live in the tree already; we only need the host deps.)
    # Skip the qt shims so we don't pull the Qt core libs back in.
    for f in /opt/google/chrome/chrome \
             /opt/google/chrome/chrome_crashpad_handler \
             /opt/google/chrome/*.so; do
        [ -e "$f" ] || continue
        case "$f" in */libqt*_shim.so) continue;; esac
        ldd "$f" 2>/dev/null | awk '/=>/{print $3} /ld-linux/{print $1}' \
            | while read -r lib; do copy "$lib"; done
    done
    # NOTE: deliberately do NOT bundle /usr/lib/.../gbm/dri_gbm.so. It dlopens
    # libgallium (+137MB libLLVM) which we don't ship; a present-but-broken
    # dri_gbm.so makes Xorg's modesetting GBM init fail HARD ("couldn't get
    # display device") instead of falling back to the shadow framebuffer that
    # rendered xterm fine. Chrome runs --disable-gpu (CPU raster → X SHM blit),
    # so no client- or server-side GL/GBM is needed.
    # NSS crypto stack — dlopen'd at runtime, not in ldd output.
    for n in libnss3 libnssutil3 libsmime3 libssl3 libnspr4 libplc4 libplds4 \
             libsoftokn3 libfreebl3 libfreeblpriv3 libnssckbi libnssdbm3 \
             libsqlite3.so.0; do
        for cand in /usr/lib/x86_64-linux-gnu/$n.so \
                    /usr/lib/x86_64-linux-gnu/$n \
                    /usr/lib/x86_64-linux-gnu/nss/$n.so; do
            [ -e "$cand" ] && copy "$cand"
        done
    done
    # A launcher: Chrome on X11/Ozone, software GL, no sandbox (no userns in the
    # minimal initramfs), shared-memory disabled (small /dev/shm), loading a
    # visible page that proves the renderer + compositor work.
    cat > "$STAGE/start-chrome.sh" <<'CSH'
#!/bin/busybox sh
export LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe HOME=/tmp
mkdir -p /tmp/cr
cat > /tmp/rusty.html <<'HTML'
<!doctype html><meta charset=utf-8><title>Rusty Penguin</title>
<style>html,body{margin:0;height:100%;background:#1d1d1f;color:#f5f5f7;
font:600 48px/1.3 sans-serif;display:flex;align-items:center;justify-content:center;
flex-direction:column}small{font-size:20px;color:#86868b;font-weight:400;margin-top:18px}</style>
<div>Rusty Penguin<small>Google Chrome rendering on a from-scratch Rust distro &mdash; X11 + Mesa + ternary init</small></div>
HTML
# Start (maximised, no GL, software compositing). --disable-gpu makes Chrome
# raster on the CPU and present via X SHM — no EGL/GBM/GL needed. /etc/machine-id
# must be non-empty or some init paths complain; give it one.
[ -s /etc/machine-id ] || /bin/busybox dd if=/dev/urandom bs=16 count=1 2>/dev/null | /bin/busybox md5sum | /bin/busybox cut -c1-32 > /etc/machine-id
DISPLAY=:0 /opt/google/chrome/chrome \
    --no-sandbox --no-zygote --ozone-platform=x11 --disable-gpu --disable-gpu-compositing \
    --disable-dev-shm-usage --no-first-run --no-default-browser-check \
    --disable-features=Translate --user-data-dir=/tmp/cr \
    --enable-logging=stderr --v=0 \
    --start-maximized --window-size=1280,800 --window-position=0,0 \
    file:///tmp/rusty.html >/tmp/chrome.log 2>&1 &
CPID=$!
echo "[start-chrome] chrome launched pid=$CPID; waiting for paint..."
sleep 30   # cold-start + first paint on a software stack / loaded host
if /bin/busybox kill -0 "$CPID" 2>/dev/null; then
    echo "[start-chrome] chrome ALIVE after 30s (pid $CPID)"
else
    echo "[start-chrome] chrome EXITED within 30s"
fi
echo "=== chrome.log (full) ==="; /bin/busybox cat /tmp/chrome.log 2>/dev/null
CSH
    chmod +x "$STAGE/start-chrome.sh"
    echo "[web-rootfs] Chrome bundled; start-x.sh will launch it (RP_WEB_CHROME)."
    # Tell start-x.sh to launch Chrome instead of the bare xterm banner.
    touch "$STAGE/.rp-web-chrome"
fi

echo "[web-rootfs] staged: $(find "$STAGE" -type f | wc -l) files, $(du -sh "$STAGE" | cut -f1)"
# Closure completeness check: every lib ldd wants for Xorg/xterm present in stage?
miss=0
for b in /usr/bin/Xorg /usr/bin/xterm; do
    ldd "$b" 2>/dev/null | awk '/=>/{print $3}' | while read -r lib; do
        [ -e "$STAGE$lib" ] || echo "  UNRESOLVED: $lib"
    done
done
echo "[web-rootfs] done."
