#!/usr/bin/env bash
# Rebuild the ISO from source, then launch QEMU.
# Called by the desktop shortcut — runs in a terminal so build progress is visible.
set -euo pipefail
cd "$(dirname "$(readlink -f "$0")")"

echo "=== Rusty Penguin — rebuilding ISO from source ==="
echo ""
bash iso/build.sh
echo ""
echo "=== Launching QEMU (mouse: click to grab, Ctrl+Alt+G to release) ==="
exec qemu-system-x86_64 \
    -machine pc \
    -cdrom rusty-penguin.iso \
    -m 512M \
    -vga std \
    -display sdl,show-cursor=off \
    -serial file:/tmp/rusty-penguin.log
