use std::io::{self, Write};
use ternary_core::{Tryte, Trit};
use scheduler::{ProcessController, TernaryState};
use ai_runtime::{TernaryLinear, TernaryTensor};
use mathematics::{mul_tryte, div_tryte, scale};

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
                println!("  trit <int>         Convert integer to balanced ternary Tryte");
                println!("  mul <a> <b>        Multiply two integers in balanced ternary");
                println!("  div <a> <b>        Divide a by b (quotient + remainder)");
                println!("  scale <n> <-1|0|1> Scale integer by a trit");
                println!("  ps                 List processes with ternary state");
                println!("  activate <pid>     SIGCONT → ACTIVE  (+1)");
                println!("  dormant  <pid>     SIGSTOP → DORMANT  (0)");
                println!("  suppress <pid>     SIGTERM → SUPPRESSED (-1)");
                println!("  ai [neurons]       Run sparse ternary inference layer");
                println!("  exit | quit        Exit psh");
            }

            "trit" => {
                if let Some(val) = parts.get(1).and_then(|s| s.parse::<i32>().ok()) {
                    let tryte = Tryte::from_i32(val);
                    let trits: Vec<i8> = tryte.trits().iter().map(|t| t.to_i8()).collect();
                    println!("  decimal : {}", val);
                    println!("  trits   : {:?}", trits);
                    println!("  verify  : {}", tryte.to_i32());
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
                let n = parts.get(1).and_then(|s| s.parse::<usize>().ok()).unwrap_or(12);
                println!("  Ternary inference layer: {}x{} weights", n, n);
                let mut layer = TernaryLinear::new(n, n);
                for i in 0..(n * n) {
                    layer.weights.data[i] = match i % 3 {
                        0 => Trit::Pos,
                        1 => Trit::Neg,
                        _ => Trit::Zero,
                    };
                }
                let inp = TernaryTensor::new(vec![Trit::Pos; n], vec![n]);
                let (out, total, skipped) = layer.forward(&inp);
                let out_vals: Vec<i8> = out.data.iter().map(|t| t.to_i8()).collect();
                println!("  output       : {:?}", out_vals);
                println!("  total ops    : {}", total);
                println!("  skipped ops  : {} (Zero-dormant)", skipped);
                println!("  sparsity     : {:.1}%", skipped as f64 / total as f64 * 100.0);
            }

            "exit" | "quit" => {
                println!("Goodbye.");
                break;
            }

            _ => println!("Unknown command: {}  (type 'help')", command),
        }
    }
}
