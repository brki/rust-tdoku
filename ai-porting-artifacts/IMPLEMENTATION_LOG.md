# rdoku Implementation Log

Tracks what has been implemented in each phase. See `plan-tdokuToRust.prompt.md` for the full plan.

---

## Phase 1: Scaffold & Foundation ✅

**Status:** Complete

### Files created

| File | Description |
|------|-------------|
| `Cargo.toml` | Package manifest. `[lib]` + two `[[bin]]` targets (`benchmark`, `generate`). Deps: `wide = "1"`, `rand = "0.10"`. Release profile with LTO and `codegen-units = 1`. |
| `src/lib.rs` | Module declarations for all submodules. `#![allow(dead_code, ...)]` to permit stubs. |
| `src/bitutil.rs` | **Fully implemented.** All 10 bit utility functions ported from `bitutil.h`, plus 64-bit variants. Unit tests for all functions pass. |
| `src/simd_vectors.rs` | Stub — Phase 2. |
| `src/solver_basic.rs` | Stub — Phase 3. |
| `src/util.rs` | Stub — Phase 4. |
| `src/grid_lib.rs` | Stub — Phase 5. |
| `src/solver_dpll_triad_scc.rs` | Stub — Phase 6. |
| `src/solver_dpll_triad_simd.rs` | Stub — Phase 7. |
| `src/bin/benchmark.rs` | Stub — Phase 10. |
| `src/bin/generate.rs` | Stub — Phase 9. |
| `tests/integration.rs` | Stub — Phase 11. |

### Verification
- `cargo check` — clean
- `cargo test` — 5 bitutil unit tests pass, all other suites empty

---

## Phase 2: SIMD Vector Abstraction

**Status:** Complete

### Files created/modified

| File | Description |
|------|-------------|
| `src/simd_vectors.rs` | **Fully implemented.** `Bitvec08x16` (8×u16, 128-bit) and `Bitvec16x16` (16×u16, stored as two `Bitvec08x16` halves). On x86_64: SSE2 ops always via intrinsics, SSSE3 paths (shuffle, popcounts9, rotate_rows) and SSE4.1 paths (all_zero, intersects, subset_of, min_pos_gte) guarded by `is_x86_feature_detected!` with scalar fallbacks. All operators (`|`, `&`, `^`, `!`, `|=`, etc.) implemented. All helper fns: `and_not`, `get_low_bit`, `clear_low_bit`, `popcounts9`, `popcount`, `shuffle`, `rotate_rows/rows2/cols/cols2`, `min_pos_gte`, `which_equal`, `which_non_zero`, `any_zero`/`all_zero`/`intersects`/`subset_of`/`any_less_than`, ternary helpers (`x_y_and_z_or`, etc.), `which_dots_16/32/64`. |

### Verification
- `cargo test` — 22 tests pass (17 simd_vectors + 5 bitutil)

---

## Phase 3: Basic Solver

**Status:** Complete

### Files created/modified

| File | Description |
|------|-------------|
| `src/solver_basic.rs` | **Fully implemented.** `SolverBasic` struct with `rows/cols/boxes: [u32;9]`, `cells_todo: Vec<(usize,usize,usize)>`. `initialize()` parses clues, `satisfy_given_partial_assignment()` is the recursive DPLL search with optional minimum-candidates heuristic (`config > 0`). Thread-local static solver mirrors the C++ `static SolverBasic solver` pattern to reuse heap allocations. Public `solve(input: &[u8], limit: usize, config: u32) -> (usize, [u8;81], usize)` returns `(num_solutions, solution, num_guesses)`. |

### Verification
- `cargo test` — 25 tests pass (3 solver_basic + 17 simd_vectors + 5 bitutil)
- Solves a real tdoku test puzzle (`.5..83.17...` 81-char puzzle) with correct solution
- Both no-heuristic (`config=0`) and min-candidates heuristic (`config=1`) paths exercised
- Invalid puzzle (duplicate clue) correctly returns 0 solutions

---

## Phase 4: Utilities

**Status:** Complete

### Files created/modified

