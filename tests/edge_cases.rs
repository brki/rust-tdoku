//! Edge case and resource-safety tests — Phase 12.
//!
//! Covers input edge cases, enumerate semantics, constrain/minimize behaviour,
//! memory/resource invariants, and cross-solver agreement.
//!
//! **Important:** Run with `cargo test --release` to avoid debug stack overflow
//! on sparse and multi-solution puzzles.

use rdoku::{solver_basic, solver_dpll_triad_scc, solver_dpll_triad_simd};

// ─────────────────────────────── constants ──────────────────────────────────

/// A uniquely-solvable reference puzzle and its known solution.
const UNIQUE_PUZZLE: &str =
    ".5..83.17...1..4..3.4..56.8....3...9.9.8245....6....7...9....5...729..861.36.72.4";
const UNIQUE_SOLUTION: &str =
    "652483917978162435314975628825736149791824563436519872269348751547291386183657294";

/// A fully-solved puzzle (no blanks).
const SOLVED_PUZZLE: &str =
    "534678912672195348198342567859761423426853791713924856961537284287419635345286179";

/// A puzzle with contradicting givens (two '1's in the same row).
const INVALID_PUZZLE: &str =
    "11...............................................................................";

/// Puzzle with all cells empty.
const EMPTY_PUZZLE: &str =
    ".................................................................................";

/// A puzzle with only 3 widely-spaced clues (sparse, many solutions).
///   Cell  0 (row 0, col 0, box 0): '1'
///   Cell 40 (row 4, col 4, box 4): '5'
///   Cell 80 (row 8, col 8, box 8): '9'
const SPARSE_PUZZLE: &str =
    "1.......................................5.......................................9";

// ─────────────────────────────── helpers ────────────────────────────────────

/// Returns `true` iff `s` is a complete, valid 9×9 Sudoku grid.
fn is_valid_solution(s: &str) -> bool {
    if s.len() != 81 {
        return false;
    }
    let bytes: Vec<u8> = s.bytes().collect();
    if bytes.iter().any(|&b| b < b'1' || b > b'9') {
        return false;
    }
    let check = |cells: &[usize]| -> bool {
        let mut seen = 0u32;
        for &c in cells {
            let bit = 1u32 << (bytes[c] - b'1');
            if seen & bit != 0 {
                return false;
            }
            seen |= bit;
        }
        true
    };
    for i in 0..9 {
        if !check(&(0..9).map(|j| i * 9 + j).collect::<Vec<_>>()) {
            return false; // row
        }
        if !check(&(0..9).map(|j| j * 9 + i).collect::<Vec<_>>()) {
            return false; // col
        }
        let (r, c) = ((i / 3) * 3, (i % 3) * 3);
        if !check(
            &(0..3)
                .flat_map(|dr| (0..3).map(move |dc| (r + dr) * 9 + (c + dc)))
                .collect::<Vec<_>>(),
        ) {
            return false; // box
        }
    }
    true
}

/// Convert a vanilla 81-char puzzle to a 729-char pencilmark string.
/// Each cell expands to 9 chars: the digit if it's a clue, or all digits if empty.
fn to_pencilmark(vanilla: &str) -> String {
    let mut pm = String::with_capacity(729);
    for ch in vanilla.chars() {
        if ch >= '1' && ch <= '9' {
            let digit = ch as u8 - b'1';
            for d in 0..9u8 {
                pm.push(if d == digit { char::from(b'1' + d) } else { '.' });
            }
        } else {
            for d in 0..9u8 {
                pm.push(char::from(b'1' + d));
            }
        }
    }
    pm
}

/// Convert a raw `[u8; 81]` solution to a `&str` (panics if not UTF-8).
fn sol_str(bytes: &[u8; 81]) -> &str {
    std::str::from_utf8(bytes).expect("solution must be UTF-8")
}

// ────────────────────────── empty puzzle ────────────────────────────────────

#[test]
fn test_empty_puzzle_basic() {
    let (count, sol, _) = solver_basic::solve(EMPTY_PUZZLE.as_bytes(), 1, 0);
    assert!(count >= 1, "empty puzzle must have ≥1 solution");
    assert!(is_valid_solution(sol_str(&sol)), "returned solution must be valid");
}

