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
cargo build --release -p init -p shell -p desktop -p installer --manifest-path "$REPO_ROOT/Cargo.toml" 2>&1 \
    || echo "[build] Note: init/shell/desktop/installer step skipped (using cached artifacts)"
INIT_BIN="$REPO_ROOT/target/release/init"
PSH_BIN="$REPO_ROOT/target/release/shell"

# 2. Locate host kernel vmlinuz
# CRITICAL: the kernel and the virtio_input.ko module MUST be the same version.
# /boot/vmlinuz is usually only root-readable; use sudo to copy it so the
# bundled module (from uname -r) always matches the booted kernel.
KVER_HOST=$(uname -r)
VMLINUZ_CACHED="$ISO_DIR/vmlinuz-${KVER_HOST}"

VMLINUZ=""

# Fast path: we already cached this kernel version
if [ -f "$VMLINUZ_CACHED" ] && [ -r "$VMLINUZ_CACHED" ]; then
    VMLINUZ="$VMLINUZ_CACHED"
    echo "[build] Using cached host kernel: $VMLINUZ"
fi

# Try direct read first
if [ -z "$VMLINUZ" ]; then
    for candidate in /boot/vmlinuz-${KVER_HOST} /boot/vmlinuz; do
        if [ -f "$candidate" ] && [ -r "$candidate" ]; then
            VMLINUZ="$candidate"
            echo "[build] Host vmlinuz readable directly: $VMLINUZ"
            break
        fi
    done
fi

# sudo copy — ensures host kernel matches the bundled module
if [ -z "$VMLINUZ" ]; then
    echo "[build] vmlinuz not directly readable; copying via sudo..."
    for candidate in /boot/vmlinuz-${KVER_HOST} /boot/vmlinuz; do
        if [ -f "$candidate" ]; then
            mkdir -p "$(dirname "$VMLINUZ_CACHED")"
            if echo '19950617123' | sudo -S sh -c "cp '$candidate' '$VMLINUZ_CACHED' && chmod 644 '$VMLINUZ_CACHED'" 2>/dev/null; then
                VMLINUZ="$VMLINUZ_CACHED"
                echo "[build] Copied host kernel via sudo: $KVER_HOST"
                break
            fi
        fi
    done
fi

if [ -z "$VMLINUZ" ]; then
    echo "[build] ERROR: Could not obtain host vmlinuz — aborting."
    echo "[build] Run once as root: cp /boot/vmlinuz-\$(uname -r) $VMLINUZ_CACHED && chmod 644 $VMLINUZ_CACHED"
    exit 1
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

# desktop binary
DESKTOP_BIN="$REPO_ROOT/target/release/desktop"
if [ -f "$DESKTOP_BIN" ]; then
    cp "$DESKTOP_BIN" "$INITRAMFS_DIR/bin/desktop"
    cp "$DESKTOP_BIN" "$INITRAMFS_DIR/usr/local/bin/desktop"
    chmod +x "$INITRAMFS_DIR/bin/desktop" "$INITRAMFS_DIR/usr/local/bin/desktop"
fi

# rp-install (install-to-disk)
RPINSTALL_BIN="$REPO_ROOT/target/release/rp-install"
if [ -f "$RPINSTALL_BIN" ]; then
    cp "$RPINSTALL_BIN" "$INITRAMFS_DIR/bin/rp-install"
    chmod +x "$INITRAMFS_DIR/bin/rp-install"
fi

# Bundle static busybox — provides mke2fs (format the persistence disk), mount,
# fdisk, etc. in a single dependency-free binary. init shells out to it to
# provision a blank disk for persistent /home.
BUSYBOX_SRC=$(command -v busybox || echo /usr/bin/busybox)
if [ -x "$BUSYBOX_SRC" ] && file "$BUSYBOX_SRC" | grep -q "statically linked"; then
    cp "$BUSYBOX_SRC" "$INITRAMFS_DIR/bin/busybox"
    chmod +x "$INITRAMFS_DIR/bin/busybox"
    echo "[build] bundled static busybox (mke2fs/mount/fdisk/udhcpc)"
else
    echo "[build] WARNING: static busybox not found — disk auto-provisioning disabled"
fi

# udhcpc lease handler (busybox DHCP client calls this to apply the lease).
mkdir -p "$INITRAMFS_DIR/etc"
if [ -f "$ISO_DIR/assets/udhcpc.script" ]; then
    cp "$ISO_DIR/assets/udhcpc.script" "$INITRAMFS_DIR/etc/udhcpc.script"
    chmod +x "$INITRAMFS_DIR/etc/udhcpc.script"
    echo "[build] bundled udhcpc lease script"
