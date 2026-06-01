//! RPFS v2 — a real filesystem for Rusty Penguin.
//!
//! RPFS v1 was a demo: 16 files, one flat directory, append-only data with a
//! monotonic write pointer that leaked blocks forever (delete/overwrite never
//! reclaimed space). This is the real thing:
//!
//!   * a **block bitmap** — deleting or overwriting a file frees its blocks for
//!     reuse, so the disk no longer leaks,
//!   * **thousands of files** (8192-entry directory, vs 16),
//!   * **hierarchical directories** — '/'-separated paths with real directory
//!     entries you can mkdir and list, and parent dirs auto-created on write.
//!
//! The core is generic over a `BlockDev` so it can be hammered on the host
//! against a RAM disk (`tools/rpfs_test.rs`) — the same trick `bignum.rs` uses —
//! and then wired to the AHCI driver by `diskfs.rs`. No kernel deps, only `alloc`.
//!
//! On-disk layout (512-byte sectors), from a base LBA:
//!   base+0            superblock (self-describing: magic, geometry)
//!   bitmap_lba..      free-block bitmap (1 bit/data block, set = used)
//!   dir_lba..         directory: DIR_ENTRIES × 128-byte entries
//!   data_lba..        data blocks (contiguous-extent allocation)

#![allow(dead_code)]

use alloc::vec;
use alloc::vec::Vec;
use alloc::string::String;

pub const SECTOR: usize = 512;
pub const MAGIC: &[u8; 8] = b"RPFS2027";
pub const VERSION: u32 = 2;
pub const ENTRY_SZ: usize = 128;
// 2048 entries → 256 KiB directory. Sized to fit the kernel's small static heap
// (the user program loads at 4 MiB, capping kernel .bss); still 128× v1's 16.
pub const DIR_ENTRIES: u32 = 2048;
pub const MAX_NAME: usize = 111;   // path length that fits an entry
pub const MAX_FILE: usize = 1 << 20; // 1 MiB per file (2048 blocks)
// All block I/O goes through one scratch buffer allocated at mount, so file
// reads/writes never touch the heap again (the kernel allocator is bump-only).
const SCRATCH_SECTORS: usize = 64; // 32 KiB staging

/// A 512-byte block device. `read`/`write` operate on `count` sectors at `lba`.
pub trait BlockDev {
    fn read(&mut self, lba: u64, count: u16, buf: &mut [u8]) -> bool;
    fn write(&mut self, lba: u64, count: u16, buf: &[u8]) -> bool;
}

// Entry byte layout (128 bytes):
//   [0..111]   name / path (NUL-padded)
//   [111]      kind  (0 = file, 1 = directory)
//   [112]      trit  (i8: +1 active, 0 empty, -1 deleted)  — ternary slot state
//   [113..116] pad
//   [116..120] start_block (u32 LE, index into the data region)
//   [120..124] size        (u32 LE, bytes)
//   [124..128] nblocks      (u32 LE, allocated blocks — for reclamation)
const K_FILE: u8 = 0;
const K_DIR: u8 = 1;

pub struct Rpfs<D: BlockDev> {
    dev: D,
    base: u64,
    pub total_blocks: u32,
    bitmap_lba: u64,
    bitmap_sectors: u32,
    dir_lba: u64,
    data_lba: u64,
    bitmap: Vec<u8>,
    dir: Vec<u8>,
    scratch: Vec<u8>, // one-time I/O staging — keeps reads/writes off the heap
    ready: bool,
}

/// A directory listing item.
pub struct DirItem {
    pub name: String,
    pub is_dir: bool,
    pub size: u32,
}

fn rd_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn wr_u32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

/// Parent path of a '/'-separated key ("a/b/c" -> "a/b", "x" -> "").
fn parent_of(name: &[u8]) -> &[u8] {
    match name.iter().rposition(|&c| c == b'/') {
        Some(i) => &name[..i],
        None => &[],
    }
}

