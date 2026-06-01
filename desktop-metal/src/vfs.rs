// In-memory flat filesystem with RPFS-backed persistence. Files are stored as
// raw byte Vecs; directories are marker entries only. EVERY user file (not just
// settings) is mirrored to the on-disk RPFS via syscall 25 (sys_disk_write), so
// documents saved in the editor or created in the terminal survive a reboot.
//
// Reload uses a tiny on-disk MANIFEST (".vfsmanifest") listing every persisted
// file name. On Vfs::new() we read the manifest and pull each listed file back
// via syscall 26 (sys_disk_read). Persistence is armed only AFTER the built-in
// default files are laid down, so the bulky demo/readme content is never
// re-written to disk on each boot — only genuine user changes hit the platter.
//
// (Note: RPFS allocation is currently append-only — overwriting a file does not
// reclaim its old sectors. Fine for normal use on a roomy disk; a compaction
// pass is a separate follow-up.)

use alloc::vec::Vec;
use alloc::string::String;

/// On-disk file that lists every persisted file name, one per line. Lets boot
/// reload the full working set with only read-by-name syscalls.
const MANIFEST: &str = ".vfsmanifest";

/// Should this name be mirrored to disk? RPFS keys are capped at 51 bytes; the
/// manifest itself is managed separately and must never list itself.
fn persistable(name: &str) -> bool {
    !name.is_empty() && name.len() <= 51 && name != MANIFEST
}

// ── Syscall 25/26: RPFS disk persistence ─────────────────────────────────────
// name_len and data_len are packed into rsi:  low 8 bits = name_len,
// bits 16-31 = data_len / out_max.  Mirrors the sys_http_get packing pattern.

