#!/usr/bin/env bash
# Launch Rusty Penguin in QEMU.
# Default: use the existing rusty-penguin.iso (fast, no rebuild).
# Pass --rebuild to rebuild the ISO from source first.
set -euo pipefail
cd "$(dirname "$(readlink -f "$0")")"

REBUILD=0
for arg in "$@"; do
    case "$arg" in --rebuild|-r) REBUILD=1 ;; esac
done

if [ "$REBUILD" -eq 1 ]; then
    echo "=== Rusty Penguin — rebuilding ISO from source ==="
    echo ""
    bash iso/build.sh
    echo ""
fi

if [ ! -f rusty-penguin.iso ]; then
    echo "ERROR: rusty-penguin.iso not found. Run: bash launch.sh --rebuild"
    exit 1
fi

# Persistent disk — files you create (editor saves, terminal `echo > f`) are
# mirrored to RPFS on this image and reloaded on the next boot. Created once,
# 256 MiB, and kept across runs. Delete it to start with a clean filesystem.
DISK=rusty-penguin-disk.img
if [ ! -f "$DISK" ]; then
    echo "=== Creating persistent disk $DISK (256 MiB) ==="
    qemu-img create -f raw "$DISK" 256M >/dev/null 2>&1 \
        || dd if=/dev/zero of="$DISK" bs=1M count=256 2>/dev/null
fi

echo "=== Launching Rusty Penguin (click window to grab mouse, Ctrl+Alt to release) ==="
exec qemu-system-x86_64 \
    -machine q35 \
    -cdrom rusty-penguin.iso \
    -drive id=hd0,file="$DISK",format=raw,if=none \
    -device ich9-ahci,id=ahci \
    -device ide-hd,drive=hd0,bus=ahci.0 \
    -m 512M \
    -vga std \
    -display sdl,show-cursor=off \
    -audiodev wav,id=a0,path=/tmp/rusty-penguin-audio.wav \
    -device intel-hda \
    -device hda-duplex,audiodev=a0 \
    -netdev user,id=n0 \
    -device rtl8139,netdev=n0 \
    -serial file:/tmp/rusty-penguin.log
