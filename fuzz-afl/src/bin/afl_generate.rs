//! AFL++ fuzz harness for the `generate` binary.
//!
//! Feeds arbitrary byte sequences as CLI flags to `generate`, verifying that
//! it never panics, hangs, or produces garbage output.
//!
//! Profiles are split into two groups:
//! - **Normal** (0–11): fuzz bytes derive sanitised numeric parameters so
//!   AFL++ can explore the legitimate parameter space efficiently.
//! - **Chaos** (12–17): raw fuzz bytes are passed directly as argument values
//!   — null bytes, unicode, negative numbers, shell metacharacters, etc.
//!   These verify that `generate` rejects or handles malformed input cleanly.
//!
//! `generate` does **not** read stdin — it either generates puzzles from
//! scratch or loads seed puzzles from a pattern file given on the command line.
//!
//! Usage: `cargo afl build && cargo afl fuzz -i corpus -o output -- target/debug/afl_generate`

use rdoku_fuzz_afl::harness_util::*;
use std::io::Write;

const N_NORMAL: usize = 12;
const N_CHAOS: usize = 8;
const N_PROFILES: usize = N_NORMAL + N_CHAOS;

/// Convert arbitrary bytes to a lossy `String` for use as a CLI argument.
/// Non-UTF-8 sequences become `�`; null bytes and control chars are kept.
fn bytes_to_arg(data: &[u8]) -> String {
    String::from_utf8_lossy(data).into_owned()
}

/// Write raw bytes to a temp file (nulls, non-UTF-8, etc. — no conversion).
fn write_temp_file_raw(data: &[u8]) -> String {
    let path = format!("/tmp/rdoku_afl_input_{}.txt", std::process::id());
    let mut f = std::fs::File::create(&path).unwrap_or_else(|e| {
        eprintln!("AFL_HARNESS: cannot create temp file {}: {}", path, e);
        std::process::abort();
    });
    f.write_all(data).unwrap_or_else(|e| {
        eprintln!("AFL_HARNESS: cannot write temp file {}: {}", path, e);
        std::process::abort();
    });
    f.flush().ok();
    path
}

