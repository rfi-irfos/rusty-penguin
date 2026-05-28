use std::io::{self, Write};
use ternary_core::{Tryte, Trit};
use scheduler::{ProcessController, TernaryState};
use ai_runtime::{TernaryLinear, TernaryTensor};
use mathematics::{mul_tryte, div_tryte, scale};

mod pkg;

fn to_tern(mut n: i64) -> String {
    if n == 0 { return "0".to_string(); }
    let flip = n < 0;
    if n < 0 { n = -n; }
    let mut digits = [0i8; 40];
    let mut len = 0;
    let mut v = n;
    while v != 0 {
        let rem = (v % 3) as i8; v /= 3;
        if rem == 2 { digits[len] = -1; v += 1; } else { digits[len] = rem; }
        len += 1;
    }
    let mut s = String::new();
    for k in 0..len {
        let d = if flip { -digits[len-1-k] } else { digits[len-1-k] };
        s.push(match d { 1 => '+', -1 => '-', _ => '0' });
    }
    s
}

fn lcg(s: u64) -> u64 {
    s.wrapping_mul(6_364_136_223_846_793_005)
     .wrapping_add(1_442_695_040_888_963_407)
}

fn seed_trits(buf: &mut Vec<Trit>, seed: u64) {
    let mut s = seed;
    for t in buf.iter_mut() {
        s = lcg(s);
        *t = match (s >> 33) % 3 { 0 => Trit::Neg, 1 => Trit::Zero, _ => Trit::Pos };
    }
}

fn run_ai_inference(n_tokens: usize) {
    const DIM: usize = 8;
    const LAYERS: usize = 4;

    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(42)
        .wrapping_add(n_tokens as u64 * 7);

    println!("albert. [inference]");
    println!("sparse ternary inference -- {} layers x {} tokens\n", LAYERS, n_tokens);

    // Build weight matrices
    let mut w_all = vec![Trit::Zero; LAYERS * DIM * DIM];
    seed_trits(&mut w_all, seed ^ 0xCAFE_BABE_DEAD_BEEF_u64);

    let mut act = vec![Trit::Zero; DIM];
    seed_trits(&mut act, seed);

    // Show input
    let input_str: String = act.iter().map(|t| match t {
        Trit::Pos => '+', Trit::Zero => '0', Trit::Neg => '-',
    }).collect();
    println!("  input  [{}]", input_str);

    let mut total_total = 0usize;
    let mut total_skip  = 0usize;

    for l in 0..LAYERS {
        let in_act = act.clone();
        let w = &w_all[l * DIM * DIM..(l + 1) * DIM * DIM];

        let mut layer = TernaryLinear::new(DIM, DIM);
        layer.weights.data = w.to_vec();
        let inp_tensor = TernaryTensor::new(in_act.clone(), vec![DIM]);
        let (out_tensor, t, sk) = layer.forward(&inp_tensor);
        total_total += t;
        total_skip  += sk;
        let dorm = if t > 0 { sk * 100 / t } else { 0 };

        act = out_tensor.data.clone();

        let in_str:  String = in_act.iter().map(|t| match t { Trit::Pos=>'+', Trit::Zero=>'0', Trit::Neg=>'-' }).collect();
        let out_str: String = act.iter().map(|t| match t { Trit::Pos=>'+', Trit::Zero=>'0', Trit::Neg=>'-' }).collect();
        println!("  L{}     [{}] -> [{}]  dormancy {}%", l, in_str, out_str, dorm);
    }

    let avg_dorm = if total_total > 0 { total_skip * 100 / total_total } else { 0 };
    println!("\n{} tokens  avg dormancy {}%  skipped {}/{} ops", n_tokens, avg_dorm, total_skip, total_total);
    println!("ACTIVE -- Binary hardware. Ternary mind.");
}

