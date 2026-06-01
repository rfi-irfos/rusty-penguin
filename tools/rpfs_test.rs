// Host test for RPFS v2. Pulls in the REAL kernel/src/rpfs.rs via #[path] and
// drives it over a RAM-backed BlockDev, asserting the things v1 could NOT do:
//   * thousands of files across nested directories,
//   * block reclamation — deleting/overwriting frees blocks, no monotonic leak,
//   * directory listing returns the right immediate children,
//   * everything survives a remount (re-open the same RAM disk),
//   * file contents round-trip exactly.
//
//   rustc -O tools/rpfs_test.rs -o /tmp/rpfstest && /tmp/rpfstest
extern crate alloc;

#[path = "../kernel/src/rpfs.rs"]
mod rpfs;

use rpfs::{BlockDev, Rpfs};

const SECTOR: usize = 512;

// A simple RAM disk.
struct RamDisk {
    data: Vec<u8>,
}
impl RamDisk {
    fn new(sectors: usize) -> Self {
        RamDisk { data: vec![0u8; sectors * SECTOR] }
    }
}
impl BlockDev for RamDisk {
    fn read(&mut self, lba: u64, count: u16, buf: &mut [u8]) -> bool {
        let off = lba as usize * SECTOR;
        let n = count as usize * SECTOR;
        if off + n > self.data.len() || buf.len() < n {
            return false;
        }
        buf[..n].copy_from_slice(&self.data[off..off + n]);
        true
    }
    fn write(&mut self, lba: u64, count: u16, buf: &[u8]) -> bool {
        let off = lba as usize * SECTOR;
        let n = count as usize * SECTOR;
        if off + n > self.data.len() || buf.len() < n {
            return false;
        }
        self.data[off..off + n].copy_from_slice(&buf[..n]);
        true
    }
}

fn main() {
    let base = 8192u64;
    // 64 MiB region (131072 sectors) — enough for thousands of files, fast on host.
    let disk_sectors = base + 131072;
    let disk = RamDisk::new(disk_sectors as usize);

    // ---- format + basic geometry sanity ----
    let mut fs = Rpfs::open(disk, base, disk_sectors);
    assert!(fs.is_ready());
    assert!(fs.total_blocks > 50_000, "data region too small: {}", fs.total_blocks);
    let free0 = fs.free_blocks();
    assert_eq!(free0, fs.total_blocks, "fresh fs should be all-free");
    println!("formatted: {} data blocks, {} free", fs.total_blocks, free0);

    // ---- thousands of files across nested directories (within DIR_ENTRIES) ----
    const N: usize = 1800;
    for i in 0..N {
        let path = format!("home/user/dir{}/file{}.txt", i % 50, i);
        let body = format!("contents of file {} :: {}", i, "x".repeat(i % 300));
        assert!(fs.write_file(path.as_bytes(), body.as_bytes()), "write {} failed", path);
    }
    println!("wrote {} files; active entries = {}", N, fs.entry_count());
    assert!(fs.entry_count() >= N as u32, "lost entries");

    // round-trip a sample
    let mut buf = vec![0u8; 4096];
    let probe = "home/user/dir7/file1207.txt";
    let want = format!("contents of file {} :: {}", 1207, "x".repeat(1207 % 300));
    let n = fs.read_file(probe.as_bytes(), &mut buf).expect("read probe");
    assert_eq!(&buf[..n], want.as_bytes(), "content mismatch");

    // ---- directory listing ----
    let kids = fs.list_dir(b"home/user");
    let dirs: Vec<_> = kids.iter().filter(|k| k.is_dir).collect();
    assert_eq!(dirs.len(), 50, "expected 50 subdirs under home/user, got {}", dirs.len());
    let d7 = fs.list_dir(b"home/user/dir7");
    assert!(d7.iter().all(|k| !k.is_dir), "dir7 should hold only files");
    println!("list_dir(home/user) -> {} entries ({} dirs)", kids.len(), dirs.len());

    // ---- RECLAMATION: the v1-killer test ----
    let free_full = fs.free_blocks();
    // delete every other file; their blocks must come back.
    let mut deleted = 0;
    for i in (0..N).step_by(2) {
        let path = format!("home/user/dir{}/file{}.txt", i % 50, i);
        assert!(fs.delete_file(path.as_bytes()), "delete {} failed", path);
        deleted += 1;
    }
    let free_after_del = fs.free_blocks();
    assert!(free_after_del > free_full, "deleting freed no blocks (v1 leak!)");
    println!("deleted {} files; free blocks {} -> {}", deleted, free_full, free_after_del);

    // re-write the same number of files; allocator must REUSE freed blocks, so
    // we must NOT run out (v1 would have leaked and eventually failed).
    for i in (0..N).step_by(2) {
        let path = format!("reuse/blk{}.dat", i);
        let body = format!("reused payload {} {}", i, "y".repeat(i % 250));
        assert!(fs.write_file(path.as_bytes(), body.as_bytes()), "reuse write {} failed", path);
    }
    println!("re-wrote {} files into reclaimed space; free now {}", deleted, fs.free_blocks());

    // overwrite-in-place must not leak: write a file big, then small, then check free grew
    fs.write_file(b"ow.bin", &vec![1u8; 200 * 1024]).then_some(()).expect("big write");
    let free_big = fs.free_blocks();
    fs.write_file(b"ow.bin", &vec![2u8; 1024]).then_some(()).expect("small overwrite");
    let free_small = fs.free_blocks();
    assert!(free_small > free_big, "overwrite shrink didn't reclaim blocks");
    println!("overwrite 200K->1K reclaimed {} blocks", free_small - free_big);

    // ---- persistence across remount ----
    let blocks_used_before = fs.total_blocks - fs.free_blocks();
    let entries_before = fs.entry_count();
    let disk_back = fs.into_dev();
    let mut fs2 = Rpfs::open(disk_back, base, disk_sectors);
    assert!(fs2.is_ready());
    assert_eq!(fs2.entry_count(), entries_before, "entries changed across remount");
    assert_eq!(fs2.total_blocks - fs2.free_blocks(), blocks_used_before, "bitmap not persisted");
    // a surviving (odd-index) file must still read back
    let survive = "home/user/dir9/file1209.txt";
    let want2 = format!("contents of file {} :: {}", 1209, "x".repeat(1209 % 300));
    let n2 = fs2.read_file(survive.as_bytes(), &mut buf).expect("survivor read after remount");
    assert_eq!(&buf[..n2], want2.as_bytes(), "survivor content lost across remount");
    println!("remount OK: {} entries, {} blocks used preserved", fs2.entry_count(), blocks_used_before);

    println!("ALL CHECKS PASSED");
}