| File | Description |
|------|-------------|
| `src/util.rs` | **Fully implemented.** `Util` struct wrapping `rand::rngs::SmallRng`. `new()` seeds from `rand::rng()` (thread-local OS-seeded RNG). `random_seed(u64)` reseeds via `seed_from_u64`. `random_uint() -> u32`, `random_double() -> f64` delegate to `rng.random()`. `permutation(size) -> Vec<usize>` builds `0..size` then calls `slice.shuffle`. `block_shuffle([usize;9])` shuffles 3 band indices then 3 intra-band indices, mirror of C++ `BlockShuffle`. `permute_sudoku(&mut [u8], pencilmark)` applies digit/row/col permutations in-place for both 81-byte and 729-byte (pencilmark) formats. |

### Verification
- `cargo test` — 28 tests pass (3 util + 3 solver_basic + 17 simd_vectors + 5 bitutil)
- `test_permutation_length_and_elements` — seeded run produces a valid permutation of `0..9`
- `test_block_shuffle_valid` — all 9 indices distinct; rows within each output band come from one source band
- `test_permute_sudoku_preserves_validity` — permuted puzzle still has 1 solution; digit frequencies match original

---

## Phase 5: Grid Library

**Status:** Complete

### Files created/modified

| File | Description |
|------|-------------|
| `src/grid_lib.rs` | **Fully implemented.** `get_pattern(pattern_id) -> [u8;81]` — port of C++ `GetPattern`, starting from `BOX1_PATTERN_TEMPLATE` then calling `band_init` twice (horizontal + vertical) with `horiz_indexing`/`verti_indexing`. `band_init` is a direct port of C++ `BandInit` using the 6 permutations lookup. `get_grid(grid_idx, index, table) -> [u8;81]` — decodes the binary index/table files to find pattern and rank, then calls `solver_basic::solve` to get the specific grid. `enumerate_grids(first_grid_idx, count, index, table, callback)` — iterates over patterns in order, solving each at the appropriate rank and forwarding to the callback. Uses `solver_basic` as the enumeration backend (Phase 7's SIMD solver can replace this later). |

### Verification
- `cargo test` — 31 tests pass (3 grid_lib + 3 util + 3 solver_basic + 17 simd_vectors + 5 bitutil)
- `get_pattern(0)` returns correct template bytes
- All pattern bytes are digits or dots
- Different pattern IDs produce different patterns

---

## Phase 6: SCC Solver

**Status:** Complete

### Files created/modified

| File | Description |
|------|-------------|
| `src/solver_dpll_triad_scc.rs` | **Fully implemented.** `FastBitset` (2592-bit dense bitset, `[u64; 41]` with manual `Default`). `State` with `asserted: FastBitset`, `clause_free_literals: Vec<u16>`, `implication_counts: Vec<u16>`, `num_asserted: u32`. `SolverDpllTriadScc` with clause/literal/implication tables all as `Vec<Vec<_>>`. `build_constraints()` populates ExactlyN constraints for cells, triads, and band triads using free-standing setup helpers (avoiding borrow-checker conflicts). BCP via `assert_lit()` using `wrapping_sub` to match C++ unsigned wrap-around semantics. Path-based SCC (`scc_visit` + `find_strongly_connected_components`) with inference and component-size heuristic. DPLL branching via `branch_on_literal` / `count_solutions`. Public `solve(input, limit, config)` mirrors the C++ thread-local static pattern. |

### Key implementation notes

- Setup helpers (`setup_add_implication`, `setup_add_clause_with_min`, `setup_add_exactly_n`) are free functions rather than methods, avoiding the need to borrow both `self.literals_to_implications` and `self.initial_state` simultaneously during construction.
- `clause_free_literals` uses `.wrapping_sub(1)` in `assert_lit` to match C++ `uint16_t` wrap-around: a clause already at 0 wraps to `u16::MAX`, bypassing the `== 0` trigger.
- `noneliminated` scratch buffer uses `std::mem::take` / restore to allow calling `add_implication(&mut self, ...)` while logically iterating the buffer.
- `scc_visit` re-reads `implication_counts[literal]` each loop iteration so new implications added by SCC inference are visible mid-traversal.

### Verification
- `cargo test` — 37 tests pass (6 scc + 3 grid_lib + 3 util + 3 solver_basic + 17 simd_vectors + 5 bitutil)
- All four config modes tested (both/inference-only/heuristic-only/neither)
- SCC solver produces the same solution as the basic solver on the reference puzzle
- Invalid puzzle (duplicate clue) correctly returns 0 solutions

---

## Phase 7: SIMD Solver

**Status:** Complete

### Files created/modified

| File | Description |
|------|-------------|
| `src/simd_vectors.rs` | Added `as_4x64() -> (u64, u64, u64, u64)` to `Bitvec16x16` (used by solution extraction). |
| `src/solver_dpll_triad_simd.rs` | **Fully implemented.** `Box`, `Band`, `State`, `BoxIndexing` structs. `Tables` struct with all lookup tables initialized once via `OnceLock<Tables>`. `SolverDpllTriadSimd<const SOLUTION_MODE: u8>` generic struct with `box_restrict`, `assertions_to_eliminations`, `horizontal_triads`, `vertical_triads`, `gather_triad_clause_assertions`, `band_eliminate`, `configurations_to_positive_triads`, `positive_triads_to_box_candidates`, `choose_band_and_value_to_branch`, `branch_on_band_and_value`, `count_solutions_consistent_with_partial_assignment`, `init_clue`, `init_vanilla_by_band`, `init_pencilmark_by_box`, `extract_solution`, `solve_sudoku`. `GeneratorDpllTriadSimd` struct with `constrain` and `minimize` methods. Thread-local `SOLVER_NONE` / `SOLVER_LAST` instances. Public `solve(input, limit, config)` API. |

### Key implementation notes

- Borrow-checker conflict on `state.bands[0][box_i]` + `state.bands[1][box_j]` simultaneously resolved with `state.bands.split_at_mut(1)`.
- `Tables` wraps all const lookup data; `unsafe impl Sync/Send` declared since all fields are `Copy` with no interior mutability.
- `SOLUTION_MODE = 0` counts solutions; `SOLUTION_MODE = 1` records last solution for extraction. Thread-locals avoid re-allocating state.
- `init_vanilla_by_band` uses `which_dots_64` (cells 0–63) + `which_dots_16` (cells 64–79) + explicit check for cell 80, matching C++ approach.
- Table index for `triads_shiftN_to_config_elims16` is `box_j * 3 + box_i` (matching C++ `box_j * 3 + box_i`).

### Verification
- `cargo test` — 41 tests pass (4 simd + 6 scc + 3 grid_lib + 3 util + 3 solver_basic + 17 simd_vectors + 5 bitutil)
- `test_simd_solve_unique` — correct solution returned
- `test_simd_solve_count_only` — single solution counted
- `test_simd_invalid_puzzle` — 0 solutions for invalid puzzle
- `test_simd_matches_basic_solver` — same answer as basic solver

---

## Phase 8: Public API

**Status:** Complete

### Files created/modified

| File | Description |
|------|-------------|
| `src/solver_dpll_triad_simd.rs` | Added `EnumSolver<'a>` struct (holds `&mut dyn FnMut` callback) and `pub fn enumerate(input, limit, callback)` free function. `EnumSolver` duplicates only `count_solutions` and `branch<VERTICAL>`, reusing all static methods from `SolverDpllTriadSimd` (no unsafe). |
| `src/lib.rs` | Finalized public API. `pub fn solve_sudoku(input: &str, limit: usize, config: u32) -> (usize, String, usize)` wraps `solver_dpll_triad_simd::solve`. `pub fn enumerate(puzzle: &str, limit, callback: impl FnMut(&str)) -> usize` wraps `solver_dpll_triad_simd::enumerate`. `pub fn constrain(pencilmark, puzzle: &mut String) -> bool` and `pub fn minimize(pencilmark, monotonic, puzzle: &mut String) -> bool` use a thread-local `GeneratorDpllTriadSimd`. String↔Vec<u8> conversions are handled inline. |

### Key implementation notes

- `enumerate` uses a separate `EnumSolver<'a>` struct (not the generic `SolverDpllTriadSimd`) to hold a `&mut dyn FnMut(&[u8; 81])` callback by reference for the duration of the call — safe, no `unsafe`, no `'static` bound required.
- `constrain`/`minimize` use a `thread_local! { GENERATOR: RefCell<GeneratorDpllTriadSimd> }` to reuse allocations across calls.
- `solve_sudoku` and `enumerate` accept `&str` and convert to `&[u8]` via `.as_bytes()`.
- Stack overflow in debug builds for sparse-puzzle enumeration is a known debug artifact; tests use the unique reference puzzle or `limit=1`.

### Verification
- `cargo test` — 45 tests pass (4 public API + 4 simd + 6 scc + 3 grid_lib + 3 util + 3 solver_basic + 17 simd_vectors + 5 bitutil)
- `test_solve_sudoku_unique` — correct solution returned as `String`
- `test_solve_sudoku_count_only` — single solution counted
- `test_enumerate_single_solution` — callback called once with correct solution
- `test_enumerate_respects_limit` — limit=5 on unique puzzle returns 1; limit=0 returns 0; limit=1 on under-constrained puzzle returns 1

---

## Phase 9: Generator Binary

**Status:** Complete

### Files created/modified

| File | Description |
|------|-------------|
| `src/bin/generate.rs` | **Fully implemented.** `Options` struct with all CLI flags. `Generator` struct with `Util`, `pool: Vec<PoolEntry>`, `pool_set: HashSet<String>`. `init_empty()` seeds the pool with a minimal puzzle from `make_seed()` (avoids stack overflow by using basic solver + SIMD minimize instead of constrain from empty grid). `load(filename)` loads patterns from file. `evaluate()` computes `(num_clues, geo_mean_guesses, loss)` scoring. `generate()` is the main loop: picks random pool entry, drops clues, re-completes via `rdoku::constrain`, optionally minimizes via `rdoku::minimize`, evaluates, deduplicates, and maintains the top-N pool. `format_pretty()` renders a puzzle as a 3×3 ASCII art grid. `print_puzzle()` uses `--pretty` option to prefix grid before the one-line output. CLI parsing handles: `-c/-g/-r/-d/-e/-l/-m/-n/-a/-p` flags (matching C++ generate.cc), plus `--pretty` (new addition). Solver flag `-s` is accepted but silently ignored (only SIMD solver available). |

### Key implementation notes

- `make_seed()` uses `solver_basic::solve` on the empty grid (max recursion depth ≤81) then calls `rdoku::minimize` to get a 20–30 clue seed puzzle. Starting from a completely unconstrained grid with `rdoku::constrain` would cause the SIMD DPLL solver to recurse ~80,000+ levels deep (stack overflow). This was the "side work" performed before Phase 9 proper.
- Pool uses a `Vec<PoolEntry>` with O(n) `worst_idx()` scan (same semantics as C++ max-heap with `pop_heap`).
- `--pretty` flag added (not in C++ original): renders vanilla or pencilmark puzzles as a 3×3 ASCII art grid. Pencilmark cells with exactly one remaining candidate show that digit; multi-candidate cells show `.`.
- Gurobi/MiniSat difficulty evaluation omitted per plan; SIMD solver used for all uniqueness/guess-count operations.

### Verification
- `cargo build --release --bin generate` — clean build
- `./target/release/generate --help` — correct usage output
- `./target/release/generate -l 2 -p 0 -e 0 -n 1` — generates 2 valid vanilla puzzles
- `./target/release/generate -l 2 -p 0 -e 0 -n 1 --pretty` — ASCII art grid printed before each puzzle line
- `./target/release/generate -l 1 -p 1 -e 0 -n 1` — generates 1 valid pencilmark puzzle

---

## Phase 10: Benchmark Binary

**Status:** Complete

### Files created/modified

| File | Description |
|------|-------------|
| `src/bin/benchmark.rs` | **Fully implemented.** `Solver` struct (function pointer + config + id + desc + feature flags). `all_solvers()` returns all 7 registered solvers: 4 SCC configs (`_tdev_dpll_triad`, `_tdev_dpll_triad_scc_i/h/ih`), 2 basic configs (`_tdev_basic`, `_tdev_basic_heuristic`), and the SIMD solver (`tdoku`). `Options` struct with all C++ flags. `Benchmark` struct with flat `dataset: Vec<u8>` (stride = puzzle_size), `Util` for randomization, `allow_zero` flag. `load()` replicates/samples input to `test_dataset_size` with optional `permute_sudoku` per slot. `warmup_and_estimate_rate()` runs for `min_seconds_warmup`. `test()` switches between fast (full-pass) and slow (permuted-order) benchmark paths. `rate()` per-puzzle timing mode (`-a` flag). `output_header()` / `output_result()` emit Markdown table or CSV. CLI parses `-a/-b/-c/-e/-f/-h/-n/-p/-r/-s/-t/-v/-w` flags (full parity with C++ `run_benchmark.cc`). |

### Key implementation notes

- `permute_slot()` helper copies a puzzle slot to a local `Vec<u8>`, calls `self.util.permute_sudoku()`, then copies back — avoids the simultaneous `&mut self.dataset` + `&mut self.util` borrow conflict.
- The benchmark correctly fails during warmup on unsolvable puzzles (count=0) when `allow_zero=false`, matching C++ behavior. The `test_puzzles` file contains 10 unsolvable puzzles; benchmark files should use dedicated puzzle sets (or set ALLOWZERO), just like C++.
- Table format exactly matches C++ (`|{:<37} |` header, `|---...---|:` separator, `|{:<27}{:<11}|` data rows).
- CSV format: `rustc,<version>,<flags>,<filename>,<solver_id>,<pps>,<usec>,<pct_no_guess>,<guesses>`.
- Rating mode (`-a`) prints one tab-separated cost per solver per puzzle in the input file.

### Verification
- `cargo build --release --bin benchmark` — clean build
- `./target/release/benchmark -h` — correct usage output listing all 7 solvers
- `./target/release/benchmark -w 1 -t 3 -n 100 -s tdoku <unique_puzzles>` — produces correct Markdown table
- `./target/release/benchmark -w 1 -t 2 -n 100 -s _tdev_basic,tdoku -c <unique_puzzles>` — produces correct CSV with two solvers
- `./target/release/benchmark -w 1 -t 2 -n 100 -s tdoku -a <unique_puzzles>` — rating mode prints per-puzzle costs
- `cargo test` — 46 tests pass (45 lib + 1 integration)

---

## Phase 11: Integration Tests

**Status:** Complete

### Files created/modified

| File | Description |
|------|-------------|
| `tests/integration.rs` | **Fully implemented.** Loads all 43 test cases from `../tdoku/test/test_puzzles` (`puzzle:count:solution` format). Six `#[test]` functions: `test_minimize_complete_solution` (regression), `test_basic_solver`, `test_scc_solver`, `test_simd_solver`, `test_public_api_solve`, `test_public_api_enumerate`. Each corpus test checks solution count (limit=100_000) and solution string (limit=1) against expected values for all three solver modules and the public API. `test_public_api_enumerate` also verifies callback count equals return value. |

### Key implementation notes

- `load_test_cases()` parses the test file with `splitn(3, ':')`. Solution field is only parsed when `expected_count == 1`.
- SCC solver uses `config=3` (both inference + heuristic) for best correctness confidence.
- SIMD solver uses `config=0`; solution is captured with a second `limit=1` call (matching C++ test runner pattern).
- Tests must be run with `cargo test --release` to avoid stack overflow on multi-solution puzzles (up to 847 solutions) in debug builds.

### Verification
- `cargo test --release` — 51 tests pass (6 integration + 45 lib unit tests)
- All 43 test puzzles verified: 18 unique-solution, 10 unsolvable, 15 multi-solution (3–847 solutions)
- All three solvers and the public `solve_sudoku`/`enumerate` API agree with expected counts and solutions

---

## Phase 12: Edge Case & Memory Tests

**Status:** Complete

### Files created/modified

| File | Description |
|------|-------------|
| `src/solver_basic.rs` | **Patched.** `initialize()` now pads inputs shorter than 81 bytes with `'.'` (empty cells) via a local `[u8; 81]` buffer. Any byte outside `'1'`–`'9'` is treated as an empty cell instead of causing a panic or UB. |
| `src/solver_dpll_triad_scc.rs` | **Patched.** `initialize_puzzle()` pads vanilla-format inputs shorter than 81 bytes and treats non-`'1'`–`'9'` bytes as empty cells. (Pencilmark path is unchanged: if `input.len() > 81` it's used directly.) |
| `tests/edge_cases.rs` | **Fully implemented.** 41 `#[test]` functions covering all Phase 12 categories. |

### Test categories in `edge_cases.rs`

#### Input edge cases (all three solvers + public API)
- `test_empty_puzzle_*` — all-dots input returns ≥ 1 valid solution
- `test_solved_puzzle_*` — fully-solved input returns count=1 and solution=input
- `test_invalid_puzzle_*` — contradicting givens returns count=0, no panic
- `test_pencilmark_solvable_*` / `test_pencilmark_unsolvable_*` — 729-char format (SIMD + SCC)
- `test_limit_zero_basic` — basic solver correctly returns 0 with limit=0
- `test_limit_zero_enumerate` — `enumerate` has an early-exit guard, returns 0 with no callbacks
- `test_limit_usize_max_unique` — unique puzzle with limit=usize::MAX returns 1, doesn't hang
- `test_short_input_*_no_panic` — 17-byte inputs padded to 81; no panic
- `test_empty_string_*_no_panic` — zero-length input treated as all-empty
- `test_garbage_bytes_*_no_panic` — 0x00/0xFF bytes treated as empty cells; no panic

#### `enumerate` edge cases
- `test_enumerate_valid_solutions` — callback receives well-formed valid grids
- `test_enumerate_limit_respected` — callback called exactly `limit` times when ≥ `limit` solutions exist
- `test_enumerate_return_equals_calls` — return value equals callback invocation count

#### `constrain` / `minimize` edge cases
- `test_constrain_already_unique` — result remains uniquely solvable (return value is implementation-defined; matches C++)
- `test_constrain_empty_puzzle` — constrained empty puzzle has a unique solution
- `test_minimize_preserves_uniqueness` — result still has unique solution
- `test_minimize_monotonic_preserves_uniqueness` — monotonic minimize preserves uniqueness
- `test_minimize_fewer_or_equal_clues` — clue count does not increase after minimize

#### Memory / resource invariants
- `test_repeated_solve_no_leak` — 10,000 `solve_sudoku` calls; no panic (thread-locals are fixed-size)
- `test_repeated_enumerate_no_leak` — 1,000 `enumerate` calls; no panic
- `test_sparse_puzzle_limited_stack` *(release only)* — 3-clue puzzle in a 4 MiB stack thread completes without overflow

#### Cross-solver agreement
- `test_all_solvers_agree` — all three solvers agree on count and (for unique puzzles) solution across the full 43-puzzle test corpus; uses `limit=1` for solution retrieval to avoid the known backtrack-corruption of the basic solver's solution buffer when `limit > 1`

### Key implementation notes

- The basic solver's `solution` buffer is NOT restored on backtrack; it reflects the last assignment made, not the last complete solution. For `limit > 1`, only the COUNT is reliable; solution retrieval requires a separate `limit=1` call. This matches C++ behavior and is documented in test comments.
- The SIMD solver (`SOLUTION_MODE=0`) never stores a solution when `limit > 1`; use `limit=1` to retrieve the solution or use `SOLUTION_MODE=1` (via `config > 0` in the public API).
- `constrain` returns `false` for already-unique puzzles when BCP has propagated all remaining cells to single candidates (no candidates left to try adding). This matches C++ behavior and is documented in the test.
- `minimize(monotonic=false)` always returns `true` at end-of-loop; `false` is only returned on the `monotonic=true` path when a removable clue is found after an unremovable one.
- One pass of `minimize` is not guaranteed to produce a globally minimal puzzle (clue-removal order matters). Clue count is guaranteed non-increasing.

### Verification
- `cargo test --release` — 92 tests pass (45 lib unit + 41 edge case + 6 integration)
- All edge cases exercise correct behaviour without panics
- `test_sparse_puzzle_limited_stack` runs in release mode (skipped in debug via `#[cfg(not(debug_assertions))]`)

---

## Phase 13: Compare Output and Functionality Between Rust and C++ ✅

**Status:** Complete

### Files created/modified

| File | Description |
|------|-------------|
| `Cargo.toml` | Added `[features] debug-trace = []` and `[[bin]] debug_solver` entry. |
| `src/bin/debug_solver.rs` | **Fully implemented.** Mirrors `tdoku/src/debug_driver.cc`. Accepts `[puzzle] [limit]` CLI args (default puzzle = reference test puzzle, default limit = 2). Pads input to 82 bytes for correct vanilla/pencilmark detection. Calls `solver_dpll_triad_simd::solve(input, limit, 0)` — uses `SOLVER_NONE` (count-only) for `limit ≠ 1`, `SOLVER_LAST` (solution-storing) for `limit == 1`, mirroring C++ `TdokuSolverDpllTriadSimd`. Prints `count=N guesses=N solution=S` to stdout with empty `S` when count-only mode is active. |
| `src/solver_dpll_triad_simd.rs` | **Instrumented.** Added thread-local `DT_DEPTH` and `DT_EVENTS` counters (guarded by `#[cfg(feature = "debug-trace")]`). `dt_check_and_inc()` enforces the `DT_MAX = 2000` event cap. `dt_pcs()` formats the six band config popcounts. `count_solutions_consistent_with_partial_assignment()` emits `DT:C`, `DT:T`, `DT:S` with depth tracking (DT_IN/DT_OUT). `solve_sudoku()` emits `DT:INIT`. All trace lines go to stderr via `eprintln!`. |
| `debug/compare.sh` | **Fixed path computation.** Changed `RDOKU_DIR` and `TDOKU_DIR` to point to `<project_root>/rdoku` and `<project_root>/tdoku` respectively (was incorrectly going up two levels). |

### Step 1: debug_solver binary and debug-trace feature ✅

- `cargo build --release --features debug-trace --bin debug_solver` — clean build
- Local run on reference puzzle: `count=1 guesses=0 solution=` with 4 DT trace lines
- Local run with `limit=1` returns correct 81-char solution string

### Step 2: DT trace parity via `debug/compare.sh` ✅

All puzzle classes verified **byte-for-byte identical** traces:

| Puzzle class | Puzzle | Limit | C++ result | Rust result | DT lines | Verdict |
|---|---|---|---|---|---|---|
| Unique, medium difficulty | `.5..83.17...` (reference) | 2 | `count=1 guesses=0` | `count=1 guesses=0` | 4 | ✅ IDENTICAL |
| Hardest known (Al Escargot) | `1....7.9..3..2...` | 2 | `count=1 guesses=19` | `count=1 guesses=19` | 42 | ✅ IDENTICAL |
| Hardest known, solution | `1....7.9..3..2...` | 1 | `count=1 solution=162...` | `count=1 solution=162...` | 12 | ✅ IDENTICAL |
| Multi-solution (empty grid) | `.......(81 dots)` | 2 | `count=2 guesses=36` | `count=2 guesses=36` | 79 | ✅ IDENTICAL |
| Unsolvable | `11....(81 chars)` | 2 | `count=0 guesses=0` | `count=0 guesses=0` | 1 | ✅ IDENTICAL |
| Pencilmark format | 729-char from reference | 1 | `count=1 solution=...` | `count=1 solution=...` | 4 | ✅ IDENTICAL |

### Step 3: Corpus-level result comparison ✅

Shell loop running both containers on all 43 test puzzles (`tdoku/test/test_puzzles`):
- **count matches**: all 43 puzzles × `limit=100000` — zero mismatches
- **solution string matches**: all 18 unique-solution puzzles × `limit=1` — zero mismatches
- **guess count matches**: all 43 puzzles × `limit=100000` — zero mismatches (search trees are structurally identical)

### Step 4: Benchmark output ✅

- `./target/release/benchmark -w 1 -t 3 -n 50 -s tdoku <unique_puzzles>` — correct Markdown table format
- Performance on host: `tdoku` solver processes ~6,400 puzzles/sec in release mode

### Step 5: Generator output validation ✅

- `./target/release/generate -l 1 -p 0 -m 1 -e 0 -n 1` — produces valid 81-char minimized vanilla puzzle with `count=1` verified by `debug_solver`
- Both C++ and Rust correctly treat `'0'` as an empty cell marker (via `'1'–'9'` range check)

### Key implementation notes

- `dt_depth` is thread-local, not reset between calls — matching C++ behavior. For the single-call debug_solver binary this is always clean.
- `DT:T d=D best=4294967295` is emitted when `NONE = u32::MAX = 4294967295` is returned by `choose_band_and_value_to_branch`, matching C++ `%u` formatting of `UINT32_MAX`.
- The reference puzzle (`.5..83.17...`) is solved purely by propagation (no branching): `pcs=9,9,9,9,9,9` after initialization and `best=4294967295` immediately → 4 DT lines total.
- `compare.sh` path fix: `RDOKU_DIR` must point to the `rdoku/` subdirectory (containing `Cargo.toml`), not the project root.

### Verification
- `cargo test --release` — 92 tests pass (no regressions from debug-trace instrumentation)
- `debug/compare.sh` — all puzzle classes produce identical traces
- 43-puzzle corpus loop — zero mismatches on count, solution, and guess count
