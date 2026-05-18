//! Decode an AFL++ fuzz input file and print what command it represents.
//!
//! Usage: cargo run --release --bin decode_fuzz_input -- <input_file>
//! Example: cargo run --release --bin decode_fuzz_input -- fuzz-afl/output/afl_solve/default/crashes/id:000000,sig:06,...

use std::env;
use std::fs;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <input_file>", args[0]);
        eprintln!("Example: {} fuzz-afl/output/afl_solve/default/crashes/id:000000", args[0]);
        std::process::exit(1);
    }

    let input_path = &args[1];
    let data = match fs::read(input_path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error reading {}: {}", input_path, e);
            std::process::exit(1);
        }
    };

    // Extract target from path (e.g., "afl_solve" from path containing "fuzz-afl/output/afl_solve/...")
    let target = if input_path.contains("afl_solve") {
        "afl_solve"
    } else if input_path.contains("afl_generate") {
        "afl_generate"
    } else {
        "unknown"
    };

    println!("Input file: {}", input_path);
    println!("Target: {}", target);
    println!("File size: {} bytes", data.len());
    println!();

    if data.len() < 2 {
        println!("Input too short (< 2 bytes), no profile selection possible");
        return;
    }

    // Decode profile selection (first 2 bytes)
    let sel = ((data[0] as usize) << 8) | (data[1] as usize);
    let n_profiles = if target == "afl_solve" { 20 } else { 20 };
    let profile = sel % n_profiles;
    let payload = &data[2..];

    // Print hex dump of first 32 bytes
    println!("Raw bytes (first 32): {}", 
        data.iter()
            .take(32)
            .map(|b| format!("{:02x}", b))
            .collect::<Vec<_>>()
            .join(" ")
    );
    println!();

    // Decode based on target
    match target {
        "afl_solve" => decode_solve_profile(profile, payload),
        "afl_generate" => decode_generate_profile(profile, payload),
        _ => println!("Unknown target, cannot decode"),
    }
}

fn decode_solve_profile(profile: usize, payload: &[u8]) {
    const N_NORMAL: usize = 14;

    println!("Profile: {} ({})", profile, if profile >= N_NORMAL { "chaos" } else { "normal" });
    println!();

    match profile {
        // Normal profiles
        0 => println!("CLI: solve (no args, vanilla puzzle)"),
        1 => println!("CLI: solve -l 2"),
        2 => println!("CLI: solve -c"),
        3 => println!("CLI: solve -p (pencilmark puzzle)"),
        4 => println!("CLI: solve -l 0"),
        5 => println!("CLI: solve -s basic"),
        6 => println!("CLI: solve -s scc"),
        7 => println!("CLI: solve --pretty --stats"),
        8 => println!("CLI: solve -l 2 -c -p (pencilmark)"),
        9 => println!("CLI: solve -l 99999999999999999999"),
        10 => println!("CLI: solve -s nonexistent"),
        11 => println!("CLI: solve <file_path> (file-based input)"),
        12 => println!("CLI: solve (multi-puzzle stdin, 2-5 puzzles)"),
        13 => println!("CLI: solve -l 2 (multi-puzzle stdin, 2-5 puzzles)"),
        
        // Chaos profiles
        14 => println!("CLI: solve (chaos: raw bytes as vanilla puzzle)"),
        15 => println!("CLI: solve -l <raw_bytes> (chaos)"),
        16 => println!("CLI: solve -s <raw_bytes> (chaos)"),
        17 => println!("CLI: solve <raw_bytes> -p <raw_bytes> (chaos: unrecognized flags)"),
        18 => println!("CLI: solve (chaos: raw bytes directly as puzzle, no encoding)"),
        19 => println!("CLI: solve -p (chaos: very long raw puzzle, >729 bytes)"),
        _ => println!("Profile {} out of range", profile),
    }

    println!();
    if profile < 11 || profile >= 14 {
        // Print decoded puzzle
        let puzzle = bytes_to_puzzle(payload, false);
        println!("Stdin (vanilla puzzle): {}", &puzzle[..puzzle.len().min(100)]);
        if puzzle.len() > 100 {
            println!("  ... ({} chars total)", puzzle.len());
        }
    }
}

fn decode_generate_profile(profile: usize, payload: &[u8]) {
    const N_NORMAL: usize = 12;

    println!("Profile: {} ({})", profile, if profile >= N_NORMAL { "chaos" } else { "normal" });
    println!();

    match profile {
        // Normal profiles — sanitised numeric parameters
        0..=11 => {
            let b = |i: usize| payload.get(i).copied().unwrap_or(0);
            let pool_size = (b(1) as usize % 40) + 1;
            let num_evals = b(2) as usize % 15;
            let clues_to_drop = (b(3) as usize % 12) + 1;
            let clue_weight = (b(4) as f64 / 255.0) * 5.0;

            println!("CLI: generate [args] (profile {})", profile);
            println!("  pool_size: {}", pool_size);
            println!("  num_evals: {}", num_evals);
            println!("  clues_to_drop: {}", clues_to_drop);
            println!("  clue_weight: {:.2}", clue_weight);
        }
        
        // Chaos profiles
        12..=19 => {
            println!("CLI: generate (chaos profile {}: raw bytes as flag values)", profile - N_NORMAL);
        }
        _ => println!("Profile {} out of range", profile),
    }
}

fn bytes_to_puzzle(data: &[u8], _pencilmark: bool) -> String {
    let mut buf = [b'.'; 81];
    for i in 0..81 {
        let b = data.get(i).copied().unwrap_or(0);
        if b & 1 == 1 {
            buf[i] = b'1' + ((b >> 1) % 9);
        }
    }
    String::from_utf8(buf.to_vec()).unwrap_or_else(|_| "(invalid utf8)".to_string())
}
