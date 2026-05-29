#!/usr/bin/env bash
# Fast iteration path for the rp.web (X11/browser) boot mode: builds ONLY the
# init binary + the web initrd (base initramfs merged with the X11/Chrome
# web-rootfs), then boots it directly via QEMU -kernel/-initrd — no bare-metal
# kernel, no grub-mkrescue, no ISO. modesetting runs on virtio-gpu DRM, which
# doesn't need efifb, so a direct kernel boot is fine (no UEFI needed).
#
#   RP_WEB_CHROME=1 iso/build-web-test.sh           # build + boot headless (serial)
#   RP_WEB_CHROME=1 iso/build-web-test.sh --shot OUT.png   # build + screenshot via QMP
set -uo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ISO_DIR="$REPO_ROOT/iso"
KVER=$(uname -r)
VMLINUZ="$ISO_DIR/vmlinuz-${KVER}"
[ -r "$VMLINUZ" ] || { echo "ERROR: cached kernel $VMLINUZ missing — run iso/build.sh once."; exit 1; }

echo "[web-test] building init..."
cargo build --release -p init --manifest-path "$REPO_ROOT/Cargo.toml" 2>&1 | tail -3
INIT_BIN="$REPO_ROOT/target/release/init"
PSH_BIN="$REPO_ROOT/target/release/shell"

D="$(mktemp -d)"; trap "rm -rf $D" EXIT
mkdir -p "$D/lib/x86_64-linux-gnu" "$D/lib64" "$D/bin" "$D/usr/local/bin" \
         "$D/proc" "$D/sys" "$D/dev" "$D/tmp" "$D/lib/modules" "$D/etc"
cp "$INIT_BIN" "$D/init"; chmod +x "$D/init"
[ -f "$PSH_BIN" ] && { cp "$PSH_BIN" "$D/bin/psh"; cp "$PSH_BIN" "$D/usr/local/bin/psh"; }
BB=$(command -v busybox || echo /usr/bin/busybox)
cp "$BB" "$D/bin/busybox"; chmod +x "$D/bin/busybox"
[ -f "$ISO_DIR/assets/udhcpc.script" ] && { cp "$ISO_DIR/assets/udhcpc.script" "$D/etc/udhcpc.script"; chmod +x "$D/etc/udhcpc.script"; }
for lib in libc.so.6 libgcc_s.so.1; do
    src=$(ldconfig -p 2>/dev/null | awk "/$lib/"'{print $NF}' | head -1)
    [ -n "$src" ] && cp "$src" "$D/lib/x86_64-linux-gnu/$lib"
done
LD_SRC=$(readlink -f /lib64/ld-linux-x86-64.so.2)
cp "$LD_SRC" "$D/lib64/ld-linux-x86-64.so.2"
ln -sf /lib64/ld-linux-x86-64.so.2 "$D/lib/ld-linux-x86-64.so.2" 2>/dev/null || true
VM=$(find /lib/modules/$KVER -name "virtio_input.ko*" 2>/dev/null | head -1)
[ -n "$VM" ] && { [[ "$VM" == *.zst ]] && zstd -d "$VM" -o "$D/lib/modules/virtio_input.ko" -f >/dev/null 2>&1 || cp "$VM" "$D/lib/modules/virtio_input.ko"; }

echo "[web-test] building web-rootfs (RP_WEB_CHROME=${RP_WEB_CHROME:-0})..."
WS="$(mktemp -d)"; trap "rm -rf $D $WS" EXIT
bash "$ISO_DIR/build-web-rootfs.sh" "$WS" 2>&1 | sed 's/^/    /'
cp -a "$WS/." "$D/"

WEB_INITRD="$ISO_DIR/initrd-web.img"
echo "[web-test] packing initrd (this can take a moment with Chrome)..."
(cd "$D" && find . | cpio -o -H newc 2>/dev/null | gzip -1 > "$WEB_INITRD")
echo "[web-test] initrd-web.img: $(du -sh "$WEB_INITRD" | cut -f1)"

# ── boot ──────────────────────────────────────────────────────────────────
SHOT=""; [ "${1:-}" = "--shot" ] && SHOT="${2:-/tmp/web-chrome.png}"
QMP=/tmp/rp-web-qmp.sock
COMMON=( -m "${RP_QEMU_MEM:-2560}" -smp 2 -kernel "$VMLINUZ" -initrd "$WEB_INITRD"
    -append "rp.web console=ttyS0 loglevel=4"
    -vga none -device virtio-gpu-pci -device virtio-keyboard-pci -device virtio-mouse-pci
    -nographic -serial mon:stdio -qmp unix:$QMP,server,nowait )
if [ -n "$SHOT" ]; then
    rm -f "$QMP"
    echo "[web-test] booting headless; will screenshot to $SHOT after settle..."
    qemu-system-x86_64 "${COMMON[@]}" >/tmp/rp-web-serial.log 2>&1 &
    QPID=$!
    for i in $(seq 1 60); do [ -S "$QMP" ] && break; sleep 0.5; done
    sleep "${RP_SETTLE:-95}"   # boot (decompress 522MB tmpfs, slow on swap) + Xorg + Chrome cold-start
    python3 - "$QMP" "$SHOT" <<'PY'
import socket,json,sys,time
qmp,out=sys.argv[1],sys.argv[2]
s=socket.socket(socket.AF_UNIX); s.connect(qmp); f=s.makefile("rw")
f.readline(); f.write(json.dumps({"execute":"qmp_capabilities"})+"\n"); f.flush(); f.readline()
ppm="/tmp/rp-web.ppm"
f.write(json.dumps({"execute":"screendump","arguments":{"filename":ppm}})+"\n"); f.flush()
time.sleep(2)
print("screendump requested ->",ppm)
PY
    [ -f /tmp/rp-web.ppm ] && command -v convert >/dev/null && convert /tmp/rp-web.ppm "$SHOT" && echo "[web-test] wrote $SHOT"
    sleep 1; kill $QPID 2>/dev/null
    echo "[web-test] serial tail:"; tail -n 30 /tmp/rp-web-serial.log
else
    echo "[web-test] booting (serial, Ctrl-A X to quit)..."
    exec qemu-system-x86_64 "${COMMON[@]}"
fi