fi

# ── Installer tooling (rp-install: partition + format + bootloader to disk) ────
# Helper: copy a dynamic binary plus every shared lib ldd reports.
bundle_with_libs() {
    local bin="$1" name="${2:-$(basename "$1")}"
    [ -x "$bin" ] || { echo "[build] WARNING: $bin missing — skipping $name"; return 1; }
    cp "$bin" "$INITRAMFS_DIR/bin/$name"; chmod +x "$INITRAMFS_DIR/bin/$name"
    ldd "$bin" 2>/dev/null | awk '/=>/{print $3} /ld-linux/{print $1}' | while read -r lib; do
        [ -f "$lib" ] || continue
        case "$lib" in
            */ld-linux*) cp "$lib" "$INITRAMFS_DIR/lib64/$(basename "$lib")" ;;
            *)           cp "$lib" "$INITRAMFS_DIR/lib/x86_64-linux-gnu/$(basename "$lib")" ;;
        esac
    done
    echo "[build] bundled $name (+libs)"
}
# sgdisk (GPT partitioning), mkfs.fat (ESP). mke2fs comes from busybox.
bundle_with_libs "$(command -v sgdisk || echo /usr/sbin/sgdisk)" sgdisk
bundle_with_libs "$(command -v mkfs.fat || echo /usr/sbin/mkfs.fat)" mkfs.fat

# Kernel modules the installer loads (busybox insmod): isofs (read kernel/initrd
# off the CD) and nls_iso8859-1 (vfat's default IO charset, needed to mount the
# FAT ESP). Both decompressed from .zst to raw ELF.
mkdir -p "$INITRAMFS_DIR/lib/modules"
for modname in isofs nls_iso8859-1; do
    MODSRC=$(find /lib/modules/$KVER_HOST -name "$modname.ko*" 2>/dev/null | head -1)
    [ -z "$MODSRC" ] && { echo "[build] WARNING: $modname.ko not found"; continue; }
    if [[ "$MODSRC" == *.zst ]]; then
        zstd -d "$MODSRC" -o "$INITRAMFS_DIR/lib/modules/$modname.ko" --force >/dev/null 2>&1
    else
        cp "$MODSRC" "$INITRAMFS_DIR/lib/modules/$modname.ko"
    fi
    echo "[build] bundled $modname.ko"
done

# Standalone GRUB EFI image for installed disks. Embeds a grub.cfg that finds
# the ESP (the partition holding this very EFI) and boots the kernel from it.
if command -v grub-mkstandalone >/dev/null; then
    GRUBCFG_TMP="$(mktemp)"
    cat > "$GRUBCFG_TMP" <<'GRUBCFG'
insmod part_gpt
insmod fat
insmod ext2
search --no-floppy --file --set=root /EFI/BOOT/BOOTX64.EFI
linux ($root)/vmlinuz quiet loglevel=0 console=tty0 init=/init rw
initrd ($root)/initrd.img
boot
GRUBCFG
    mkdir -p "$INITRAMFS_DIR/usr/lib/rp"
    grub-mkstandalone -O x86_64-efi \
        -o "$INITRAMFS_DIR/usr/lib/rp/BOOTX64.EFI" \
        "boot/grub/grub.cfg=$GRUBCFG_TMP" 2>/dev/null \
        && echo "[build] bundled standalone BOOTX64.EFI ($(du -h "$INITRAMFS_DIR/usr/lib/rp/BOOTX64.EFI" | cut -f1))"
    rm -f "$GRUBCFG_TMP"
fi

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

# Bundle virtio_input kernel module — MUST match the kernel being booted.
# KVER_HOST was set when we copied vmlinuz, guaranteeing version parity.
# finit_module syscall requires raw ELF (not .ko.zst); decompress here.
VIRTIO_MOD=$(find /lib/modules/$KVER_HOST -name "virtio_input.ko*" 2>/dev/null | head -1)
if [ -n "$VIRTIO_MOD" ]; then
    mkdir -p "$INITRAMFS_DIR/lib/modules"
    if [[ "$VIRTIO_MOD" == *.zst ]]; then
        zstd -d "$VIRTIO_MOD" -o "$INITRAMFS_DIR/lib/modules/virtio_input.ko" --force
        echo "[build] bundled virtio_input (decompressed from .zst): $(basename $VIRTIO_MOD)"
    else
        cp "$VIRTIO_MOD" "$INITRAMFS_DIR/lib/modules/virtio_input.ko"
        echo "[build] bundled virtio_input: $(basename $VIRTIO_MOD)"
    fi
else
    echo "[build] WARNING: virtio_input module not found"
