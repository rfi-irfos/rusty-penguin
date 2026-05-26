// keyboard.rs — stdin raw-mode keyboard reader
// Puts fd 0 in raw mode, reads bytes, sends via channel to main thread.

use std::sync::mpsc::Sender;

pub fn keyboard_thread(tx: Sender<Vec<u8>>) {
    let fd: i32 = 0; // stdin
    let orig = unsafe { set_raw(fd) };

    let mut buf = [0u8; 64];
    loop {
        let n = unsafe {
            libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len())
        };
        if n <= 0 { break; }
        if tx.send(buf[..n as usize].to_vec()).is_err() { break; }
    }

    if let Some(o) = orig {
        unsafe { restore(fd, o); }
    }
}

unsafe fn set_raw(fd: i32) -> Option<libc::termios> {
    let mut t: libc::termios = std::mem::zeroed();
    if libc::tcgetattr(fd, &mut t) != 0 { return None; }
    let orig = t;
    libc::cfmakeraw(&mut t);
    if libc::tcsetattr(fd, libc::TCSAFLUSH, &t) != 0 { return None; }
    Some(orig)
}

unsafe fn restore(fd: i32, orig: libc::termios) {
    libc::tcsetattr(fd, libc::TCSAFLUSH, &orig);
}
