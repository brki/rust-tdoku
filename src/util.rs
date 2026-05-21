//! Random-number and puzzle-permutation utilities — port of `tdoku/src/util.h` / `util.cc`.
//!
//! Provides [`Util`] for seeded random number generation and structure-preserving
//! Sudoku permutations (digit / row / column / band shuffles).

use rand::prelude::*;
use rand::rngs::SmallRng;

/// Open a path that must be a regular file, returning a buffered reader.
///
/// Returns `Err` with a human-readable message if the path does not exist,
/// is not a regular file, or cannot be opened.
pub fn open_regular_file(path: &str) -> Result<std::io::BufReader<std::fs::File>, String> {
    match std::fs::metadata(path) {
        Ok(m) if m.is_file() && m.len() == 0 => {
            Err(format!("'{}' is empty (0 bytes)", path))
        }
        Ok(m) if m.is_file() => std::fs::File::open(path)
            .map(std::io::BufReader::new)
            .map_err(|e| format!("cannot open '{}': {}", path, e)),
        Ok(_) => Err(format!("'{}' is not a regular file", path)),
        Err(e) => Err(format!("cannot access '{}': {}", path, e)),
    }
}

/// Wraps a seeded RNG and provides Sudoku-specific permutation helpers.
pub struct Util {
    rng: SmallRng,
}

impl Util {
    /// Create a new `Util` seeded from the thread-local RNG (which is OS-seeded).
    pub fn new() -> Self {
        Self {
            rng: SmallRng::from_rng(&mut rand::rng()),
        }
    }

    /// Re-seed the RNG with a fixed value for reproducible output.
    pub fn random_seed(&mut self, seed: u64) {
        self.rng = SmallRng::seed_from_u64(seed);
    }

    /// Return a uniformly-distributed random `u32`.
    pub fn random_uint(&mut self) -> u32 {
        self.rng.random()
    }

    /// Return a uniformly-distributed random `f64` in `[0.0, 1.0)`.
    pub fn random_double(&mut self) -> f64 {
        self.rng.random()
    }

    /// Return a Fisher-Yates shuffled permutation of `0..size`.
    pub fn permutation(&mut self, size: usize) -> Vec<usize> {
        let mut perm: Vec<usize> = (0..size).collect();
        perm.shuffle(&mut self.rng);
        perm
    }

    /// Shuffle `vec` so that bands (groups of 3) may be reordered and rows/columns
    /// within a band may be reordered, but rows/columns may not cross band boundaries.
    pub fn block_shuffle(&mut self, vec: &mut [usize; 9]) {
        let mut blocks = [0usize, 1, 2];
        blocks.shuffle(&mut self.rng);
        for i in 0..3 {
            let mut block = [0usize, 1, 2];
            block.shuffle(&mut self.rng);
            for j in 0..3 {
                vec[i * 3 + j] = blocks[i] * 3 + block[j];
            }
        }
    }

    /// Apply a structure-preserving random permutation to a Sudoku puzzle in-place.
    ///
    /// When `pencilmark` is `false`, `puzzle` is a 81-byte ASCII string (`'1'`–`'9'`
    /// or `'.'`).  When `pencilmark` is `true`, `puzzle` is a 729-byte string (9
    /// cells per cell, one char per candidate).
    pub fn permute_sudoku(&mut self, puzzle: &mut [u8], pencilmark: bool) {
        let mut digit_perm = [0usize, 1, 2, 3, 4, 5, 6, 7, 8];
        digit_perm.shuffle(&mut self.rng);

        let mut row_perm = [0usize; 9];
        let mut col_perm = [0usize; 9];
        self.block_shuffle(&mut col_perm);
        self.block_shuffle(&mut row_perm);

        let row_size: usize = if pencilmark { 81 } else { 9 };
        let puzzle_size = row_size * 9;
        let mut out = vec![0u8; puzzle_size];

        for row in 0..9 {
            for col in 0..9 {
                if pencilmark {
                    for digit in 0..9 {
                        let src = puzzle[row * 81 + col * 9 + digit];
                        let eliminated = src == b'.';
                        out[row_perm[row] * 81 + col_perm[col] * 9 + digit_perm[digit]] =
                            if eliminated {
                                b'.'
                            } else {
                                b'1' + digit_perm[digit] as u8
                            };
                    }
                } else {
                    let ch = puzzle[row * 9 + col];
                    let out_ch = if ch != b'.' {
                        b'1' + digit_perm[(ch - b'1') as usize] as u8
                    } else {
                        b'.'
                    };
                    out[row_perm[row] * 9 + col_perm[col]] = out_ch;
                }
            }
        }
        puzzle[..puzzle_size].copy_from_slice(&out);
    }
}

impl Default for Util {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PUZZLE: &[u8] =
        b".5..83.17...1..4..3.4..56.8....3...9.9.8245....6....7...9....5...729..861.36.72.4";
    const SOLUTION: &[u8] =
        b"652483917978162435314975628825736149791824563436519872269348751547291386183657294";

    #[test]
    fn test_permutation_length_and_elements() {
        let mut util = Util::new();
        util.random_seed(42);
        let perm = util.permutation(9);
        assert_eq!(perm.len(), 9);
        let mut sorted = perm.clone();
        sorted.sort();
        assert_eq!(sorted, vec![0, 1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn test_block_shuffle_valid() {
        let mut util = Util::new();
        util.random_seed(99);
        let mut vec = [0usize; 9];
        util.block_shuffle(&mut vec);
        // Each band's rows must come from the same original band.
        for band in 0..3 {
            let mut band_src: Vec<usize> =
                vec[band * 3..band * 3 + 3].iter().map(|&v| v / 3).collect();
            band_src.dedup();
            assert_eq!(
                band_src.len(),
                1,
                "rows in band {band} mixed across source bands"
            );
        }
        // All 9 indices must be distinct.
        let mut sorted = vec;
        sorted.sort();
        assert_eq!(sorted, [0, 1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn test_permute_sudoku_preserves_validity() {
        use crate::solver_basic;

        let mut util = Util::new();
        util.random_seed(1234);

        let mut puzzle: [u8; 81] = PUZZLE.try_into().unwrap();
        util.permute_sudoku(&mut puzzle, false);

        // Permuted puzzle must still be uniquely solvable.
        let (count, sol, _) = solver_basic::solve(&puzzle, 1, 0);
        assert_eq!(count, 1, "permuted puzzle should still have 1 solution");

        // The solution must be a permutation of the original solution (same digit
        // frequencies).
        let mut orig_counts = [0u32; 9];
        let mut perm_counts = [0u32; 9];
        for &b in SOLUTION.iter() {
            orig_counts[(b - b'1') as usize] += 1;
        }
        for &b in sol.iter() {
            perm_counts[(b - b'1') as usize] += 1;
        }
        orig_counts.sort();
        perm_counts.sort();
        assert_eq!(orig_counts, perm_counts);
    }
}