fn main() {
    println!("Rusty Penguin Shell (psh) v0.1.0");
    println!("  \"Binary hardware. Ternary mind.\"");
    println!("  Type 'help' for commands.\n");

    loop {
        print!("psh> ");
        io::stdout().flush().unwrap();

        let mut input = String::new();
        if io::stdin().read_line(&mut input).unwrap_or(0) == 0 {
            break; // EOF
        }
        let input = input.trim();
        if input.is_empty() { continue; }

        let parts: Vec<&str> = input.split_whitespace().collect();
        let command = parts[0];

        match command {
            "help" => {
                println!("Commands:");
                println!("  trit <n>           Convert integer to balanced ternary");
                println!("  mul <a> <b>        Multiply two integers in balanced ternary");
                println!("  div <a> <b>        Divide a by b (quotient + remainder)");
                println!("  scale <n> <-1|0|1> Scale integer by a trit");
                println!("  ps                 List processes with ternary state");
                println!("  activate <pid>     SIGCONT → ACTIVE  (+1)");
                println!("  dormant  <pid>     SIGSTOP → DORMANT  (0)");
                println!("  suppress <pid>     SIGTERM → SUPPRESSED (-1)");
                println!("  ai [n]             Sparse ternary inference (n tokens, default 32)");
                println!("  rpm <cmd>          Package manager (install|list|info|remove|search)");
                println!("  exit | quit        Exit psh");
            }

            "trit" => {
                if let Some(val) = parts.get(1).and_then(|s| s.parse::<i64>().ok()) {
                    println!("{}  ({})", val, to_tern(val));
                } else {
                    println!("Usage: trit <integer>");
                }
            }

            "mul" => {
                let a = parts.get(1).and_then(|s| s.parse::<i32>().ok());
                let b = parts.get(2).and_then(|s| s.parse::<i32>().ok());
                match (a, b) {
                    (Some(a), Some(b)) => {
                        let (lo, hi) = mul_tryte(Tryte::from_i32(a), Tryte::from_i32(b));
                        let product = hi.to_i32() as i64 * 19683 + lo.to_i32() as i64;
                        println!("  {} * {} = {} (lo={} hi={})", a, b, product, lo.to_i32(), hi.to_i32());
                    }
                    _ => println!("Usage: mul <a> <b>"),
                }
            }

            "div" => {
                let a = parts.get(1).and_then(|s| s.parse::<i32>().ok());
                let b = parts.get(2).and_then(|s| s.parse::<i32>().ok());
                match (a, b) {
                    (Some(a), Some(b)) => {
                        let (q, r) = div_tryte(Tryte::from_i32(a), Tryte::from_i32(b));
                        println!("  {} / {} = {} remainder {}", a, b, q.to_i32(), r.to_i32());
                    }
                    _ => println!("Usage: div <a> <b>"),
                }
            }

            "scale" => {
                let n = parts.get(1).and_then(|s| s.parse::<i32>().ok());
                let t = parts.get(2).and_then(|s| s.parse::<i8>().ok());
                match (n, t) {
                    (Some(n), Some(t)) => {
                        let trit = match t {
                             1 => Trit::Pos,
                             0 => Trit::Zero,
                            -1 => Trit::Neg,
                             _ => { println!("Trit must be -1, 0, or 1"); continue; }
                        };
                        let result = scale(Tryte::from_i32(n), trit);
                        println!("  scale({}, {}) = {}", n, t, result.to_i32());
                    }
                    _ => println!("Usage: scale <integer> <-1|0|1>"),
                }
            }

            "ps" => {
                let procs = ProcessController::list_processes();
                println!("  {:<8} {:<18} {:<6} {:<10}", "PID", "NAME", "RSS(kb)", "STATE");
                println!("  {}", "-".repeat(48));
                for p in procs.iter().take(20) {
                    println!("  {:<8} {:<18} {:<6} ({}) {}",
                        p.pid,
                        &p.name[..p.name.len().min(17)],
                        p.vmrss_kb,
                        p.state.symbol(),
                        p.state.label(),
                    );
                }
                if procs.len() > 20 {
                    println!("  ... and {} more", procs.len() - 20);
                }
                println!("  Total: {} processes", procs.len());
            }

            "activate" | "dormant" | "suppress" => {
                let Some(pid) = parts.get(1).and_then(|s| s.parse::<i32>().ok()) else {
                    println!("Usage: {} <pid>", command);
                    continue;
                };
                let state = match command {
                    "activate" => TernaryState::Active,
                    "dormant"  => TernaryState::Dormant,
                    _          => TernaryState::Suppressed,
                };
                match ProcessController::transition(pid, state) {
                    Ok(_)  => println!("  pid {} -> ({}) {}", pid, state.symbol(), state.label()),
                    Err(e) => println!("  error: {}", e),
                }
            }

            "ai" => {
                let n_tokens = parts.get(1).and_then(|s| s.parse::<usize>().ok())
                    .unwrap_or(32).max(1).min(256);
                run_ai_inference(n_tokens);
            }

            "rpm" => {
                let args: Vec<&str> = parts.iter().skip(1).copied().collect();
                match pkg::cmd_rpm(&args) {
                    Ok(output) => println!("{}", output),
                    Err(err) => println!("rpm error: {}", err),
                }
            }

            "exit" | "quit" => {
                println!("Goodbye.");
                break;
            }

            _ => println!("Unknown command: {}  (type 'help')", command),
        }
    }
}
