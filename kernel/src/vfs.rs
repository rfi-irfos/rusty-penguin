// Minimal VFS: path → fd, backed by ramfs.
// Fds 0/1/2 = stdin/stdout/stderr (handled by syscall layer).
// Fds 3+ are open ramfs files.

const MAX_FDS: usize = 16;

#[derive(Clone, Copy)]
struct FdEntry {
    data: *const u8,
    size: usize,
    pos:  usize,
}

static mut FD_TABLE: [Option<FdEntry>; MAX_FDS] = [None; MAX_FDS];

/// Open a file by path. Returns fd on success, u64::MAX on error.
pub fn open(path: &[u8]) -> u64 {
    let slice = match crate::ramfs::find(path) {
        Some(s) => s,
        None    => return u64::MAX,
    };
    unsafe {
        for fd in 3..MAX_FDS {
            if FD_TABLE[fd].is_none() {
                FD_TABLE[fd] = Some(FdEntry { data: slice.as_ptr(), size: slice.len(), pos: 0 });
                return fd as u64;
            }
        }
    }
    u64::MAX  // no free slots
}

/// Close an fd. Returns 0 on success, u64::MAX on error.
pub fn close(fd: u64) -> u64 {
    let i = fd as usize;
    if i < 3 || i >= MAX_FDS { return u64::MAX; }
    unsafe {
        if FD_TABLE[i].is_some() { FD_TABLE[i] = None; 0 } else { u64::MAX }
    }
}

/// Read up to `len` bytes from fd into buf. Returns bytes read, or u64::MAX on error.
pub fn read(fd: u64, buf: *mut u8, len: usize) -> u64 {
    let i = fd as usize;
    if i < 3 || i >= MAX_FDS { return u64::MAX; }
    unsafe {
        let entry = match FD_TABLE[i].as_mut() { Some(e) => e, None => return u64::MAX };
        let avail = entry.size.saturating_sub(entry.pos);
        let n = avail.min(len);
        if n == 0 { return 0; }
        core::ptr::copy_nonoverlapping(entry.data.add(entry.pos), buf, n);
        entry.pos += n;
        n as u64
    }
}
