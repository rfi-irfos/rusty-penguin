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
                   help\n");
        v.write("motd.txt",
            b" _______________________________________________\n\
               < Welcome to RustyPenguin 1.0.0 - bare metal! >\n\
               -----------------------------------------------\n\
                   \\  ^___^\n\
                    \\ (o o)\n\
                      ( =^= )\n\
                      (\"   \")\n");
        v.write("QUICKSTART.txt",
            b"RustyPenguin Quick Start\n\
              =======================\n\
              \n\
              This is a bare-metal x86_64 OS written entirely in Rust.\n\
              No kernel, no glibc - pure systems programming.\n\
              \n\
              Key Commands:\n\
              - help           : show all available commands\n\
              - ls             : list files\n\
              - nano <file>    : edit files\n\
              - demo.psh       : run full feature demo\n\
              - sysinfo        : show system info\n\
              - lsb_release -a : show distro info\n\
              \n\
              Scripting:\n\
              - psh script.psh : run shell script\n\
              - Supports: for/do/done, if/then/else/fi, pipes (|), redirects (>, >>)\n\
              \n\
              For more info:\n\
              - cat readme.txt\n\
              - cat QUICKSTART.txt\n");
        v.write("demo.psh",
            b"#!/bin/psh\n\
              # RustyPenguin OS v1.0.0 - Full feature demonstration\n\
              echo \"\x1b[1;32m=== RustyPenguin v1.0.0 - Bare Metal Rust OS ===\"\x1b[0m\n\
              echo \"Kernel: $(uname) | CPU: x86_64 | Binary: bare-metal\"\n\
              sysinfo\n\
              echo \"\"\n\
              echo \"\x1b[1;36m-- Conditional Logic --\"\x1b[0m\n\
              if test -f readme.txt\n\
              then\n\
                echo \"  readme.txt exists - file I/O working\"\n\
              fi\n\
              echo \"\"\n\
              echo \"\x1b[1;36m-- Arithmetic --\"\x1b[0m\n\
              echo -n \"  (2 + 3) * 7 = \"\n\
              calc (2 + 3) * 7\n\
              echo -n \"  1024 / 32 = \"\n\
              calc 1024 / 32\n\
              echo \"\"\n\
              echo \"\x1b[1;36m-- Sequences and Loops --\"\x1b[0m\n\
              echo \"  Counting 1 to 5:\"\n\
              for i in $(seq 1 5)\n\
              do\n\
                echo -n \"  $i\"\n\
              done\n\
              echo \"\"\n\
              echo \"\"\n\
              echo \"\x1b[1;36m-- Files and Pipes --\"\x1b[0m\n\
              echo \"  Creating languages.txt...\"\n\
              echo \"Rust\" > languages.txt\n\
              echo \"Python\" >> languages.txt\n\
              echo \"Lisp\" >> languages.txt\n\
              echo \"Go\" >> languages.txt\n\
              echo \"  Sorted output:\"\n\
              cat languages.txt | sort\n\
              echo \"\"\n\
              echo \"\x1b[1;36m-- Text Processing --\"\x1b[0m\n\
              echo \"  Languages with 'u': $(cat languages.txt | grep u | wc -l) found\"\n\
              echo \"\"\n\
              echo \"\x1b[1;36m-- System Introspection --\"\x1b[0m\n\
              echo \"  Hostname: $(hostname)\"\n\
              echo \"  User ID: $(id)\"\n\
              lsb_release\n\
              echo \"\"\n\
              echo \"\x1b[1;36m-- Sparse Ternary Inference --\"\x1b[0m\n\
              ai 8\n\
              echo \"\"\n\
              echo \"\x1b[1;32m=== Demo Complete ===\"\x1b[0m\n");
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