fn persist_write(name: &str, data: &[u8]) -> bool {
    if name.is_empty() || name.len() > 51 || data.is_empty() { return false; }
    let nb  = name.as_bytes();
    let ret: u64;
    unsafe {
        core::arch::asm!(
            "syscall",
            inout("rax") 25u64 => ret,
            in("rdi") nb.as_ptr() as u64,
            in("rsi") (nb.len() as u64) | ((data.len() as u64) << 16),
            in("rdx") data.as_ptr() as u64,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    ret == 0
}

fn persist_read(name: &str, out: &mut [u8]) -> usize {
    if name.is_empty() || name.len() > 51 || out.is_empty() { return 0; }
    let nb  = name.as_bytes();
    let ret: u64;
    unsafe {
        core::arch::asm!(
            "syscall",
            inout("rax") 26u64 => ret,
            in("rdi") nb.as_ptr() as u64,
            in("rsi") (nb.len() as u64) | ((out.len() as u64) << 16),
            in("rdx") out.as_mut_ptr() as u64,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    if ret == u64::MAX { 0 } else { ret as usize }
}

pub struct VfsEntry {
    pub name:   String,
    pub data:   Vec<u8>,
    pub is_dir: bool,
}

pub struct Vfs {
    entries: Vec<VfsEntry>,
    // Disk mirroring is armed only after the built-in defaults are written, so
    // the default files are not needlessly re-persisted on every boot.
    persist_on: bool,
}

impl Vfs {
    fn new() -> Self {
        let mut v = Vfs { entries: Vec::new(), persist_on: false };
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
        // Reload the persisted working set from disk (settings AND user files),
        // then arm mirroring so subsequent writes hit the platter.
        v.load_persisted();
        v.persist_on = true;
        v
    }

    pub fn exists(&self, name: &str) -> bool {
        self.entries.iter().any(|e| e.name == name)
    }

    // Write to in-memory VFS only (no disk flush). Used internally to avoid
    // re-persisting data that was just read back from disk.
    fn write_mem(&mut self, name: &str, data: &[u8]) {
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

    pub fn write(&mut self, name: &str, data: &[u8]) {
        let was_new = !self.exists(name);
        self.write_mem(name, data);
        // Mirror every user file to the on-disk RPFS so it survives a reboot.
        if self.persist_on && persistable(name) {
            persist_write(name, data);
            // The manifest only changes when the SET of files changes — keep it
            // off the hot path for ordinary overwrites/saves.
            if was_new {
                self.flush_manifest();
            }
        }
    }

    /// Rewrite the on-disk manifest = the names of every persistable file
    /// currently in memory. Called when a file is created or removed.
    fn flush_manifest(&self) {
        let mut list = String::new();
        for e in &self.entries {
            if !e.is_dir && persistable(&e.name) {
                list.push_str(&e.name);
                list.push('\n');
            }
        }
        if list.is_empty() {
            // RPFS rejects empty writes; a single newline = "manifest, no files".
            list.push('\n');
        }
        persist_write(MANIFEST, list.as_bytes());
    }

    /// Create directory marker entries for every path prefix of `name`, so the
    /// File Manager shows reloaded files inside their folders.
    fn ensure_parent_dirs(&mut self, name: &str) {
        let bytes = name.as_bytes();
        for (i, &b) in bytes.iter().enumerate() {
            if b == b'/' {
                let dir = &name[..i];
                if !dir.is_empty() && !self.exists(dir) {
                    self.entries.push(VfsEntry {
                        name: String::from(dir),
                        data: Vec::new(),
                        is_dir: true,
                    });
                }
            }
        }
    }

    // Reload the persisted working set from the RPFS on-disk store into the
    // in-memory VFS. Manifest-driven: read the file list, then pull each entry.
    // A legacy single-file path keeps older disks (pre-manifest) loading their
    // settings. Called once at init, before mirroring is armed.
    fn load_persisted(&mut self) {
        // 65535, not 65536: sys_disk_read packs out_max into a 16-bit field, so
        // a 0x10000 buffer would truncate to 0 and read nothing back.
        let mut buf = Vec::new();
        buf.resize(65535, 0u8);

        // Manifest-driven reload of the full working set.
        let mn = persist_read(MANIFEST, &mut buf);
        if mn > 0 {
            // Copy the name list out before reusing `buf` for file contents.
            let names: Vec<String> = core::str::from_utf8(&buf[..mn])
                .unwrap_or("")
                .split('\n')
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect();
            for name in &names {
                let n = persist_read(name, &mut buf);
                if n > 0 {
                    self.ensure_parent_dirs(name);
                    let data = buf[..n].to_vec();
                    self.write_mem(name, &data);
                }
            }
        }

        // Legacy fallback: disks written before the manifest existed still have
        // a bare settings.ini. Load it if the manifest didn't already.
        if !self.exists(".config/rusty-penguin/settings.ini") {
            let n = persist_read(".config/rusty-penguin/settings.ini", &mut buf);
            if n > 0 {
                self.ensure_parent_dirs(".config/rusty-penguin/settings.ini");
                let data = buf[..n].to_vec();
                self.write_mem(".config/rusty-penguin/settings.ini", &data);
            }
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
        let removed = self.entries.len() < before;
        // Drop it from the manifest so it does not reload on the next boot. The
        // file's data sectors stay on disk (RPFS is append-only) but, unlisted,
        // they are never read back.
        if removed && self.persist_on && persistable(name) {
            self.flush_manifest();
        }
        removed
    }

    pub fn rename(&mut self, from: &str, to: &str) -> bool {
        let found = self.entries.iter_mut().find(|e| e.name == from);
        let (data, is_dir) = match found {
            Some(e) => {
                e.name = String::from(to);
                (e.data.clone(), e.is_dir)
            }
            None => return false,
        };
        // Persist under the new name and refresh the manifest (the name set
        // changed). The old name's sectors linger but, unlisted, never reload.
        if self.persist_on && !is_dir && persistable(to) {
            persist_write(to, &data);
            self.flush_manifest();
        }
        true
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
