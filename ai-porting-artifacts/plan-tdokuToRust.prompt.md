# Plan: Rust Port of tdoku → rdoku

## TL;DR
Port all three tdoku C++ Sudoku solver implementations (basic, SCC, SIMD), grid/utility infrastructure, puzzle generator, and benchmark runner to idiomatic Rust in the crate root. SIMD solver uses `wide` crate plus `std::arch` supplements for operations `wide` doesn't expose (shuffle_epi8, minpos_epu8, movemask_epi8). All three solvers plus full tooling.

The latest stable version of rust should be used.

---

## Decisions
- **Solvers**: All three (basic, dpll_triad_scc, dpll_triad_simd)
- **SIMD**: `wide` crate as primary, `std::arch::x86_64` for unavailable ops (shuffle, minpos, movemask), scalar fallbacks for non-x86
- **Scope**: Full parity (solver lib + benchmark binary + generate binary)
- **API**: Idiomatic Rust (no C FFI)
- **Test data**: Symlink/reference `tdoku/test/test_puzzles` from integration tests

---

## Phase 1: Scaffold & Foundation
1. Create `Cargo.toml` — `[lib]` + `[[bin]]` targets; deps: `wide = "0.7"`, `rand = "0.8"`, `once_cell = "1"` (or use `std::sync::OnceLock` ≥1.70)
2. Create `src/lib.rs` — module declarations for all submodules; public re-exports of solver functions
3. Create `src/bitutil.rs` — port `bitutil.h`:
   - `num_bits_set(x: u32) -> u32` → `x.count_ones()`
   - `get_low_bit(x: u32) -> u32` → `x & x.wrapping_neg()`
   - `clear_low_bit(x: u32) -> u32` → `x & (x - 1)`
   - `low_order_bit_index(x: u32) -> u32` → `x.trailing_zeros()`
   - `high_order_bit_index(x: u32) -> u32` → `31 - x.leading_zeros()`

---

## Phase 2: SIMD Vector Abstraction *(blocks Phases 6 & 7)*
4. Create `src/simd_vectors.rs` — port `simd_vectors.h`:
   - `Bitvec08x16` wrapping `[u16; 8]` (8×16-bit lanes, 128-bit total) — matching C++ `Bitvec08x16` which is `__m128i` of 8×u16
   - `Bitvec16x16` wrapping `[u16; 16]` (16×16-bit lanes, 256-bit logical) — matching C++ `Bitvec16x16`
   - On x86_64 with SSSE3: use `std::arch::x86_64::_mm_shuffle_epi8` via `#[target_feature(enable="ssse3")]` unsafe fns
   - On x86_64 with SSE4.1: use `_mm_testz_si128`, `_mm_blend_epi16`, `_mm_minpos_epu8`
   - Scalar fallback paths for non-x86 (correctness over performance)
   - Key methods to implement (matching C++ API):
     - `All(value) -> Self`, `and_not(other) -> Self`
     - `AllZero() -> bool`, `AnyZero() -> bool`, `SubsetOf(other) -> bool`, `Intersects(other) -> bool`
     - `WhichEqual(other) -> Self`, `WhichNonZero() -> Self`, `AnyLessThan(other) -> bool`
     - `Popcounts9() -> Self` (count bits within 9-bit masks per lane)
     - `Shuffle(control: Self) -> Self` (PSHUFB semantics — requires `std::arch`)
     - `RotateRows() -> Self`, `RotateCols() -> Self`, `RotateRows2() -> Self`, `RotateCols2() -> Self`
     - `RotateCols() for Bitvec08x16` (used in BandEliminate)
     - `MinPosGreaterThanOrEqual(threshold: u16) -> u32` (uses `_mm_minpos_epu8`)
     - `GetLowBit() -> Self`, `ClearLowBit() -> Self`, `Popcount() -> u32`
     - `GetLo() -> Bitvec08x16`, `GetHi() -> Bitvec08x16` (split 256→128)
     - `Insert(pos: usize, val: u16)` (mutable set element)
     - `X_Y_and_Z_or`, `X_Y_andnot_Z_or`, `X_Y_or_Z_or`, `X_Y_xor_Z_or` (ternary logic helpers)

---

