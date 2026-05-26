#!/usr/bin/env bash
# Builds a bootable Rusty Penguin ISO using the host Linux kernel + a minimal initramfs.
# Requires: grub-mkrescue, qemu-system-x86_64 (for test), cargo

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ISO_DIR="$REPO_ROOT/iso"
OUT_ISO="$REPO_ROOT/rusty-penguin.iso"

echo "[build] Rusty Penguin ISO builder"

# 1. Compile init and shell binaries
echo "[build] Compiling init and shell crates..."
cargo build --release -p init -p shell --manifest-path "$REPO_ROOT/Cargo.toml" 2>&1
INIT_BIN="$REPO_ROOT/target/release/init"
PSH_BIN="$REPO_ROOT/target/release/shell"

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

# 3. Build initramfs: init binary + required shared libraries
echo "[build] Building initramfs..."
INITRAMFS_DIR="$(mktemp -d)"
trap "rm -rf $INITRAMFS_DIR" EXIT

mkdir -p "$INITRAMFS_DIR/lib/x86_64-linux-gnu"
mkdir -p "$INITRAMFS_DIR/lib64"
mkdir -p "$INITRAMFS_DIR/proc" "$INITRAMFS_DIR/sys" "$INITRAMFS_DIR/dev" "$INITRAMFS_DIR/tmp"
mkdir -p "$INITRAMFS_DIR/bin" "$INITRAMFS_DIR/usr/local/bin"

cp "$INIT_BIN" "$INITRAMFS_DIR/init"
chmod +x "$INITRAMFS_DIR/init"

# psh (Penguin Shell) — both locations init looks for
cp "$PSH_BIN" "$INITRAMFS_DIR/bin/psh"
cp "$PSH_BIN" "$INITRAMFS_DIR/usr/local/bin/psh"
chmod +x "$INITRAMFS_DIR/bin/psh" "$INITRAMFS_DIR/usr/local/bin/psh"

# Bundle shared libraries required by the dynamically-linked init binary
for lib in libc.so.6 libgcc_s.so.1; do
    src=$(ldconfig -p 2>/dev/null | awk "/$lib/"'{print $NF}' | head -1)
    [ -z "$src" ] && src=$(find /lib /lib64 /usr/lib -name "$lib" 2>/dev/null | head -1)
    if [ -n "$src" ]; then
        cp "$src" "$INITRAMFS_DIR/lib/x86_64-linux-gnu/$lib"
    fi
done
# Copy the dynamic linker
LD_SRC=$(readlink -f /lib64/ld-linux-x86-64.so.2 2>/dev/null || echo "/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2")
cp "$LD_SRC" "$INITRAMFS_DIR/lib64/ld-linux-x86-64.so.2"
ln -sf /lib64/ld-linux-x86-64.so.2 "$INITRAMFS_DIR/lib/ld-linux-x86-64.so.2" 2>/dev/null || true

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
grub-mkrescue -o "$OUT_ISO" "$ISO_DIR" 2>&1

echo ""
echo "[build] Done: $OUT_ISO ($(du -sh "$OUT_ISO" | cut -f1))"
echo ""
echo "  Test with QEMU:"
echo "    qemu-system-x86_64 -cdrom $OUT_ISO -m 512M -nographic"
echo ""
echo "  Boot in VirtualBox:"
echo "    VBoxManage startvm 'Rusty Penguin'"
