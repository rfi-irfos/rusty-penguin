// CPIO newc parser. Builds a static inode table from a module loaded by GRUB.
//
// newc header: 110 bytes ASCII hex fields.
// Layout (bytes): [magic:6][ino:8][mode:8][uid:8][gid:8][nlink:8][mtime:8]
//                 [filesize:8][devmajor:8][devminor:8][rdevmajor:8][rdevminor:8]
//                 [namesize:8][check:8]  → then name, pad4, data, pad4.

const MAX_INODES: usize = 64;

#[derive(Copy, Clone)]
pub struct Inode {
    pub name: [u8; 64],
    pub name_len: usize,
    pub data: *const u8,
    pub size: usize,
}

static mut INODES: [Option<Inode>; MAX_INODES] = [None; MAX_INODES];
static mut INODE_COUNT: usize = 0;
static mut DELETED_MASK: u64 = 0;  // Bitmap for deleted inodes (supports up to 64)

fn parse_hex8(s: &[u8]) -> u32 {
    let mut v: u32 = 0;
    for &b in s.iter().take(8) {
        v <<= 4;
        v |= match b {
            b'0'..=b'9' => (b - b'0') as u32,
            b'a'..=b'f' => (b - b'a' + 10) as u32,
            b'A'..=b'F' => (b - b'A' + 10) as u32,
            _ => 0,
        };
    }
    v
}

fn align4(n: usize) -> usize { (n + 3) & !3 }

/// Parse a CPIO newc archive at `base` (physical = virtual in our identity map).
/// Fills the static inode table. Safe to call once at boot.
pub fn init(base: *const u8, total_size: usize) {
    let mut off = 0usize;
    unsafe {
        INODE_COUNT = 0;
        while off + 110 <= total_size {
            let hdr = core::slice::from_raw_parts(base.add(off), 110);

            // Verify magic "070701"
            if &hdr[0..6] != b"070701" { break; }

            let namesize = parse_hex8(&hdr[94..102]) as usize;
            let filesize = parse_hex8(&hdr[54..62]) as usize;

            off += 110;  // skip header

            if off + namesize > total_size { break; }
            let name_bytes = core::slice::from_raw_parts(base.add(off), namesize);

            // Strip trailing NUL for comparison
            let name_len = name_bytes.iter().position(|&b| b == 0).unwrap_or(namesize);

            off = align4(off + namesize);  // name + pad

            let data_ptr = base.add(off);

            // End-of-archive sentinel
            if name_len == 11 && &name_bytes[..11] == b"TRAILER!!!\0" { break; }

            if INODE_COUNT < MAX_INODES && filesize > 0 {
                let mut n = [0u8; 64];
                let copy_len = name_len.min(63);
                n[..copy_len].copy_from_slice(&name_bytes[..copy_len]);

                INODES[INODE_COUNT] = Some(Inode {
                    name: n,
                    name_len: copy_len,
                    data: data_ptr,
                    size: filesize,
                });
                INODE_COUNT += 1;
            }

            off = align4(off + filesize);
        }
    }
}

pub fn inode_count() -> usize { unsafe { INODE_COUNT } }

pub fn inode(i: usize) -> Option<Inode> {
    unsafe {
        if i >= MAX_INODES { return None; }
        if (DELETED_MASK & (1u64 << i)) != 0 { return None; }  // Skip deleted
        INODES.get(i).copied().flatten()
    }
}

/// Mark an inode as deleted
pub fn delete(i: usize) -> bool {
    unsafe {
        if i >= MAX_INODES || INODES[i].is_none() { return false; }
        if (DELETED_MASK & (1u64 << i)) != 0 { return false; }  // Already deleted
        DELETED_MASK |= 1u64 << i;
        true
    }
}

/// Find a file by path (e.g. b"bin/psh"). Returns a static slice of its data.
/// Skips deleted inodes.
pub fn find(path: &[u8]) -> Option<&'static [u8]> {
    unsafe {
        for i in 0..INODE_COUNT {
            if (DELETED_MASK & (1u64 << i)) != 0 { continue; }  // Skip deleted
            if let Some(ino) = INODES[i] {
                let name = &ino.name[..ino.name_len];
                // Strip leading "./" from stored names
                let stored = if name.starts_with(b"./") { &name[2..] } else { name };
                if stored == path {
                    return Some(core::slice::from_raw_parts(ino.data, ino.size));
                }
            }
        }
        None
    }
}