## Phase 3: Basic Solver *(depends on Phase 1)*
5. Create `src/solver_basic.rs` — port `solver_basic.cc`:
   - `SolverBasic` struct: `rows: [u32; 9]`, `cols: [u32; 9]`, `boxes: [u32; 9]`, `cells_todo: Vec<(usize, usize, usize)>` (row,col,box)
   - `num_candidates()`, `move_best_todo_to_front()`, `satisfy_given_partial_assignment()` (recursive DPLL)
   - `initialize()`, `solve(input: &str, limit: usize, config: u32) -> (usize, [u8;81], usize)`

---

## Phase 4: Utilities *(depends on Phase 1)*
6. Create `src/util.rs` — port `util.h/cc`:
   - `Util` struct using `rand::rngs::SmallRng` (or `StdRng`)
   - `random_seed(seed: u64)`, `random_uint() -> u32`, `random_double() -> f64`
   - `permutation(size: usize) -> Vec<usize>` (Fisher-Yates shuffle)
   - `block_shuffle(vec: &mut [usize; 9])`, `permute_sudoku(puzzle: &mut [u8], pencilmark: bool)`

---

## Phase 5: Grid Library *(depends on Phase 4)*
7. Create `src/grid_lib.rs` — port `grid_lib.h/cc`:
   - `get_pattern(pattern_id: usize) -> String`
   - `get_grid(grid_id: usize, index: usize) -> String`
   - `enumerate_grids(callback: impl FnMut(&str))`

---

## Phase 6: SCC Solver *(depends on Phase 1)*
8. Create `src/solver_dpll_triad_scc.rs` — port `solver_dpll_triad_scc.cc`:
   - `FastBitset<const N: usize>` using `[u64; ...]`
   - `State` struct: `asserted: FastBitset`, `clause_free_literals: Vec<u16>`, `implication_counts: [u16; NUM_LITERALS]`, `num_asserted: u32`
   - `SolverDpllTriadScc` struct with clause/literal maps, implication lists
   - `setup_constraints()` — builds ExactlyN constraints for cells, triads, bands
   - BCP propagation loop, SCC heuristic, DPLL search
   - `solve(input: &str, limit: usize) -> (usize, [u8;81], usize)`

---

## Phase 7: SIMD Solver *(depends on Phase 2, most complex)*
9. Create `src/solver_dpll_triad_simd.rs` — port `solver_dpll_triad_simd.cc`:
   - Struct types: `Box { cells: Bitvec16x16 }`, `Band { configurations: Bitvec08x16, eliminations: Bitvec08x16 }`, `State { bands: [[Band;3];2], boxen: [Box;9] }`, `BoxIndexing`
   - `Tables` struct with all static lookup tables — initialize once via `OnceLock<Tables>`
     - `cell_assignment_eliminations: [Bitvec16x16; 9*16]`
     - `peer_x_elem_to_config_mask`, `triads_shift{0,1,2}_to_config_elims`, shuffle control vectors
     - `shuffle_configs_to_triads`, `pos_triads_to_candidates`, `row_rotate_3x3_{1,2}`
     - `one_value_mask`, `box_peers`, `div3`, `mod3`, `box_indexing`
   - Core functions:
     - `box_restrict<const FROM_VERTICAL: bool>(state, box_idx, candidates) -> bool`
     - `gather_triad_clause_assertions(cells, rotate_fn, assertions)`
     - `assertions_to_eliminations(assertions, box_i, box_j, box_elims, h_band_elims, v_band_elims)`
     - `band_eliminate<const VERTICAL: bool>(state, band_idx, from_peer) -> bool`
     - `configurations_to_positive_triads(configs) -> Bitvec16x16`
     - `positive_triads_to_box_candidates(triads, orientation) -> Bitvec16x16`
     - `choose_band_and_value_to_branch(state) -> (u32, Bitvec08x16)`
     - `branch_on_band_and_value<const VERTICAL: bool>(band_idx, value_mask, state)`
     - `count_solutions_consistent_with_partial_assignment(state)`
     - `init_clue(input, state, pos)`, `initialize(input, limit) -> Option<State>`
     - `solve(input: &[u8], limit: usize, config: u32) -> (usize, [u8;81], usize)`
   - Generic template `solution_mode` → use const generic `SOLUTION_MODE: u8` in Rust

---

