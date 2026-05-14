//! Generate puzzles with `constrain` / `minimize` and verify each has exactly
//! one solution.
//!
//! Usage:
//!   cargo run --release --bin generate_verify -- --count 200 [--verbose]

use std::process::ExitCode;

const SEED_SOLUTION: &str =
    "652483917978162435314975628825736149791824563436519872269348751547291386183657294";

const EMPTY_PUZZLE: &str =
    ".................................................................................";

fn main() -> ExitCode {
    let mut count: usize = 1000;
    let mut verbose = false;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--count" => {
                count = args
                    .next()
                    .expect("--count requires a value")
                    .parse()
                    .expect("--count must be a positive integer");
            }
            "--verbose" => verbose = true,
            "-h" | "--help" => {
                eprintln!("Usage: generate_verify --count N [--verbose]");
                eprintln!();
                eprintln!("Generates N puzzles using constrain + minimize and verifies");
                eprintln!("each has exactly one solution.  Exits non-zero on failure.");
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("Unknown flag: {other}");
                return ExitCode::FAILURE;
            }
        }
    }

    let mut errors = 0usize;

    for i in 0..count {
        // ── constrain: empty puzzle → unique puzzle ──────────────────────────
        let mut puzzle = EMPTY_PUZZLE.to_string();
        if rdoku::constrain(false, &mut puzzle) {
            let (cnt, sol, _) = rdoku::solve_sudoku(&puzzle, 1, 1);
            if cnt != 1 {
                eprintln!("[{i}] FAIL constrain: {cnt} solutions (expected 1)\n  puzzle: {puzzle}");
                errors += 1;
            } else if verbose {
                eprintln!("[{i}] constrain:\n  puzzle:   {puzzle}\n  solution: {sol}");
            }
        }

        // ── minimize: solved grid → minimal unique puzzle ────────────────────
        let mut puzzle = SEED_SOLUTION.to_string();
        let initial_clues = 81usize;
        if rdoku::minimize(false, false, &mut puzzle) {
            let (cnt, sol, _) = rdoku::solve_sudoku(&puzzle, 1, 1);
            let final_clues = puzzle.bytes().filter(|&b| b != b'.').count();
            if cnt != 1 {
                eprintln!("[{i}] FAIL minimize: {cnt} solutions (expected 1)\n  puzzle: {puzzle}");
                errors += 1;
            } else if final_clues > initial_clues {
                eprintln!("[{i}] FAIL minimize: gained clues ({initial_clues} → {final_clues})");
                errors += 1;
            } else if verbose {
                eprintln!(
                    "[{i}] minimize ({initial_clues} → {final_clues} clues):\n  puzzle:   {puzzle}\n  solution: {sol}"
                );
            }
        }
    }

    if errors > 0 {
        eprintln!("\n{errors} failure(s) in {count} iterations");
        ExitCode::FAILURE
    } else {
        if !verbose {
            eprintln!("{count} puzzles verified OK");
        }
        ExitCode::SUCCESS
    }
}