fn main() {
    let bin = bin_path("generate");

    afl::fuzz!(|data: &[u8]| {
        if data.is_empty() {
            return;
        }
        let profile = (data[0] as usize) % N_PROFILES;

        // ── chaos profiles: raw bytes as argument values ─────────────
        if profile >= N_NORMAL {
            let raw = bytes_to_arg(&data[1..]);
            let args = match profile - N_NORMAL {
                // Inject raw bytes as -l value.
                0 => vec!["-p", "0", "-l", &raw],
                // Inject raw bytes as -n value.
                1 => vec!["-p", "0", "-l", "1", "-n", &raw],
                // Inject raw bytes as every numeric flag.
                2 => vec![
                    "-p", &raw, "-l", &raw, "-n", &raw,
                    "-e", &raw, "-d", &raw, "-c", &raw,
                    "-g", &raw, "-r", &raw,
                ],
                // Raw bytes as boolean flags (unrecognized flags).
                3 => vec![&raw, "-p", "0", "-l", "1"],
                // Negative / large / weird numbers.
                4 => {
                    let neg = format!("-{}", raw);
                    let a: Vec<String> = vec![
                        "-p".into(), "0".into(), "-l".into(), neg.clone(),
                        "-n".into(), neg.clone(), "-e".into(), neg,
                    ];
                    let a_refs: Vec<&str> = a.iter().map(|s| s.as_str()).collect();
                    let result = run_binary(&bin, &a_refs, "", 8, data);
                    crash_if_bad(&result, true, true);
                    return;
                }
                // ── pattern-file chaos ───────────────────────────────
                // Raw bytes as pattern file (lossy UTF-8, any length).
                5 => {
                    let path = write_temp_file(&raw);
                    let args: Vec<&str> = vec!["-p", "0", "-l", "1", &path];
                    let result = run_binary(&bin, &args, "", 8, data);
                    std::fs::remove_file(&path).ok();
                    crash_if_bad(&result, true, true);
                    return;
                }
                // Pattern file with deliberately weird lengths / content.
                6 => {
                    let payload = &data[1..];
                    let content = match data.get(1).map(|b| b % 6) {
                        Some(0) => "0".to_string(),                      // single "0"
                        Some(1) => "a".repeat(40),                       // < 81 ascii
                        Some(2) => bytes_to_puzzle(payload, PuzzleVariant::Vanilla), // exactly 81
                        Some(3) => "漢字パズル".repeat(50),               // unicode
                        Some(4) => "ABCDEFGHIJKLMNOPQRSTUVWXYZ".repeat(10), // ascii letters
                        _ => ".................................................................................".to_string(), // all dots (81)
                    };
                    let path = write_temp_file(&content);
                    let args: Vec<&str> = vec!["-p", "0", "-l", "1", &path];
                    let result = run_binary(&bin, &args, "", 8, data);
                    std::fs::remove_file(&path).ok();
                    crash_if_bad(&result, true, true);
                    return;
                }
                // Raw binary bytes as pattern file (nulls, non-UTF-8).
                _ => {
                    let path = write_temp_file_raw(&data[1..]);
                    let args: Vec<&str> = vec!["-p", "0", "-l", "1", &path];
                    let result = run_binary(&bin, &args, "", 8, data);
                    std::fs::remove_file(&path).ok();
                    crash_if_bad(&result, true, true);
                    return;
                }
            };
            let args_refs: Vec<&str> = args.iter().map(|s| *s).collect();
            let result = run_binary(&bin, &args_refs, "", 8, data);
            crash_if_bad(&result, true, true);
            return;
        }

        // ── normal profiles: sanitised numeric parameters ────────────
        let b = |i: usize| data.get(i).copied().unwrap_or(0);

        let pool_size     = (b(1) as usize % 40) + 1;        // 1..=40
        let num_evals     = b(2) as usize % 15;               // 0..=14
        let clues_to_drop = (b(3) as usize % 12) + 1;         // 1..=12
        let clue_weight   = (b(4) as f64 / 255.0) * 5.0;      // 0.0..5.0
        let guess_weight  = (b(5) as f64 / 255.0) * 3.0;      // 0.0..3.0
        let random_weight = (b(6) as f64 / 255.0) * 5.0;      // 0.0..5.0
        let limit         = (b(7) as usize % 6) + 1;          // 1..=6

        // ── pattern file profile ────────────────────────────────────
        if profile == 11 {
            let puzzle = bytes_to_puzzle(&data[1..], PuzzleVariant::Vanilla);
            let path = write_temp_file(&format!("# pattern file\n{puzzle}\n"));
            let args: Vec<String> = vec![
                "-p".into(), "0".into(),
                "-l".into(), limit.to_string(),
                "-n".into(), pool_size.to_string(),
                "-e".into(), num_evals.to_string(),
            ];
            let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            let result = run_binary_with_file(&bin, &args_refs, &path, "", 10, data);
            std::fs::remove_file(&path).ok();
            crash_if_bad(&result, true, true);
            return;
        }

        // ── build args with fuzz-derived numeric values ───────────────
        let mut args: Vec<String> = Vec::new();

        let pencilmark = (b(8) & 1) == 1;
        args.push("-p".into());
        args.push(if pencilmark { "1" } else { "0" }.into());

        args.push("-l".into());
        args.push(limit.to_string());

        args.push("-n".into());
        args.push(pool_size.to_string());
        args.push("-e".into());
        args.push(num_evals.to_string());

        match profile {
            0 => {}
            1 => { args.push("-d".into()); args.push(clues_to_drop.to_string()); }
            2 => {
                args.push("-c".into()); args.push(format!("{:.1}", clue_weight * 0.4));
                args.push("-g".into()); args.push(format!("{:.1}", guess_weight + 1.0));
            }
            3 => {
                args.push("-c".into()); args.push(format!("{:.1}", clue_weight + 1.0));
                args.push("-g".into()); args.push(format!("{:.1}", guess_weight * 0.5));
            }
            4 => { args.push("--json".into()); args.push("--pretty".into()); }
            5 => { args.push("--solution".into()); }
            6 => { args.push("-m".into()); args.push("0".into()); }
            7 => { args.push("-a".into()); args.push("1".into()); }
            8 => { args.push("-r".into()); args.push(format!("{:.1}", random_weight)); }
            9 => {
                args.push("-d".into()); args.push(clues_to_drop.to_string());
                args.push("-c".into()); args.push(format!("{:.1}", clue_weight));
                args.push("-g".into()); args.push(format!("{:.1}", guess_weight));
                args.push("-r".into()); args.push(format!("{:.1}", random_weight));
                args.push("-a".into());
                args.push(if (b(9) & 1) == 1 { "1" } else { "0" }.into());
            }
            _ => {
                args.push("--skip".into());
                args.push((limit + 5).to_string());
            }
        }

        let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let result = run_binary(&bin, &args_refs, "", 10, data);
        crash_if_bad(&result, true, true);
    });
}

/// Variant of run_binary that passes a file as a positional argument.
fn run_binary_with_file(
    bin: &str,
    base_args: &[&str],
    file_path: &str,
    stdin_str: &str,
    timeout_secs: u64,
    raw_data: &[u8],
) -> BinaryResult {
    let mut all_args: Vec<&str> = base_args.to_vec();
    all_args.push(file_path);
    run_binary(bin, &all_args, stdin_str, timeout_secs, raw_data)
}
