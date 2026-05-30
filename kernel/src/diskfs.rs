// RPFS — Rusty Penguin FileSystem.
// Simple persistent named-file store on top of the AHCI driver.
// Files and settings written here survive a power cycle.
//
// Disk layout (512-byte sectors):
//   LBA 8192        Superblock: magic[8] + data_next_lba[8] + zeros
//   LBA 8193-8194   Directory:  16 entries × 64 bytes  (1024 bytes)
//   LBA 8195+       File data (append-only; old entries tombstoned on overwrite)
//
// Ternary Trit state per directory slot:
//    1i8  (+1, Pos)  — active file
//    0i8  ( 0, Zero) — empty slot
//   -1i8  (-1, Neg)  — deleted / tombstone

const SUPER_LBA:   u64 = 8192;
const DIR_LBA:     u64 = 8193; // occupies 2 sectors
const DATA_START:  u64 = 8195;
const MAGIC:       &[u8; 8] = b"RPFS2026";
const MAX_FILES:   usize = 16;
const ENTRY_SZ:    usize = 64;

// Directory entry byte layout (64 bytes):
//   [0..52]  name  (null-padded, max 51 chars)
//   [52]     trit  (i8: +1/0/-1)
//   [53..56] _pad
//   [56..60] data_lba  (u32 LE)
//   [60..64] size      (u32 LE, bytes)

static mut DIR:          [[u8; ENTRY_SZ]; MAX_FILES] = [[0u8; ENTRY_SZ]; MAX_FILES];
static mut DATA_NEXT_LBA: u64 = DATA_START;
static mut READY:         bool = false;

#[inline] fn e_trit(e: &[u8; ENTRY_SZ]) -> i8 { e[52] as i8 }
#[inline] fn e_name(e: &[u8; ENTRY_SZ]) -> &[u8] {
    let end = e[..52].iter().position(|&b| b == 0).unwrap_or(52);
    &e[..end]
}
#[inline] fn e_lba(e: &[u8; ENTRY_SZ]) -> u64 {
    u32::from_le_bytes([e[56], e[57], e[58], e[59]]) as u64
}
#[inline] fn e_size(e: &[u8; ENTRY_SZ]) -> usize {
    u32::from_le_bytes([e[60], e[61], e[62], e[63]]) as usize
}
fn set_entry(e: &mut [u8; ENTRY_SZ], name: &[u8], trit: i8, lba: u64, size: usize) {
    e.fill(0);
    let n = name.len().min(51);
    e[..n].copy_from_slice(&name[..n]);
    e[52] = trit as u8;
    e[56..60].copy_from_slice(&(lba as u32).to_le_bytes());
    e[60..64].copy_from_slice(&(size as u32).to_le_bytes());
}

fn flush_super() {
    let mut buf = [0u8; 512];
    buf[..8].copy_from_slice(MAGIC);
    let lba = unsafe { DATA_NEXT_LBA };
    buf[8..16].copy_from_slice(&lba.to_le_bytes());
    unsafe { crate::ahci::write_sectors(SUPER_LBA, 1, &buf); }
}

fn flush_dir() {
    let mut buf = [0u8; 1024];
    unsafe {
        for (i, e) in DIR.iter().enumerate() {
            buf[i * ENTRY_SZ..(i + 1) * ENTRY_SZ].copy_from_slice(e);
        }
    }
    crate::ahci::write_sectors(DIR_LBA, 2, &buf);
}

pub fn init() {
    if !crate::ahci::is_ready() {
        crate::serial::write_str("  [diskfs] AHCI not ready — RPFS skipped\n");
        return;
    }
    let mut sbuf = [0u8; 512];
    if !crate::ahci::read_sectors(SUPER_LBA, 1, &mut sbuf) {
        crate::serial::write_str("  [diskfs] superblock read failed\n");
        return;
    }
    if &sbuf[..8] == MAGIC {
        // Load existing filesystem.
        let lba = u64::from_le_bytes(sbuf[8..16].try_into().unwrap_or([0u8; 8]));
        unsafe { DATA_NEXT_LBA = if lba >= DATA_START { lba } else { DATA_START }; }
        let mut dbuf = [0u8; 1024];
        if crate::ahci::read_sectors(DIR_LBA, 2, &mut dbuf) {
            unsafe {
                for i in 0..MAX_FILES {
                    DIR[i].copy_from_slice(&dbuf[i * ENTRY_SZ..(i + 1) * ENTRY_SZ]);
                }
            }
        }
        crate::serial::write_str("  [diskfs] RPFS loaded\n");
    } else {
        // First boot — format the filesystem.
        unsafe { DATA_NEXT_LBA = DATA_START; }
        flush_super();
        let empty = [0u8; 1024];
        crate::ahci::write_sectors(DIR_LBA, 2, &empty);
        crate::serial::write_str("  [diskfs] RPFS formatted (first boot)\n");
    }
    unsafe { READY = true; }
}

/// Write (or overwrite) a named file. Returns true on success.
/// Max name length: 51 bytes. Max file size: 65536 bytes (128 sectors).
pub fn write_file(name: &[u8], data: &[u8]) -> bool {
    unsafe {
        if !READY || name.is_empty() || name.len() > 51 || data.len() > 65536 {
            return false;
        }
        // Tombstone any existing active entry with the same name.
        for e in DIR.iter_mut() {
            if e_trit(e) == 1 && e_name(e) == name {
                e[52] = (-1i8) as u8;  // Trit::Neg
            }
        }
        // Find the first empty or deleted slot.
        let slot = match DIR.iter_mut().position(|e| e_trit(e) <= 0) {
            Some(i) => i,
            None => { crate::serial::write_str("  [diskfs] directory full\n"); return false; }
        };
        // Pad data to whole sectors and write.
        let sectors = ((data.len() + 511) / 512).max(1) as u16;
        let byte_count = sectors as usize * 512;
        let mut padded = alloc::vec![0u8; byte_count];
        padded[..data.len()].copy_from_slice(data);
        let lba = DATA_NEXT_LBA;
        if !crate::ahci::write_sectors(lba, sectors, &padded) {
            crate::serial::write_str("  [diskfs] write_sectors failed\n");
            return false;
        }
        DATA_NEXT_LBA += sectors as u64;
        set_entry(&mut DIR[slot], name, 1, lba, data.len());
        flush_dir();
        flush_super();
        true
    }
}

/// Read a named file into `out`. Returns Some(bytes_read) if found, None otherwise.
pub fn read_file(name: &[u8], out: &mut [u8]) -> Option<usize> {
    unsafe {
        if !READY || name.is_empty() { return None; }
        for e in DIR.iter() {
            if e_trit(e) == 1 && e_name(e) == name {
                let file_lba  = e_lba(e);
                let file_size = e_size(e);
                let sectors   = ((file_size + 511) / 512).max(1) as u16;
                let mut tmp   = alloc::vec![0u8; sectors as usize * 512];
                if !crate::ahci::read_sectors(file_lba, sectors, &mut tmp) {
                    return None;
                }
                let n = file_size.min(out.len());
                out[..n].copy_from_slice(&tmp[..n]);
                return Some(n);
            }
        }
        None
    }
}

pub fn is_ready() -> bool { unsafe { READY } }
