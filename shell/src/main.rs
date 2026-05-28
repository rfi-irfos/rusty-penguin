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
            "help" => println!("Commands: desktop, ps, ls, pwd, cd, exit"),
            "desktop" => {
                let _ = Command::new("desktop").status();
            }
            "ps" => print_ternary_ps(),
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

/// Built-in `ps` that surfaces the scheduler crate's ternary process states.
/// Reads /proc, annotates each process as Active/Dormant/Suppressed using
/// the same heuristic the doctrine maps to +1/0/-1.
fn print_ternary_ps() {
    use scheduler::ProcessController;
    let mut procs = ProcessController::list_processes();
    // Show only those with a name and resident memory > 0 first for readability.
    procs.sort_by_key(|p| (-(p.vmrss_kb as i64), p.pid));

    println!("  PID  ST  NAME             VmRSS (KiB)");
    println!("  ---  --  ---------------- -----------");
    for p in procs.iter().take(40) {
        let name = if p.name.len() > 16 { &p.name[..16] } else { &p.name };
        println!("  {:>3}  {}  {:<16} {:>11}",
                 p.pid, p.state.symbol(), name, p.vmrss_kb);
    }
}
