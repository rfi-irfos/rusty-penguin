#!/usr/bin/env bash
# Linux-ABI layer test harness. Builds the bare-metal kernel + a minimal GRUB
# ISO that boots `kernel.elf linuxtest` with a chosen Linux binary as
# bin/linuxtest in the initrd, boots headless, and prints the serial log.
# Proof that the from-scratch pure-Rust kernel executes unmodified Linux ELFs.
#
#   iso/build-linux-abi-test.sh [path-to-static-linux-elf]
#       (default: the freestanding kernel/linux-abi-test/linux-hello)
#   RP_NO_KERNEL=1  skip the kernel rebuild (only swap the test binary/initrd)
set -uo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

echo "[lx-abi] building freestanding Linux test ELF..."
gcc -static -nostdlib -no-pie -fno-stack-protector \
    -o "$REPO_ROOT/kernel/linux-abi-test/linux-hello" \
    "$REPO_ROOT/kernel/linux-abi-test/linux-hello.c" || exit 1

TESTBIN="${1:-$REPO_ROOT/kernel/linux-abi-test/linux-hello}"
[ -f "$TESTBIN" ] || { echo "[lx-abi] test binary not found: $TESTBIN"; exit 1; }
echo "[lx-abi] test binary: $TESTBIN"

KELF="$REPO_ROOT/target/x86_64-rusty-penguin/release/kernel"
if [ "${RP_NO_KERNEL:-0}" != "1" ]; then
    echo "[lx-abi] building bare-metal kernel..."
    (cd "$REPO_ROOT/kernel" && cargo +nightly build --release \
        -Zjson-target-spec -Zbuild-std=core,compiler_builtins,alloc \
        -Zbuild-std-features=compiler-builtins-mem \
        --target x86_64-rusty-penguin.json 2>&1 | grep -E "error|Finished") || exit 1
fi

echo "[lx-abi] assembling test ISO (test binary in initrd as bin/linuxtest)..."
T=/tmp/lx-iso; rm -rf "$T"; mkdir -p "$T/boot/grub"
cp "$KELF" "$T/boot/kernel.elf"
# Real cpio initrd carrying the test binary.
IR=/tmp/lx-initrd; rm -rf "$IR"; mkdir -p "$IR/bin"
cp "$TESTBIN" "$IR/bin/linuxtest"
# Dynamic-linking support: ship the interpreter + libc so ld.so can resolve
# DT_NEEDED at the standard paths (no ld.so.cache → built-in search dirs).
mkdir -p "$IR/lib64" "$IR/lib/x86_64-linux-gnu"
cp -L /lib64/ld-linux-x86-64.so.2     "$IR/lib64/ld-linux-x86-64.so.2"      2>/dev/null
cp -L /lib/x86_64-linux-gnu/libc.so.6 "$IR/lib/x86_64-linux-gnu/libc.so.6"  2>/dev/null
cp -L /lib/x86_64-linux-gnu/libc.so.6 "$IR/lib64/libc.so.6"                 2>/dev/null
(cd "$IR" && find . | cpio -o -H newc 2>/dev/null > "$T/boot/initrd-bare.img")
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
