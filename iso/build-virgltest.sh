#!/usr/bin/env bash
# Build and verify the virgl 3D GPU control path end-to-end.
# Requires: qemu-system-x86_64 with virtio-gpu-gl + egl-headless support.
# Usage:  bash iso/build-virgltest.sh
set -uo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

echo "[virgltest] building..."
( cd "$REPO/kernel" && cargo +nightly build --release -Zjson-target-spec \
    -Zbuild-std=core,compiler_builtins,alloc -Zbuild-std-features=compiler-builtins-mem \
    --target x86_64-rusty-penguin.json 2>&1 | grep -E "error|Finished" ) || exit 1
( cd "$REPO/user-psh" && cargo +nightly build --release -Zjson-target-spec \
    -Zbuild-std=core,compiler_builtins -Zbuild-std-features=compiler-builtins-mem 2>&1 | grep -E "error|Finished" ) || exit 1
( cd "$REPO/desktop-metal" && cargo +nightly build --release -Zjson-target-spec \
    -Zbuild-std=core,alloc,compiler_builtins -Zbuild-std-features=compiler-builtins-mem 2>&1 | grep -E "error|Finished" ) || exit 1

T=/tmp/virgltest-iso; rm -rf "$T"; mkdir -p "$T/boot/grub"
cp "$REPO/target/x86_64-rusty-penguin/release/kernel" "$T/boot/kernel.elf"
IR=/tmp/virgltest-initrd; rm -rf "$IR"; mkdir -p "$IR/bin"
cp "$REPO/user-psh/target/x86_64-user-psh/release/user-psh" "$IR/bin/psh"
cp "$REPO/desktop-metal/target/x86_64-user-psh/release/desktop-metal" "$IR/bin/desktop"
( cd "$IR" && find . | cpio -o -H newc 2>/dev/null > "$T/boot/initrd.img" )
cat > "$T/boot/grub/grub.cfg" << 'CFG'
set timeout=0
set default=0
menuentry "Rusty Penguin -- virgl 3D test" {
    multiboot2 /boot/kernel.elf virgltest
    module2    /boot/initrd.img initrd
    boot
}
CFG
ISO="$REPO/rusty-penguin-virgltest.iso"
grub-mkrescue -o "$ISO" "$T" 2>/dev/null
rm -rf "$T" "$IR"
echo "[virgltest] ISO: $ISO"

SERIAL=/tmp/virgl-serial.log; rm -f "$SERIAL"
timeout 25 qemu-system-x86_64 \
  -machine q35 -m 512 \
  -device virtio-gpu-gl,max_outputs=1 \
  -display egl-headless \
  -cdrom "$ISO" \
  -no-reboot \
  -serial file:"$SERIAL" \
  -audiodev none,id=a0 2>/dev/null &
QPID=$!
sleep 18
kill $QPID 2>/dev/null; wait $QPID 2>/dev/null

echo "[virgltest] --- serial ---"
grep -E "virgl|VIRGL|3D|virtio-gpu" "$SERIAL" 2>/dev/null

if grep -q "virgl 3D resource path PROVED" "$SERIAL" 2>/dev/null; then
    echo "[virgltest] === VIRGL 3D RESOURCE PATH PASS ==="
else
    echo "[virgltest] FAIL — 3D resource path not confirmed"; exit 1
fi
