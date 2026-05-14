//! Basic DPLL backtracking solver — port of `tdoku/src/solver_basic.cc`.
//!
//! Reference implementation. Maintains candidate bitmasks for each row,
//! column, and box and uses a minimum-candidates heuristic to choose the
//! next cell to assign.

use crate::bitutil::{clear_low_bit, get_low_bit, low_order_bit_index};
use std::cell::RefCell;

type Bits = u32;
const K_ALL: Bits = 0x1ff;

#[rustfmt::skip]
const BOXEN: [usize; 81] = [
    0, 0, 0, 1, 1, 1, 2, 2, 2,
    0, 0, 0, 1, 1, 1, 2, 2, 2,
    0, 0, 0, 1, 1, 1, 2, 2, 2,
    3, 3, 3, 4, 4, 4, 5, 5, 5,
    3, 3, 3, 4, 4, 4, 5, 5, 5,
    3, 3, 3, 4, 4, 4, 5, 5, 5,
    6, 6, 6, 7, 7, 7, 8, 8, 8,
    6, 6, 6, 7, 7, 7, 8, 8, 8,
    6, 6, 6, 7, 7, 7, 8, 8, 8,
];

struct SolverBasic {
    rows: [Bits; 9],
    cols: [Bits; 9],
    boxes: [Bits; 9],
    cells_todo: Vec<(usize, usize, usize)>,
    limit: usize,
    min_heuristic: bool,
    /// Index of the last element in `cells_todo` (i.e. `len - 1`).
    num_todo: usize,
    num_guesses: usize,
    num_solutions: usize,
    solution: [u8; 81],
}

impl SolverBasic {
    fn new() -> Self {
        Self {
            rows: [K_ALL; 9],
            cols: [K_ALL; 9],
            boxes: [K_ALL; 9],
            cells_todo: Vec::new(),
            limit: 1,
            min_heuristic: false,
            num_todo: 0,
            num_guesses: 0,
            num_solutions: 0,
            solution: [0u8; 81],
        }
    }

    fn num_candidates(&self, (row, col, bx): (usize, usize, usize)) -> u32 {
        (self.rows[row] & self.cols[col] & self.boxes[bx]).count_ones()
    }

    /// Swaps the cell with the fewest candidates to `cells_todo[todo_index]`.
    fn move_best_todo_to_front(&mut self, todo_index: usize) {
        let mut best = todo_index;
        let mut best_count = self.num_candidates(self.cells_todo[todo_index]);
        let mut i = todo_index + 1;
        while best_count > 1 && i < self.cells_todo.len() {
            let count = self.num_candidates(self.cells_todo[i]);
            if count < best_count {
                best_count = count;
                best = i;
            }
            i += 1;
        }
        self.cells_todo.swap(todo_index, best);
    }

    fn satisfy_given_partial_assignment(&mut self, todo_index: usize) {
        if self.min_heuristic {
            self.move_best_todo_to_front(todo_index);
        }

        let (row, col, bx) = self.cells_todo[todo_index];
        let mut candidates = self.rows[row] & self.cols[col] & self.boxes[bx];

        while candidates != 0 {
            let candidate = get_low_bit(candidates);

            // Only count as a guess when there are multiple candidates.
            if candidates ^ candidate != 0 {
                self.num_guesses += 1;
            }

            self.rows[row] ^= candidate;
            self.cols[col] ^= candidate;
            self.boxes[bx] ^= candidate;
            self.solution[row * 9 + col] = b'1' + low_order_bit_index(candidate) as u8;

            if todo_index < self.num_todo {
                self.satisfy_given_partial_assignment(todo_index + 1);
            } else {
                self.num_solutions += 1;
            }

            if self.num_solutions == self.limit {
                return;
            }

            // Restore candidates for backtracking.
            self.rows[row] ^= candidate;
            self.cols[col] ^= candidate;
            self.boxes[bx] ^= candidate;

            candidates = clear_low_bit(candidates);
        }
    }

