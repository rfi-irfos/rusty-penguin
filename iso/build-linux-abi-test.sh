#!/usr/bin/env bash
# Linux-ABI layer test harness. Builds the bare-metal kernel + a minimal GRUB
# ISO whose only entry boots the kernel with `linuxtest` on the cmdline, then
# boots it headless and prints the serial log. Proof that the from-scratch
# pure-Rust kernel executes an unmodified static Linux x86-64 ELF.
#
#   iso/build-linux-abi-test.sh
set -uo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

echo "[lx-abi] building freestanding Linux test ELF..."
gcc -static -nostdlib -no-pie -fno-stack-protector \
    -o "$REPO_ROOT/kernel/linux-abi-test/linux-hello" \
    "$REPO_ROOT/kernel/linux-abi-test/linux-hello.c" || exit 1

echo "[lx-abi] building bare-metal kernel..."
(cd "$REPO_ROOT/kernel" && cargo +nightly build --release \
    -Zjson-target-spec -Zbuild-std=core,compiler_builtins,alloc \
    -Zbuild-std-features=compiler-builtins-mem \
    --target x86_64-rusty-penguin.json 2>&1 | grep -E "error|Finished") || exit 1
KELF="$REPO_ROOT/target/x86_64-rusty-penguin/release/kernel"

echo "[lx-abi] assembling minimal test ISO..."
T=/tmp/lx-iso; rm -rf "$T"; mkdir -p "$T/boot/grub"
cp "$KELF" "$T/boot/kernel.elf"
echo "x" > "$T/boot/initrd-bare.img"
cat > "$T/boot/grub/grub.cfg" <<'CFG'
set timeout=0
set default=0
menuentry "Linux ABI test" {
    multiboot2 /boot/kernel.elf linuxtest
    module2    /boot/initrd-bare.img initrd
    boot
}
CFG
grub-mkrescue -o /tmp/lx-test.iso "$T" 2>/dev/null | tail -1

echo "[lx-abi] booting headless (serial capture)..."
rm -f /tmp/lx-serial.log
timeout "${RP_TIMEOUT:-22}" qemu-system-x86_64 -cdrom /tmp/lx-test.iso \
    -serial file:/tmp/lx-serial.log -display none -m 512 -no-reboot 2>/dev/null
echo "=== serial: Linux-ABI section ==="
sed -n '/LINUX-ABI/,$p' /tmp/lx-serial.log 2>/dev/null
