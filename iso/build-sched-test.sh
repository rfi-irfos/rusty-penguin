#!/usr/bin/env bash
# Build a headless test ISO for a kernel self-test boot flag, and a trace runner.
# Used to verify the scheduler / VMM migration increments (docs/SCHEDULER.md,
# docs/VMM_HIGHER_HALF.md). QEMU is blocked in the Claude agent's sandbox, so
# the human runs the trace; this script regenerates the harness from source.
#
#   bash iso/build-sched-test.sh <flag>
#     <flag> ∈ schedtest schedtest2 schedtest3 schedtest4 schedtest5 physmaptest
#       schedtest    Inc 1  cooperative context switch
#       schedtest2   Inc 2  timer preemption
#       schedtest3   Inc 3a per-process address spaces (CR3)
#       schedtest4   Inc 3b per-task CR3 switching under preemption
#       schedtest5   Inc 3c ring-3 task in a private address space
#       physmaptest  VMM step 1a  higher-half physmap
#
#   Then run:  bash /tmp/rp-schedtrace.sh   →  reads /tmp/sched-serial.log
set -uo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FLAG="${1:-schedtest}"

echo "[sched-test] building kernel..."
( cd "$REPO/kernel" && cargo +nightly build --release -Zjson-target-spec 2>&1 \
    | grep -E "error|Finished" ) || exit 1
KELF="$REPO/target/x86_64-rusty-penguin/release/kernel"

T=/tmp/sched-iso; rm -rf "$T"; mkdir -p "$T/boot/grub"
cp "$KELF" "$T/boot/kernel.elf"
IR=/tmp/sched-initrd; rm -rf "$IR"; mkdir -p "$IR/bin"; echo keep > "$IR/bin/.keep"
( cd "$IR" && find . | cpio -o -H newc 2>/dev/null > "$T/boot/initrd-bare.img" )
cat > "$T/boot/grub/grub.cfg" <<CFG
set timeout=0
set default=0
menuentry "Rusty Penguin -- $FLAG" {
    multiboot2 /boot/kernel.elf $FLAG
    module2    /boot/initrd-bare.img initrd
    boot
}
CFG
grub-mkrescue -o /tmp/sched.iso "$T" 2>/dev/null | tail -1

cat > /tmp/rp-schedtrace.sh <<'EOF'
#!/usr/bin/env bash
rm -f /tmp/sched-serial.log
timeout 14 qemu-system-x86_64 -machine q35 -cdrom /tmp/sched.iso -m 512 \
  -serial file:/tmp/sched-serial.log -display none -no-reboot 2>/dev/null
echo "---done--- ($(wc -l < /tmp/sched-serial.log 2>/dev/null) lines)"
EOF
chmod +x /tmp/rp-schedtrace.sh
echo "[sched-test] ISO=/tmp/sched.iso flag=$FLAG ; run: bash /tmp/rp-schedtrace.sh"
