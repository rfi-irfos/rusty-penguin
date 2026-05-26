// Rusty Penguin PID 1
// Mounts essential filesystems, sets hostname, then hands control to psh.
// On psh exit, halts the system cleanly.

use nix::mount::{mount, MsFlags};
use nix::sys::reboot::{reboot, RebootMode};
use nix::unistd::sethostname;
use std::io::{self, Write};
use std::process::Command;

const BANNER: &str = r#"
  ____            _           ____                        _
 |  _ \ _   _ ___| |_ _   _ |  _ \ ___ _ __   __ _ _  _(_)_ __
 | |_) | | | / __| __| | | || |_) / _ \ '_ \ / _` | | | | '_ \
 |  _ <| |_| \__ \ |_| |_| ||  __/  __/ | | | (_| | |_| | | | |
 |_| \_\\__,_|___/\__|\__, ||_|   \___|_| |_|\__, |\__,_|_| |_|
                       |___/                  |___/

  "Binary hardware. Ternary mind."  v0.1.0
"#;

fn mount_fs(source: &str, target: &str, fstype: &str, flags: MsFlags) {
    std::fs::create_dir_all(target).ok();
    mount(
        Some(source),
        target,
        Some(fstype),
        flags,
        None::<&str>,
    ).unwrap_or_else(|e| eprintln!("[init] warning: mount {} failed: {}", target, e));
}

fn main() {
    // Mount essential virtual filesystems
    mount_fs("proc",    "/proc", "proc",    MsFlags::MS_NOSUID | MsFlags::MS_NODEV | MsFlags::MS_NOEXEC);
    mount_fs("sysfs",   "/sys",  "sysfs",   MsFlags::MS_NOSUID | MsFlags::MS_NODEV | MsFlags::MS_NOEXEC);
    mount_fs("devtmpfs","/dev",  "devtmpfs",MsFlags::MS_NOSUID);
    mount_fs("tmpfs",   "/tmp",  "tmpfs",   MsFlags::MS_NOSUID | MsFlags::MS_NODEV);

    // Set hostname
    sethostname("rusty-penguin").unwrap_or_else(|e| eprintln!("[init] sethostname: {}", e));

    println!("{}", BANNER);
    println!("  Architecture: ternary-first · scheduler: active/dormant/suppressed");
    println!("  Type 'help' for commands.\n");

    // Try to launch the graphical desktop first.
    // If /dev/fb0 is not available, desktop will exec psh itself.
    let desktop_candidates = ["/bin/desktop", "/usr/local/bin/desktop"];
    for desktop in &desktop_candidates {
        if std::path::Path::new(desktop).exists() {
            let status = Command::new(desktop).status();
            match status {
                Ok(s) => {
                    println!("[init] desktop exited: {}", s);
                    // Fall through to psh below
                }
                Err(e) => eprintln!("[init] failed to exec {}: {}", desktop, e),
            }
            break;
        }
    }

    // Spawn psh — if the binary is embedded alongside init, use it directly.
    // During development on a host system, fall back to the host psh path.
    let psh_candidates = [
        "/bin/psh",
        "/usr/local/bin/psh",
        // dev fallback: run cargo-built shell directly
    ];

    let mut launched = false;
    for psh in &psh_candidates {
        if std::path::Path::new(psh).exists() {
            let status = Command::new(psh).status();
            match status {
                Ok(s) => {
                    println!("[init] psh exited: {}", s);
                    launched = true;
                    break;
                }
                Err(e) => eprintln!("[init] failed to exec {}: {}", psh, e),
            }
        }
    }

    if !launched {
        // Fallback: minimal emergency shell using stdin/stdout
        eprintln!("[init] psh not found — dropping to emergency prompt");
        emergency_shell();
    }

    // Shutdown
    println!("[init] Halting system...");
    io::stdout().flush().ok();
    if let Err(e) = reboot(RebootMode::RB_HALT_SYSTEM) {
        eprintln!("[init] reboot failed: {}", e);
    }
}

fn emergency_shell() {
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    loop {
        print!("emergency# ");
        stdout.flush().ok();
        let mut line = String::new();
        if stdin.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let cmd = line.trim();
        if cmd == "halt" || cmd == "exit" || cmd == "quit" {
            break;
        }
        // Execute raw command
        let parts: Vec<&str> = cmd.split_whitespace().collect();
        if let Some(prog) = parts.first() {
            Command::new(prog).args(&parts[1..]).status().ok();
        }
    }
}