## Phase 8: Public API *(depends on Phases 3, 6, 7)*
10. Finalize `src/lib.rs`:
    - `pub fn solve_sudoku(input: &str, limit: usize, config: u32) -> (usize, String, usize)` (calls SIMD solver)
    - `pub fn enumerate(puzzle: &str, limit: usize, callback: impl FnMut(&str))  -> usize`
    - `pub fn constrain(pencilmark: bool, puzzle: &mut String) -> bool`
    - `pub fn minimize(pencilmark: bool, monotonic: bool, puzzle: &mut String) -> bool`

---

## Phase 9: Generator Binary *(depends on Phase 8)*
11. Create `src/bin/generate.rs` — port `generate.cc`:
    - `Generator` struct with `Util`, pattern heap
    - `has_unique_solution()`, `init_empty()`, `add_clue()`, `drop_clues()`, `minimize()`, `generate_puzzle()`
    - CLI args for `max_puzzles`, `minimize`, `pencilmark`, etc.
    - Note: Omit Gurobi/MiniSat difficulty evaluation (use rdoku SIMD solver for uniqueness only)

---

## Phase 10: Benchmark Binary *(depends on Phase 8)*
12. Create `src/bin/benchmark.rs` — port `run_benchmark.cc`:
    - `Options` struct matching C++ options
    - `Benchmark` struct: `load(filename)`, `warmup()`, `run()`, `print_results()`
    - Load puzzle file, replicate/sample to `test_dataset_size` (default 100K)
    - Warmup phase (default 4s), test phase (default 10s)
    - Report puzzles/sec, solution count, guess count
    - CLI using `std::env::args()` (or `clap` crate if preferred)
    - Benchmark all three registered solvers (basic, scc, simd)

---

## Phase 11: Integration Tests *(depends on Phase 8)*
13. Create `tests/integration.rs` — port `run_tests.cc`:
    - Read `tdoku/test/test_puzzles` (format: `puzzle:expected_count:solution`)
    - Run all three solvers (`solver_basic::solve`, `solver_dpll_triad_scc::solve`, `solver_dpll_triad_simd::solve`) on every puzzle
    - Verify solution counts match `expected_count` for every puzzle × solver
    - Verify returned solution string matches for every puzzle × solver (when `limit == 1`)
    - Also test the public API (`rdoku::solve_sudoku`, `rdoku::enumerate`) against the same corpus
    - Add `#[test]` per solver so failures are clearly attributed (e.g. `test_basic_solver`, `test_scc_solver`, `test_simd_solver`, `test_public_api`)
    - Use `cargo test --release` in CI to avoid debug stack overflow on harder puzzles

---

## Phase 12: Edge Case & Memory Tests *(depends on Phase 11)*
14. Add `tests/edge_cases.rs` — exhaustive correctness and resource-safety tests:

    ### Input edge cases (all three solvers + public API)
    - **Empty puzzle** (`"."×81`): must return a valid completed grid, count ≥ 1
    - **Fully solved puzzle** (all 81 clues valid): must return count = 1, solution = input
    - **0-solution puzzle** (two clashing clues): must return count = 0, no panic
    - **Pencilmark format** (729-char): basic solvable and unsolvable cases
    - **Boundary `limit`**: `limit = 0` → 0 solutions, no callback; `limit = usize::MAX` → doesn't hang for unique puzzle
    - **Short/truncated input**: input shorter than 81 bytes — must not panic (solver pads with dots)
    - **Non-ASCII / garbage bytes** in clue positions — must not panic or produce UB

    ### `enumerate` edge cases
    - Callback receives valid 81-byte solution strings (all digits, rows/cols/boxes each have 1–9)
    - Callback called exactly `limit` times when puzzle has ≥ `limit` solutions
    - Callback count always equals return value

    ### `constrain` / `minimize` edge cases
    - `constrain` on already-unique puzzle: returns `true`, puzzle unchanged
    - `constrain` on empty puzzle: returns `true`, result has unique solution
    - `minimize` on minimal puzzle (no clue removable): returns `false`
    - `minimize` with `monotonic = true` terminates faster than `monotonic = false`

    ### Memory / resource invariants
    - Call `solve_sudoku` 10 000 times in a loop: measure heap via `jemalloc` or `dhat` (or simply assert no unbounded growth using a custom allocator counter or by checking that thread-local state sizes are fixed)
    - Alternatively: `cargo test --features dhat-heap` using the `dhat` crate to assert total heap bytes after N iterations is ≤ initial + constant
    - Thread-local `SOLVER_NONE`, `SOLVER_LAST`, `GENERATOR`, and the SCC/basic thread-locals must not allocate unboundedly across repeated solve calls
    - Stack depth: run solver on a sparse puzzle (`< 5 clues`) in a thread with a known stack size (e.g. 4 MiB) in release mode and verify it completes without overflow

    ### Cross-solver agreement
    - For every puzzle in `test_puzzles`, all three solvers must agree on solution count and solution string (already in Phase 11, but add a single `test_all_solvers_agree` test that iterates the corpus and `assert_eq!` across solvers)

