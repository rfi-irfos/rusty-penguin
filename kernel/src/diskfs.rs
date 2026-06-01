// RPFS — Rusty Penguin FileSystem, persistent storage on the AHCI/SATA disk.
//
// This is the thin kernel adapter: it binds the real filesystem core in
// `rpfs.rs` (block-bitmap reclamation, thousands of files, hierarchical
// directories — host-tested in tools/rpfs_test.rs) to the AHCI block driver,
// and keeps the small public API the rest of the kernel already calls
// (write_file / read_file / is_ready, plus delete/mkdir/list for callers that
// want them). RPFS v1 — the flat 16-file append-only demo — is gone.
//
// The filesystem lives in the disk region starting at LBA 8192. The geometry is
// self-describing in the superblock, so a v1 disk (magic "RPFS2026") simply
// fails the v2 magic check and gets reformatted on first boot.

use crate::rpfs::{BlockDev, DirItem, Rpfs};
use alloc::vec::Vec;

const FS_BASE: u64 = 8192;
// The dev disk (launch.sh / installer) is 256 MiB. AHCI exposes no capacity
// query, so we size the FS for that; a larger disk just leaves a tail unused.
const DISK_SECTORS: u64 = 256 * 1024 * 1024 / 512; // 524288

/// BlockDev over the AHCI driver.
struct AhciDev;
impl BlockDev for AhciDev {
    fn read(&mut self, lba: u64, count: u16, buf: &mut [u8]) -> bool {
        crate::ahci::read_sectors(lba, count, buf)
    }
    fn write(&mut self, lba: u64, count: u16, buf: &[u8]) -> bool {
        crate::ahci::write_sectors(lba, count, buf)
    }
}

static mut FS: Option<Rpfs<AhciDev>> = None;

pub fn init() {
    if !crate::ahci::is_ready() {
        crate::serial::write_str("  [diskfs] AHCI not ready — RPFS skipped\n");
        return;
    }
    let fs = Rpfs::open(AhciDev, FS_BASE, DISK_SECTORS);
    let entries = fs.entry_count();
    let free = fs.free_blocks();
    let total = fs.total_blocks;
    unsafe { FS = Some(fs); }
    crate::serial::write_str("  [diskfs] RPFS v2 ready: ");
    log_dec(entries as u64);
    crate::serial::write_str(" files, ");
    log_dec(free as u64);
    crate::serial::write_str("/");
    log_dec(total as u64);
    crate::serial::write_str(" blocks free (real dirs + reclamation)\n");
}

#[allow(static_mut_refs)]
fn fs() -> Option<&'static mut Rpfs<AhciDev>> {
    unsafe { FS.as_mut() }
}

/// Write (or overwrite) a file at `name` (a '/'-separated path). Overwrites
/// reclaim the old blocks. Returns true on success.
pub fn write_file(name: &[u8], data: &[u8]) -> bool {
    match fs() {
        Some(f) => f.write_file(name, data),
        None => false,
    }
}

/// Read a file into `out`. Returns Some(bytes_read) if found.
pub fn read_file(name: &[u8], out: &mut [u8]) -> Option<usize> {
    match fs() {
        Some(f) => f.read_file(name, out),
        None => None,
    }
}

/// Delete a file (or empty directory), freeing its blocks.
pub fn delete_file(name: &[u8]) -> bool {
    match fs() {
        Some(f) => f.delete_file(name),
        None => false,
    }
}

/// Create a directory (and any missing parents).
pub fn mkdir(name: &[u8]) -> bool {
    match fs() {
        Some(f) => f.mkdir(name),
        None => false,
    }
}

/// List the immediate children of directory `path` ("" = root).
pub fn list_dir(path: &[u8]) -> Vec<DirItem> {
    match fs() {
        Some(f) => f.list_dir(path),
        None => Vec::new(),
    }
}

pub fn is_ready() -> bool {
    fs().map(|f| f.is_ready()).unwrap_or(false)
}

/// On-hardware self-test (gated by the `fstest` cmdline flag). Proves the
/// AHCI-backed FS does what the host test proves over RAM: persistence across a
/// real reboot, a write/read round-trip, block reclamation on overwrite, and
/// directory listing. Run it twice (same disk) to see the persistence line flip.
pub fn selftest() {
    let f = match fs() {
        Some(f) => f,
        None => {
            crate::serial::write_str("  [fstest] RPFS not ready\n");
            return;
        }
    };

    // 1. Persistence: is last boot's marker still on disk?
    let mut buf = [0u8; 64];
    match f.read_file(b"sys/rpfs-marker.txt", &mut buf) {
        Some(n) => {
            crate::serial::write_str("  [fstest] marker PERSISTED across reboot: ");
            crate::serial::write_str(core::str::from_utf8(&buf[..n]).unwrap_or("?"));
            crate::serial::write_str("\n");
        }
        None => crate::serial::write_str("  [fstest] no marker yet (first boot on this disk)\n"),
    }
    f.write_file(b"sys/rpfs-marker.txt", b"rpfs-v2-persist-ok");

    // 2. Reclamation: write big, overwrite small, free blocks must grow.
    // Heap, NOT stack — the boot stack is only 64 KiB; a 200 KiB stack array
    // here underflows it into the VGA/BIOS hole and triple-faults at boot.
    let big = alloc::vec![0xABu8; 200 * 1024];
    f.write_file(b"sys/probe.bin", &big);
    let free_big = f.free_blocks();
    f.write_file(b"sys/probe.bin", b"small");
    let free_small = f.free_blocks();
    crate::serial::write_str("  [fstest] overwrite 200K->5B free blocks ");
    log_dec(free_big as u64);
    crate::serial::write_str(" -> ");
    log_dec(free_small as u64);
    if free_small > free_big {
        crate::serial::write_str("  (RECLAIMED)\n");
    } else {
        crate::serial::write_str("  (NO RECLAIM!)\n");
    }

    // 3. Round-trip the small content.
    let mut rb = [0u8; 16];
    match f.read_file(b"sys/probe.bin", &mut rb) {
        Some(n) if &rb[..n] == b"small" => crate::serial::write_str("  [fstest] read-back OK\n"),
        _ => crate::serial::write_str("  [fstest] read-back FAILED\n"),
    }
    f.delete_file(b"sys/probe.bin");

    // 4. Directory hierarchy: write into a nested path, list the parent.
    f.write_file(b"docs/notes/a.txt", b"alpha");
    f.write_file(b"docs/notes/b.txt", b"beta");
    let kids = f.list_dir(b"docs/notes");
    crate::serial::write_str("  [fstest] list_dir(docs/notes) -> ");
    log_dec(kids.len() as u64);
    crate::serial::write_str(" entries; docs has ");
    log_dec(f.list_dir(b"docs").len() as u64);
    crate::serial::write_str(" child(ren)\n");
}

fn log_dec(mut v: u64) {
    if v == 0 {
        crate::serial::write_byte(b'0');
        return;
    }
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    while v > 0 {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    for &b in &buf[i..] {
        crate::serial::write_byte(b);
    }
}
