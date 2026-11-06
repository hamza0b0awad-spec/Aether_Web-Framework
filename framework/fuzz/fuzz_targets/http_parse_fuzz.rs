//! # HTTP parser fuzz runner
//!
//! A sustained, deterministic campaign over the parser entry points. See `src/lib.rs` for the
//! oracle and the mutation strategy, and for how to read the outcome-bucket count honestly.
//!
//! ```text
//! cargo run -p aether-fuzz --release -- --seconds 120 --seed 20261030
//! cargo run -p aether-fuzz --release -- --iterations 5000000 --no-heartbeat
//! ```
//!
//! Exit status: 0 when the campaign completed with no crash, 1 when an input panicked (the
//! input is saved under `fuzz/artifacts/`), 2 for a usage error.

use aether_fuzz::{Config, campaign};
use std::time::Duration;

/// `--help` is a request, not a mistake, so it answers on stdout and exits 0; every other
/// caller of this block is a usage error.
fn usage(code: i32) -> ! {
    let text = "usage: http_parse_fuzz [--seconds N] [--iterations N] [--seed N] \
         [--max-input-len N] [--no-heartbeat] [--help]\n\
         \n\
         --seconds N        wall-clock budget (default 60)\n\
         --iterations N     also stop after N executions (0 = clock only, the default)\n\
         --seed N           generator seed, decimal or 0x-prefixed (default 0x51eed1ce20261030)\n\
         --max-input-len N  cap on a mutated input (default 8192)\n\
         --no-heartbeat     do not print progress every 10 s";
    if code == 0 {
        println!("{text}");
    } else {
        eprintln!("{text}");
    }
    std::process::exit(code);
}

/// Unsigned flags accept decimal or `0x`-prefixed hex, so a seed copied from a register entry
/// reads the same either way.
fn unsigned(flag: &str, value: Option<&String>) -> u64 {
    let parsed = value.and_then(|v| match v.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16).ok(),
        None => v.parse::<u64>().ok(),
    });
    parsed.unwrap_or_else(|| {
        eprintln!("{flag} needs an unsigned number, got {value:?}");
        usage(2)
    })
}

fn seconds(flag: &str, value: Option<&String>) -> f64 {
    value
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s >= 0.0)
        .unwrap_or_else(|| {
            eprintln!("{flag} needs a non-negative number of seconds, got {value:?}");
            usage(2)
        })
}

fn main() {
    let mut config = Config::default();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--help" | "-h" => usage(0),
            "--seconds" => {
                index += 1;
                config.seconds = seconds("--seconds", args.get(index));
            }
            "--iterations" => {
                index += 1;
                config.max_iterations = unsigned("--iterations", args.get(index));
            }
            "--seed" => {
                index += 1;
                config.seed = unsigned("--seed", args.get(index));
            }
            "--max-input-len" => {
                index += 1;
                let len = unsigned("--max-input-len", args.get(index));
                config.max_input_len = usize::try_from(len).unwrap_or(usize::MAX).max(1);
            }
            "--no-heartbeat" => config.heartbeat = None,
            other => {
                eprintln!("unknown argument {other}");
                usage(2);
            }
        }
        index += 1;
    }
    if let Some(period) = config.heartbeat {
        config.heartbeat = Some(period.max(Duration::from_secs(1)));
    }

    let report = campaign(&config);
    report.print();
    std::process::exit(if report.crashes.is_empty() { 0 } else { 1 });
}