---

## Phase 13: Compare output and functionality between Rust and C++ implementation *(depends on Phase 12)*
15. Compare the output of the compiled programs with various flags. The `PROJECT_ROOT/debug/docker-compose.yml` file can be used to build / run the C++ executables.

    ### Step 1: Add `debug_solver` binary and `debug-trace` feature to `Cargo.toml`

    The `debug/Dockerfile.rdoku` already references `cargo build --release --features debug-trace --bin debug_solver`, but neither the binary nor the feature exist yet.

    - Add `[features] debug-trace = []` to `Cargo.toml`
    - Add `[[bin]] name = "debug_solver" path = "src/bin/debug_solver.rs"` to `Cargo.toml`
    - Create `src/bin/debug_solver.rs` — mirrors `tdoku/src/debug_driver.cc`:
      - Accept `[puzzle] [limit]` CLI args with the same defaults as C++ (`DEFAULT_PUZZLE` and `limit=2`)
      - Call `solver_dpll_triad_simd::solve(input, limit, 0)` (config 0 = count-only mode, no solution stored when count > 1)
      - Print `count={N} guesses={N} solution={S}\n` to stdout — exactly matching C++ format:
        - `solution` field is the 81-char solution string when `count >= 1` and `limit == 1`, otherwise an **empty string** (not dots — C++ emits empty because it zero-initializes the buffer and only copies when a solution is found)
    - Instrument `solver_dpll_triad_simd.rs` with `#[cfg(feature = "debug-trace")]` macros to emit DT trace lines to stderr, matching C++ format exactly:
      - `DT:INIT ok={0|1} pcs={b0},{b1},{b2},{b3},{b4},{b5}` — emitted after initialization; `ok=1` if puzzle loaded without contradiction; `pcs` = 6 band configuration counts (3 horizontal + 3 vertical)
      - `DT:C d={depth} pcs={b0},{b1},{b2},{b3},{b4},{b5}` — emitted at the start of each `count_solutions_consistent_with_partial_assignment` call; `pcs` are the same 6 band config counts at current state
      - `DT:T d={depth} best={best_value}` — emitted just before each branch; `best` is the value index chosen by `choose_band_and_value_to_branch`

    ### Step 2: Verify DT trace parity using `debug/compare.sh`

    Run `debug/compare.sh [puzzle] [limit]` for each puzzle class below. The script captures `DT:` lines from both containers, diffs them, and reports divergence. All traces must be **byte-for-byte identical**.

    | Puzzle class | Example | Limit | Expected |
    |---|---|---|---|
    | Unique, medium difficulty | `.5..83.17...` (compare.sh default) | 2 | `count=1 guesses=N solution=<81chars>` |
    | Hardest known (Al Escargot) | `800000000003600000...` | 2 | `count=1 guesses=N solution=<81chars>` |
    | Multi-solution | `....................` (empty) | 2 | `count=2 guesses=N solution=` |
    | Unsolvable | `11...` (two 1s in same row) | 2 | `count=0 guesses=0 solution=` |
    | Pencilmark format | 729-char string | 1 | `count=1 guesses=N solution=<81chars>` |

    For each run, verify:
    - `trace_diff.txt` is empty (traces identical)
    - `result_tdoku.txt` and `result_rdoku.txt` are identical

    ### Step 3: Corpus-level result comparison

    Run both solvers over the full `tdoku/test/test_puzzles` corpus (43 puzzles: 18 unique, 10 unsolvable, 15 multi-solution) and verify:
    - **count** matches for every puzzle × limit combination
    - **solution string** matches for every unique-solution puzzle (limit=1)
    - **guess count** matches for every puzzle (the search tree must be structurally identical)

    This can be done with a shell loop using the docker images, or by a new `#[test] fn test_guess_counts_match_cpp()` in `tests/integration.rs` that reads the expected guess counts from a golden file generated by the C++ solver.

    ### Step 4: Benchmark output comparison

    Run both benchmark binaries on the same puzzle file (e.g. `tdoku/test/test_puzzles` filtered to unique-solution puzzles) and confirm:
    - The Markdown table format is identical (column widths, separator line, solver ID column)
    - The CSV format is parseable with the same fields
    - Performance: the `tdoku` (SIMD) solver row in `rdoku` should be within 2× of the C++ `tdoku` solver row on the same hardware
    - The `_tdev_basic` row ratios between rdoku and C++ should be within 2× as well

    ### Step 5: Generator output validation

    Note: The generators are **not** expected to produce identical puzzle sequences because `rand::rngs::SmallRng` uses a different algorithm than the C++ `rand()` / `mt19937` RNG. Validate functional equivalence instead:
    - Both produce valid minimal sudoku puzzles (unique solution, no redundant clues)
    - Clue count distribution is similar (typically 20–30 clues for minimized vanilla puzzles)
    - `rdoku generate -l 100 | cargo run --release --bin benchmark -- -w 0 -t 1 -n 100 -s tdoku /dev/stdin` produces a solvable dataset (no count=0 failures)