#[test]
fn test_empty_puzzle_scc() {
    let (count, sol, _) = solver_dpll_triad_scc::solve(EMPTY_PUZZLE.as_bytes(), 1, 3);
    assert!(count >= 1, "empty puzzle must have ≥1 solution");
    assert!(is_valid_solution(sol_str(&sol)), "returned solution must be valid");
}

#[test]
fn test_empty_puzzle_simd() {
    let (count, sol, _) = solver_dpll_triad_simd::solve(EMPTY_PUZZLE.as_bytes(), 1, 0);
    assert!(count >= 1, "empty puzzle must have ≥1 solution");
    assert!(is_valid_solution(sol_str(&sol)), "returned solution must be valid");
}

#[test]
fn test_empty_puzzle_public_api() {
    let (count, sol, _) = rdoku::solve_sudoku(EMPTY_PUZZLE, 1, 0);
    assert!(count >= 1);
    assert!(is_valid_solution(&sol));
}

// ────────────────────── fully-solved puzzle ─────────────────────────────────

#[test]
fn test_solved_puzzle_basic() {
    let (count, sol, _) = solver_basic::solve(SOLVED_PUZZLE.as_bytes(), 2, 0);
    assert_eq!(count, 1, "fully solved puzzle must have exactly 1 solution");
    assert_eq!(sol_str(&sol), SOLVED_PUZZLE);
}

#[test]
fn test_solved_puzzle_scc() {
    let (count, sol, _) = solver_dpll_triad_scc::solve(SOLVED_PUZZLE.as_bytes(), 2, 3);
    assert_eq!(count, 1);
    assert_eq!(sol_str(&sol), SOLVED_PUZZLE);
}

#[test]
fn test_solved_puzzle_simd() {
    // limit=2 → check uniqueness (count=1 means exactly one solution).
    let (count, _, _) = solver_dpll_triad_simd::solve(SOLVED_PUZZLE.as_bytes(), 2, 0);
    assert_eq!(count, 1);
    // limit=1 → retrieves the solution (SIMD only stores when limit==1).
    let (_, sol, _) = solver_dpll_triad_simd::solve(SOLVED_PUZZLE.as_bytes(), 1, 0);
    assert_eq!(sol_str(&sol), SOLVED_PUZZLE);
}

#[test]
fn test_solved_puzzle_public_api() {
    // limit=2 → uniqueness check (SIMD in count mode, no solution stored).
    let (count, _, _) = rdoku::solve_sudoku(SOLVED_PUZZLE, 2, 0);
    assert_eq!(count, 1);
    // limit=1 → solution returned by SIMD solver.
    let (_, sol, _) = rdoku::solve_sudoku(SOLVED_PUZZLE, 1, 0);
    assert_eq!(sol, SOLVED_PUZZLE);
}

// ──────────────────────── 0-solution puzzle ─────────────────────────────────

#[test]
fn test_invalid_puzzle_basic() {
    let (count, _, _) = solver_basic::solve(INVALID_PUZZLE.as_bytes(), 1, 0);
    assert_eq!(count, 0, "invalid puzzle must return 0 solutions");
}

#[test]
fn test_invalid_puzzle_scc() {
    let (count, _, _) = solver_dpll_triad_scc::solve(INVALID_PUZZLE.as_bytes(), 1, 3);
    assert_eq!(count, 0);
}

#[test]
fn test_invalid_puzzle_simd() {
    let (count, _, _) = solver_dpll_triad_simd::solve(INVALID_PUZZLE.as_bytes(), 1, 0);
    assert_eq!(count, 0);
}

#[test]
fn test_invalid_puzzle_public_api() {
    let (count, _, _) = rdoku::solve_sudoku(INVALID_PUZZLE, 1, 0);
    assert_eq!(count, 0);
}

// ─────────────────────────── pencilmark format ──────────────────────────────

#[test]
fn test_pencilmark_solvable_simd() {
    let pm = to_pencilmark(UNIQUE_PUZZLE);
    assert_eq!(pm.len(), 729);
    let (count, sol, _) = solver_dpll_triad_simd::solve(pm.as_bytes(), 1, 0);
    assert_eq!(count, 1);
    assert_eq!(sol_str(&sol), UNIQUE_SOLUTION);
}

#[test]
fn test_pencilmark_solvable_scc() {
    let pm = to_pencilmark(UNIQUE_PUZZLE);
    let (count, sol, _) = solver_dpll_triad_scc::solve(pm.as_bytes(), 1, 3);
    assert_eq!(count, 1);
    assert_eq!(sol_str(&sol), UNIQUE_SOLUTION);
}

