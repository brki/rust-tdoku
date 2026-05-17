# rust-tdoku

A Rust port of [tdoku](https://github.com/t-dillon/tdoku) — a high-performance Sudoku solver and generator.

⚠️ Notice ⚠️ This is an AI-assist ported project.

Why? tdoku doesn't run on arm64 architecture, rust-tdoku runs on both amd64 and arm64.

Code quality and correctness efforts were made, including checking that Sudoku solving and generating
work equivalently to the original tdoku project.

## Overview

The `rdoku` crate provides three solver implementations ported from tdoku's C++ source, along with a puzzle generator and benchmark runner:

| Solver | Description |
|--------|-------------|
| `solver_dpll_triad_simd` | The fastest solver. Uses DPLL search with triad constraints and SIMD constraint propagation (`Bitvec08x16` / `Bitvec16x16` backed by SSE/AVX intrinsics on x86_64). |
| `solver_dpll_triad_scc` | DPLL search with triad constraints and Strongly Connected Component (SCC) variable-selection heuristic. Models the puzzle as a SAT instance with 1296 literals. |
| `solver_basic` | Simple DPLL backtracking with a minimum-candidates heuristic. Reference implementation. |

See the [tdoku README](tdoku/README.md) and [tdoku blog](https://t-dillon.github.io/tdoku) for a detailed explanation of the algorithms.

## Input Format

**Standard Sudoku (81 chars):** For row `r` (0–8) and column `c` (0–8), `input[r * 9 + c]` is `'1'`–`'9'` for a given clue or `'.'` for an empty cell.

**Pencilmark Sudoku (729 chars):** For row `r`, column `c`, and digit `d` (1–9), `input[r * 81 + c * 9 + d - 1]` is `'.'` if the digit is eliminated, or `('0' + d)` otherwise.

## Usage

### Library

```toml
[dependencies]
rdoku = { path = "../rdoku" }
```

```rust
use rdoku::solve_sudoku;

let puzzle = "53..7....6..195....98....6.8...6...34..8.3..17....2...6.6....28....419..5....8..79";
let (count, solution, guesses) = solve_sudoku(puzzle, 1, 0);
assert_eq!(count, 1);
println!("{solution}");
```

### Solve binary

```sh
cargo run --release --bin solve -- [OPTIONS] [puzzle_file ...]

Options:
  -l <limit>    Max solutions to count per puzzle (default: 1; use 2 to check uniqueness)
  -c            Count-only mode: output count and guesses, not the solution string
  -p            Input is pencilmark format (729 chars per puzzle)
  --pretty      Print each solution as an ASCII art grid
  --stats       Print a summary (total puzzles, solved, guesses, rate) to stderr
  -s <solver>   Solver to use: simd (default) | scc | basic
  -h            Full usage and examples
```

Examples:

```sh
# Solve a puzzle from stdin:
echo '53..7....6..195....98....6.8...6...34..8.3..17...2...6.6....28....419..5....8..79' | solve

# Check uniqueness of all puzzles in a file:
solve -l 2 -c puzzles.txt

# Display solutions as ASCII art with a performance summary:
solve --pretty --stats tdoku/test/test_puzzles
```

### Benchmark binary

```sh
cargo run --release --bin benchmark -- [OPTIONS] <puzzle-file>

Options:
  -n <size>       Target dataset size (default: 100000)
  -w <seconds>    Warmup duration in seconds (default: 4)
  -t <seconds>    Test duration in seconds (default: 10)
  -p              Input is pencilmark format (729 chars)
  -r              Disable randomization of puzzle order
  -s <seed>       Random seed (0 = use system entropy)
  -c              CSV output instead of Markdown table
  -f              Stop at first solution (skip uniqueness check)
```

### Generate binary

```sh
cargo run --release --bin generate -- [OPTIONS] [pattern_file]

Scoring weights:
  -c <clue_weight>    Weight for clue count in loss (higher = fewer clues). Default: 1.0
  -g <guess_weight>   Exponent scaling solver-guess reward (higher = harder). Default: 0.5
  -r <random_weight>  Weight for random noise in loss (0 = deterministic). Default: 1.0

Generation control:
  -d <drop>           Clues to remove per iteration before re-completing. Default: 3
  -e <num_evals>      Permutations for difficulty estimate (0 = skip). Default: 10
  -m [0|1]            Minimize puzzles before scoring and printing. Default: 1
  -n <pool_size>      Pool size for hill-climbing search. Default: 500

Output control:
  -l <limit>          Stop after this many puzzles. Default: unlimited
  -a [0|1]            1 = print all evaluated puzzles; 0 = pool-accepted only. Default: 0
  -p [0|1]            1 = pencilmark format (729 chars); 0 = vanilla (81 chars). Default: 1
  --pretty            Print each puzzle as an ASCII art grid before the one-line output
  -h                  Full usage, output format, difficulty tuning guide, and examples
```

Examples:

```sh
# Generate 10 vanilla puzzles:
generate -p 0 -l 10

# Generate 5 hard vanilla puzzles (maximize solver guesses):
generate -p 0 -l 5 -c 0.0 -g 2.0

# Seed from an existing file and generate 50 new variations:
generate -p 0 -l 50 my_puzzles.txt
```

## Building

```sh
cargo build --release
```

To run the [comparison tests](#comparison-tests) (Docker-based C++/Rust trace comparison), initialize the tdoku submodule first:

```sh
git submodule update --init
```

SIMD acceleration requires x86_64 with SSSE3 and SSE4.1. On other platforms the scalar fallbacks are used automatically.

To explicitly enable AVX2 for better performance on supported hardware:

```sh
RUSTFLAGS="-C target-cpu=native" cargo build --release
```

## Testing

A task runner ([`just`](https://github.com/casey/just)) orchestrates formatting, linting, tests, fuzzing, and benchmarks:

```sh
# Default pre-commit suite: fmt + clippy + tests
just validate

# Individual checks
just format           # cargo fmt --check
just lint             # cargo clippy
just test-unit        # unit tests
just test-integration # integration tests

# Custom commands
just all                             # run everything
just fuzz 60                         # fuzz targets (60s each)
just comparison                      # Docker comparison tests
just generated 1000                  # verify 1000 generated puzzles
just bench                           # criterion + legacy benchmarks
```

See `just --list` for all available recipes.

Individual test commands are also available:

Integration tests read `tdoku/test/test_puzzles` and verify all three solvers produce correct solution counts and solution strings:

```sh
cargo test --release
```

To skip the comparison tests (which require Docker):

```sh
cargo test --release -- --skip comparison
```

Or to run only unit and integration tests without comparison tests:

```sh
cargo test --release --lib --test integration --test edge_cases
```

> **Why `--release`?** The SIMD solver uses deep mutual recursion (the `branch` and `count_solutions` functions call each other). In debug builds Rust does not apply tail-call or inlining optimizations, so searching a multi-solution puzzle with hundreds of solutions can exhaust the default 8 MiB thread stack and panic with a stack overflow. Release mode inlines heavily enough that the effective call depth stays well within stack limits. Unit tests and uniqueness-puzzle tests are safe in debug mode; the multi-solution corpus cases (up to 847 solutions in the test file) are what require `--release`.

### Comparison tests

These tests build both the Rust and C++ debug solvers in Docker containers, run them on the same puzzles, and assert that their `DT:` trace output and results are byte-for-byte identical. Artifacts are saved to `debug/artifacts/` (gitignored) and are preserved after the run for inspection.

```sh
cargo test --test comparison -- --nocapture
```

Requires Docker with amd64 support (e.g. [colima](https://github.com/abiosoft/colima) on macOS). The test is silently skipped when Docker is unavailable.

You can also run the comparison interactively with a specific puzzle:

```sh
debug/compare.sh [puzzle] [limit]
```

## Project Structure

```
.
├── Cargo.toml
├── Cargo.lock                      # Pinned dependency versions
├── LICENSE.md
├── README.md
├── src/
│   ├── lib.rs                      # Public API + module declarations
│   ├── bitutil.rs                  # Bit manipulation utilities
│   ├── simd_vectors.rs             # Bitvec08x16 / Bitvec16x16 SIMD abstractions
│   ├── solver_basic.rs             # Basic DPLL solver
│   ├── solver_dpll_triad_scc.rs    # SCC solver
│   ├── solver_dpll_triad_simd.rs   # SIMD solver (fastest)
│   ├── util.rs                     # RNG + puzzle permutation utilities
│   ├── grid_lib.rs                 # Grid enumeration utilities
│   └── bin/
│       ├── benchmark.rs            # Benchmark runner
│       ├── debug_solver.rs         # Debug solver (used for trace comparison)
│       ├── generate.rs             # Puzzle generator (hill-climbing pool)
│       ├── generate_verify.rs      # Generate + verify unique-solution puzzles (used for testing)
│       └── solve.rs                # Solver CLI (read puzzles, output solutions)
├── tests/
│   ├── integration.rs              # Correctness tests against tdoku test data
│   ├── edge_cases.rs               # Edge case and resource-safety tests
│   ├── comparison.rs               # Docker-based C++/Rust trace comparison
│   ├── property_tests.rs           # Proptest property-based tests
│   └── test_puzzles                # Committed copy of the test puzzle corpus
├── benches/
│   └── solver_bench.rs             # Criterion benchmarks for all solvers
├── fuzz/
│   ├── Cargo.toml                  # Separate crate for libfuzzer-based fuzzing
│   └── fuzz_targets/
│       ├── solve_fuzz.rs           # Fuzz target: solve arbitrary inputs
│       └── generator_fuzz.rs       # Fuzz target: generate + verify puzzles
├── justfile                        # Task runner (just validate, just bench, …)
├── debug/
│   ├── Dockerfile.tdoku            # Docker image for C++ tdoku debug solver
│   ├── Dockerfile.rdoku            # Docker image for Rust rdoku debug solver
│   ├── docker-compose.yml          # Compose file for running both solvers
│   ├── compare.sh                  # Interactive comparison script
│   └── artifacts/                  # Gitignored; populated by comparison tests
└── tdoku/                          # Git submodule: github.com/brki/tdoku porting-changes
    └── test/test_puzzles           # Test corpus used by integration tests
```

## Credits

All algorithms are a direct port of [tdoku](https://github.com/t-dillon/tdoku) by [t-dillon](https://github.com/t-dillon). See the original project for the full discussion of the solver design.
