#!/usr/bin/env bash
# Headless boot of the real bare-metal desktop (psh + desktop + meta.rpv in the
# initrd), screendumped via QMP. Regression proof that the desktop boots and the
# dock/start-menu render with all apps wired in.
#
#   bash iso/build-desktop-test.sh [OUT.png]
set -uo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ISO_DIR="$REPO/iso"
OUT="${1:-$REPO/docs/screenshots/desktop-with-media-screenshot.png}"

echo "[desk] building kernel + desktop + user-psh..."
( cd "$REPO/kernel" && cargo +nightly build --release -Zjson-target-spec \
    -Zbuild-std=core,compiler_builtins,alloc -Zbuild-std-features=compiler-builtins-mem \
    --target x86_64-rusty-penguin.json 2>&1 | grep -E "error|Finished" ) || exit 1
( cd "$REPO/user-psh" && cargo +nightly build --release -Zjson-target-spec \
    -Zbuild-std=core,compiler_builtins -Zbuild-std-features=compiler-builtins-mem 2>&1 | grep -E "error|Finished" ) || exit 1
( cd "$REPO/desktop-metal" && cargo +nightly build --release -Zjson-target-spec \
    -Zbuild-std=core,alloc,compiler_builtins -Zbuild-std-features=compiler-builtins-mem 2>&1 | grep -E "error|Finished" ) || exit 1

KELF="$REPO/target/x86_64-rusty-penguin/release/kernel"
PSH="$REPO/user-psh/target/x86_64-user-psh/release/user-psh"
DESK="$REPO/desktop-metal/target/x86_64-user-psh/release/desktop-metal"

T=/tmp/desk-iso; rm -rf "$T"; mkdir -p "$T/boot/grub"
cp "$KELF" "$T/boot/kernel.elf"
IR=/tmp/desk-initrd; rm -rf "$IR"; mkdir -p "$IR/bin"
cp "$PSH" "$IR/bin/psh"
cp "$DESK" "$IR/bin/desktop"
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
menuentry "Rusty Penguin (bare metal)" {
    multiboot2 /boot/kernel.elf
    module2    /boot/initrd-bare.img initrd
    boot
}
CFG
grub-mkrescue -o /tmp/desk.iso "$T" 2>/dev/null | tail -1

QMP=/tmp/rp-desk-qmp.sock; rm -f "$QMP" /tmp/desk-serial.log
echo "[desk] booting QEMU..."
qemu-system-x86_64 -machine q35 -m 512 -cdrom /tmp/desk.iso \
    -vga std -device intel-hda -device hda-duplex -device usb-tablet \
    -serial file:/tmp/desk-serial.log -display none -no-reboot \
    -qmp unix:$QMP,server,nowait >/dev/null 2>&1 &
QPID=$!
for i in $(seq 1 40); do [ -S "$QMP" ] && break; sleep 0.5; done
sleep "${RP_SETTLE:-14}"
# Optional interaction: open the start menu via the tablet (absolute coords).
# dingir "Menu" button center ~ (66,761) at 1280x800.
python3 - "$QMP" "${RP_CLICK:-0}" <<'PY'
import socket,json,sys,time
qmp=sys.argv[1]; do_click=sys.argv[2]=="1"
s=socket.socket(socket.AF_UNIX); s.connect(qmp); f=s.makefile("rw")
f.readline(); f.write(json.dumps({"execute":"qmp_capabilities"})+"\n"); f.flush(); f.readline()
def ev(evs):
    f.write(json.dumps({"execute":"input-send-event","arguments":{"events":evs}})+"\n"); f.flush(); f.readline()
def absxy(x,y):  # qemu abs axis is 0..32767 over the screen
    return [{"type":"abs","data":{"axis":"x","value":int(x/1280*32767)}},
            {"type":"abs","data":{"axis":"y","value":int(y/800*32767)}}]
def click(x,y):
    ev(absxy(x,y)); time.sleep(0.3)
    ev([{"type":"btn","data":{"button":"left","down":True}}]); time.sleep(0.1)
    ev([{"type":"btn","data":{"button":"left","down":False}}]); time.sleep(0.5)
if do_click:
    click(66,761)   # open start menu
    time.sleep(0.6)
f.write(json.dumps({"execute":"screendump","arguments":{"filename":"/tmp/desk.ppm"}})+"\n"); f.flush()
time.sleep(2); print("screendump requested (click=%s)"%do_click)
PY
[ -f /tmp/desk.ppm ] && command -v convert >/dev/null && convert /tmp/desk.ppm "$OUT" && echo "[desk] wrote $OUT"
sleep 1; kill $QPID 2>/dev/null
echo "[desk] serial tail:"; tail -n 15 /tmp/desk-serial.log
