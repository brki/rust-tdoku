//! Core fuzz logic for the `generate` harness.
//!
//! Extracted into a module so it can be shared between the AFL-instrumented
//! harness (`afl_generate`) and the standalone replay binary (`replay_afl_generate`).

use crate::harness_util::*;

const N_NORMAL: usize = 12;
const N_CHAOS: usize = 8;
const N_PROFILES: usize = N_NORMAL + N_CHAOS;

pub fn process(data: &[u8]) {
    let bin = bin_path("generate");

    if data.len() < 2 {
        return;
    }
    // Use two bytes for profile selection so AFL++ bit-flipping crosses
    // profile boundaries uniformly for any N_PROFILES value.
    let sel = (data[0] as usize) << 8 | (data[1] as usize);
    let profile = sel % N_PROFILES;

    // ── chaos profiles: raw bytes as argument values ─────────────
    if profile >= N_NORMAL {
        let raw = bytes_to_arg(&data[2..]);
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
                let result = run_binary(&bin, &a_refs, "", 60, data);
                crash_if_bad(&result, true, true);
                return;
            }
            // ── pattern-file chaos ──────────────────────────────────
            // Raw bytes as pattern file (lossy UTF-8, any length).
            5 => {
                let temp = write_temp_file(&raw);
                let args: Vec<&str> = vec!["-p", "0", "-l", "1", temp.path()];
                let result = run_binary(&bin, &args, "", 60, data);
                crash_if_bad(&result, true, true);
                return;
            }
            // Pattern file with deliberately weird lengths / content.
            6 => {
                let payload = &data[2..];
                let content = match data.get(2).map(|b| b % 6) {
                    Some(0) => "0".to_string(),
                    Some(1) => "a".repeat(40),
                    Some(2) => bytes_to_puzzle(payload, PuzzleVariant::Vanilla),
                    Some(3) => "漢字パズル".repeat(50),
                    Some(4) => "ABCDEFGHIJKLMNOPQRSTUVWXYZ".repeat(10),
                    _ => ".".repeat(81),
                };
                let temp = write_temp_file(&content);
                let args: Vec<&str> = vec!["-p", "0", "-l", "1", temp.path()];
                let result = run_binary(&bin, &args, "", 60, data);
                crash_if_bad(&result, true, true);
                return;
            }
            // Raw binary bytes as pattern file (nulls, non-UTF-8).
            _ => {
                let temp = write_temp_file_raw(&data[2..]);
                let args: Vec<&str> = vec!["-p", "0", "-l", "1", temp.path()];
                let result = run_binary(&bin, &args, "", 60, data);
                crash_if_bad(&result, true, true);
                return;
            }
        };
        let args_refs: Vec<&str> = args.iter().map(|s| *s).collect();
        let result = run_binary(&bin, &args_refs, "", 60, data);
        crash_if_bad(&result, true, true);
        return;
    }

    // ── normal profiles: sanitised numeric parameters ────────────
    // data[0..2] = profile selector; data[2..] = parameter bytes.
    // The `b(i)` closure accesses parameter bytes starting at data[2].
    let b = |i: usize| data.get(i + 2).copied().unwrap_or(0);

    let pool_size     = (b(1) as usize % 40) + 1;        // 1..=40
    let num_evals     = b(2) as usize % 15;               // 0..=14
    let clues_to_drop = (b(3) as usize % 12) + 1;         // 1..=12
    let clue_weight   = (b(4) as f64 / 255.0) * 5.0;      // 0.0..5.0
    let guess_weight  = (b(5) as f64 / 255.0) * 3.0;      // 0.0..3.0
    let random_weight = (b(6) as f64 / 255.0) * 5.0;      // 0.0..5.0
    let limit         = (b(7) as usize % 6) + 1;          // 1..=6

    // ── pattern file profile ─────────────────────────────────────
    if profile == 11 {
        let puzzle = bytes_to_puzzle(&data[2..], PuzzleVariant::Vanilla);
        let temp = write_temp_file(&format!("# pattern file\n{puzzle}\n"));
        let args: Vec<String> = vec![
            "-p".into(), "0".into(),
            "-l".into(), limit.to_string(),
            "-n".into(), pool_size.to_string(),
            "-e".into(), num_evals.to_string(),
        ];
        let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let result = run_binary_with_file(&bin, &args_refs, temp.path(), "", 10, data);
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
}
