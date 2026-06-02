#!/usr/bin/env bash
# Build a bootable ISO that runs id Software's *real* DOOM (fbDOOM, a dynamic PIE)
# WINDOWED — as an isolated, preemptively-scheduled Linux process whose private
# framebuffer the bare-metal desktop composites into an on-screen window. The
# headline demo: real DOOM, in a window, on the pure-Rust desktop, playable.
#
# Pipeline (all from scratch, no Linux/libc underneath the kernel):
#   spawn_linux_dyn_elf loads fbdoom via our ld.so + glibc into its own address
#   space -> fbdoom mmaps/writes a private 640x400 /dev/fb0 surface -> the desktop
#   (itself a scheduled process) blits that surface into a titled window -> the
#   keyboard IRQ routes scancodes to DOOM so you can play it in the window.
#
#   bash iso/build-windowed-doom.sh
#       -> ./rusty-penguin-windowed-doom.iso
#       watch + play:
#       qemu-system-x86_64 -machine q35 -cdrom rusty-penguin-windowed-doom.iso \
#           -m 512 -vga std -display gtk
set -uo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

FBDOOM="$REPO/iso/doom-assets/fbdoom"
WAD="$REPO/iso/doom-assets/doom1.wad"
[ -f "$FBDOOM" ] || { echo "[win-doom] missing $FBDOOM"; exit 1; }
[ -f "$WAD" ]    || { echo "[win-doom] missing $WAD";    exit 1; }

echo "[win-doom] building kernel + desktop + user-psh..."
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

echo "[win-doom] assembling initrd (desktop + fbdoom + WAD + ld.so + libc)..."
T=/tmp/win-doom-iso; rm -rf "$T"; mkdir -p "$T/boot/grub"
cp "$KELF" "$T/boot/kernel.elf"
IR=/tmp/win-doom-initrd; rm -rf "$IR"; mkdir -p "$IR/bin" "$IR/mnt" "$IR/lib64" "$IR/lib/x86_64-linux-gnu"
cp "$PSH"  "$IR/bin/psh"
cp "$DESK" "$IR/bin/desktop"
# fbdoom runs as bin/linuxtest (the linuxwin boot mode spawns it scheduled).
cp "$FBDOOM" "$IR/bin/linuxtest"
cp "$WAD"    "$IR/mnt/DOOM1.WAD"
# ld.so + libc so our dynamic loader can resolve fbdoom's DT_NEEDED.
cp -L /lib64/ld-linux-x86-64.so.2     "$IR/lib64/ld-linux-x86-64.so.2"     2>/dev/null
cp -L /lib/x86_64-linux-gnu/libc.so.6 "$IR/lib/x86_64-linux-gnu/libc.so.6" 2>/dev/null
cp -L /lib/x86_64-linux-gnu/libc.so.6 "$IR/lib64/libc.so.6"                2>/dev/null
[ -f "$REPO/iso/assets/meta.rpv" ] && cp "$REPO/iso/assets/meta.rpv" "$IR/bin/meta.rpv"
( cd "$IR" && find . | cpio -o -H newc 2>/dev/null > "$T/boot/initrd-bare.img" )

cat > "$T/boot/grub/grub.cfg" <<'CFG'
insmod all_video
set timeout=0
set default=0
set gfxmode=1280x800x32,auto
set gfxpayload=keep
menuentry "Rusty Penguin -- DOOM windowed on the desktop" {
    multiboot2 /boot/kernel.elf linuxwin
    module2    /boot/initrd-bare.img initrd
    boot
}
CFG

echo "[win-doom] making ISO..."
grub-mkrescue -o "$REPO/rusty-penguin-windowed-doom.iso" "$T" 2>/dev/null | tail -1
rm -rf "$T" "$IR"
echo "[win-doom] done: $(du -h "$REPO/rusty-penguin-windowed-doom.iso" | cut -f1) -> rusty-penguin-windowed-doom.iso"
echo "[win-doom] watch + play:"
echo "  qemu-system-x86_64 -machine q35 -cdrom rusty-penguin-windowed-doom.iso -m 512 -vga std -display gtk"
echo "  (give it ~20s to load the WAD; press a key in the window to start a game)"
