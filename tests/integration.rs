//! Integration tests — port of `tdoku/test/run_tests.cc`.
//!
//! Reads `tdoku/test/test_puzzles` (format: `puzzle:count:solution`)
//! and verifies all three solver implementations produce the correct
//! solution counts and solution strings.
//!
//! **Important:** Run with `cargo test --release` to avoid stack overflow on
//! multi-solution puzzles in debug builds.

use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Test data helpers
// ---------------------------------------------------------------------------

struct TestCase {
    puzzle: String,
    expected_count: usize,
    /// Present only when `expected_count == 1`.
    expected_solution: Option<String>,
}

fn test_puzzles_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/test_puzzles")
}

fn load_test_cases() -> Vec<TestCase> {
    let path = test_puzzles_path();
    let contents = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read {path:?}: {e}"));

    contents
        .lines()
        .filter(|l| !l.is_empty())
        .map(|line| {
            let mut parts = line.splitn(3, ':');
            let puzzle = parts
                .next()
                .unwrap_or_else(|| panic!("Missing puzzle field in: {line}"))
                .to_string();
            let count_str = parts
                .next()
                .unwrap_or_else(|| panic!("Missing count field in: {line}"));
            let expected_count = count_str
                .parse::<usize>()
                .unwrap_or_else(|_| panic!("Bad count field '{count_str}' in: {line}"));
            let expected_solution = if expected_count == 1 {
                parts.next().map(|s| s.to_string())
            } else {
                None
            };
            TestCase { puzzle, expected_count, expected_solution }
        })
        .collect()
}

/// Convert a raw `[u8; 81]` solution buffer to a `String`.
fn sol_to_string(bytes: &[u8; 81]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

// ---------------------------------------------------------------------------
// Regression test (kept from Phase 10 stub)
// ---------------------------------------------------------------------------

/// Regression test for the clear_low_bit bug:
/// the SIMD solver must not infinite-loop on sparse puzzles.
#[test]
fn test_minimize_complete_solution() {
    let puzzle = ".5..83.17...1..4..3.4..56.8....3...9.9.8245....6....7...9....5...729..861.36.72.4";
    let (count, _sol, _) = rdoku::solve_sudoku(puzzle, 2, 0);
    assert_eq!(count, 1);

    let easy = "534678912672195348198342567859761423426853791713924856961537284287419635.45286179";
    let (ec, _, _) = rdoku::solve_sudoku(easy, 2, 0);
    assert_eq!(ec, 1);

    let puzzle24 = "....83.17...1..4..3.4..56.8....3...9.9.8245....6....7...9....5...729..861.36.72.4";
    let (c24, _, _) = rdoku::solve_sudoku(puzzle24, 2, 0);
    assert!(c24 >= 1);
}

// ---------------------------------------------------------------------------
// Per-solver corpus tests
// ---------------------------------------------------------------------------

/// Test the basic DPLL solver against every puzzle in the test corpus.
#[test]
fn test_basic_solver() {
    for tc in load_test_cases() {
        let (count, _, _) =
            rdoku::solver_basic::solve(tc.puzzle.as_bytes(), 100_000, 0);
        assert_eq!(
            count,
            tc.expected_count,
            "basic solver: count mismatch for puzzle {}",
            tc.puzzle
        );
        if let Some(ref expected_sol) = tc.expected_solution {
            let (_, sol_bytes, _) =
                rdoku::solver_basic::solve(tc.puzzle.as_bytes(), 1, 0);
            assert_eq!(
                &sol_to_string(&sol_bytes),
                expected_sol,
                "basic solver: solution mismatch for puzzle {}",
                tc.puzzle
            );
        }
    }
}

/// Test the DPLL-triad-SCC solver (config=3: both inference + heuristic).
#[test]
fn test_scc_solver() {
    for tc in load_test_cases() {
        let (count, _, _) =
            rdoku::solver_dpll_triad_scc::solve(tc.puzzle.as_bytes(), 100_000, 3);
        assert_eq!(
            count,
            tc.expected_count,
            "scc solver: count mismatch for puzzle {}",
            tc.puzzle
        );
        if let Some(ref expected_sol) = tc.expected_solution {
            let (_, sol_bytes, _) =
                rdoku::solver_dpll_triad_scc::solve(tc.puzzle.as_bytes(), 1, 3);
            assert_eq!(
                &sol_to_string(&sol_bytes),
                expected_sol,
                "scc solver: solution mismatch for puzzle {}",
                tc.puzzle
            );
        }
    }
}

/// Test the DPLL-triad-SIMD solver against every puzzle in the test corpus.
#[test]
fn test_simd_solver() {
    for tc in load_test_cases() {
        let (count, _, _) =
            rdoku::solver_dpll_triad_simd::solve(tc.puzzle.as_bytes(), 100_000, 0);
        assert_eq!(
            count,
            tc.expected_count,
            "simd solver: count mismatch for puzzle {}",
            tc.puzzle
        );
        if let Some(ref expected_sol) = tc.expected_solution {
            let (_, sol_bytes, _) =
                rdoku::solver_dpll_triad_simd::solve(tc.puzzle.as_bytes(), 1, 0);
            assert_eq!(
                &sol_to_string(&sol_bytes),
                expected_sol,
                "simd solver: solution mismatch for puzzle {}",
                tc.puzzle
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Public API tests
// ---------------------------------------------------------------------------

/// Test the `rdoku::solve_sudoku` public API against every puzzle in the corpus.
#[test]
fn test_public_api_solve() {
    for tc in load_test_cases() {
        let (count, _, _) = rdoku::solve_sudoku(&tc.puzzle, 100_000, 0);
        assert_eq!(
            count,
            tc.expected_count,
            "public API solve: count mismatch for puzzle {}",
            tc.puzzle
        );
        if let Some(ref expected_sol) = tc.expected_solution {
            let (_, sol_str, _) = rdoku::solve_sudoku(&tc.puzzle, 1, 0);
            assert_eq!(
                &sol_str,
                expected_sol,
                "public API solve: solution mismatch for puzzle {}",
                tc.puzzle
            );
        }
    }
}

/// Test the `rdoku::enumerate` public API: callback count must equal return value
/// and both must equal `expected_count` for every puzzle in the corpus.
///
/// Note: run with `--release` to avoid stack overflow on multi-solution puzzles.
#[test]
fn test_public_api_enumerate() {
    for tc in load_test_cases() {
        let mut callback_count = 0usize;
        let ret = rdoku::enumerate(&tc.puzzle, 100_000, |_sol| {
            callback_count += 1;
        });
        assert_eq!(
            ret,
            tc.expected_count,
            "enumerate return value mismatch for puzzle {}",
            tc.puzzle
        );
        assert_eq!(
            callback_count,
            tc.expected_count,
            "enumerate callback count mismatch for puzzle {}",
            tc.puzzle
        );
    }
}
