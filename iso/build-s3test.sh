#!/usr/bin/env bash
# Build a bootable ISO and verify ACPI S3 suspend-to-RAM end-to-end using QMP.
# The kernel boots with the "s3test" cmdline flag, sets up the real-mode resume
# trampoline at physical 0x8000, writes firmware_waking_vector to FACS, and
# enters S3.  This script waits for QEMU to enter "suspended" state, issues
# system_wakeup, and confirms the trampoline fired via the serial log.
#
# Verified: QEMU status="suspended" + serial "[acpi] resumed from S3"
# Usage:  bash iso/build-s3test.sh
set -uo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

echo "[s3test] building..."
( cd "$REPO/kernel" && cargo +nightly build --release -Zjson-target-spec \
    -Zbuild-std=core,compiler_builtins,alloc -Zbuild-std-features=compiler-builtins-mem \
    --target x86_64-rusty-penguin.json 2>&1 | grep -E "error|Finished" ) || exit 1
( cd "$REPO/user-psh" && cargo +nightly build --release -Zjson-target-spec \
    -Zbuild-std=core,compiler_builtins -Zbuild-std-features=compiler-builtins-mem 2>&1 | grep -E "error|Finished" ) || exit 1
( cd "$REPO/desktop-metal" && cargo +nightly build --release -Zjson-target-spec \
    -Zbuild-std=core,alloc,compiler_builtins -Zbuild-std-features=compiler-builtins-mem 2>&1 | grep -E "error|Finished" ) || exit 1

T=/tmp/s3test-iso; rm -rf "$T"; mkdir -p "$T/boot/grub"
cp "$REPO/target/x86_64-rusty-penguin/release/kernel" "$T/boot/kernel.elf"
IR=/tmp/s3test-initrd; rm -rf "$IR"; mkdir -p "$IR/bin"
cp "$REPO/user-psh/target/x86_64-user-psh/release/user-psh"  "$IR/bin/psh"
cp "$REPO/desktop-metal/target/x86_64-user-psh/release/desktop-metal" "$IR/bin/desktop"
( cd "$IR" && find . | cpio -o -H newc 2>/dev/null > "$T/boot/initrd.img" )
cat > "$T/boot/grub/grub.cfg" << 'CFG'
set timeout=0
set default=0
menuentry "Rusty Penguin -- S3 suspend/resume test" {
    multiboot2 /boot/kernel.elf s3test
    module2    /boot/initrd.img initrd
    boot
}
CFG
ISO="$REPO/rusty-penguin-s3test.iso"
grub-mkrescue -o "$ISO" "$T" 2>/dev/null | tail -1
rm -rf "$T" "$IR"
echo "[s3test] ISO: $ISO ($(du -h "$ISO" | cut -f1))"

SERIAL=/tmp/s3-serial.log; QMP=/tmp/s3-qmp.sock; rm -f "$SERIAL" "$QMP"
qemu-system-x86_64 -machine q35 -m 512 -cdrom "$ISO" \
  -display none -serial file:"$SERIAL" -no-reboot \
  -qmp unix:"$QMP",server,nowait -audiodev none,id=a0 &
QEMU_PID=$!

echo "[s3test] waiting for S3 suspend..."
for i in $(seq 1 45); do
    sleep 1
    grep -q "entering S3" "$SERIAL" 2>/dev/null && { echo "[s3test] SUSPENDED at t=${i}s"; break; }
    [ $i -eq 45 ] && { echo "[s3test] TIMEOUT"; kill $QEMU_PID 2>/dev/null; exit 1; }
done
sleep 1

python3 - "$QMP" << 'PYEOF'
import socket, json, sys, time
def qmp(path):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.connect(path); s.settimeout(3)
    def rx():
        b = b""
        while True:
            try: chunk = s.recv(4096)
            except socket.timeout: break
            if not chunk: break
            b += chunk
            try: json.loads(b.decode()); break
            except: pass
        return b.decode()
    rx()  # greeting
    s.sendall(b'{"execute":"qmp_capabilities"}\n'); rx()
    s.sendall(b'{"execute":"query-status"}\n')
    st = json.loads(rx())
    print("[s3test] QEMU status:", st["return"]["status"])
    assert st["return"]["status"] == "suspended", "not suspended"
    s.sendall(b'{"execute":"system_wakeup"}\n'); rx()
    print("[s3test] wakeup sent")
    s.close()
qmp(sys.argv[1])
PYEOF

sleep 3
if grep -q "resumed from S3" "$SERIAL" 2>/dev/null; then
    echo "[s3test] RESUME CONFIRMED — serial: 'resumed from S3 — kernel alive'"
    echo "[s3test] === S3 SUSPEND/RESUME PASS ==="
else
    echo "[s3test] FAIL — resume not seen"; tail -15 "$SERIAL"; kill $QEMU_PID 2>/dev/null; exit 1
fi
kill $QEMU_PID 2>/dev/null; wait $QEMU_PID 2>/dev/null
