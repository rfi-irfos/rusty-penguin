#!/usr/bin/env bash
# Fast headless screendump of a system background. Boots the desktop with
# `wallpaper=N` (no 58MB clip in the initrd → quick boot). N = 0..7.
#   bash iso/build-wp-shot.sh <N> <out.png> [settle]
set -uo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
N="${1:?wallpaper index}"; OUT="${2:?out png}"; SETTLE="${3:-12}"
KELF="$REPO/target/x86_64-rusty-penguin/release/kernel"
PSH="$REPO/user-psh/target/x86_64-user-psh/release/user-psh"
DESK="$REPO/desktop-metal/target/x86_64-user-psh/release/desktop-metal"
T=/tmp/wp-iso; rm -rf "$T"; mkdir -p "$T/boot/grub"
cp "$KELF" "$T/boot/kernel.elf"
IR=/tmp/wp-initrd; rm -rf "$IR"; mkdir -p "$IR/bin"
cp "$PSH" "$IR/bin/psh"; cp "$DESK" "$IR/bin/desktop"
( cd "$IR" && find . | cpio -o -H newc 2>/dev/null > "$T/boot/initrd-bare.img" )
cat > "$T/boot/grub/grub.cfg" <<CFG
insmod all_video
insmod vbe
set timeout=0
set default=0
set gfxmode=1280x800x32,auto
set gfxpayload=keep
menuentry "wp $N" { multiboot2 /boot/kernel.elf wallpaper=$N autostart=200 ; module2 /boot/initrd-bare.img initrd ; boot }
CFG
grub-mkrescue -o /tmp/wp.iso "$T" 2>/dev/null | tail -1
QMP=/tmp/wp-qmp.sock; rm -f "$QMP"
qemu-system-x86_64 -machine q35 -m 512 -cdrom /tmp/wp.iso -vga std -display none -no-reboot -qmp unix:$QMP,server,nowait >/dev/null 2>&1 &
QPID=$!
until [ -S "$QMP" ]; do sleep 1; done
sleep "$SETTLE"
python3 - "$QMP" <<'PY'
import socket,json,sys,time
s=socket.socket(socket.AF_UNIX); s.connect(sys.argv[1]); f=s.makefile("rw")
f.readline(); f.write(json.dumps({"execute":"qmp_capabilities"})+"\n"); f.flush(); f.readline()
f.write(json.dumps({"execute":"screendump","arguments":{"filename":"/tmp/wp.ppm"}})+"\n"); f.flush(); time.sleep(2)
PY
[ -f /tmp/wp.ppm ] && convert /tmp/wp.ppm "$OUT" && echo "wrote $OUT"
kill $QPID 2>/dev/null