impl<D: BlockDev> Rpfs<D> {
    /// Compute the geometry for a disk region of `disk_sectors` total sectors
    /// starting at `base`. Bitmap and directory are sized to fit; the rest is data.
    fn geometry(base: u64, disk_sectors: u64) -> (u32, u32) {
        let dir_sectors = (DIR_ENTRIES as u64 * ENTRY_SZ as u64 / SECTOR as u64) as u32; // 2048
        // available after super(1) + dir; split remaining between bitmap + data.
        let avail = disk_sectors.saturating_sub(base + 1 + dir_sectors as u64);
        // Each bitmap sector covers SECTOR*8 = 4096 data blocks. Solve for the
        // largest data region whose bitmap also fits in `avail`.
        // avail = bitmap_sectors + data_blocks, bitmap_sectors = ceil(data/4096).
        let per = (SECTOR * 8) as u64; // 4096
        let mut data_blocks = (avail * per) / (per + 1);
        let mut bitmap_sectors = (data_blocks + per - 1) / per;
        // tighten in case of rounding overshoot
        while bitmap_sectors + data_blocks > avail && data_blocks > 0 {
            data_blocks -= 1;
            bitmap_sectors = (data_blocks + per - 1) / per;
        }
        (data_blocks as u32, bitmap_sectors.max(1) as u32)
    }

    /// Mount an existing RPFS, or format a fresh one if no valid superblock is
    /// present. `disk_sectors` is the total size of the region (used only when
    /// formatting; a mount reads the geometry back from the superblock).
    pub fn open(mut dev: D, base: u64, disk_sectors: u64) -> Self {
        let mut sb = [0u8; SECTOR];
        let have_sb = dev.read(base, 1, &mut sb) && &sb[..8] == MAGIC && rd_u32(&sb, 8) == VERSION;
        if have_sb {
            let total_blocks = rd_u32(&sb, 12);
            let bitmap_sectors = rd_u32(&sb, 16);
            let bitmap_lba = base + 1;
            let dir_lba = bitmap_lba + bitmap_sectors as u64;
            let dir_sectors = DIR_ENTRIES * ENTRY_SZ as u32 / SECTOR as u32;
            let data_lba = dir_lba + dir_sectors as u64;
            let mut fs = Rpfs {
                dev, base, total_blocks, bitmap_lba, bitmap_sectors, dir_lba, data_lba,
                bitmap: vec![0u8; bitmap_sectors as usize * SECTOR],
                dir: vec![0u8; (DIR_ENTRIES as usize) * ENTRY_SZ],
                scratch: vec![0u8; SCRATCH_SECTORS * SECTOR],
                ready: false,
            };
            fs.dev.read(bitmap_lba, bitmap_sectors as u16, &mut fs.bitmap);
            // directory can exceed a single multi-sector read cap; chunk it.
            fs.load_dir();
            fs.ready = true;
            fs
        } else {
            let (total_blocks, bitmap_sectors) = Self::geometry(base, disk_sectors);
            let bitmap_lba = base + 1;
            let dir_lba = bitmap_lba + bitmap_sectors as u64;
            let dir_sectors = DIR_ENTRIES * ENTRY_SZ as u32 / SECTOR as u32;
            let data_lba = dir_lba + dir_sectors as u64;
            let mut fs = Rpfs {
                dev, base, total_blocks, bitmap_lba, bitmap_sectors, dir_lba, data_lba,
                bitmap: vec![0u8; bitmap_sectors as usize * SECTOR],
                dir: vec![0u8; (DIR_ENTRIES as usize) * ENTRY_SZ],
                scratch: vec![0u8; SCRATCH_SECTORS * SECTOR],
                ready: false,
            };
            fs.format();
            fs.ready = true;
            fs
        }
    }

    fn dir_sectors(&self) -> u32 {
        DIR_ENTRIES * ENTRY_SZ as u32 / SECTOR as u32
    }

    fn format(&mut self) {
        for b in self.bitmap.iter_mut() {
            *b = 0;
        }
        for b in self.dir.iter_mut() {
            *b = 0;
        }
        self.flush_super();
        self.flush_bitmap_all();
        self.flush_dir_all();
    }

    fn flush_super(&mut self) {
        let mut sb = [0u8; SECTOR];
        sb[..8].copy_from_slice(MAGIC);
        wr_u32(&mut sb, 8, VERSION);
        wr_u32(&mut sb, 12, self.total_blocks);
        wr_u32(&mut sb, 16, self.bitmap_sectors);
        wr_u32(&mut sb, 20, DIR_ENTRIES);
        self.dev.write(self.base, 1, &sb);
    }

