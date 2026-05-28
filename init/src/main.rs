use std::process::Command;
use std::fs;
use std::path::Path;

fn main() {
    println!("Rusty Penguin init (PID 1) starting...");

    // Set up basic environment
    setup_environment();

    // Create home directory if needed
    setup_home_directory();

    // Try to launch shell
    if let Err(e) = launch_shell() {
        eprintln!("Failed to launch shell: {}", e);
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
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

fn launch_shell() -> Result<(), Box<dyn std::error::Error>> {
    println!("[init] Launching shell...\n");

    // Try to find shell binary
    let shell_paths = [
        "/bin/shell",
        "/usr/local/bin/shell",
        "/bin/psh",
        "/usr/local/bin/psh",
    ];

    for path in &shell_paths {
        if Path::new(path).exists() {
            println!("[init] Found shell: {}", path);
            let status = Command::new(path).status()?;
            println!("[init] Shell exited with status: {:?}", status.code());
            return Ok(());
        }
    }

    Err("No shell binary found".into())
}
