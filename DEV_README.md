# Development Guide — rdoku

This document covers the development workflow for **rdoku**, a Rust port of [tdoku](https://github.com/t-dillon/tdoku) — a high-performance Sudoku solver and generator.

## Prerequisites

| Tool | Version | Notes |
|------|---------|-------|
| Rust | ≥ 1.86 | `rustup` recommended; see [rustup.rs](https://rustup.rs) |
| Cargo | (bundled with Rust) | |
| just | ≥ 1.50 | `brew install just` or `cargo install just` |
| cargo-fuzz | latest | `cargo install cargo-fuzz` (requires nightly) |
| Docker | (optional) | For C++/Rust trace comparison tests |
| rustfmt | (bundled) | Formatting |
| clippy | (bundled) | Linting |

Verify your setup:

```sh
rustc --version   # should be ≥ 1.86.0
cargo --version
cargo fmt --version
cargo clippy --version
```

## Quick Start

```sh
# Clone with submodules (tdoku C++ reference)
git clone --recurse-submodules <repo-url> && cd tdoku-to-rust
# Or if already cloned: git submodule update --init --recursive

# Build (debug)
cargo build

# Build (release, with LTO)
cargo build --release

# Run the default pre-commit suite (formatting + clippy + tests)
just validate

# Run benchmarks
just bench
```

> **Tip:** `just` is a single native binary — no compilation step. Run `just --list` to see all available recipes.

## Project Structure

```
tdoku-to-rust/
├── Cargo.toml              # Workspace manifest (MSRV 1.86)
├── Cargo.lock              # Pinned dependency versions
├── README.md               # User-facing documentation
├── DEV_README.md           # This file — development guide
├── src/
│   ├── lib.rs              # Crate root & public API
│   ├── bitutil.rs          # Bit manipulation utilities
│   ├── grid_lib.rs         # Grid pattern enumeration
│   ├── simd_vectors.rs     # SIMD abstraction (Bitvec08x16, Bitvec16x16)
│   ├── solver_basic.rs     # Reference DPLL solver
│   ├── solver_dpll_triad_scc.rs  # DPLL + triad + SCC SAT solver
│   ├── solver_dpll_triad_simd.rs # DPLL + triad + SIMD (fastest)
│   ├── util.rs             # Puzzle permutation utilities
│   └── bin/
│       ├── solve.rs        # `solve` binary
│       ├── benchmark.rs    # `benchmark` binary (tdoku-style)
│       ├── generate.rs     # `generate` binary (puzzle generator)
│       └── debug_solver.rs # `debug_solver` binary
├── benches/
│   └── solver_bench.rs     # Criterion micro-benchmarks
├── tests/
│   ├── comparison.rs       # Docker-based C++/Rust trace comparison
│   ├── edge_cases.rs       # Edge case tests
│   ├── integration.rs      # Integration tests
│   ├── property_tests.rs   # Proptest property-based tests
│   └── test_puzzles        # Puzzle corpus
├── fuzz/
│   ├── Cargo.toml          # Fuzz workspace
│   └── fuzz_targets/
│       ├── solve_fuzz.rs   # Solver fuzz target
│       └── generator_fuzz.rs # Generator fuzz target
├── justfile                # Task runner (format, test, fuzz, bench, …)
├── debug/                  # Docker-based C++/Rust comparison tooling
├── benchmark-results/      # Historical benchmark output
└── tdoku/                  # Original C++ tdoku source (reference)
```

## Development Workflow

### Building

```sh
# Debug build (fast compile, no optimizations)
cargo build

# Release build (optimized, LTO enabled)
cargo build --release

# Check only (no codegen, fastest)
cargo check
```

### Running Binaries

```sh
# Solve a puzzle
echo '53..7....6..195....98....6.8...6...34..8.3..17...2...6.6....28....419..5....8..79' \
  | cargo run --release --bin solve

# Benchmark puzzles
cargo run --release --bin benchmark -- tests/test_puzzles -n 1000 -w 1 -t 5

# Generate puzzles with JSON output
cargo run --release --bin generate -- --json -l 5
```

### Testing

The project uses [just](https://github.com/casey/just) as its task runner. Run `just --list` to see all available recipes.

**Task runner commands:**

```sh
# Formatting check
just format

# Auto-fix formatting
just format-fix

# Clippy lint
just lint

# All checks (format + lint)
just check

# Unit tests only
just test-unit

# Integration + edge case + property tests
just test-integration

# All tests
just test

# Tests in release mode (skip Docker comparison)
just test-release

# Pre-commit suite: checks + tests
just validate

# Debug build
just build

# Release build
just build-release

# Fuzz solver & generator (30s default)
just fuzz 60

# Docker C++/Rust trace comparison
just comparison

# Generated puzzle verification
just generated 500

# Criterion + legacy benchmarks
just bench

# Full CI suite
just all
```

Run individual test groups directly:

```sh
# Unit tests only
cargo test --lib

# Integration tests
cargo test --test integration

# Edge case tests
cargo test --test edge_cases

# Property-based tests
cargo test --test property_tests

# Run tests in release mode (faster)
cargo test --release

# Run a specific test by name
cargo test test_solve_sudoku_unique
```

### Formatting & Linting

```sh
# Check formatting
just format

# Auto-format
just format-fix

# Lint with strict warnings
just lint

# All checks at once
just check
```

Or use cargo directly:

```sh
cargo fmt --check
cargo fmt
cargo clippy -- -D warnings
cargo clippy --fix --allow-dirty
```

### Fuzzing

Fuzzing requires a nightly Rust toolchain:

```sh
# Install cargo-fuzz (one-time)
cargo install cargo-fuzz

# Via just (recommended): run both fuzz targets for 60s each
just fuzz 60

# Show full fuzzer output (no stderr suppression)
just fuzz verbose=1 60

# Or run fuzz targets directly:
cargo +nightly fuzz run solve_fuzz -- -max_total_time=60
cargo +nightly fuzz run generator_fuzz -- -runs=50000

# Minimize a crashing input
cargo +nightly fuzz run solve_fuzz fuzz/artifacts/solve_fuzz/crash-<hash>
```

Fuzz corpora and artifacts are stored in `fuzz/corpus/` and `fuzz/artifacts/` respectively.

### Benchmarking

```sh
# Run criterion micro-benchmarks + legacy tdoku-style benchmark
just bench

# Criterion benchmarks only
cargo bench

# Legacy benchmark only (control exact parameters)
cargo run --release --bin benchmark -- tests/test_puzzles -n 10000 -w 2 -t 10
```

Benchmark results are timestamped and saved to `benchmark-results/<YYYYMMDD_HHMMSS>/`.

## Dependencies

### Direct Dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| `wide` | 1 | SIMD vector types for x86 (scalar fallback on ARM) |
| `rand` | 0.10 | Random number generation (puzzle generation) |
| `serde_json` | 1 | JSON output for `generate` binary |

### Dev Dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| `criterion` | 0.8 | Micro-benchmarking framework |
| `proptest` | 1 | Property-based testing |

### Fuzz Dependencies (`fuzz/Cargo.toml`)

| Crate | Version | Purpose |
|-------|---------|---------|
| `libfuzzer-sys` | 0.4 | LibFuzzer bindings |

### Updating Crates to Latest Versions

Cargo uses [semver](https://semver.org) to determine compatible updates. Version specifiers like `"1"` or `"0.8"` allow updates within the same major (or minor, for `0.x`) version.

#### Step-by-step: Update All Dependencies

```sh
# 1. Check which crates have updates available
cargo update --dry-run --verbose

# 2. Update Cargo.lock to latest compatible versions
cargo update

# 3. For breaking changes (e.g., criterion 0.5 → 0.8), edit Cargo.toml:
#    - Change the version specifier (e.g., `"0.5"` → `"0.8"`)
#    - Check the crate's changelog for API changes
#    - Update `rust-version` if the new crate requires a newer MSRV

# 4. Verify everything compiles and tests pass
cargo check --benches
cargo test
cargo clippy -- -D warnings

# 5. Commit the updated Cargo.toml and Cargo.lock
git add Cargo.toml Cargo.lock
git commit -m "chore: update dependencies"
```

#### Checking for Outdated Crates

```sh
# See what cargo update would do
cargo update --dry-run --verbose

# Search for a specific crate's latest version
cargo search <crate-name> --limit 1

# Get detailed info about a crate
cargo info <crate-name>
```

#### Updating to a New Major Version

When updating across major versions (e.g., `rand` 0.9 → 0.10):

1. Read the crate's changelog (`CHANGELOG.md` or [crates.io](https://crates.io))
2. Update `Cargo.toml` version specifier
3. Fix any API incompatibilities
4. Run the full test suite: `just validate`
5. Run benchmarks to check for performance regressions: `just bench`

#### Minimum Supported Rust Version (MSRV)

The `rust-version` field in `Cargo.toml` declares the minimum Rust version required. When updating crates, check if any dependency requires a newer Rust version:

```sh
# Check the MSRV of a specific crate
cargo info <crate-name> | grep rust-version
```

If a dependency requires a newer Rust version than declared, update the `rust-version` field in `Cargo.toml`.

### Dependency Tree

To understand how dependencies are resolved:

```sh
# Show full dependency tree
cargo tree

# Show only direct dependencies
cargo tree --depth 1

# Show what depends on a specific crate
cargo tree --invert <crate-name>

# Show duplicates (potential bloat)
cargo tree --duplicates
```

## Solver Architecture

Three solver implementations, from simplest to fastest:

| Solver | Module | Description |
|--------|--------|-------------|
| Basic | `solver_basic` | Simple DPLL backtracking with minimum-candidates heuristic. Reference implementation. |
| SCC | `solver_dpll_triad_scc` | DPLL with triad constraints + Strongly Connected Component variable selection. Models 1296-literals SAT. |
| SIMD | `solver_dpll_triad_simd` | DPLL with triads + SIMD constraint propagation. Fastest solver. Uses `Bitvec08x16`/`Bitvec16x16` with SSE/AVX intrinsics on x86_64, scalar fallback on ARM64. |

The public API in `lib.rs` defaults to the SIMD solver. See the [tdoku blog](https://t-dillon.github.io/tdoku) for algorithm details.

## Configuration

### Release Profile

The `[profile.release]` in `Cargo.toml` enables:
- `opt-level = 3` — maximum optimization
- `lto = true` — link-time optimization (whole-program)
- `codegen-units = 1` — single codegen unit for better inlining

### Feature Flags

| Flag | Description |
|------|-------------|
| `debug-trace` | Enable detailed trace output in solvers (for debugging comparison tests) |

Enable with:

```sh
cargo build --features debug-trace
cargo test --features debug-trace
```

## Docker Comparison Tests

The `debug/` directory contains tooling to compare solver traces between the original C++ tdoku and the Rust port.

**Prerequisite:** The `tdoku/` subdirectory is a git submodule pointing to the C++ tdoku source. It must be checked out before running comparison tests:

```sh
git submodule update --init --recursive
```

Once the submodule is available:

```sh
# Run comparison tests (requires Docker)
just comparison

# Or run all checks + tests + comparison:
just validate && just comparison

# Or directly:
cargo test --release --test comparison -- --nocapture
```

This builds both solvers in Docker containers, runs them against test puzzles, and diffs the traces to ensure identical behavior.

## Common Tasks

### Adding a New Solver

1. Create `src/solver_<name>.rs` with a `solve()` function matching the signature in `solver_basic.rs`
2. Add `pub mod solver_<name>;` to `src/lib.rs`
3. Add tests to `tests/integration.rs` comparing against existing solvers
4. Add benchmarks to `benches/solver_bench.rs`
5. Add fuzz target if applicable

### Adding a New Binary

1. Create `src/bin/<name>.rs`
2. Add `[[bin]]` section to `Cargo.toml`
3. Add usage docs to `README.md`

### Debugging Solver Behavior

```sh
# Run the debug solver (prints detailed trace)
cargo run --bin debug_solver -- <puzzle-string>

# Compare with original tdoku (if Docker available)
./debug/compare.sh <puzzle-string>
```

## CI / Pre-commit Checklist

Before committing, run:

```sh
# Quick check: format + clippy + tests
just validate

# Full check: everything including fuzz, comparison, and generated puzzles
just all
```
