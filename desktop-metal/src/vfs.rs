// In-memory flat filesystem. Single-threaded bare-metal — no locking needed.
// Files are stored as raw byte Vecs. Directories are marker entries only.

use alloc::vec::Vec;
use alloc::string::String;

pub struct VfsEntry {
    pub name:   String,
    pub data:   Vec<u8>,
    pub is_dir: bool,
}

pub struct Vfs {
    entries: Vec<VfsEntry>,
}

impl Vfs {
    fn new() -> Self {
        let mut v = Vfs { entries: Vec::new() };
        v.write("readme.txt",
            b"Welcome to RustyPenguin OS.\n\
              Bare-metal Rust. Ternary mind.\n\
              \n\
              Try: nano readme.txt\n\
                   ls\n\
                   cat readme.txt\n");
        v.write("motd.txt",
            b"The penguin runs bare metal today.\n");
        v
    }

    pub fn exists(&self, name: &str) -> bool {
        self.entries.iter().any(|e| e.name == name)
    }

    pub fn write(&mut self, name: &str, data: &[u8]) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.name == name && !e.is_dir) {
            e.data.clear();
            e.data.extend_from_slice(data);
        } else {
            self.entries.push(VfsEntry {
                name:   String::from(name),
                data:   data.to_vec(),
                is_dir: false,
            });
        }
    }

    pub fn mkdir(&mut self, name: &str) {
        if !self.exists(name) {
            self.entries.push(VfsEntry {
                name:   String::from(name),
                data:   Vec::new(),
                is_dir: true,
            });
        }
    }

    pub fn read(&self, name: &str) -> Option<&[u8]> {
        self.entries.iter()
            .find(|e| e.name == name && !e.is_dir)
            .map(|e| e.data.as_slice())
    }

    pub fn delete(&mut self, name: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.name != name);
        self.entries.len() < before
    }

    pub fn rename(&mut self, from: &str, to: &str) -> bool {
        if let Some(e) = self.entries.iter_mut().find(|e| e.name == from) {
            e.name = String::from(to);
            true
        } else {
            false
        }
    }

    pub fn list(&self) -> &[VfsEntry] {
        &self.entries
    }
}

static mut VFS_STORAGE: Option<Vfs> = None;

pub fn vfs() -> &'static mut Vfs {
    unsafe {
        if VFS_STORAGE.is_none() {
            VFS_STORAGE = Some(Vfs::new());
        }
        VFS_STORAGE.as_mut().unwrap()
    }
}
