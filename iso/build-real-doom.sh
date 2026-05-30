#!/usr/bin/env bash
# Build a bootable ISO that runs id Software's *real* DOOM (fbDOOM, an
# unmodified dynamically-linked glibc PIE) on the bare-metal pure-Rust kernel
# via the Linux ABI layer — no Linux kernel underneath.
#
# This is the reproducible form of the 2026-05-30 milestone (commit 91c18b8):
# ld.so links fbdoom against libc.so.6, then D_DoomMain -> W_Init (DOOM1.WAD)
# -> R_Init -> I_InitGraphics renders to our framebuffer.
#
#   bash iso/build-real-doom.sh
#       -> ./rusty-penguin-real-doom.iso  (boot: -display gtk to watch DOOM)
#
# Headless verification (serial trace, no display):
#   qemu-system-x86_64 -machine q35 -cdrom rusty-penguin-real-doom.iso -m 512 \
#       -serial file:/tmp/doom-trace.log -display none -no-reboot
set -uo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

echo "[real-doom] building bare-metal kernel..."
( cd kernel && cargo +nightly build --release -Zjson-target-spec 2>&1 \
    | grep -E "error|Finished" ) || exit 1
KELF="$REPO_ROOT/target/x86_64-rusty-penguin/release/kernel"
[ -f "$KELF" ] || { echo "[real-doom] kernel build failed"; exit 1; }

FBDOOM="$REPO_ROOT/iso/doom-assets/fbdoom"
WAD="$REPO_ROOT/iso/doom-assets/doom1.wad"
[ -f "$FBDOOM" ] || { echo "[real-doom] missing $FBDOOM"; exit 1; }
[ -f "$WAD" ]    || { echo "[real-doom] missing $WAD"; exit 1; }

echo "[real-doom] assembling initrd (fbdoom + WAD + ld.so + libc)..."
T=$(mktemp -d); IR="$T/initrd"
mkdir -p "$IR/bin" "$IR/lib64" "$IR/lib/x86_64-linux-gnu" "$IR/mnt" "$T/boot/grub"
# fbdoom runs as bin/linuxtest (the kernel's `linuxtest` boot mode execs it).
cp "$FBDOOM" "$IR/bin/linuxtest"
# fbdoom with no args searches /mnt/*.wad; DOOM1.WAD = shareware IWAD.
cp "$WAD" "$IR/mnt/DOOM1.WAD"
# Dynamic-linking support: interpreter + libc at the standard search paths
# (no ld.so.cache, so glibc falls back to its built-in trusted dirs).
cp -L /lib64/ld-linux-x86-64.so.2     "$IR/lib64/ld-linux-x86-64.so.2"     2>/dev/null
cp -L /lib/x86_64-linux-gnu/libc.so.6 "$IR/lib/x86_64-linux-gnu/libc.so.6" 2>/dev/null
cp -L /lib/x86_64-linux-gnu/libc.so.6 "$IR/lib64/libc.so.6"                2>/dev/null

cp "$KELF" "$T/boot/kernel.elf"
( cd "$IR" && find . | cpio -o -H newc 2>/dev/null > "$T/boot/initrd-realdoom.img" )

cat > "$T/boot/grub/grub.cfg" <<'CFG'
set timeout=0
set default=0
menuentry "Rusty Penguin -- DOOM (real id Software DOOM, on our own kernel)" {
    multiboot2 /boot/kernel.elf linuxtest
    module2    /boot/initrd-realdoom.img initrd
    boot
}
CFG

echo "[real-doom] making ISO..."
grub-mkrescue -o "$REPO_ROOT/rusty-penguin-real-doom.iso" "$T" 2>/dev/null | tail -1
rm -rf "$T"
echo "[real-doom] done: $(du -h "$REPO_ROOT/rusty-penguin-real-doom.iso" | cut -f1) -> rusty-penguin-real-doom.iso"
echo "[real-doom] watch it:  qemu-system-x86_64 -machine q35 -cdrom rusty-penguin-real-doom.iso -m 512 -vga std -display gtk"