    // Some block backends cap a single transfer; write/read the directory and
    // bitmap in modest chunks to stay within those limits.
    fn flush_dir_all(&mut self) {
        let total = self.dir_sectors();
        let mut done = 0u32;
        while done < total {
            let n = (total - done).min(64);
            let off = done as usize * SECTOR;
            let slice = &self.dir[off..off + n as usize * SECTOR];
            self.dev.write(self.dir_lba + done as u64, n as u16, slice);
            done += n;
        }
    }
    fn load_dir(&mut self) {
        let total = self.dir_sectors();
        let mut done = 0u32;
        while done < total {
            let n = (total - done).min(64);
            let off = done as usize * SECTOR;
            let lba = self.dir_lba + done as u64;
            // Read straight into the in-RAM directory (disjoint field borrow).
            self.dev.read(lba, n as u16, &mut self.dir[off..off + n as usize * SECTOR]);
            done += n;
        }
    }
    fn flush_bitmap_all(&mut self) {
        let total = self.bitmap_sectors;
        let mut done = 0u32;
        while done < total {
            let n = (total - done).min(64);
            let off = done as usize * SECTOR;
            let slice = &self.bitmap[off..off + n as usize * SECTOR];
            self.dev.write(self.bitmap_lba + done as u64, n as u16, slice);
            done += n;
        }
    }
    /// Flush only the single directory sector holding entry `idx`.
    fn flush_dir_entry(&mut self, idx: usize) {
        let sector = idx * ENTRY_SZ / SECTOR;
        let off = sector * SECTOR;
        let mut buf = [0u8; SECTOR];
        buf.copy_from_slice(&self.dir[off..off + SECTOR]);
        self.dev.write(self.dir_lba + sector as u64, 1, &buf);
    }
    /// Flush the bitmap sectors covering data blocks [start, start+n).
    fn flush_bitmap_range(&mut self, start: u32, n: u32) {
        let first = (start / 8 / SECTOR as u32) as u64;
        let last = ((start + n).saturating_sub(1) / 8 / SECTOR as u32) as u64;
        for s in first..=last {
            let off = s as usize * SECTOR;
            if off + SECTOR > self.bitmap.len() {
                break;
            }
            let mut buf = [0u8; SECTOR];
            buf.copy_from_slice(&self.bitmap[off..off + SECTOR]);
            self.dev.write(self.bitmap_lba + s, 1, &buf);
        }
    }

    // ---- bitmap ops ----
    fn bit_get(&self, b: u32) -> bool {
        (self.bitmap[(b / 8) as usize] >> (b % 8)) & 1 != 0
    }
    fn bit_set(&mut self, b: u32, v: bool) {
        let byte = (b / 8) as usize;
        let mask = 1u8 << (b % 8);
        if v {
            self.bitmap[byte] |= mask;
        } else {
            self.bitmap[byte] &= !mask;
        }
    }
    /// First-fit contiguous run of `n` free blocks; marks them used. None if full.
    fn alloc_run(&mut self, n: u32) -> Option<u32> {
        if n == 0 {
            return None;
        }
        let mut start = 0u32;
        let mut run = 0u32;
        for b in 0..self.total_blocks {
            if !self.bit_get(b) {
                if run == 0 {
                    start = b;
                }
                run += 1;
                if run == n {
                    for k in start..start + n {
                        self.bit_set(k, true);
                    }
                    return Some(start);
                }
            } else {
                run = 0;
            }
        }
        None
    }
    fn free_run(&mut self, start: u32, n: u32) {
        for k in start..start + n {
            if k < self.total_blocks {
                self.bit_set(k, false);
            }
        }
    }
    /// Count of free data blocks (for reclamation tests / df).
    pub fn free_blocks(&self) -> u32 {
        let mut c = 0;
        for b in 0..self.total_blocks {
            if !self.bit_get(b) {
                c += 1;
            }
        }
        c
    }

