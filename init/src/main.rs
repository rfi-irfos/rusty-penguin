use std::process::Command;
use std::fs;
use std::path::Path;

fn main() {
    println!("Rusty Penguin init (PID 1) starting...");

    setup_environment();
    setup_home_directory();

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
