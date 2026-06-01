#!/usr/bin/env bash
# Deterministic headless screendump of a single desktop app. Boots the bare-metal
# desktop with `autostart=N` so the kernel tells the desktop to open app N on
# launch — no mouse driving needed. Verifies GUI apps end-to-end.
#
#   bash iso/build-app-shot.sh <app_index> <OUT.png> [settle_secs]
# App indices (MenuLaunch::App tags): 0 Files 1 Editor 2 Calc 3 Help 4 Settings
#   5 TIS 6 Snake 7 Mines 8 Doom 9 Browser 10 KernelMgr 11 Sound 12 Media
#   13 Screenshot 100 Terminal
set -uo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ISO_DIR="$REPO/iso"
APP="${1:?app index required}"
OUT="${2:?output png required}"
SETTLE="${3:-12}"

KELF="$REPO/target/x86_64-rusty-penguin/release/kernel"
PSH="$REPO/user-psh/target/x86_64-user-psh/release/user-psh"
DESK="$REPO/desktop-metal/target/x86_64-user-psh/release/desktop-metal"
for b in "$KELF" "$PSH" "$DESK"; do [ -f "$b" ] || { echo "missing $b — build first"; exit 1; }; done

T=/tmp/appshot-iso; rm -rf "$T"; mkdir -p "$T/boot/grub"
cp "$KELF" "$T/boot/kernel.elf"
IR=/tmp/appshot-initrd; rm -rf "$IR"; mkdir -p "$IR/bin"
cp "$PSH" "$IR/bin/psh"; cp "$DESK" "$IR/bin/desktop"
[ -f "$ISO_DIR/assets/meta.rpv" ] && cp "$ISO_DIR/assets/meta.rpv" "$IR/bin/meta.rpv"
( cd "$IR" && find . | cpio -o -H newc 2>/dev/null > "$T/boot/initrd-bare.img" )

cat > "$T/boot/grub/grub.cfg" <<CFG
insmod all_video
insmod video_bochs
insmod vbe
set timeout=0
set default=0
set gfxmode=1280x800x32,1024x768x32,auto
set gfxpayload=keep
menuentry "Rusty Penguin (app $APP)" {
    multiboot2 /boot/kernel.elf autostart=$APP
    module2    /boot/initrd-bare.img initrd
    boot
}
CFG
grub-mkrescue -o /tmp/appshot.iso "$T" 2>/dev/null | tail -1

QMP=/tmp/rp-appshot-qmp.sock; rm -f "$QMP"
qemu-system-x86_64 -machine q35 -m 512 -cdrom /tmp/appshot.iso \
    -vga std -audiodev none,id=snd0 -device intel-hda -device hda-duplex,audiodev=snd0 \
    -display none -no-reboot -qmp unix:$QMP,server,nowait >/dev/null 2>&1 &
QPID=$!
until [ -S "$QMP" ]; do sleep 1; done
sleep "$SETTLE"
# KEYS env: space-separated QMP qcodes to send before the shot (e.g. "f", "n n").
# RCLICK=1 sends a right-click at the current cursor (opens the context menu).
python3 - "$QMP" "${KEYS:-}" "${RCLICK:-0}" <<'PY'
import socket,json,sys,time
s=socket.socket(socket.AF_UNIX); s.connect(sys.argv[1]); f=s.makefile("rw")
f.readline(); f.write(json.dumps({"execute":"qmp_capabilities"})+"\n"); f.flush(); f.readline()
def send(o):
    f.write(json.dumps(o)+"\n"); f.flush(); f.readline()
keys=sys.argv[2].split() if len(sys.argv)>2 and sys.argv[2] else []
def kev(qc, down):
    send({"execute":"input-send-event","arguments":{"events":[{"type":"key","data":{"down":down,"key":{"type":"qcode","data":qc}}}]}})
for k in keys:
    parts = k.split("+")           # e.g. "ctrl+g" → hold ctrl, tap g
    for m in parts[:-1]: kev(m, True)
    kev(parts[-1], True); time.sleep(0.1); kev(parts[-1], False)
    for m in reversed(parts[:-1]): kev(m, False)
    for m in ("ctrl","shift","alt","altgr"): kev(m, False)  # ensure no stuck modifier
    time.sleep(0.5)
if len(sys.argv)>3 and sys.argv[3]=="1":
    # nudge the mouse so the PS/2 packet stream is live, then HOLD right ~0.3s so
    # a 100 Hz desktop poll catches the press edge before release.
    send({"execute":"input-send-event","arguments":{"events":[{"type":"rel","data":{"axis":"x","value":3}},{"type":"rel","data":{"axis":"y","value":3}}]}})
    time.sleep(0.2)
    send({"execute":"input-send-event","arguments":{"events":[{"type":"btn","data":{"button":"right","down":True}}]}})
    time.sleep(0.3)
    send({"execute":"input-send-event","arguments":{"events":[{"type":"btn","data":{"button":"right","down":False}}]}})
    time.sleep(0.6)
if keys or (len(sys.argv)>3 and sys.argv[3]=="1"): time.sleep(1.0)
send({"execute":"screendump","arguments":{"filename":"/tmp/appshot.ppm"}})
time.sleep(2); print("screendump ok")
PY
[ -f /tmp/appshot.ppm ] && command -v convert >/dev/null && convert /tmp/appshot.ppm "$OUT" && echo "wrote $OUT"
kill $QPID 2>/dev/null
