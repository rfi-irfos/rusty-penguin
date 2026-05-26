// Input handling — evdev via /dev/input/event*
// Handles both relative (PS/2 mouse) and absolute (USB tablet) devices.
// USB tablet preferred: no grab required, absolute coordinates.
use std::fs::{File, OpenOptions};
use std::io::Read;
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

// QEMU USB tablet reports ABS coordinates in 0..32767
const TABLET_MAX: i32 = 32767;

fn open_event_device() -> Option<File> {
    for i in 0..8 {
        let path = format!("/dev/input/event{}", i);
        if let Ok(f) = OpenOptions::new().read(true).open(&path) {
            eprintln!("[desktop] input: opened {}", path);
            return Some(f);
        }
    }
    // Also try /dev/input/mice as last resort (requires mousedev module)
    OpenOptions::new().read(true).open("/dev/input/mice").ok()
}

pub fn mouse_thread(state: Arc<Mutex<MouseState>>, width: i32, height: i32) {
    let mut file = match open_event_device() {
        Some(f) => f,
        None => {
            eprintln!("[desktop] input: no input device found");
            return;
        }
    };

    let mut buf = [0u8; INPUT_EVENT_SIZE];
    let mut pending_x: Option<i32> = None;
    let mut pending_y: Option<i32> = None;
    let mut abs_mode = false;

    loop {
        if file.read_exact(&mut buf).is_err() {
            break;
        }

        // Parse input_event fields (little-endian, native byte order)
        let ev_type  = u16::from_ne_bytes([buf[16], buf[17]]);
        let ev_code  = u16::from_ne_bytes([buf[18], buf[19]]);
        let ev_value = i32::from_ne_bytes([buf[20], buf[21], buf[22], buf[23]]);

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
                // Relative movement (PS/2 mouse fallback)
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
                // Flush accumulated absolute position
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
