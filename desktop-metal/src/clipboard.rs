// Process-global clipboard for copy/paste between windows. Single-threaded
// bare metal, so a `static mut Option<String>` with a thin accessor is fine —
// mirrors the pattern already used in vfs.rs.

use alloc::string::String;

static mut CLIPBOARD: Option<String> = None;

pub fn set(s: &str) {
    unsafe { CLIPBOARD = Some(String::from(s)); }
}

pub fn get() -> Option<String> {
    unsafe { CLIPBOARD.clone() }
}
