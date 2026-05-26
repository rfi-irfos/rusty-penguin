// Input handling — evdev via /dev/input/event*
// Scans all event devices and picks the first pointer (EV_ABS or EV_REL) device.
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::sync::{Arc, Mutex};

pub struct MouseState {
    pub x: i32,
    pub y: i32,
    pub buttons: u8,
}

// Linux input_event: 24 bytes on x86_64
// [u64 tv_sec][u64 tv_usec][u16 type][u16 code][i32 value]
const INPUT_EVENT_SIZE: usize = 24;

const EV_SYN: u16 = 0x00;
const EV_KEY: u16 = 0x01;
const EV_REL: u16 = 0x02;
const EV_ABS: u16 = 0x03;

const REL_X: u16 = 0x00;
const REL_Y: u16 = 0x01;
const ABS_X: u16 = 0x00;
const ABS_Y: u16 = 0x01;
const BTN_LEFT: u16  = 0x110;
const BTN_RIGHT: u16 = 0x111;

// QEMU virtio-tablet reports ABS coordinates in 0..32767
const TABLET_MAX: i32 = 32767;

// EVIOCGBIT(0, 1) — returns bitmask of supported event types in first byte.
// bit 2 = EV_REL (relative mouse), bit 3 = EV_ABS (absolute tablet)
const EVIOCGBIT_TYPES: u64 = 0x80014520;

fn log(msg: &str) {
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open("/tmp/desktop.log") {
        let _ = writeln!(f, "{}", msg);
    }
}

unsafe fn is_pointer_device(fd: i32) -> bool {
    let mut bits = [0u8; 1];
    if libc::ioctl(fd, EVIOCGBIT_TYPES, bits.as_mut_ptr()) >= 0 {
        let has_rel = (bits[0] >> (EV_REL as u32)) & 1 != 0;
        let has_abs = (bits[0] >> (EV_ABS as u32)) & 1 != 0;
        has_rel || has_abs
    } else {
        false
    }
}

fn open_event_device() -> Option<File> {
    log("=== input: scanning event devices ===");
    // Wait up to 3 seconds for virtio_input to finish creating devices
    for attempt in 0..30 {
        for i in 0..8 {
            let path = format!("/dev/input/event{}", i);
            match OpenOptions::new().read(true).open(&path) {
                Ok(f) => {
                    let is_ptr = unsafe { is_pointer_device(f.as_raw_fd()) };
                    log(&format!("  event{}: exists, is_pointer={}", i, is_ptr));
                    if is_ptr {
                        log(&format!("  -> using event{}", i));
                        return Some(f);
                    }
                }
                Err(_) => {}
            }
        }
        if attempt == 0 {
            log("  no pointer device yet, retrying...");
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    log("  FAILED: no pointer device found after 3s");
    None
}

pub fn mouse_thread(state: Arc<Mutex<MouseState>>, width: i32, height: i32) {
    let mut file = match open_event_device() {
        Some(f) => f,
        None => return,
    };

    let mut buf = [0u8; INPUT_EVENT_SIZE];
    let mut pending_x: Option<i32> = None;
    let mut pending_y: Option<i32> = None;
    let mut abs_mode = false;
    let mut event_count: u64 = 0;

    loop {
        if file.read_exact(&mut buf).is_err() {
            log("input: read_exact failed, stopping");
            break;
        }

        let ev_type  = u16::from_ne_bytes([buf[16], buf[17]]);
        let ev_code  = u16::from_ne_bytes([buf[18], buf[19]]);
        let ev_value = i32::from_ne_bytes([buf[20], buf[21], buf[22], buf[23]]);

        event_count += 1;
        if event_count <= 20 || event_count % 100 == 0 {
            log(&format!("  ev #{}: type={} code={} val={}", event_count, ev_type, ev_code, ev_value));
        }

        match ev_type {
            EV_ABS => {
                abs_mode = true;
                match ev_code {
                    ABS_X => pending_x = Some(ev_value * width  / (TABLET_MAX + 1)),
                    ABS_Y => pending_y = Some(ev_value * height / (TABLET_MAX + 1)),
                    _ => {}
                }
            }
            EV_REL => {
                let mut s = state.lock().unwrap();
                match ev_code {
                    REL_X => s.x = (s.x + ev_value).max(0).min(width  - 1),
                    REL_Y => s.y = (s.y + ev_value).max(0).min(height - 1),
                    _ => {}
                }
            }
            EV_KEY => {
                let mut s = state.lock().unwrap();
                match ev_code {
                    BTN_LEFT  => { if ev_value != 0 { s.buttons |= 0x01 } else { s.buttons &= !0x01 } }
                    BTN_RIGHT => { if ev_value != 0 { s.buttons |= 0x02 } else { s.buttons &= !0x02 } }
                    _ => {}
                }
            }
            EV_SYN => {
                if abs_mode {
                    let mut s = state.lock().unwrap();
                    if let Some(x) = pending_x.take() { s.x = x; }
                    if let Some(y) = pending_y.take() { s.y = y; }
                }
            }
            _ => {}
        }
    }
}