    // ---- entry helpers ----
    fn ent(&self, i: usize) -> &[u8] {
        &self.dir[i * ENTRY_SZ..(i + 1) * ENTRY_SZ]
    }
    fn e_trit(&self, i: usize) -> i8 {
        self.dir[i * ENTRY_SZ + 112] as i8
    }
    fn e_kind(&self, i: usize) -> u8 {
        self.dir[i * ENTRY_SZ + 111]
    }
    fn e_name(&self, i: usize) -> &[u8] {
        let e = self.ent(i);
        let end = e[..MAX_NAME].iter().position(|&b| b == 0).unwrap_or(MAX_NAME);
        &e[..end]
    }
    fn e_start(&self, i: usize) -> u32 {
        rd_u32(self.ent(i), 116)
    }
    fn e_size(&self, i: usize) -> u32 {
        rd_u32(self.ent(i), 120)
    }
    fn e_nblocks(&self, i: usize) -> u32 {
        rd_u32(self.ent(i), 124)
    }
    fn set_entry(&mut self, i: usize, name: &[u8], kind: u8, trit: i8, start: u32, size: u32, nblocks: u32) {
        let off = i * ENTRY_SZ;
        for b in &mut self.dir[off..off + ENTRY_SZ] {
            *b = 0;
        }
        let n = name.len().min(MAX_NAME);
        self.dir[off..off + n].copy_from_slice(&name[..n]);
        self.dir[off + 111] = kind;
        self.dir[off + 112] = trit as u8;
        wr_u32(&mut self.dir[off..], 116, start);
        wr_u32(&mut self.dir[off..], 120, size);
        wr_u32(&mut self.dir[off..], 124, nblocks);
    }

    /// Find an active entry (file or dir) by exact name; returns its index.
    fn find(&self, name: &[u8]) -> Option<usize> {
        for i in 0..DIR_ENTRIES as usize {
            if self.e_trit(i) == 1 && self.e_name(i) == name {
                return Some(i);
            }
        }
        None
    }
    /// First free (empty or tombstoned) slot.
    fn free_slot(&self) -> Option<usize> {
        for i in 0..DIR_ENTRIES as usize {
            if self.e_trit(i) <= 0 {
                return Some(i);
            }
        }
        None
    }

    /// Ensure every parent directory along `name` exists (creates dir entries).
    fn ensure_parents(&mut self, name: &[u8]) {
        let mut idx = 0;
        while let Some(pos) = name[idx..].iter().position(|&c| c == b'/') {
            let cut = idx + pos;
            if cut > 0 {
                let dirpath = &name[..cut];
                if self.find(dirpath).is_none() {
                    if let Some(slot) = self.free_slot() {
                        self.set_entry(slot, dirpath, K_DIR, 1, 0, 0, 0);
                        self.flush_dir_entry(slot);
                    }
                }
            }
            idx = cut + 1;
        }
    }

    /// Create or overwrite a file. Reclaims the old extent on overwrite.
    pub fn write_file(&mut self, name: &[u8], data: &[u8]) -> bool {
        if !self.ready || name.is_empty() || name.len() > MAX_NAME || data.len() > MAX_FILE {
            return false;
        }
        let nblocks = ((data.len() + SECTOR - 1) / SECTOR).max(1) as u32;

        // Overwrite: free the old extent first so its blocks can be reused.
        let slot = if let Some(i) = self.find(name) {
            if self.e_kind(i) == K_DIR {
                return false; // can't write over a directory
            }
            let (os, on) = (self.e_start(i), self.e_nblocks(i));
            self.free_run(os, on);
            self.flush_bitmap_range(os, on);
            i
        } else {
            match self.free_slot() {
                Some(i) => i,
                None => return false, // directory full
            }
        };

        let start = match self.alloc_run(nblocks) {
            Some(s) => s,
            None => {
                // out of space — if this was an overwrite we already freed the
                // old extent; the file becomes empty rather than corrupt.
                self.set_entry(slot, name, K_FILE, -1, 0, 0, 0);
                self.flush_dir_entry(slot);
                return false;
            }
        };

        // Write the data, zero-padded to whole sectors, staging each chunk
        // through the fixed scratch buffer (no per-write heap allocation).
        let mut blk = 0u32;
        let mut written = 0usize;
        while blk < nblocks {
            let cnt = (nblocks - blk).min(SCRATCH_SECTORS as u32);
            let cbytes = cnt as usize * SECTOR;
            let copy = (data.len() - written).min(cbytes);
            self.scratch[..copy].copy_from_slice(&data[written..written + copy]);
            for b in &mut self.scratch[copy..cbytes] {
                *b = 0; // zero-pad the tail of the final chunk
            }
            let lba = self.data_lba + (start + blk) as u64;
            if !self.dev.write(lba, cnt as u16, &self.scratch[..cbytes]) {
                return false;
            }
            written += copy;
            blk += cnt;
        }

        self.ensure_parents(name);
        self.set_entry(slot, name, K_FILE, 1, start, data.len() as u32, nblocks);
        self.flush_bitmap_range(start, nblocks);
        self.flush_dir_entry(slot);
        true
    }

