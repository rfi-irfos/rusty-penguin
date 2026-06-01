#!/usr/bin/env bash
# Headless verify for the windowed video service (boot flag `videowin`) — the
# exact path the desktop Media app drives: service_open -> service_advance
# (decode + HDA audio) -> service_blit (scale into a window rect). Boots the
# bare-metal kernel with bin/meta.rpv in the initrd, renders the clip into a
# centred faux window on the framebuffer, and screendumps it via QMP.
#
#   bash iso/build-videowin-test.sh [OUT.png]
set -uo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ISO_DIR="$REPO/iso"
OUT="${1:-$ISO_DIR/../docs/media-player-on-rusty-penguin.png}"

echo "[videowin] building kernel..."
( cd "$REPO/kernel" && cargo +nightly build --release -Zjson-target-spec \
    -Zbuild-std=core,compiler_builtins,alloc -Zbuild-std-features=compiler-builtins-mem \
    --target x86_64-rusty-penguin.json 2>&1 | grep -E "error|Finished" ) || exit 1
KELF="$REPO/target/x86_64-rusty-penguin/release/kernel"

T=/tmp/videowin-iso; rm -rf "$T"; mkdir -p "$T/boot/grub"
cp "$KELF" "$T/boot/kernel.elf"
IR=/tmp/videowin-initrd; rm -rf "$IR"; mkdir -p "$IR/bin"
if [ -f "$ISO_DIR/assets/meta.rpv" ]; then
    cp "$ISO_DIR/assets/meta.rpv" "$IR/bin/meta.rpv"
    echo "[videowin] + bin/meta.rpv ($(du -sh "$ISO_DIR/assets/meta.rpv" | cut -f1))"
else
    echo "[videowin] ERROR: iso/assets/meta.rpv missing (run scripts/make_meta_video.sh)"; exit 1
fi
( cd "$IR" && find . | cpio -o -H newc 2>/dev/null > "$T/boot/initrd-bare.img" )

cat > "$T/boot/grub/grub.cfg" <<CFG
insmod all_video
insmod video_bochs
insmod vbe
set timeout=0
set default=0
set gfxmode=1280x800x32,1024x768x32,auto
set gfxpayload=keep
menuentry "Rusty Penguin -- videowin" {
    multiboot2 /boot/kernel.elf videowin
    module2    /boot/initrd-bare.img initrd
    boot
}
CFG
grub-mkrescue -o /tmp/videowin.iso "$T" 2>/dev/null | tail -1

QMP=/tmp/rp-videowin-qmp.sock; rm -f "$QMP"
rm -f /tmp/videowin-serial.log
echo "[videowin] booting QEMU (std VGA + Intel HDA)..."
qemu-system-x86_64 -machine q35 -m 512 -cdrom /tmp/videowin.iso \
    -vga std -device intel-hda -device hda-duplex \
    -serial file:/tmp/videowin-serial.log -display none -no-reboot \
    -qmp unix:$QMP,server,nowait >/dev/null 2>&1 &
QPID=$!
for i in $(seq 1 40); do [ -S "$QMP" ] && break; sleep 0.5; done
# Let it boot + decode well into the clip so the screenshot lands on a real frame.
sleep "${RP_SETTLE:-22}"
python3 - "$QMP" <<'PY'
import socket,json,sys,time
qmp=sys.argv[1]
s=socket.socket(socket.AF_UNIX); s.connect(qmp); f=s.makefile("rw")
f.readline(); f.write(json.dumps({"execute":"qmp_capabilities"})+"\n"); f.flush(); f.readline()
f.write(json.dumps({"execute":"screendump","arguments":{"filename":"/tmp/videowin.ppm"}})+"\n"); f.flush()
time.sleep(2); print("screendump requested")
PY
if [ -f /tmp/videowin.ppm ] && command -v convert >/dev/null; then
    convert /tmp/videowin.ppm "$OUT" && echo "[videowin] wrote $OUT"
fi
sleep 1; kill $QPID 2>/dev/null
echo "[videowin] serial tail:"; tail -n 20 /tmp/videowin-serial.log