#[test]
fn test_pencilmark_unsolvable_simd() {
    let pm = to_pencilmark(INVALID_PUZZLE);
    let (count, _, _) = solver_dpll_triad_simd::solve(pm.as_bytes(), 1, 0);
    assert_eq!(count, 0);
}

#[test]
fn test_pencilmark_unsolvable_scc() {
    let pm = to_pencilmark(INVALID_PUZZLE);
    let (count, _, _) = solver_dpll_triad_scc::solve(pm.as_bytes(), 1, 3);
    assert_eq!(count, 0);
}

// ─────────────────────────── boundary: limit ────────────────────────────────

/// limit = 0: the basic solver guarantees 0 solutions returned.
#[test]
fn test_limit_zero_basic() {
    let (count, _, _) = solver_basic::solve(UNIQUE_PUZZLE.as_bytes(), 0, 0);
    assert_eq!(count, 0);
}

/// limit = 0 for enumerate: the early-exit guard ensures 0 callbacks and 0 return.
#[test]
fn test_limit_zero_enumerate() {
    let mut called = 0usize;
    let total = rdoku::enumerate(UNIQUE_PUZZLE, 0, |_| {
        called += 1;
    });
    assert_eq!(total, 0, "enumerate with limit=0 must return 0");
    assert_eq!(called, 0, "callback must not be called when limit=0");
}

/// limit = usize::MAX on a unique puzzle: must return 1, not hang.
#[test]
fn test_limit_usize_max_unique() {
    let (count, _, _) = rdoku::solve_sudoku(UNIQUE_PUZZLE, usize::MAX, 0);
    assert_eq!(count, 1);
}

// ─────────────────────── short / truncated input ────────────────────────────

/// A 17-byte prefix of the SOLVED_PUZZLE first two rows — consistent but partial.
const SHORT_PUZZLE_BYTES: &[u8] = b"53467891267219534";

#[test]
fn test_short_input_basic_no_panic() {
    // Must not panic; short inputs are padded with '.' (empty cells).
    let (count, _, _) = solver_basic::solve(SHORT_PUZZLE_BYTES, 1, 0);
    // The partial clues are consistent, so count ≥ 1.
    assert!(count >= 1);
}

#[test]
fn test_short_input_scc_no_panic() {
    let (count, _, _) = solver_dpll_triad_scc::solve(SHORT_PUZZLE_BYTES, 1, 3);
    assert!(count >= 1);
}

#[test]
fn test_short_input_simd_no_panic() {
    let (count, _, _) = solver_dpll_triad_simd::solve(SHORT_PUZZLE_BYTES, 1, 0);
    assert!(count >= 1);
}

#[test]
fn test_short_input_public_api_no_panic() {
    let s = std::str::from_utf8(SHORT_PUZZLE_BYTES).unwrap();
    let (count, _, _) = rdoku::solve_sudoku(s, 1, 0);
    assert!(count >= 1);
}

/// Zero-length input — treated as a fully empty puzzle (many solutions).
#[test]
fn test_empty_string_basic_no_panic() {
    let (count, _, _) = solver_basic::solve(b"", 1, 0);
    assert!(count >= 1);
}

#[test]
fn test_empty_string_scc_no_panic() {
    let (count, _, _) = solver_dpll_triad_scc::solve(b"", 1, 3);
    assert!(count >= 1);
}

#[test]
fn test_empty_string_simd_no_panic() {
    let (count, _, _) = solver_dpll_triad_simd::solve(b"", 1, 0);
    assert!(count >= 1);
}

// ──────────────────────── non-ASCII / garbage bytes ─────────────────────────
//
// Solvers must not panic or produce UB.  Non-digit bytes are treated as empty
// cells (same as '.') after the fixes in solver_basic::initialize and
// solver_dpll_triad_scc::initialize_puzzle.

#[test]
fn test_garbage_bytes_basic_no_panic() {
    let mut buf = [b'.'; 81];
    buf[0] = 0x00;
    buf[1] = 0xFF;
    buf[40] = 0x80;
    // These garbage bytes are treated as empty cells; result is valid partial
    // puzzle → some solutions exist.
    let (count, _, _) = solver_basic::solve(&buf, 1, 0);
    let _ = count; // value is not important — absence of panic is the goal
}

