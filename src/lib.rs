//! rdoku — Rust port of tdoku, a high-performance Sudoku solver.
//!
//! The primary public API mirrors tdoku's interface:
//! - [`solve_sudoku`] — solve a puzzle using the fastest (SIMD) solver
//! - [`enumerate`] — find all solutions up to a given limit
//! - [`constrain`] — add random clues until the solution is unique
//! - [`minimize`] — remove clues while keeping the solution unique
//!
//! Three solver implementations are provided in sub-modules:
//! - [`solver_basic`] — simple DPLL backtracking (reference implementation)
//! - [`solver_dpll_triad_scc`] — DPLL with triad + SCC heuristics
//! - [`solver_dpll_triad_simd`] — DPLL with triads, SIMD constraint propagation (fastest)

pub mod bitutil;
pub mod grid_lib;
pub mod simd_vectors;
pub mod solver_basic;
pub mod solver_dpll_triad_scc;
pub mod solver_dpll_triad_simd;
pub mod util;

use std::cell::RefCell;

thread_local! {
    static GENERATOR: RefCell<solver_dpll_triad_simd::GeneratorDpllTriadSimd> =
        RefCell::new(solver_dpll_triad_simd::GeneratorDpllTriadSimd::default());
}

/// Solve a Sudoku puzzle using the SIMD DPLL triad solver.
///
/// `input` must be an 81-character string with digits `'1'`–`'9'` for given clues and `'.'`
/// for empty cells, or a 729-character pencilmark string.
///
/// Returns `(num_solutions, solution_string, num_guesses)`.  The `solution_string` is
/// meaningful only when `limit == 1` or `config > 0`.
pub fn solve_sudoku(input: &str, limit: usize, config: u32) -> (usize, String, usize) {
    let (count, sol_bytes, guesses) =
        solver_dpll_triad_simd::solve(input.as_bytes(), limit, config);
    let sol_str = String::from_utf8_lossy(&sol_bytes).into_owned();
    (count, sol_str, guesses)
}

/// Enumerate all solutions of a puzzle up to `limit`, calling `callback` for each solution.
///
/// `input` must be an 81-character (or 729-character pencilmark) puzzle string.
/// Returns the total number of solutions found (capped at `limit`).
pub fn enumerate(puzzle: &str, limit: usize, mut callback: impl FnMut(&str)) -> usize {
    solver_dpll_triad_simd::enumerate(puzzle.as_bytes(), limit, |sol_bytes| {
        let s = std::str::from_utf8(sol_bytes).unwrap_or("");
        callback(s);
    })
}

/// Add random clues to `puzzle` until it has a unique solution.
///
/// `pencilmark` selects pencilmark (729-char) vs. vanilla (81-char) format.
/// Returns `true` if the puzzle was successfully constrained to a unique solution.
pub fn constrain(pencilmark: bool, puzzle: &mut String) -> bool {
    let mut bytes = puzzle.as_bytes().to_vec();
    let result = GENERATOR.with(|g| g.borrow_mut().constrain(pencilmark, &mut bytes));
    *puzzle = String::from_utf8(bytes)
        .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
    result
}

/// Remove clues from `puzzle` while keeping a unique solution.
///
/// `pencilmark` selects pencilmark (729-char) vs. vanilla (81-char) format.
/// `monotonic` — when `true`, stop as soon as a removed clue must be restored.
/// Returns `true` if any clue was successfully dropped.
pub fn minimize(pencilmark: bool, monotonic: bool, puzzle: &mut String) -> bool {
    let mut bytes = puzzle.as_bytes().to_vec();
    let result = GENERATOR.with(|g| g.borrow_mut().minimize(pencilmark, monotonic, &mut bytes));
    *puzzle = String::from_utf8(bytes)
        .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    const PUZZLE: &str =
        ".5..83.17...1..4..3.4..56.8....3...9.9.8245....6....7...9....5...729..861.36.72.4";
    const SOLUTION: &str =
        "652483917978162435314975628825736149791824563436519872269348751547291386183657294";

    #[test]
    fn test_solve_sudoku_unique() {
        let (count, sol, _) = solve_sudoku(PUZZLE, 1, 0);
        assert_eq!(count, 1);
        assert_eq!(sol, SOLUTION);
    }

    #[test]
    fn test_solve_sudoku_count_only() {
        let (count, _, _) = solve_sudoku(PUZZLE, 2, 0);
        assert_eq!(count, 1);
    }

    #[test]
    fn test_enumerate_single_solution() {
        let mut solutions = Vec::new();
        let count = enumerate(PUZZLE, 10, |s| solutions.push(s.to_string()));
        assert_eq!(count, 1);
        assert_eq!(solutions.len(), 1);
        assert_eq!(solutions[0], SOLUTION);
    }

    #[test]
    fn test_enumerate_respects_limit() {
        // limit=5 on a unique puzzle: finds exactly 1 solution (not 5).
        let mut found = 0usize;
        let count = enumerate(PUZZLE, 5, |_| found += 1);
        assert_eq!(count, found, "callback count must match return value");
        assert_eq!(count, 1);

        // limit=0: no solutions returned regardless of puzzle.
        let mut found0 = 0usize;
        let count0 = enumerate(PUZZLE, 0, |_| found0 += 1);
        assert_eq!(count0, 0);
        assert_eq!(found0, 0);

        // limit=1 stops after finding 1 solution even if more exist.
        // Remove one clue from PUZZLE: position 72 ('1' → '.') → 2+ solutions.
        // With limit=1 we should get exactly 1 callback and return value 1.
        let multi: String = PUZZLE
            .char_indices()
            .map(|(i, c)| if i == 72 { '.' } else { c })
            .collect();
        let mut found1 = 0usize;
        let count1 = enumerate(&multi, 1, |_| found1 += 1);
        assert_eq!(count1, found1, "callback count must match return value");
        assert_eq!(count1, 1, "limit=1 must yield exactly 1 solution");
    }
}
