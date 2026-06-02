use super::{sys_open, sys_read_fd, sys_close, write, split_args};

pub fn run(args: &[u8]) {
    let (parts, count) = split_args(args);
    if count < 2 { write(b"usage: grep <pattern> <file>\n"); return; }
    let pattern = parts[0];
    let path    = parts[1];

    let fd = sys_open(path);
    if fd == u64::MAX { write(b"grep: file not found\n"); return; }
    let mut buf = [0u8; 4096];
    let n = sys_read_fd(fd, &mut buf);
    sys_close(fd);

    let data = &buf[..n];
    let mut line_start = 0;
    let mut matched = 0u64;
    while line_start <= data.len() {
        let line_end = data[line_start..]
            .iter()
            .position(|&b| b == b'\n')
            .map(|p| line_start + p)
            .unwrap_or(data.len());
        let line = &data[line_start..line_end];
        if contains(line, pattern) {
            write(line);
            write(b"\n");
            matched += 1;
        }
        if line_end >= data.len() { break; }
        line_start = line_end + 1;
    }
    if matched == 0 { write(b"(no matches)\n"); }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() { return true; }
    if needle.len() > haystack.len() { return false; }
    haystack.windows(needle.len()).any(|w| w == needle)
}
