//! AFL++ fuzz harness for the `solve` binary.
//!
//! Feeds arbitrary byte sequences as CLI flags + puzzle data to `solve`,
//! verifying that it never panics, hangs, or produces garbage output.
//!
//! Profiles are split into two groups:
//! - **Normal** (0–11): sanitised puzzle strings + known flag combinations.
//! - **Chaos** (12–17): raw fuzz bytes passed directly as argument values
//!   and puzzle data — null bytes, unicode, shell metacharacters, etc.
//!
//! Usage: `cargo afl build && cargo afl fuzz -i corpus -o output -- target/debug/afl_solve`

use rdoku_fuzz_afl::harness_util::*;

const N_NORMAL: usize = 12;
const N_CHAOS: usize = 6;
const N_PROFILES: usize = N_NORMAL + N_CHAOS;

fn bytes_to_arg(data: &[u8]) -> String {
    String::from_utf8_lossy(data).into_owned()
}

fn main() {
    let bin = bin_path("solve");

    afl::fuzz!(|data: &[u8]| {
        if data.is_empty() {
            return;
        }
        let profile = (data[0] as usize) % N_PROFILES;
        let payload = &data[1..];

        // ── chaos profiles: raw bytes as CLI args / puzzle data ──────
        if profile >= N_NORMAL {
            let raw = bytes_to_arg(payload);
            let (args, stdin_data) = match profile - N_NORMAL {
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
            let result = run_binary(&bin, &args_refs, &stdin_data, 5, data);
            crash_if_bad(&result, true, true);
            return;
        }

        // ── normal profiles ──────────────────────────────────────────
        // File-based input (profile 11) handled separately.
        if profile == 11 {
            let puzzle = bytes_to_puzzle(payload, PuzzleVariant::Vanilla);
            let path = write_temp_file(&puzzle);
            let result = run_binary(&bin, &[&path], "", 5, data);
            std::fs::remove_file(&path).ok();
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
            _ => (vec!["-s", "nonexistent"], PuzzleVariant::Vanilla),
        };

        let puzzle = bytes_to_puzzle(payload, variant);
        let result = run_binary(&bin, &args, &puzzle, 5, data);
        crash_if_bad(&result, true, true);
    });
}