---

## Relevant Files (source)
- `tdoku/include/tdoku.h` — public API reference
- `tdoku/src/bitutil.h` — bit ops
- `tdoku/src/simd_vectors.h` — `Bitvec08x16`, `Bitvec16x16` with all intrinsics
- `tdoku/src/solver_basic.cc` — ~250 lines, basic DPLL
- `tdoku/src/solver_dpll_triad_scc.cc` — ~500 lines, SAT/BCP/SCC solver
- `tdoku/src/solver_dpll_triad_simd.cc` — ~700 lines, SIMD DPLL (most critical)
- `tdoku/src/util.h` / `util.cc` — Util class
- `tdoku/src/grid_lib.h` / `grid_lib.cc` — grid utilities
- `tdoku/src/generate.cc` — puzzle generator
- `tdoku/src/run_benchmark.cc` — benchmark runner
- `tdoku/test/run_tests.cc` — test runner
- `tdoku/test/test_puzzles` — test data file

## Relevant Files (to create)
- `Cargo.toml`
- `src/lib.rs`
- `src/bitutil.rs`
- `src/simd_vectors.rs`
- `src/solver_basic.rs`
- `src/util.rs`
- `src/grid_lib.rs`
- `src/solver_dpll_triad_scc.rs`
- `src/solver_dpll_triad_simd.rs`
- `src/bin/benchmark.rs`
- `src/bin/generate.rs`
- `tests/integration.rs`

---

## Verification
1. `cd rdoku && cargo check` — no compile errors
2. `cargo test` — all integration tests pass against `tdoku/test/test_puzzles`
3. Basic solver: solve known puzzle, verify 1 solution returned and solution string matches
4. SIMD solver: same puzzle gives same solution as basic solver
5. SCC solver: same
6. `cargo run --release --bin benchmark -- tdoku/test/test_puzzles` — outputs benchmark table
7. `cargo run --release --bin generate -- --help` — CLI works

---

## Further Considerations
1. **`wide` gap**: `_mm_shuffle_epi8`, `_mm_minpos_epu8`, `_mm_movemask_epi8` are NOT in the `wide` crate. These must be implemented via `std::arch::x86_64` intrinsics with scalar fallbacks. The `wide` crate is still useful for other operations.
2. **Tables initialization**: C++ uses a global `const Tables tables{}` constructor. Rust equivalent: `static TABLES: OnceLock<Tables>` initialized on first use, or a `lazy_static!`/`once_cell::sync::Lazy`.
3. **Template `solution_mode`**: C++ `SolverDpllTriadSimd<solution_mode>` becomes a Rust const generic `SolverDpllTriadSimd<const SOLUTION_MODE: u8>` or an enum parameter.
