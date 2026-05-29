#!/usr/bin/env bash
# Fast iterate on desktop-metal only: rebuild ELF, repack bare initrd, remaster ISO.
set -e
cd /home/eri-irfos/projects/rusty-penguin
( cd desktop-metal && cargo +nightly build --release -Zjson-target-spec \
    -Zbuild-std=core,alloc,compiler_builtins -Zbuild-std-features=compiler-builtins-mem 2>&1 | tail -2 )
TMP=$(mktemp -d); mkdir -p "$TMP/bin"
cp kernel/user-psh.elf "$TMP/bin/psh"
cp desktop-metal/target/x86_64-user-psh/release/desktop-metal "$TMP/bin/desktop"
( cd "$TMP" && find . | cpio -o -H newc 2>/dev/null > /home/eri-irfos/projects/rusty-penguin/iso/boot/initrd-bare.img )
rm -rf "$TMP"
grub-mkrescue -o rusty-penguin.iso iso 2>/dev/null | tail -1
echo "[repack] done: $(du -sh rusty-penguin.iso | cut -f1)"