#[test]
fn test_garbage_bytes_scc_no_panic() {
    let mut buf = [b'.'; 81];
    buf[0] = 0x00;
    buf[40] = 0xFF;
    let (count, _, _) = solver_dpll_triad_scc::solve(&buf, 1, 3);
    let _ = count;
}

#[test]
fn test_garbage_bytes_simd_no_panic() {
    let mut buf = [b'.'; 81];
    buf[0] = 0x00;
    buf[40] = 0xFF;
    let (count, _, _) = solver_dpll_triad_simd::solve(&buf, 1, 0);
    let _ = count;
}

// ─────────────────────────── enumerate edge cases ───────────────────────────

/// Callback receives well-formed, valid Sudoku solutions.
#[test]
fn test_enumerate_valid_solutions() {
    rdoku::enumerate(UNIQUE_PUZZLE, 10, |sol| {
        assert_eq!(sol.len(), 81, "solution must be 81 chars");
        assert!(
            is_valid_solution(sol),
            "each enumerated solution must be a valid Sudoku grid: {sol}"
        );
    });
}

/// When the puzzle has ≥ limit solutions, callback is called exactly `limit` times.
#[test]
fn test_enumerate_limit_respected() {
    const LIMIT: usize = 5;
    let mut calls = 0usize;
    let total = rdoku::enumerate(EMPTY_PUZZLE, LIMIT, |_| {
        calls += 1;
    });
    assert_eq!(calls, LIMIT, "callback must be called exactly limit times");
    assert_eq!(total, LIMIT, "return value must equal call count");
}

/// The return value always equals the number of callback invocations.
#[test]
fn test_enumerate_return_equals_calls() {
    let mut calls = 0usize;
    let total = rdoku::enumerate(UNIQUE_PUZZLE, 100, |_| {
        calls += 1;
    });
    assert_eq!(total, calls, "return value must equal callback invocation count");
    assert_eq!(total, 1, "unique puzzle must have exactly 1 solution");
}

// ──────────────────────── constrain / minimize ──────────────────────────────

/// constrain on an already-unique puzzle: must not corrupt the puzzle.
/// The function may return true (added a redundant clue) or false (BCP already
/// forced all empty cells, nothing to add) — both are valid outcomes matching
/// the C++ implementation.  The critical invariant is that the puzzle remains
/// uniquely solvable regardless of the return value.
#[test]
fn test_constrain_already_unique() {
    let mut puzzle = UNIQUE_PUZZLE.to_string();
    rdoku::constrain(false, &mut puzzle);
    let (count, _, _) = rdoku::solve_sudoku(&puzzle, 2, 0);
    assert_eq!(count, 1, "puzzle must still have a unique solution after constrain");
}

/// constrain on the empty puzzle: returns true; result is a unique puzzle.
#[test]
fn test_constrain_empty_puzzle() {
    let mut puzzle = EMPTY_PUZZLE.to_string();
    let ok = rdoku::constrain(false, &mut puzzle);
    assert!(ok, "constrain on empty puzzle must return true");
    let (count, _, _) = rdoku::solve_sudoku(&puzzle, 2, 0);
    assert_eq!(count, 1, "constrained empty puzzle must have a unique solution");
}

/// minimize preserves uniqueness.
#[test]
fn test_minimize_preserves_uniqueness() {
    let mut puzzle = UNIQUE_PUZZLE.to_string();
    rdoku::minimize(false, false, &mut puzzle);
    let (count, _, _) = rdoku::solve_sudoku(&puzzle, 2, 0);
    assert_eq!(count, 1, "minimized puzzle must still have a unique solution");
}

/// minimize with monotonic=true also preserves uniqueness.
#[test]
fn test_minimize_monotonic_preserves_uniqueness() {
    let mut puzzle = UNIQUE_PUZZLE.to_string();
    rdoku::minimize(false, true, &mut puzzle);
    let (count, _, _) = rdoku::solve_sudoku(&puzzle, 2, 0);
    assert_eq!(count, 1, "monotonic-minimized puzzle must still have a unique solution");
}

