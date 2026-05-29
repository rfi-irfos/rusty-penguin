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

echo "=== Launching Rusty Penguin (click window to grab mouse, Ctrl+Alt to release) ==="
exec qemu-system-x86_64 \
    -machine q35 \
    -cdrom rusty-penguin.iso \
    -m 512M \
    -vga std \
    -display sdl,show-cursor=off \
    -audiodev wav,id=a0,path=/tmp/rusty-penguin-audio.wav \
    -device intel-hda \
    -device hda-duplex,audiodev=a0 \
    -serial file:/tmp/rusty-penguin.log
