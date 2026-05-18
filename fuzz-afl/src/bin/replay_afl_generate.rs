//! Standalone replay binary for inspecting AFL++ crash/hang inputs for `generate`.
//!
//! Reads raw fuzz bytes from stdin and runs the same harness logic as
//! `afl_generate`, but without AFL++ instrumentation.  Safe to run standalone
//! (unlike the AFL-instrumented binary, which hangs waiting for a forkserver).
//!
//! Usage:
//!   replay_afl_generate < fuzz-afl/output/afl_generate/default/crashes/id:000000,...

use std::io::Read;

fn main() {
    let mut data = Vec::new();
    std::io::stdin()
        .read_to_end(&mut data)
        .expect("failed to read stdin");
    rdoku_fuzz_afl::generate_logic::process(&data);
}
