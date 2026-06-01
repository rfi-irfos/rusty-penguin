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
- **RPFS v2** (`kernel/src/rpfs.rs` core + `kernel/src/diskfs.rs` AHCI adapter)
  is a real filesystem: a self-describing superblock at LBA 8192, a **free-block
  bitmap**, a **2048-entry directory**, and a data region with first-fit
  contiguous-extent allocation. Deleting or overwriting a file **reclaims** its
  blocks (v1 leaked them forever). Paths are `/`-separated with real directory
  entries (`mkdir`, `list_dir`); parent dirs are auto-created on write. It rides
  on the from-scratch **AHCI/SATA** driver (`kernel/src/ahci.rs`). The core is
  generic over a `BlockDev` trait and host-tested in `tools/rpfs_test.rs`.
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

On every boot you'll see e.g. `[diskfs] RPFS v2 ready: N files, F/T blocks free
(real dirs + reclamation)`.

## Verified

- **Host test** (`tools/rpfs_test.rs`, over a RAM disk): 1800 files across 50
  nested directories, `list_dir` correctness, **block reclamation** (delete 900
  files → blocks freed → rewrite 900 reuses the freed blocks, no leak),
  overwrite-shrink reclaim, and full persistence across a remount.
- **On hardware** (QEMU AHCI, `fstest` boot flag, two boots on one disk image):
  boot 1 formats, reclaims on overwrite, lists a nested directory; boot 2 reads
  the persisted marker back (`rpfs-v2-persist-ok`) with its files intact.

## Known limits

- The directory holds **2048 entries** and file allocation uses **contiguous
  extents**, so a very fragmented disk could fail a large allocation even with
  enough total free space. Block lists / extents-with-holes are a follow-up.
- The read syscall packs the output length into a 16-bit field, so a single
  file reloads up to **65535 bytes** through that path.
- A v1 disk (magic `RPFS2026`) is reformatted to v2 (`RPFS2027`) on first boot.
