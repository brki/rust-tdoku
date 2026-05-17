//! AFL++ fuzz harness for the `debug_solver` binary.
//!
//! Feeds arbitrary byte sequences as positional puzzle + limit arguments,
//! verifying that `debug_solver` never panics, hangs, or produces garbage output.
//!
//! Profiles 0–3 are normal (sanitised puzzle strings), profiles 4–5 are chaos
//! (raw bytes as puzzle, null bytes, unicode, shell metacharacters).
//!
//! Usage: `cargo afl build && cargo afl fuzz -i corpus -o output -- target/debug/afl_debug_solver`

use rdoku_fuzz_afl::harness_util::*;

const N_NORMAL: usize = 4;
const N_CHAOS: usize = 2;
const N_PROFILES: usize = N_NORMAL + N_CHAOS;

fn bytes_to_arg(data: &[u8]) -> String {
    String::from_utf8_lossy(data).into_owned()
}

fn main() {
    let bin = bin_path("debug_solver");

    afl::fuzz!(|data: &[u8]| {
        if data.is_empty() {
            return;
        }
        let profile = (data[0] as usize) % N_PROFILES;
        let payload = &data[1..];

        // ── chaos profiles ────────────────────────────────────────────
        if profile >= N_NORMAL {
            let raw = bytes_to_arg(payload);
            match profile - N_NORMAL {
                // Raw bytes as puzzle (nulls, unicode, control chars).
                0 => {
                    let result = run_binary(&bin, &[&raw], "", 5, data);
                    crash_if_bad(&result, true, true);
                }
                // Raw bytes as puzzle + raw bytes as limit.
                _ => {
                    let result = run_binary(&bin, &[&raw, &raw], "", 5, data);
                    crash_if_bad(&result, true, true);
                }
            }
            return;
        }

        match profile {
            0 => {
                let puzzle = bytes_to_puzzle(payload, PuzzleVariant::Vanilla);
                let result = run_binary(&bin, &[&puzzle], "", 5, data);
                crash_if_bad(&result, true, true);
            }
            1 => {
                let puzzle = bytes_to_puzzle(payload, PuzzleVariant::Vanilla);
                let limit = ((payload.len().max(1) as u64) % 10 + 1).to_string();
                let result = run_binary(&bin, &[&puzzle, &limit], "", 5, data);
                crash_if_bad(&result, true, true);
            }
            2 => {
                let puzzle = bytes_to_puzzle(payload, PuzzleVariant::Pencilmark);
                let result = run_binary(&bin, &[&puzzle], "", 5, data);
                crash_if_bad(&result, true, true);
            }
            _ => {
                let result = run_binary(&bin, &["--help"], "", 5, data);
                crash_if_bad(&result, true, true);
                if !result.success {
                    std::process::abort();
                }
            }
        }
    });
}
