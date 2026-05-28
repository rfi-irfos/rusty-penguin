use std::process::Command;
use std::io::{self, Write};

mod pkg;

fn main() {
    // In recovery mode (the desktop bounced us here because there's no display)
    // do NOT try to relaunch the desktop — that would loop forever. Go straight
    // to the console REPL.
    if std::env::var_os("RP_RECOVERY").is_none() {
        if try_launch_desktop().is_ok() {
            return;
        }
    } else {
        println!("Rusty Penguin recovery console (no display available)");
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
                "Commands:\n  ps [filter] [--all] list processes with ternary state;\n  \
                                     optional substring filter on name\n  \
                 kill <pid|name> <st> transition by PID or name; +1 (resume),\n  \
                                     0 (stop), -1 (terminate); name match must be unambiguous\n  \
                 tis [dim] [layers]  multi-layer sparse-skip inference; reports per-layer dormancy\n  \
                                     (alias: `ai`, matches bare-metal psh)\n  \
                 tri <a> <op> <b>    balanced-ternary arithmetic (+, -, *, /, %)\n  \
                 rpm install <pkg|url>  install a .rpkg (local path or http(s) URL)\n  \
                 rpm list/info/remove   manage installed packages (persist in /opt)\n  \
                 desktop             launch graphical session\n  \
                 ls / pwd / cd / exit  standard"
            ),
            "desktop" => {
                let _ = Command::new("desktop").status();
            }
            "ps" => print_ternary_ps(&parts[1..]),
            "kill" => kill_transition(&parts[1..]),
            "tis" | "ai" => tis_demo(&parts[1..]),  // `ai` is the bare-metal alias
            "tri" => tri_calc(&parts[1..]),
            "rpm" => match pkg::cmd_rpm(&parts[1..]) {
                Ok(out)  => println!("{}", out),
                Err(err) => println!("rpm: {}", err),
            },
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

/// Render a Tryte as a +/0/- string, MSB first (so reading order matches
/// the decimal representation: leading sign on the left).
fn tryte_str(t: ternary_core::Tryte) -> String {
    use ternary_core::Trit;
    let trits = t.trits();
    let mut s = String::with_capacity(9);
    for &tr in trits.iter().rev() {
        s.push(match tr { Trit::Pos => '+', Trit::Zero => '0', Trit::Neg => '-' });
    }
    s
}

/// `tri <a> <op> <b>` — balanced-ternary arithmetic via the mathematics
/// crate. Each operand goes through Tryte::from_i32 (9-trit range
/// -9841..+9841), so out-of-range inputs are clamped by truncation
/// through the conversion. Multiplication's result is double-width;
/// we report both low and high tryte if the high part is non-zero.
fn tri_calc(args: &[&str]) {
    use mathematics::{mul_tryte, div_tryte, mod_tryte};
    use ternary_core::Tryte;

    if args.len() != 3 {
        println!("usage: tri <a> <op> <b>   op is + - * / %");
        return;
    }
    let a: i32 = match args[0].parse() {
        Ok(n) => n,
        Err(_) => { println!("tri: bad operand '{}'", args[0]); return; }
    };
    let b: i32 = match args[2].parse() {
        Ok(n) => n,
        Err(_) => { println!("tri: bad operand '{}'", args[2]); return; }
    };
    let ta = Tryte::from_i32(a);
    let tb = Tryte::from_i32(b);

    match args[1] {
        "+" => {
            let r = ta + tb;
            println!("  {} + {} = {}", a, b, r.to_i32());
            println!("  ternary: {} + {} = {}", tryte_str(ta), tryte_str(tb), tryte_str(r));
        }
        "-" => {
            let r = ta + (-tb);
            println!("  {} - {} = {}", a, b, r.to_i32());
            println!("  ternary: {} - {} = {}", tryte_str(ta), tryte_str(tb), tryte_str(r));
        }
        "*" => {
            let (lo, hi) = mul_tryte(ta, tb);
            let combined = (hi.to_i32() as i64) * 19683 + (lo.to_i32() as i64);
            if hi.to_i32() == 0 {
                println!("  {} * {} = {}", a, b, combined);
                println!("  ternary: {} * {} = {}", tryte_str(ta), tryte_str(tb), tryte_str(lo));
            } else {
                println!("  {} * {} = {}  (lo={}, hi={})", a, b, combined, lo.to_i32(), hi.to_i32());
                println!("  ternary: {} * {} = hi:{} lo:{}", tryte_str(ta), tryte_str(tb), tryte_str(hi), tryte_str(lo));
            }
        }
        "/" => {
            let (q, r) = div_tryte(ta, tb);
            if b == 0 {
                println!("  {} / 0 = undefined (div_tryte returns 0, dividend as remainder)", a);
            } else {
                println!("  {} / {} = {}  (remainder {})", a, b, q.to_i32(), r.to_i32());
                println!("  ternary: {} / {} = {}", tryte_str(ta), tryte_str(tb), tryte_str(q));
            }
        }
        "%" => {
            let r = mod_tryte(ta, tb);
            println!("  {} % {} = {}", a, b, r.to_i32());
            println!("  ternary: {} % {} = {}", tryte_str(ta), tryte_str(tb), tryte_str(r));
        }
        other => println!("tri: unknown op '{}' (want + - * / %)", other),
    }
}

/// `tis [dim] [layers]` (alias: `ai`) — run a multi-layer sparse-skip
/// forward pass through ai-runtime to demonstrate the doctrine's "Sparse
/// Execution" + "Ternary" + "AI-native" principles together.
///
/// Mirrors the bare-metal psh `ai` builtin: shows per-layer in/out trit
/// trace and dormancy, plus aggregate over the whole stack. Generates a
/// deterministic ternary weight matrix per layer plus a random input
/// vector; each layer's output feeds the next.
fn tis_demo(args: &[&str]) {
    use ai_runtime::{TernaryLinear, TernaryTensor};
    use ternary_core::Trit;

    let dim: usize = args.first()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8);
    let layers: usize = args.get(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(4);
    if dim == 0 || dim > 4096 {
        println!("tis: dim must be 1..=4096"); return;
    }
    if layers == 0 || layers > 64 {
        println!("tis: layers must be 1..=64"); return;
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
    let trit_char = |t: Trit| match t { Trit::Pos => '+', Trit::Zero => '0', Trit::Neg => '-' };

    println!("albert. [Linux track]");
    println!("sparse ternary inference -- {} layers x dim {}", layers, dim);

    // Random initial activation vector.
    let mut act: Vec<Trit> = (0..dim).map(|_| next_trit()).collect();

    // Show the input trace only when it fits on one line.
    if dim <= 64 {
        print!("  input  [");
        for &t in &act { print!("{}", trit_char(t)); }
        println!("]");
    }

    let mut total_ops = 0usize;
    let mut total_skipped = 0usize;

    for l in 0..layers {
        // Re-seed weights per layer so each layer is structurally distinct
        // but still deterministic across runs.
        let mut layer = TernaryLinear::new(dim, dim);
        for w in layer.weights.data.iter_mut() { *w = next_trit(); }

        let input_tensor = TernaryTensor::new(act.clone(), vec![dim]);
        let (output_tensor, ops, skipped) = layer.forward(&input_tensor);
        total_ops += ops;
        total_skipped += skipped;
        let layer_dormancy = if ops > 0 { skipped * 100 / ops } else { 0 };

        if dim <= 64 {
            print!("  L{}     [", l);
            for &t in &act { print!("{}", trit_char(t)); }
            print!("] -> [");
            for &t in &output_tensor.data { print!("{}", trit_char(t)); }
            println!("]  dormancy {}%", layer_dormancy);
        } else {
            // Wide layers: skip the trace, keep per-layer summary.
            println!("  L{}  {}x{} dormancy {}%", l, dim, dim, layer_dormancy);
        }

        act = output_tensor.data;
    }

    let avg_dormancy = if total_ops > 0 { total_skipped * 100 / total_ops } else { 0 };
    println!();
    println!("{} layers  avg dormancy {}%  skipped {}/{} ops",
             layers, avg_dormancy, total_skipped, total_ops);
    println!("ACTIVE -- Binary hardware. Ternary mind.");
}

/// `kill <pid-or-name> <state>` — drive scheduler::ProcessController::transition()
/// which maps each ternary state to a real Linux signal:
///     +1 → SIGCONT  (resume / activate)
///      0 → SIGSTOP  (dormant)
///     -1 → SIGTERM  (terminate / suppress)
///
/// If the first arg parses as an integer, treat it as a PID. Otherwise,
/// case-insensitive substring match against process names; transition only
/// if the match is unambiguous (zero matches → error, multiple → list PIDs
/// and bail so the user picks one explicitly).
fn kill_transition(args: &[&str]) {
    use scheduler::{ProcessController, TernaryState};
    if args.len() != 2 {
        println!("usage: kill <pid-or-name> <+1|0|-1>");
        return;
    }
    let state = match args[1] {
        "+1" | "1"  | "active"     => TernaryState::Active,
        "0"  | "dormant"           => TernaryState::Dormant,
        "-1" | "suppress" | "kill" => TernaryState::Suppressed,
        other => { println!("kill: bad state '{}' (want +1, 0, -1)", other); return; }
    };

    // Resolve target: integer → PID; otherwise substring match against names.
    let target_pid: i32 = match args[0].parse::<i32>() {
        Ok(n) => n,
        Err(_) => {
            let needle = args[0].to_lowercase();
            let mut matches: Vec<_> = ProcessController::list_processes()
                .into_iter()
                .filter(|p| p.name.to_lowercase().contains(&needle))
                .collect();
            matches.sort_by_key(|p| (-(p.vmrss_kb as i64), p.pid));
            match matches.len() {
                0 => { println!("kill: no process matching '{}'", args[0]); return; }
                1 => matches[0].pid,
                n => {
                    println!("kill: '{}' is ambiguous, {} matches — specify PID:", args[0], n);
                    for p in matches.iter().take(20) {
                        println!("  {:>5}  {}  {}", p.pid, p.state.symbol(), p.name);
                    }
                    if n > 20 { println!("  ({} more)", n - 20); }
                    return;
                }
            }
        }
    };

    match ProcessController::transition(target_pid, state) {
        Ok(()) => println!("pid {} → {} ({})", target_pid, state.symbol(), state.label()),
        Err(e) => println!("kill: {}", e),
    }
}

/// Built-in `ps [filter] [--all]` that surfaces the scheduler crate's ternary
/// process states. Reads /proc, annotates each process as Active/Dormant/
/// Suppressed using the same heuristic the doctrine maps to +1/0/-1.
///
/// `filter`: optional case-insensitive substring matched against process name.
/// `--all` :  show all matches instead of the default top-40-by-RSS.
fn print_ternary_ps(args: &[&str]) {
    use scheduler::ProcessController;

    let mut filter: Option<String> = None;
    let mut show_all = false;
    for a in args {
        if *a == "--all" || *a == "-a" { show_all = true; }
        else if !a.starts_with('-')    { filter = Some(a.to_lowercase()); }
    }

    let mut procs = ProcessController::list_processes();
    if let Some(ref needle) = filter {
        procs.retain(|p| p.name.to_lowercase().contains(needle));
    }
    procs.sort_by_key(|p| (-(p.vmrss_kb as i64), p.pid));

    let limit = if show_all { procs.len() } else { 40 };
    let shown = procs.len().min(limit);

    // PID width: linux PIDs comfortably exceed 100k; reserve 7 cols so the
    // ternary state column stays aligned even with 6- and 7-digit PIDs.
    println!("    PID  ST  NAME             VmRSS (KiB)");
    println!("  -----  --  ---------------- -----------");
    for p in procs.iter().take(limit) {
        let name = if p.name.len() > 16 { &p.name[..16] } else { &p.name };
        println!("  {:>5}  {}  {:<16} {:>11}",
                 p.pid, p.state.symbol(), name, p.vmrss_kb);
    }

    if procs.len() > shown {
        println!("  ({} more — pass --all to see them)", procs.len() - shown);
    }
    if let Some(needle) = filter {
        println!("  [filter: '{}', {} matches]", needle, procs.len());
    }
}
