use std::process::Command;
use std::io::{self, Write};

fn main() {
    // Try to launch desktop first
    if try_launch_desktop().is_ok() {
        return;
    }

    // Fall back to simple shell
    run_shell();
}

fn try_launch_desktop() -> Result<(), Box<dyn std::error::Error>> {
    let paths = [
        "/bin/desktop",
        "/usr/local/bin/desktop",
        "./desktop",
    ];

    for path in &paths {
        if std::path::Path::new(path).exists() {
            Command::new(path).status()?;
            return Ok(());
        }
    }

    Err("desktop not found".into())
}

fn run_shell() {
    println!("Rusty Penguin Shell v1.0.0");
    println!("Type 'exit' to quit\n");

    let mut stdout = io::stdout();

    loop {
        print!("rp$ ");
        stdout.flush().ok();

        let mut line = String::new();
        if io::stdin().read_line(&mut line).is_err() {
            break;
        }

        let cmd = line.trim();
        if cmd.is_empty() {
            continue;
        }

        match cmd {
            "exit" | "quit" => break,
            "help" => println!("Commands: desktop, ls, pwd, cd, exit"),
            "desktop" => {
                let _ = Command::new("desktop").status();
            }
            _ => {
                let parts: Vec<&str> = cmd.split_whitespace().collect();
                if !parts.is_empty() {
                    let _ = Command::new(parts[0])
                        .args(&parts[1..])
                        .status();
                }
            }
        }
    }
}