fi

INITRD="$ISO_DIR/initrd.img"
(cd "$INITRAMFS_DIR" && find . | cpio -o -H newc 2>/dev/null | gzip -9 > "$INITRD")
echo "[build] initrd.img: $(du -sh "$INITRD" | cut -f1)"

# 2b. Web initrd: the lean initramfs + the X11 web-rootfs (Xorg/xterm + closure),
# for the `rp.web` boot mode. Kept separate so the default desktop/DOOM initrds
# stay small (the X stack only loads when you boot the Web entry).
echo "[build] Building web-rootfs (X11 stack)..."
WEB_STAGE="$(mktemp -d)"
trap "rm -rf $WEB_STAGE $INITRAMFS_DIR" EXIT
bash "$ISO_DIR/build-web-rootfs.sh" "$WEB_STAGE" 2>&1 | sed 's/^/    /'
# Merge: base initramfs (init, busybox, libs) + the X rootfs.
WEB_DIR="$(mktemp -d)"
trap "rm -rf $WEB_DIR $WEB_STAGE $INITRAMFS_DIR" EXIT
cp -a "$INITRAMFS_DIR/." "$WEB_DIR/"
cp -a "$WEB_STAGE/." "$WEB_DIR/"
cp "$WEB_STAGE/start-x.sh" "$WEB_DIR/start-x.sh" 2>/dev/null || true
WEB_INITRD="$ISO_DIR/initrd-web.img"
(cd "$WEB_DIR" && find . | cpio -o -H newc 2>/dev/null | gzip -9 > "$WEB_INITRD")
echo "[build] initrd-web.img: $(du -sh "$WEB_INITRD" | cut -f1)"

# 3a. Build user-psh (bare-metal ring-3 shell — must be before kernel)
echo "[build] Building user-psh..."
(cd "$REPO_ROOT/user-psh" && cargo +nightly build --release \
    -Zjson-target-spec \
    -Zbuild-std=core,compiler_builtins \
    -Zbuild-std-features=compiler-builtins-mem 2>&1)
USER_PSH_ELF="$REPO_ROOT/user-psh/target/x86_64-user-psh/release/user-psh"
if [ ! -f "$USER_PSH_ELF" ]; then
    echo "[build] ERROR: user-psh ELF not found at $USER_PSH_ELF"
    exit 1
fi
cp "$USER_PSH_ELF" "$REPO_ROOT/kernel/user-psh.elf"
echo "[build] user-psh.elf: $(du -sh "$REPO_ROOT/kernel/user-psh.elf" | cut -f1)"

# 3a1. Build desktop-metal (bare-metal graphical desktop — Phase 5)
echo "[build] Building desktop-metal..."
(cd "$REPO_ROOT/desktop-metal" && cargo +nightly build --release \
    -Zjson-target-spec \
    -Zbuild-std=core,alloc,compiler_builtins \
    -Zbuild-std-features=compiler-builtins-mem 2>&1) || echo "[build] WARNING: desktop-metal build failed — skipping"
DESKTOP_METAL_ELF="$REPO_ROOT/desktop-metal/target/x86_64-user-psh/release/desktop-metal"
if [ -f "$DESKTOP_METAL_ELF" ]; then
    echo "[build] desktop-metal: $(du -sh "$DESKTOP_METAL_ELF" | cut -f1)"
fi

# 3a2. Pack bare-metal initramfs (CPIO newc) for VFS
# Contains: bin/psh  (and later bin/desktop when Phase 5 is ready)
echo "[build] Building bare-metal CPIO initramfs..."
BARE_INITRAMFS_DIR="$(mktemp -d)"
trap "rm -rf $BARE_INITRAMFS_DIR $INITRAMFS_DIR" EXIT
mkdir -p "$BARE_INITRAMFS_DIR/bin"
cp "$USER_PSH_ELF" "$BARE_INITRAMFS_DIR/bin/psh"
# Add desktop-metal ELF if it exists
DESKTOP_METAL_ELF="$REPO_ROOT/desktop-metal/target/x86_64-user-psh/release/desktop-metal"
if [ -f "$DESKTOP_METAL_ELF" ]; then
    cp "$DESKTOP_METAL_ELF" "$BARE_INITRAMFS_DIR/bin/desktop"
    echo "[build]   + bin/desktop (desktop-metal)"
fi
BARE_INITRD="$ISO_DIR/initrd-bare.img"
(cd "$BARE_INITRAMFS_DIR" && find . | cpio -o -H newc 2>/dev/null > "$BARE_INITRD")
echo "[build] initrd-bare.img: $(du -sh "$BARE_INITRD" | cut -f1)"

