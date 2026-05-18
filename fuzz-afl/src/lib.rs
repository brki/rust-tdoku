//! `rdoku-fuzz-afl` — AFL++ binary-fuzzing harnesses for rdoku CLI tools.
//!
//! This crate contains harness binaries that feed arbitrary fuzzer-generated
//! input to the `solve`, `generate`, `benchmark`, and `debug_solver` binaries,
//! catching panics, hangs, signal crashes, and garbage output.

pub mod harness_util;
pub mod generate_logic;
pub mod solve_logic;