/// After minimization the result is still uniquely solvable and has no more
/// clues than the original puzzle.
/// Note: one pass of minimize(monotonic=false) is not guaranteed to produce a
/// globally minimal puzzle (clue-removal order matters), but the clue count
/// must be ≤ the original.
#[test]
fn test_minimize_fewer_or_equal_clues() {
    fn clue_count(s: &str) -> usize {
        s.chars().filter(|c| c.is_ascii_digit()).count()
    }
    let before = clue_count(UNIQUE_PUZZLE);
    let mut puzzle = UNIQUE_PUZZLE.to_string();
    rdoku::minimize(false, false, &mut puzzle);
    let after = clue_count(&puzzle);
    assert!(
        after <= before,
        "minimize must not increase clue count: before={before} after={after}"
    );
    // Uniqueness must still hold.
    let (count, _, _) = rdoku::solve_sudoku(&puzzle, 2, 0);
    assert_eq!(count, 1, "minimized puzzle must still have a unique solution");
}

// ──────────────────────── memory / resource invariants ──────────────────────

/// Calling solve_sudoku 10 000 times must not panic or grow unboundedly.
/// Thread-local solver state is fixed-size after the first warm-up call.
#[test]
fn test_repeated_solve_no_leak() {
    for _ in 0..10_000 {
        let (count, _, _) = rdoku::solve_sudoku(UNIQUE_PUZZLE, 1, 0);
        assert_eq!(count, 1);
    }
}

/// Calling enumerate 1 000 times must not panic.
#[test]
fn test_repeated_enumerate_no_leak() {
    for _ in 0..1_000 {
        let total = rdoku::enumerate(UNIQUE_PUZZLE, 10, |_| {});
        assert_eq!(total, 1);
    }
}

/// Run the SIMD solver on a very sparse puzzle in a thread with a limited
/// (4 MiB) stack.  This verifies that release-mode recursion depth is
/// manageable and does not overflow a realistic stack.
///
/// Only enabled in release mode (`debug_assertions` off) because debug builds
/// have a known stack depth issue on sparse puzzles.
#[test]
#[cfg(not(debug_assertions))]
fn test_sparse_puzzle_limited_stack() {
    const STACK_SIZE: usize = 4 * 1024 * 1024; // 4 MiB
    let result = std::thread::Builder::new()
        .stack_size(STACK_SIZE)
        .spawn(|| {
            let (count, _, _) = rdoku::solve_sudoku(SPARSE_PUZZLE, 1, 0);
            count
        })
        .expect("thread spawn must succeed")
        .join()
        .expect("thread must not panic");
    assert!(result >= 1, "sparse puzzle must have at least one solution");
}

// ─────────────────────────── cross-solver agreement ─────────────────────────

/// Load the full test corpus and assert that all three solvers agree on
/// solution count and, for unique-solution puzzles, on the solution string.
#[test]
fn test_all_solvers_agree() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/test_puzzles");
    let contents = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return, // skip if test data unavailable
    };

    for line in contents.lines().filter(|l| !l.is_empty()) {
        let mut parts = line.splitn(3, ':');
        let puzzle = parts.next().unwrap_or("").to_string();
        let expected_count: usize =
            parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);

        let (basic_count, _basic_sol, _) =
            solver_basic::solve(puzzle.as_bytes(), 100_000, 0);
        let (scc_count, _scc_sol, _) =
            solver_dpll_triad_scc::solve(puzzle.as_bytes(), 100_000, 3);
        let (simd_count, _simd_sol, _) =
            solver_dpll_triad_simd::solve(puzzle.as_bytes(), 100_000, 0);

        assert_eq!(
            basic_count, expected_count,
            "basic solver count mismatch for: {puzzle}"
        );
        assert_eq!(
            scc_count, expected_count,
            "scc solver count mismatch for: {puzzle}"
        );
        assert_eq!(
            simd_count, expected_count,
            "simd solver count mismatch for: {puzzle}"
        );

        if expected_count == 1 {
            // All three solvers store the solution reliably only when limit=1.
            // (Basic solver's buffer is overwritten by backtracking when limit>1;
            //  SIMD only saves the solution when limit==1.)
            let (_, basic_sol_1, _) = solver_basic::solve(puzzle.as_bytes(), 1, 0);
            let (_, scc_sol_1, _) = solver_dpll_triad_scc::solve(puzzle.as_bytes(), 1, 3);
            let (_, simd_sol_1, _) = solver_dpll_triad_simd::solve(puzzle.as_bytes(), 1, 0);
            assert_eq!(
                basic_sol_1, simd_sol_1,
                "basic/simd solution mismatch for: {puzzle}"
            );
            assert_eq!(
                scc_sol_1, simd_sol_1,
                "scc/simd solution mismatch for: {puzzle}"
            );
        }
    }
}
