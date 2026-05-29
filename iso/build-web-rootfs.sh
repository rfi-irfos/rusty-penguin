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

# DIAGNOSTIC: wrap xkbcomp to capture the source Xorg pipes + xkbcomp's stderr.
if [ -f "$STAGE/usr/bin/xkbcomp" ]; then
    mv "$STAGE/usr/bin/xkbcomp" "$STAGE/usr/bin/xkbcomp.real"
    cat > "$STAGE/usr/bin/xkbcomp" <<'WRAP'
#!/bin/busybox sh
/bin/busybox cat > /tmp/xkb-src.txt
/usr/bin/xkbcomp.real "$@" < /tmp/xkb-src.txt 2>/tmp/xkb-err.txt
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
    Identifier "fb"
    Driver     "fbdev"
    Option     "fbdev" "/dev/fb0"
EndSection
Section "Screen"
    Identifier "scr"
    Device     "fb"
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
sleep 3
DISPLAY=:0 /usr/bin/xterm -fa DejaVuSans -fs 14 -e /bin/busybox sh &
sleep 1
# Surface the XKB-related Xorg log lines so failures are visible headlessly.
echo "=== xkbcomp source Xorg generated (head) ==="; /bin/busybox head -n 20 /tmp/xkb-src.txt 2>/dev/null
echo "=== xkbcomp stderr ==="; /bin/busybox cat /tmp/xkb-err.txt 2>/dev/null | tail -20
wait
XSH
chmod +x "$STAGE/start-x.sh"

echo "[web-rootfs] staged: $(find "$STAGE" -type f | wc -l) files, $(du -sh "$STAGE" | cut -f1)"
# Closure completeness check: every lib ldd wants for Xorg/xterm present in stage?
miss=0
for b in /usr/bin/Xorg /usr/bin/xterm; do
    ldd "$b" 2>/dev/null | awk '/=>/{print $3}' | while read -r lib; do
        [ -e "$STAGE$lib" ] || echo "  UNRESOLVED: $lib"
    done
done
echo "[web-rootfs] done."
