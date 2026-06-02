use super::{sys_ls, write};

pub fn run() {
    let mut buf = [0u8; 4096];
    let n = sys_ls(&mut buf);
    if n == 0 { write(b"(empty)\n"); return; }
    write(&buf[..n]);
}
