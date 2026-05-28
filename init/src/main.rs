use std::process::Command;
use std::fs;
use std::path::Path;
use ternary_core::{Trit, Tryte};

fn main() {
    println!("Rusty Penguin init (PID 1) starting...");

    mount_pseudo_filesystems();
    setup_environment();

    // Storage is a TERNARY subsystem, not a binary present/absent flag:
    //   +1 Active     — disk present & formatted → mounted, persistent
    //    0 Dormant    — disk present but blank   → provisioned, then activated
    //   -1 Suppressed — no disk / mount failed   → ephemeral boot, no persistence
    let storage = setup_persistence();
    log_storage_state(storage);

    setup_home_directory();

    if storage == Trit::Pos {
        let boots = update_boot_record(storage);
        eprintln!("[init] persistent boot #{} (record: ~/.rusty/boot.tern)", boots);
    }

    // Phase 1 substrate: try to launch the graphical desktop first. If the
    // desktop binary isn't bundled or fails, fall back to the shell so the
    // system stays usable instead of looping forever.
    match launch_session() {
        Ok(()) => {
            println!("[init] Session exited cleanly. Halting init loop.");
        }
        Err(e) => {
            eprintln!("[init] Session launch failed: {}", e);
        }
    }
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

fn setup_environment() {
    // Set basic environment variables
    std::env::set_var("PATH", "/bin:/usr/local/bin:/usr/bin");
    std::env::set_var("HOME", "/home/rusty-penguin");
    std::env::set_var("SHELL", "/bin/psh");
    std::env::set_var("TERM", "xterm");

    println!("[init] Environment initialized");
}

// ─── Persistence: a ternary storage subsystem ────────────────────────────────
// The live ISO is otherwise ephemeral (initramfs only). This makes Rusty Penguin
// daily-drivable: a writable disk is mounted at /home so user files and config
// survive reboots. Disk state is a Trit (+1/0/-1), and the boot record is a
// first-class `.tern` file (balanced ternary, via ternary-core).

const PERSIST_MNT: &str = "/persist";
const PERSIST_HOME: &str = "/persist/home/rusty-penguin";

/// Mount the kernel pseudo-filesystems a PID 1 needs. devtmpfs gives us the
/// block-device nodes (/dev/vda …) that disk detection and mke2fs require.
fn mount_pseudo_filesystems() {
    use nix::mount::{mount, MsFlags};
    let mounts = [
        ("proc",     "/proc", "proc"),
        ("sysfs",    "/sys",  "sysfs"),
        ("devtmpfs", "/dev",  "devtmpfs"),
        ("tmpfs",    "/tmp",  "tmpfs"),
    ];
    for (src, target, fstype) in mounts {
        let _ = fs::create_dir_all(target);
        let _ = mount(Some(src), target, Some(fstype), MsFlags::empty(), None::<&str>);
    }
}

fn run_ok(cmd: &str, args: &[&str]) -> bool {
    Command::new(cmd).args(args).status().map(|s| s.success()).unwrap_or(false)
}

fn detect_disk() -> Option<String> {
    // Probe the usual writable block devices (the ISO itself is the CD-ROM).
    for d in ["/dev/vda", "/dev/vdb", "/dev/sda", "/dev/sdb", "/dev/nvme0n1"] {
        if Path::new(d).exists() { return Some(d.to_string()); }
    }
    None
}

fn mount_fs(dev: &str, target: &str, fstype: &str) -> bool {
    use nix::mount::{mount, MsFlags};
    let _ = fs::create_dir_all(target);
    mount(Some(dev), target, Some(fstype), MsFlags::empty(), None::<&str>).is_ok()
}

fn mount_ext4(dev: &str, target: &str) -> bool {
    mount_fs(dev, target, "ext4")
}

fn bind_mount(src: &str, dst: &str) -> bool {
    use nix::mount::{mount, MsFlags};
    let _ = fs::create_dir_all(src);
    let _ = fs::create_dir_all(dst);
    mount(Some(src), dst, None::<&str>, MsFlags::MS_BIND, None::<&str>).is_ok()
}

/// Bring up persistent storage. Returns the storage state as a Trit.
fn setup_persistence() -> Trit {
    let disk = match detect_disk() {
        Some(d) => d,
        None => return Trit::Neg, // suppressed — no writable disk, ephemeral boot
    };

    // +1 if it already holds a filesystem we can mount.
    if mount_ext4(&disk, PERSIST_MNT) {
        finish_persist_setup();
        return Trit::Pos;
    }

    // 0 → dormant: present but blank. Provision it. busybox mke2fs is a static,
    // dependency-free formatter; it doesn't take util-linux's `-t`, so we ask
    // for an ext-family fs with journaling via `-j` (ext3 layout, which the
    // kernel's ext4 driver mounts fine). `-F` forces a whole-device fs.
    eprintln!("[init] storage 0 (dormant): provisioning {} ...", disk);
    let mkfs = Command::new("/bin/busybox")
        .args(["mke2fs", "-F", "-j", "-L", "RPDATA", &disk])
        .output();
    match mkfs {
        Ok(o) if o.status.success() => {}
        Ok(o) => {
            eprintln!("[init] mke2fs failed (code {:?}): {}",
                o.status.code(), String::from_utf8_lossy(&o.stderr).trim());
            return Trit::Neg;
        }
        Err(e) => { eprintln!("[init] mke2fs not runnable: {}", e); return Trit::Neg; }
    }
    // ext4 driver mounts ext2/3/4; fall back to explicit ext2 if needed.
    if mount_ext4(&disk, PERSIST_MNT) || mount_fs(&disk, PERSIST_MNT, "ext2") {
        finish_persist_setup();
        Trit::Pos
    } else {
        eprintln!("[init] provisioned {} but mount failed (ext4/ext2 driver missing?)", disk);
        Trit::Neg
    }
}

/// Bind the persistent home over /home so user data lands on the disk.
fn finish_persist_setup() {
    let _ = fs::create_dir_all(PERSIST_HOME);
    let _ = fs::create_dir_all("/home/rusty-penguin");
    bind_mount("/persist/home", "/home");
}

fn log_storage_state(t: Trit) {
    let (sym, label) = match t {
        Trit::Pos  => ("+1", "ACTIVE — persistent disk mounted at /home"),
        Trit::Zero => ("0",  "DORMANT — provisioning"),
        Trit::Neg  => ("-1", "SUPPRESSED — ephemeral boot (no writable disk)"),
    };
    eprintln!("[init] storage {} : {}", sym, label);
}

/// Read, increment, and persist the boot counter in a `.tern` file. Returns the
/// new count. The file is balanced-ternary aware: the count is also stored as a
/// 9-trit Tryte (the OS's native numeric form), making `.tern` first-class.
fn update_boot_record(storage: Trit) -> i32 {
    let dir = format!("{}/.rusty", PERSIST_HOME);
    let _ = fs::create_dir_all(&dir);
    let path = format!("{}/boot.tern", dir);

    let prev = fs::read_to_string(&path).ok()
        .and_then(|s| parse_tern_int(&s, "boots"))
        .unwrap_or(0);
    let n = prev + 1;

    let _ = fs::write(&path, render_boot_tern(storage, n));
    // Flush to the block device. Without this the write lives only in the page
    // cache and is lost on power-off (no clean unmount yet) — the counter would
    // never advance. sync(2) pushes all dirty buffers to disk.
    nix::unistd::sync();
    n
}

/// Render a `.tern` document. Format (one record per line):
///   `@key  <trit|int>  [tryte]`
/// where a trit is `+`/`0`/`-` and a tryte is 9 balanced-ternary digits.
fn render_boot_tern(storage: Trit, boots: i32) -> String {
    let tryte = Tryte::from_i32(boots);
    format!(
        "# boot.tern — Rusty Penguin boot record (balanced ternary)\n\
         # @key   value   [tryte: 9 trits, high→low, +/0/-]\n\
         @storage {}\n\
         @boots   {}   {}\n\
         @native  ternary\n",
        trit_char(storage),
        boots,
        tryte_str(&tryte),
    )
}

fn parse_tern_int(doc: &str, key: &str) -> Option<i32> {
    for line in doc.lines() {
        let line = line.trim();
        if line.starts_with('#') { continue; }
        let mut it = line.split_whitespace();
        if it.next() == Some(&format!("@{}", key)) {
            if let Some(v) = it.next() {
                if let Ok(n) = v.parse::<i32>() { return Some(n); }
            }
        }
    }
    None
}

fn trit_char(t: Trit) -> char {
    match t { Trit::Pos => '+', Trit::Zero => '0', Trit::Neg => '-' }
}

fn tryte_str(t: &Tryte) -> String {
    // trits() is least-significant first; print high→low for readability.
    let mut s = String::with_capacity(9);
    for trit in t.trits().iter().rev() {
        s.push(trit_char(*trit));
    }
    s
}

fn setup_home_directory() {
    let home = "/home/rusty-penguin";
    
    // Create home directory
    if !Path::new(home).exists() {
        if let Ok(_) = fs::create_dir_all(home) {
            println!("[init] Created home directory: {}", home);
        }
    }

    // Create config directory
    let config_dir = format!("{}/.config/rusty-penguin", home);
    if !Path::new(&config_dir).exists() {
        if let Ok(_) = fs::create_dir_all(&config_dir) {
            println!("[init] Created config directory: {}", config_dir);
        }
    }

    // Create user shell history file
    let history_file = format!("{}/.psh_history", home);
    if !Path::new(&history_file).exists() {
        let _ = fs::File::create(&history_file);
        println!("[init] Created history file: {}", history_file);
    }

    std::env::set_var("HOME", home);
}

fn launch_session() -> Result<(), Box<dyn std::error::Error>> {
    // Preferred: graphical desktop (rp-compositor on Linux substrate).
    let desktop_paths = ["/bin/desktop", "/usr/local/bin/desktop"];
    for path in &desktop_paths {
        if Path::new(path).exists() {
            println!("[init] Found desktop: {} — launching...", path);
            match Command::new(path).status() {
                Ok(status) => {
                    println!("[init] Desktop exited with status: {:?}", status.code());
                    return Ok(());
                }
                Err(e) => {
                    eprintln!("[init] Desktop failed to spawn ({}). Falling back to shell.", e);
                    break;
                }
            }
        }
    }

    // Fallback: shell. The system is still usable as a text console.
    let shell_paths = ["/bin/shell", "/usr/local/bin/shell", "/bin/psh", "/usr/local/bin/psh"];
    for path in &shell_paths {
        if Path::new(path).exists() {
            println!("[init] Found shell: {} — launching...", path);
            let status = Command::new(path).status()?;
            println!("[init] Shell exited with status: {:?}", status.code());
            return Ok(());
        }
    }

    Err("No desktop or shell binary found".into())
}
