//! Core fuzz logic for the `solve` harness.
//!
//! Extracted into a module so it can be shared between the AFL-instrumented
//! harness (`afl_solve`) and the standalone replay binary (`replay_afl_solve`).

use crate::harness_util::*;

const N_NORMAL: usize = 16;
const N_CHAOS: usize = 8;
const N_PROFILES: usize = N_NORMAL + N_CHAOS;

pub fn process(data: &[u8]) {
    let bin = bin_path("solve");

    if data.len() < 2 {
        return;
    }
    // Use two bytes for profile selection so AFL++ bit-flipping crosses
    // profile boundaries uniformly for any N_PROFILES value.
    let sel = (data[0] as usize) << 8 | (data[1] as usize);
    let profile = sel % N_PROFILES;
    let payload = &data[2..];

    // ── chaos profiles: raw bytes as CLI args / puzzle data ──────
    if profile >= N_NORMAL {
        let raw = bytes_to_arg(payload);
        let chaos_idx = profile - N_NORMAL;

        // --find-all profiles (6–7): use a shorter timeout because exhaustive
        // search can be very slow on puzzles with few givens.
        if chaos_idx >= 6 {
            let args: Vec<&str> = if chaos_idx == 6 {
                vec!["--find-all"]
            } else {
                vec!["--find-all", "--display-multiple-solutions"]
            };
            let stdin_data = bytes_to_puzzle(payload, PuzzleVariant::Vanilla);
            let result = run_binary(&bin, &args, &stdin_data, 10, data);
            crash_if_bad(&result, true, true);
            return;
        }

        let (args, stdin_data) = match chaos_idx {
            // Raw bytes as puzzle (vanilla-length).
            0 => (vec![], bytes_to_puzzle(payload, PuzzleVariant::Vanilla)),
            // Raw bytes as puzzle + raw bytes as -l value.
            1 => (vec!["-l", &raw], bytes_to_puzzle(payload, PuzzleVariant::Vanilla)),
            // Raw bytes as puzzle + raw bytes as -s value.
            2 => (vec!["-s", &raw], bytes_to_puzzle(payload, PuzzleVariant::Vanilla)),
            // Raw bytes as unrecognized flags.
            3 => (vec![&raw, "-p", &raw], bytes_to_puzzle(payload, PuzzleVariant::Vanilla)),
            // Raw bytes directly as puzzle data (non-UTF-8, nulls, etc.).
            4 => (vec![], raw),
            // Very long raw puzzle (>729 bytes, mixes vanilla/pencilmark).
            _ => {
                let long = raw.repeat(10);
                (vec!["-p"], long)
            }
        };
        let args_refs: Vec<&str> = args.iter().map(|s| *s).collect();
        let result = run_binary(&bin, &args_refs, &stdin_data, 60, data);
        crash_if_bad(&result, true, true);
        return;
    }

    // ── normal profiles ──────────────────────────────────────────
    // File-based input (profile 11).
    if profile == 11 {
        let puzzle = bytes_to_puzzle(payload, PuzzleVariant::Vanilla);
        let temp = write_temp_file(&puzzle);
        let result = run_binary(&bin, &[temp.path()], "", 60, data);
        crash_if_bad(&result, true, true);
        return;
    }

    // Multi-puzzle stdin (profiles 12–13): 2–5 puzzles per invocation.
    if profile == 12 || profile == 13 {
        let count = 2 + (payload.first().copied().unwrap_or(0) as usize % 4);
        let mut stdin_data = String::new();
        for i in 0..count {
            let start = if payload.is_empty() {
                0
            } else {
                (i * 81) % payload.len()
            };
            stdin_data.push_str(&bytes_to_puzzle(&payload[start..], PuzzleVariant::Vanilla));
            stdin_data.push('\n');
        }
        let args: &[&str] = if profile == 13 { &["-l", "2"] } else { &[] };
        let result = run_binary(&bin, args, &stdin_data, 60, data);
        crash_if_bad(&result, true, true);
        return;
    }

    let (args, variant): (Vec<&str>, PuzzleVariant) = match profile {
        0 => (vec![], PuzzleVariant::Vanilla),
        1 => (vec!["-l", "2"], PuzzleVariant::Vanilla),
        2 => (vec!["-c"], PuzzleVariant::Vanilla),
        3 => (vec!["-p"], PuzzleVariant::Pencilmark),
        4 => (vec!["-l", "0"], PuzzleVariant::Vanilla),
        5 => (vec!["-s", "basic"], PuzzleVariant::Vanilla),
        6 => (vec!["-s", "scc"], PuzzleVariant::Vanilla),
        7 => (vec!["--pretty", "--stats"], PuzzleVariant::Vanilla),
        8 => (vec!["-l", "2", "-c", "-p"], PuzzleVariant::Pencilmark),
        9 => (vec!["-l", "99999999999999999999"], PuzzleVariant::Vanilla),
        // -- display-multiple-solutions profiles --
        14 => (vec!["--display-multiple-solutions"], PuzzleVariant::Vanilla),
        15 => (vec!["--display-multiple-solutions", "-l", "2"], PuzzleVariant::Vanilla),
        _ => (vec!["-s", "nonexistent"], PuzzleVariant::Vanilla),
    };

    let puzzle = bytes_to_puzzle(payload, variant);
    let result = run_binary(&bin, &args, &puzzle, 60, data);
    crash_if_bad(&result, true, true);
}
