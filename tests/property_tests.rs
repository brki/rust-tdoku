//! Property-based tests using `proptest`.
//!
//! Generates random Sudoku puzzle fragments from a known seed solution and
//! verifies that all three solvers agree on solution counts.

use proptest::prelude::*;

/// A known-valid, fully-solved Sudoku grid.
const SEED_SOLUTION: &str =
    "652483917978162435314975628825736149791824563436519872269348751547291386183657294";

/// Strategy: start from the seed solution and keep a random subset of cells
/// as clues.
fn puzzle_from_seed() -> impl Strategy<Value = String> {
    (25usize..=70).prop_flat_map(|keep_count| {
        proptest::sample::subsequence((0usize..81).collect::<Vec<_>>(), keep_count..=keep_count)
            .prop_map(|indices| {
                let seed = SEED_SOLUTION.as_bytes();
                let mut puzzle = vec![b'.'; 81];
                for &idx in &indices {
                    puzzle[idx] = seed[idx];
                }
                String::from_utf8(puzzle).unwrap()
            })
    })
}

#[test]
fn solvers_agree() {
    let mut runner = proptest::test_runner::TestRunner::new(ProptestConfig {
        cases: 256,
        ..ProptestConfig::default()
    });
    runner
        .run(&puzzle_from_seed(), |puzzle| {
            let input = puzzle.as_bytes();
            let (c1, _, _) = rdoku::solve_sudoku(&puzzle, 10, 0);
            let (c2, _, _) = rdoku::solver_basic::solve(input, 10, 0);
            let (c3, _, _) = rdoku::solver_dpll_triad_scc::solve(input, 10, 3);
            prop_assert_eq!(c1, c2);
            prop_assert_eq!(c1, c3);
            Ok(())
        })
        .unwrap();
}

#[test]
fn minimize_produces_unique_puzzle() {
    let mut puzzle = SEED_SOLUTION.to_string();
    let orig = puzzle.bytes().filter(|&b| b != b'.').count();
    rdoku::minimize(false, false, &mut puzzle);
    let new = puzzle.bytes().filter(|&b| b != b'.').count();
    let (count, _, _) = rdoku::solve_sudoku(&puzzle, 2, 0);
    assert_eq!(count, 1);
    assert!(new <= orig);
}

#[test]
fn constrain_on_complete_grid() {
    let mut puzzle = SEED_SOLUTION.to_string();
    rdoku::constrain(false, &mut puzzle);
    let (count, _, _) = rdoku::solve_sudoku(&puzzle, 2, 0);
    assert_eq!(count, 1);
}