# 3b. Build bare-metal kernel ELF
echo "[build] Building bare-metal kernel..."
(cd "$REPO_ROOT/kernel" && cargo +nightly build --release \
    -Zjson-target-spec \
    -Zbuild-std=core,compiler_builtins,alloc \
    -Zbuild-std-features=compiler-builtins-mem \
    --target x86_64-rusty-penguin.json 2>&1)
KERNEL_ELF="$REPO_ROOT/target/x86_64-rusty-penguin/release/kernel"
if [ ! -f "$KERNEL_ELF" ]; then
    echo "[build] ERROR: kernel.elf not found at $KERNEL_ELF"
    exit 1
fi
echo "[build] kernel.elf: $(du -sh "$KERNEL_ELF" | cut -f1)"

# 3c. Build DOOM live initramfs (Linux track, framebuffer doom on /dev/fb0)
# The "demoable" milestone: boot the ISO straight into fbDOOM. Assets are
# vendored under iso/doom-assets/ (fbdoom binary, shareware doom1.wad,
# static doom-init.c). doom-init mounts devtmpfs so /dev/fb0 exists, then
# execs fbdoom on the WAD. /dev/fb0 itself comes from GRUB gfxpayload=keep.
DOOM_ASSETS="$ISO_DIR/doom-assets"
if [ -f "$DOOM_ASSETS/fbdoom" ] && [ -f "$DOOM_ASSETS/doom1.wad" ]; then
    echo "[build] Building DOOM live initramfs..."
    DOOM_DIR="$(mktemp -d)"
    trap "rm -rf $DOOM_DIR $BARE_INITRAMFS_DIR $INITRAMFS_DIR" EXIT
    mkdir -p "$DOOM_DIR"/{proc,sys,dev,lib/x86_64-linux-gnu,lib64}
    # Rebuild static init from source if a C compiler is present; else use vendored binary
    if command -v gcc >/dev/null && [ -f "$DOOM_ASSETS/doom-init.c" ]; then
        gcc -static -O2 -o "$DOOM_DIR/init" "$DOOM_ASSETS/doom-init.c"
    else
        cp "$DOOM_ASSETS/doom-init" "$DOOM_DIR/init"
    fi
    chmod +x "$DOOM_DIR/init"
    cp "$DOOM_ASSETS/fbdoom"   "$DOOM_DIR/fbdoom"   && chmod +x "$DOOM_DIR/fbdoom"
    cp "$DOOM_ASSETS/doom1.wad" "$DOOM_DIR/doom1.wad"
    # fbdoom is dynamically linked: bundle libc + ld
    DLIBC=$(ldconfig -p 2>/dev/null | awk '/libc.so.6/{print $NF}' | head -1)
    [ -n "$DLIBC" ] && cp "$DLIBC" "$DOOM_DIR/lib/x86_64-linux-gnu/libc.so.6"
    DLD=$(readlink -f /lib64/ld-linux-x86-64.so.2 2>/dev/null)
    [ -n "$DLD" ] && cp "$DLD" "$DOOM_DIR/lib64/ld-linux-x86-64.so.2"
    ln -sf /lib64/ld-linux-x86-64.so.2 "$DOOM_DIR/lib/ld-linux-x86-64.so.2" 2>/dev/null || true
    DOOM_INITRD="$ISO_DIR/initrd-doom.img"
    (cd "$DOOM_DIR" && find . | cpio -o -H newc 2>/dev/null | gzip -9 > "$DOOM_INITRD")
    echo "[build] initrd-doom.img: $(du -sh "$DOOM_INITRD" | cut -f1)"
else
    echo "[build] WARNING: iso/doom-assets/{fbdoom,doom1.wad} missing — skipping DOOM entry"
fi

# 4. Assemble ISO tree
echo "[build] Assembling ISO tree..."
mkdir -p "$ISO_DIR/boot/grub"
cp "$VMLINUZ" "$ISO_DIR/boot/vmlinuz"
cp "$INITRD"  "$ISO_DIR/boot/initrd.img"
cp "$KERNEL_ELF" "$ISO_DIR/boot/kernel.elf"
# bare-metal CPIO module (already at ISO_DIR/initrd-bare.img)
cp "$BARE_INITRD" "$ISO_DIR/boot/initrd-bare.img"
[ -f "$ISO_DIR/initrd-doom.img" ] && cp "$ISO_DIR/initrd-doom.img" "$ISO_DIR/boot/initrd-doom.img"
[ -f "$ISO_DIR/initrd-web.img" ]  && cp "$ISO_DIR/initrd-web.img"  "$ISO_DIR/boot/initrd-web.img"
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
