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

        let parts: Vec<&str> = cmd.split_whitespace().collect();
        match parts.first().copied().unwrap_or("") {
            "exit" | "quit" => break,
            "help" => println!(
                "Commands:\n  ps             list processes with ternary state\n  \
                 kill <pid> <st>   transition pid to +1 (resume), 0 (stop), -1 (terminate)\n  \
                 tis [dim]      sparse-skip forward pass; reports dormancy %\n  \
                 desktop        launch graphical session\n  \
                 ls / pwd / cd / exit  standard"
            ),
            "desktop" => {
                let _ = Command::new("desktop").status();
            }
            "ps" => print_ternary_ps(),
            "kill" => kill_transition(&parts[1..]),
            "tis" => tis_demo(&parts[1..]),
            _ => {
                if !parts.is_empty() {
                    let _ = Command::new(parts[0])
                        .args(&parts[1..])
                        .status();
                }
            }
        }
    }
}

/// `tis [dim]` — run a sparse-skip forward pass through ai-runtime to
/// demonstrate the doctrine's "Sparse Execution" + "Ternary" principles.
/// Generates a deterministic ternary weight matrix and input vector, runs
/// the dense layer, and reports how many of the multiply-accumulate
/// operations were skipped because at least one operand was Trit::Zero.
fn tis_demo(args: &[&str]) {
    use ai_runtime::{TernaryLinear, TernaryTensor};
    use ternary_core::Trit;

    let dim: usize = args.first()
        .and_then(|s| s.parse().ok())
        .unwrap_or(64);
    if dim == 0 || dim > 4096 {
        println!("tis: dim must be 1..=4096");
        return;
    }

    // Deterministic LCG so the demo is reproducible across runs.
    let mut state: u64 = 0x9E3779B97F4A7C15;
    let mut next_trit = || -> Trit {
        state = state.wrapping_mul(6_364_136_223_846_793_005)
                     .wrapping_add(1_442_695_040_888_963_407);
        match (state >> 33) % 3 {
            0 => Trit::Neg, 1 => Trit::Zero, _ => Trit::Pos,
        }
    };

    let in_features = dim;
    let out_features = dim;
    let mut layer = TernaryLinear::new(in_features, out_features);
    for w in layer.weights.data.iter_mut() { *w = next_trit(); }
    let input: Vec<Trit> = (0..in_features).map(|_| next_trit()).collect();
    let input = TernaryTensor::new(input, vec![in_features]);

    let (output, total_ops, skipped) = layer.forward(&input);
    let dormancy_pct = if total_ops > 0 { skipped * 100 / total_ops } else { 0 };

    let pos = output.data.iter().filter(|&&t| t == Trit::Pos).count();
    let neg = output.data.iter().filter(|&&t| t == Trit::Neg).count();
    let zer = output.data.len() - pos - neg;

    println!("tis: {}x{} layer, {} ops, {} skipped ({}% dormancy)",
             out_features, in_features, total_ops, skipped, dormancy_pct);
    println!("     output trits: +{}  0:{}  -{}", pos, zer, neg);
}

/// `kill <pid> <state>` — drive scheduler::ProcessController::transition()
/// which maps each ternary state to a real Linux signal:
///     +1 → SIGCONT  (resume / activate)
///      0 → SIGSTOP  (dormant)
///     -1 → SIGTERM  (terminate / suppress)
fn kill_transition(args: &[&str]) {
    use scheduler::{ProcessController, TernaryState};
    if args.len() != 2 {
        println!("usage: kill <pid> <+1|0|-1>");
        return;
    }
    let pid: i32 = match args[0].parse() {
        Ok(n) => n,
        Err(_) => { println!("kill: bad pid '{}'", args[0]); return; }
    };
    let state = match args[1] {
        "+1" | "1"  | "active"     => TernaryState::Active,
        "0"  | "dormant"           => TernaryState::Dormant,
        "-1" | "suppress" | "kill" => TernaryState::Suppressed,
        other => { println!("kill: bad state '{}' (want +1, 0, -1)", other); return; }
    };
    match ProcessController::transition(pid, state) {
        Ok(()) => println!("pid {} → {} ({})", pid, state.symbol(), state.label()),
        Err(e) => println!("kill: {}", e),
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
