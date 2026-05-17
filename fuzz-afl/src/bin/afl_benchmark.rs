//! AFL++ fuzz harness for the `benchmark` binary.
//!
//! Feeds arbitrary byte sequences as benchmark flags + puzzle file content,
//! verifying that `benchmark` never panics, hangs, or produces garbage output.
//!
//! Profiles 0–7 are normal, 8–10 are chaos (raw bytes as flag values: nulls,
//! unicode, negative numbers, shell metacharacters).
//!
//! Usage: `cargo afl build && cargo afl fuzz -i corpus -o output -- target/debug/afl_benchmark`

use rdoku_fuzz_afl::harness_util::*;

const N_NORMAL: usize = 8;
const N_CHAOS: usize = 3;
const N_PROFILES: usize = N_NORMAL + N_CHAOS;

fn bytes_to_arg(data: &[u8]) -> String {
    String::from_utf8_lossy(data).into_owned()
}

fn main() {
    let bin = bin_path("benchmark");

    afl::fuzz!(|data: &[u8]| {
        if data.is_empty() {
            return;
        }
        let profile = (data[0] as usize) % N_PROFILES;
        let payload = &data[1..];

        // Build a puzzle file from payload bytes.
        let mut puzzle_file = String::from("#ALLOWZERO\n");
        for chunk in payload.chunks(81) {
            puzzle_file.push_str(&bytes_to_puzzle(chunk, PuzzleVariant::Vanilla));
            puzzle_file.push('\n');
        }
        let path = write_temp_file(&puzzle_file);

        // ── chaos profiles: raw bytes as flag values ─────────────────
        if profile >= N_NORMAL {
            let raw = bytes_to_arg(payload);
            let args: Vec<&str> = match profile - N_NORMAL {
                0 => vec!["-n", &raw, "-w", "1", "-t", "1", &path],
                1 => vec!["-n", "10", "-w", &raw, "-t", &raw, &path],
                _ => vec![&raw, "-n", "10", "-w", "1", "-t", "1", &path],
            };
            let result = run_binary(&bin, &args, "", 5, data);
            std::fs::remove_file(&path).ok();
            crash_if_bad(&result, true, true);
            return;
        }

        let mut args = vec!["-n", "10", "-w", "1", "-t", "1"];
        let extra: &[&str] = match profile {
            0 => &[],
            1 => &["-p"],
            2 => &["-c", "1"],
            3 => &["-f"],
            4 => &["-a"],
            5 => &["-b"],
            6 => &["-v", "0"],
            _ => &["-r", "0"],
        };
        args.extend_from_slice(extra);
        args.push(&path);

        let result = run_binary(&bin, &args, "", 5, data);
        std::fs::remove_file(&path).ok();
        crash_if_bad(&result, true, true);
    });
}
