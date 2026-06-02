use super::{sys_ps, write, write_u64};

pub fn run() {
    let mut buf = [0u8; 32 * 16];
    let count = sys_ps(buf.as_mut_ptr(), 16);
    write(b"PID  ST  NAME\n");
    for i in 0..count {
        let off = i * 32;
        let mut pid: u64 = 0;
        for j in 0..8usize { pid |= (buf[off + j] as u64) << (j * 8); }
        // Trit state: 1=Running(+), 2=Blocked(-), 0=Ready(0)
        let state_ch = match buf[off + 8] { 1 => b'+', 2 => b'-', _ => b'0' };
        let name_slice = &buf[off + 16..off + 32];
        let nlen = name_slice.iter().position(|&b| b == 0).unwrap_or(16);
        write_u64(pid);
        write(b"    ");
        super::sys_write_byte(state_ch);
        write(b"   ");
        write(&name_slice[..nlen]);
        write(b"\n");
    }
}