    fn initialize(&mut self, input: &[u8], limit: usize, configuration: u32) -> bool {
        self.rows = [K_ALL; 9];
        self.cols = [K_ALL; 9];
        self.boxes = [K_ALL; 9];
        self.limit = limit;
        self.min_heuristic = configuration > 0;
        self.num_guesses = 0;
        self.num_solutions = 0;
        self.cells_todo.clear();

        // Validate input: reject strings with invalid characters (anything
        // other than '1'–'9', '.', or '0').
        // '0' is an alternate empty-cell marker used by some puzzle formats.
        let len = input.len().min(81);
        if input[..len]
            .iter()
            .any(|&b| !matches!(b, b'0'..=b'9' | b'.'))
        {
            return false;
        }

        // Pad short inputs with '.' so we always have 81 bytes to work with.
        let mut buf = [b'.'; 81];
        let copy_len = input.len().min(81);
        buf[..copy_len].copy_from_slice(&input[..copy_len]);

        // Copy all 81 bytes to solution; blanks will be overwritten during search.
        self.solution.copy_from_slice(&buf);

        for row in 0..9 {
            for col in 0..9 {
                let cell = row * 9 + col;
                let bx = BOXEN[cell];
                let ch = buf[cell];
                // Treat anything that isn't a digit '1'-'9' as an empty cell.
                if (b'1'..=b'9').contains(&ch) {
                    let value: u32 = 1u32 << (ch - b'1') as u32;
                    if self.rows[row] & value != 0
                        && self.cols[col] & value != 0
                        && self.boxes[bx] & value != 0
                    {
                        self.rows[row] ^= value;
                        self.cols[col] ^= value;
                        self.boxes[bx] ^= value;
                    } else {
                        return false; // Contradiction in givens.
                    }
                } else {
                    self.cells_todo.push((row, col, bx));
                }
            }
        }

        self.num_todo = self.cells_todo.len().saturating_sub(1);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Puzzle from the tdoku test suite — uniquely solvable.
    const PUZZLE: &[u8] =
        b".5..83.17...1..4..3.4..56.8....3...9.9.8245....6....7...9....5...729..861.36.72.4";
    const SOLUTION: &[u8] =
        b"652483917978162435314975628825736149791824563436519872269348751547291386183657294";

    #[test]
    fn test_solve_basic_no_heuristic() {
        let (count, sol, _guesses) = solve(PUZZLE, 1, 0);
        assert_eq!(count, 1);
        assert_eq!(&sol, SOLUTION);
    }

    #[test]
    fn test_solve_basic_with_heuristic() {
        let (count, sol, _guesses) = solve(PUZZLE, 1, 1);
        assert_eq!(count, 1);
        assert_eq!(&sol, SOLUTION);
    }

    #[test]
    fn test_invalid_puzzle() {
        // Two 1s in the same row — contradiction in givens.
        let bad81 =
            b"11...............................................................................";
        assert_eq!(bad81.len(), 81);
        let (count, _sol, _guesses) = solve(bad81, 1, 0);
        assert_eq!(count, 0);
    }
}

/// Solve a Sudoku puzzle.
///
/// `input` must be exactly 81 bytes: digits `'1'`–`'9'` for givens, `'.'` for
/// blanks.  Returns `(num_solutions, solution, num_guesses)`.  `solution` is
/// only meaningful when `num_solutions > 0`.  When `limit > 1` the solver
/// counts up to `limit` solutions but only returns the last one found.
pub fn solve(input: &[u8], limit: usize, config: u32) -> (usize, [u8; 81], usize) {
    // Mirrors the C++ `static SolverBasic solver;` — reuse allocations across calls.
    thread_local! {
        static SOLVER: RefCell<SolverBasic> = RefCell::new(SolverBasic::new());
    }

    SOLVER.with(|cell| {
        let mut solver = cell.borrow_mut();
        if solver.initialize(input, limit, config) {
            if !solver.cells_todo.is_empty() {
                solver.satisfy_given_partial_assignment(0);
            } else {
                // Fully specified puzzle with no blanks.
                solver.num_solutions = 1;
            }
            (solver.num_solutions, solver.solution, solver.num_guesses)
        } else {
            (0, [0u8; 81], 0)
        }
    })
}
