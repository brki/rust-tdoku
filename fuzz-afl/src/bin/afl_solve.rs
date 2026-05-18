//! AFL++ fuzz harness for the `solve` binary.
//!
//! Feeds arbitrary byte sequences as CLI flags + puzzle data to `solve`,
//! verifying that it never panics, hangs, or produces garbage output.
//!
//! **Coverage note**: AFL++ measures coverage of this harness process only.
//! The target binary runs as a subprocess, so solver internals are not
//! visible to the coverage bitmap. These harnesses test CLI-level correctness
//! (argument parsing, exit codes, panics, hangs). Deep logic coverage is
//! provided by the libfuzzer targets in `fuzz/`.
//!
//! Profiles are split into two groups:
//! - **Normal** (0–13): sanitised puzzle strings + known flag combinations.
//!   Profiles 12–13 exercise multi-puzzle stdin.
//! - **Chaos** (14–19): raw fuzz bytes passed directly as argument values
//!   and puzzle data — null bytes, unicode, shell metacharacters, etc.
//!
//! Usage: `cargo afl build && cargo afl fuzz -i corpus -o output -- target/debug/afl_solve`

fn main() {
    afl::fuzz!(|data: &[u8]| {
        rdoku_fuzz_afl::solve_logic::process(data);
    });
}