    /// Read a file into `out`. Returns bytes read, or None if not found.
    pub fn read_file(&mut self, name: &[u8], out: &mut [u8]) -> Option<usize> {
        if !self.ready {
            return None;
        }
        let i = self.find(name)?;
        if self.e_kind(i) == K_DIR {
            return None;
        }
        let start = self.e_start(i);
        let size = self.e_size(i) as usize;
        let nblocks = self.e_nblocks(i);
        // Read chunk by chunk through the scratch buffer, copying the live bytes
        // into `out` — no full-file heap allocation.
        let mut blk = 0u32;
        let mut file_pos = 0usize; // bytes of the file consumed so far
        let mut out_pos = 0usize;  // bytes delivered into `out`
        while blk < nblocks {
            let cnt = (nblocks - blk).min(SCRATCH_SECTORS as u32);
            let cbytes = cnt as usize * SECTOR;
            let lba = self.data_lba + (start + blk) as u64;
            if !self.dev.read(lba, cnt as u16, &mut self.scratch[..cbytes]) {
                return None;
            }
            // valid file bytes within this chunk, clipped to the out buffer
            let valid = size.saturating_sub(file_pos).min(cbytes);
            let n = valid.min(out.len().saturating_sub(out_pos));
            out[out_pos..out_pos + n].copy_from_slice(&self.scratch[..n]);
            out_pos += n;
            file_pos += cbytes;
            blk += cnt;
            if out_pos >= out.len() || file_pos >= size {
                break;
            }
        }
        Some(out_pos)
    }

    /// Delete a file or empty directory; frees its blocks. Returns true if removed.
    pub fn delete_file(&mut self, name: &[u8]) -> bool {
        if !self.ready {
            return false;
        }
        let i = match self.find(name) {
            Some(i) => i,
            None => return false,
        };
        if self.e_kind(i) == K_DIR {
            // refuse to delete a non-empty directory (any child path "name/...")
            for j in 0..DIR_ENTRIES as usize {
                if self.e_trit(j) == 1 {
                    let cn = self.e_name(j);
                    if cn.len() > name.len() && cn.starts_with(name) && cn[name.len()] == b'/' {
                        return false;
                    }
                }
            }
        } else {
            let (s, n) = (self.e_start(i), self.e_nblocks(i));
            self.free_run(s, n);
            self.flush_bitmap_range(s, n);
        }
        self.dir[i * ENTRY_SZ + 112] = (-1i8) as u8; // tombstone
        self.flush_dir_entry(i);
        true
    }

    /// Make a directory (and any missing parents). Idempotent.
    pub fn mkdir(&mut self, name: &[u8]) -> bool {
        if !self.ready || name.is_empty() || name.len() > MAX_NAME {
            return false;
        }
        self.ensure_parents(name);
        if self.find(name).is_some() {
            return true;
        }
        match self.free_slot() {
            Some(slot) => {
                self.set_entry(slot, name, K_DIR, 1, 0, 0, 0);
                self.flush_dir_entry(slot);
                true
            }
            None => false,
        }
    }

    /// List the immediate children of directory `path` ("" = root).
    pub fn list_dir(&self, path: &[u8]) -> Vec<DirItem> {
        let mut out = Vec::new();
        for i in 0..DIR_ENTRIES as usize {
            if self.e_trit(i) != 1 {
                continue;
            }
            let name = self.e_name(i);
            if parent_of(name) == path {
                let child = match name.iter().rposition(|&c| c == b'/') {
                    Some(p) => &name[p + 1..],
                    None => name,
                };
                out.push(DirItem {
                    name: String::from_utf8_lossy(child).into_owned(),
                    is_dir: self.e_kind(i) == K_DIR,
                    size: self.e_size(i),
                });
            }
        }
        out
    }

    /// Number of active entries (files + dirs).
    pub fn entry_count(&self) -> u32 {
        let mut c = 0;
        for i in 0..DIR_ENTRIES as usize {
            if self.e_trit(i) == 1 {
                c += 1;
            }
        }
        c
    }

    pub fn is_ready(&self) -> bool {
        self.ready
    }

    /// Recover the underlying device (e.g. to remount). Consumes the FS.
    pub fn into_dev(self) -> D {
        self.dev
    }
}
