use std::io::{self, Write};
use ternary_core::{Tryte, Trit};
use scheduler::{ProcessController, TernaryState};
use ai_runtime::{TernaryLinear, TernaryTensor};

fn main() {
    println!("🐧 Rusty Penguin Shell (psh) v0.1.0");
    println!("'Binary hardware. Ternary mind.'");
    println!("Type 'help' for commands.");

    loop {
        print!("psh> ");
        io::stdout().flush().unwrap();

        let mut input = String::new();
        io::stdin().read_line(&mut input).unwrap();
        let input = input.trim();

        if input.is_empty() {
            continue;
        }

        let parts: Vec<&str> = input.split_whitespace().collect();
        let command = parts[0];

        match command {
            "help" => {
                println!("Available commands:");
                println!("  help              - Show this help");
                println!("  trit <int>        - Convert binary integer to ternary Tryte");
                println!("  ps                - List processes (Mock view)");
                println!("  activate <pid>    - Move process to ACTIVE (+1)");
                println!("  dormant <pid>     - Move process to DORMANT (0)");
                println!("  suppress <pid>    - Move process to SUPPRESSED (-1)");
                println!("  ai <neurons>      - Run a mock ternary inference");
                println!("  exit | quit       - Exit the shell");
            }
            "trit" => {
                if parts.len() < 2 {
                    println!("Usage: trit <int>");
                    continue;
                }
                if let Ok(val) = parts[1].parse::<i32>() {
                    let tryte = Tryte::from_i32(val);
                    println!("Binary: {}", val);
                    println!("Ternary: {:?}", tryte);
                    println!("Value: {}", tryte.to_i32());
                } else {
                    println!("Invalid integer: {}", parts[1]);
                }
            }
            "ai" => {
                let n = if parts.len() > 1 {
                    parts[1].parse::<usize>().unwrap_or(10)
                } else {
                    10
                };
                
                println!("Initializing Ternary AI Layer ({} neurons)...", n);
                let mut layer = TernaryLinear::new(n, n);
                // Fill with some pattern
                for i in 0..(n*n) {
                    layer.weights.data[i] = if i % 3 == 0 { Trit::Pos } else if i % 3 == 1 { Trit::Neg } else { Trit::Zero };
                }

                let input = TernaryTensor::new(vec![Trit::Pos; n], vec![n]);
                let (output, total, skipped) = layer.forward(&input);
                
                let efficiency = (skipped as f64 / total as f64) * 100.0;
                println!("Inference Complete.");
                println!("Output: {:?}", output.data);
                println!("Total Ops: {}", total);
                println!("Skipped Ops: {} (Dormant)", skipped);
                println!("Sparsity Efficiency: {:.2}%", efficiency);
            }
            "ps" => {
                println!("{:<8} {:<10} {:<10}", "PID", "NAME", "STATE");
                println!("{:<8} {:<10} {:<10}", "1", "kernel", "ACTIVE (+1)");
                println!("{:<8} {:<10} {:<10}", "102", "psh", "ACTIVE (+1)");
                println!("{:<8} {:<10} {:<10}", "404", "dreamer", "DORMANT (0)");
            }
            "activate" | "dormant" | "suppress" => {
                if parts.len() < 2 {
                    println!("Usage: {} <pid>", command);
                    continue;
                }
                if let Ok(pid) = parts[1].parse::<i32>() {
                    let state = match command {
                        "activate" => TernaryState::Active,
                        "dormant" => TernaryState::Dormant,
                        "suppress" => TernaryState::Suppressed,
                        _ => unreachable!(),
                    };
                    match ProcessController::transition(pid, state) {
                        Ok(_) => println!("Process {} transitioned to {:?}", pid, state),
                        Err(e) => println!("Error: {}", e),
                    }
                } else {
                    println!("Invalid PID: {}", parts[1]);
                }
            }
            "exit" | "quit" => {
                println!("Goodbye! 🐧");
                break;
            }
            _ => {
                println!("Unknown command: {}", command);
            }
        }
    }
}
