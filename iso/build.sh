#!/usr/bin/env bash
# Builds a bootable Rusty Penguin ISO using the host Linux kernel + a minimal initramfs.
# Requires: grub-mkrescue, qemu-system-x86_64 (for test), cargo

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ISO_DIR="$REPO_ROOT/iso"
OUT_ISO="$REPO_ROOT/rusty-penguin.iso"

echo "[build] Rusty Penguin ISO builder"

# 1. Compile init binary (statically linked where possible)
echo "[build] Compiling init crate..."
cargo build --release -p init --manifest-path "$REPO_ROOT/Cargo.toml" 2>&1
INIT_BIN="$REPO_ROOT/target/release/init"

# 2. Locate host kernel vmlinuz
VMLINUZ=""
for candidate in \
    /boot/vmlinuz \
    /boot/vmlinuz-$(uname -r) \
    $(ls /boot/vmlinuz-* 2>/dev/null | tail -1)
do
    if [ -f "$candidate" ] && [ -r "$candidate" ]; then
        VMLINUZ="$candidate"
        break
    fi
done

if [ -z "$VMLINUZ" ]; then
    # Fallback: grab Debian netboot kernel
    echo "[build] Host vmlinuz not readable, fetching Debian netboot kernel..."
    VMLINUZ="$ISO_DIR/vmlinuz-netboot"
    curl -fsSL "https://deb.debian.org/debian/dists/bookworm/main/installer-amd64/current/images/netboot/debian-installer/amd64/linux" \
         -o "$VMLINUZ"
fi

echo "[build] Using kernel: $VMLINUZ"

# 3. Build initramfs: only our init binary as /init
echo "[build] Building initramfs..."
INITRAMFS_DIR="$(mktemp -d)"
trap "rm -rf $INITRAMFS_DIR" EXIT

mkdir -p "$INITRAMFS_DIR"
cp "$INIT_BIN" "$INITRAMFS_DIR/init"
chmod +x "$INITRAMFS_DIR/init"

INITRD="$ISO_DIR/initrd.img"
(cd "$INITRAMFS_DIR" && find . | cpio -o -H newc 2>/dev/null | gzip -9 > "$INITRD")
echo "[build] initrd.img: $(du -sh "$INITRD" | cut -f1)"

# 4. Assemble ISO tree
echo "[build] Assembling ISO tree..."
mkdir -p "$ISO_DIR/boot/grub"
cp "$VMLINUZ" "$ISO_DIR/boot/vmlinuz"
cp "$INITRD"  "$ISO_DIR/boot/initrd.img"
# grub.cfg is already at iso/grub/grub.cfg
cp "$ISO_DIR/grub/grub.cfg" "$ISO_DIR/boot/grub/grub.cfg"

# 5. Build ISO
echo "[build] Running grub-mkrescue..."
grub-mkrescue -o "$OUT_ISO" "$ISO_DIR" -- -quiet 2>&1

echo ""
echo "[build] Done: $OUT_ISO ($(du -sh "$OUT_ISO" | cut -f1))"
echo ""
echo "  Test with QEMU:"
echo "    qemu-system-x86_64 -cdrom $OUT_ISO -m 512M -nographic"
echo ""
echo "  Boot in VirtualBox:"
echo "    VBoxManage startvm 'Rusty Penguin'"
