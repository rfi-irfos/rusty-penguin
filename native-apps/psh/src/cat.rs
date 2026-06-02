use super::{sys_open, sys_read_fd, sys_close, write};

pub fn run(path: &[u8]) {
    if path.is_empty() { write(b"usage: cat <path>\n"); return; }
    let fd = sys_open(path);
    if fd == u64::MAX { write(b"cat: not found\n"); return; }
    let mut buf = [0u8; 256];
    loop {
        let n = sys_read_fd(fd, &mut buf);
        if n == 0 { break; }
        write(&buf[..n]);
    }
    sys_close(fd);
    write(b"\n");
}
