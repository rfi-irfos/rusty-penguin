# Disk persistence (RPFS)

Files you create on Rusty Penguin survive a reboot. Save a note in the Text
Editor, run `echo hello > notes.txt` in the terminal, drop a file in the File
Manager — it's all still there next boot.

## How it works

```
Desktop VFS (in-memory)  ──persist──▶  syscall 25 (sys_disk_write)  ──▶  RPFS  ──▶  AHCI/SATA disk
        ▲                                                                              │
        └────────────  syscall 26 (sys_disk_read)  ◀── manifest-driven reload ◀────────┘
```

- **VFS** (`desktop-metal/src/vfs.rs`) is the desktop's in-memory filesystem.
  Every user file write is mirrored to the on-disk **RPFS** via syscall 25.
- **RPFS** (`kernel/src/diskfs.rs`) is a flat named-file store: a superblock at
  LBA 8192, a 16-entry directory at 8193–8194, and append-only file data from
  8195. It rides on the from-scratch **AHCI/SATA** driver (`kernel/src/ahci.rs`).
- On boot, the VFS reads a small **manifest** (`.vfsmanifest`) listing every
  persisted file, then pulls each one back with syscall 26 — so the full working
  set reappears, not just settings.

Persistence is armed only *after* the built-in default files (readme, demo.psh,
…) are laid down, so those are never needlessly re-written to disk each boot —
only genuine user changes hit the platter.

## Using it

`launch.sh` creates `rusty-penguin-disk.img` (256 MiB, raw) on first run and
attaches it as a SATA/AHCI drive. The image is kept across runs; delete it to
start from a clean filesystem.

By hand:

```sh
qemu-img create -f raw rusty-penguin-disk.img 256M
qemu-system-x86_64 -machine q35 -cdrom rusty-penguin.iso -m 512M \
  -drive id=hd0,file=rusty-penguin-disk.img,format=raw,if=none \
  -device ich9-ahci,id=ahci -device ide-hd,drive=hd0,bus=ahci.0
```

On first boot you'll see `[diskfs] RPFS formatted (first boot)`; on every boot
after, `[diskfs] RPFS loaded`.

## Verified

Two-boot test (same disk image):

1. Boot 1 — `echo persistproof42 > probe.txt`, clean power-down.
2. Boot 2 — `cat probe.txt` → `persistproof42`, and `ls` lists `probe.txt`.

## Known limits

- RPFS allocation is **append-only**: overwriting a file does not reclaim its
  old sectors, and the directory holds **16 files**. Fine for normal use on a
  roomy disk; a compaction + larger-directory pass is a follow-up.
- The read syscall packs the output length into a 16-bit field, so a single
  file reloads up to **65535 bytes**.
