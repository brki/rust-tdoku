//! AFL++ fuzz harness for the `generate` binary.
//!
//! Feeds arbitrary byte sequences as CLI flags to `generate`, verifying that
//! it never panics, hangs, or produces garbage output.
//!
//! **Coverage note**: AFL++ measures coverage of this harness process only.
//! The target binary runs as a subprocess, so generator internals are not
//! visible to the coverage bitmap. These harnesses test CLI-level correctness
//! (argument parsing, exit codes, panics, hangs). Deep logic coverage is
//! provided by the libfuzzer targets in `fuzz/`.
//!
//! Profiles are split into two groups:
//! - **Normal** (0–11): fuzz bytes derive sanitised numeric parameters so
//!   AFL++ can explore the legitimate parameter space efficiently.
//! - **Chaos** (12–19): raw fuzz bytes are passed directly as argument values
//!   — null bytes, unicode, negative numbers, shell metacharacters, etc.
//!   These verify that `generate` rejects or handles malformed input cleanly.
//!
//! `generate` does **not** read stdin — it either generates puzzles from
//! scratch or loads seed puzzles from a pattern file given on the command line.
//!
//! Usage: `cargo afl build && cargo afl fuzz -i corpus -o output -- target/debug/afl_generate`

fn main() {
    afl::fuzz!(|data: &[u8]| {
        rdoku_fuzz_afl::generate_logic::process(data);
    });
}
