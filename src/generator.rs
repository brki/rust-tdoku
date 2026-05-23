//! Puzzle generator — core logic extracted from `tdoku/src/generate.cc`.
//!
//! Provides a reusable [`Generator`] struct that implements the pool-based
//! hill-climbing search. Callers drive the loop via callbacks:
//! - [`Generator::run_accepted`] — callback fires for each puzzle accepted into the pool.
//! - [`Generator::run_all`] — callback fires for every *evaluated* puzzle (used by the
//!   `--all` / `-a 1` mode in the `generate` binary).
//!
//! Formatting helpers [`format_pretty`] and [`format_puzzle_json`] are also
//! exported so both the CLI and network clients can render puzzles consistently.

use crate::util::Util;
use std::collections::HashSet;
use std::io::BufRead;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

// ── options ────────────────────────────────────────────────────────────────

/// All tunable parameters for the pool-based hill-climbing generator.
///
/// These control *generation* behaviour (pool shape, scoring, format).
/// Output presentation (pretty-printing, JSON wrapping, solution appending)
/// lives outside this struct in the binary-specific option types.
#[derive(Clone, Debug)]
pub struct GeneratorOptions {
    /// Weight for the clue-count term in the loss function.
    /// Higher → prefer puzzles with fewer clues.
    pub clue_weight: f64,
    /// Exponent scaling the solver-guess reward.
    /// Higher → prefer puzzles that require more guesses (harder).
    pub guess_weight: f64,
    /// Weight for uniform random noise added to the loss.
    /// 0.0 = fully deterministic greedy selection.
    pub random_weight: f64,
    /// Number of clues randomly removed before re-completing each iteration.
    pub clues_to_drop: usize,
    /// Number of permuted solves used to estimate `geo_mean_guesses`.
    /// 0 skips difficulty evaluation entirely.
    pub num_evals: usize,
    /// Number of puzzles to keep in the hill-climbing pool.
    pub num_puzzles_in_pool: usize,
    /// Whether to minimize (remove redundant clues) before scoring.
    pub do_minimize: bool,
    /// `true` = pencilmark format (729 chars); `false` = vanilla (81 chars).
    pub pencilmark: bool,
}

impl Default for GeneratorOptions {
    fn default() -> Self {
        Self {
            clue_weight: 1.0,
            guess_weight: 0.5,
            random_weight: 1.0,
            clues_to_drop: 3,
            num_evals: 10,
            num_puzzles_in_pool: 500,
            do_minimize: true,
            pencilmark: true,
        }
    }
}

// ── output type ────────────────────────────────────────────────────────────

/// A single puzzle produced by the generator.
#[derive(Clone, Debug)]
pub struct GeneratedPuzzle {
    /// The puzzle string (81 chars vanilla or 729 chars pencilmark).
    pub puzzle: String,
    /// Number of clues (given digits for vanilla; eliminations for pencilmark).
    pub num_clues: usize,
    /// Geometric mean of solver guesses across evaluation permutations.
    pub geo_mean_guesses: f64,
    /// Loss score used for pool selection (lower = better).
    pub loss: f64,
}

// ── internal pool entry ────────────────────────────────────────────────────

#[derive(Clone)]
struct PoolEntry {
    loss: f64,
    puzzle: String,
}

// ── generator ─────────────────────────────────────────────────────────────

/// Pool-based hill-climbing Sudoku puzzle generator.
///
/// Maintain a set of [`GeneratorOptions::num_puzzles_in_pool`] puzzle candidates.
/// Each iteration picks one at random, mutates it (drops clues, re-constrains,
/// minimizes), evaluates its difficulty, and replaces the worst pool entry if
/// the new puzzle scores better.
pub struct Generator {
    pub options: GeneratorOptions,
    util: Util,
    pool: Vec<PoolEntry>,
    pool_set: HashSet<String>,
    /// Shared flag — set to `false` to stop the generation loop gracefully.
    running: Arc<AtomicBool>,
}

impl Generator {
    /// Create a new generator with the given options.
    ///
    /// `running` is polled at the top of every loop iteration; set it to
    /// `false` (e.g. from a Ctrl-C handler) to stop generation cleanly.
    pub fn new(options: GeneratorOptions, running: Arc<AtomicBool>) -> Self {
        Self {
            options,
            util: Util::new(),
            pool: Vec::new(),
            pool_set: HashSet::new(),
            running,
        }
    }

    /// Seed the pool with a single minimal puzzle derived from the empty grid.
    ///
    /// Uses the basic cell-level solver (safe recursion depth ≤ 81) to produce
    /// a complete solution, then minimizes it.  The result is replicated to fill
    /// [`GeneratorOptions::num_puzzles_in_pool`] slots.
    pub fn init_empty(&mut self) {
        let initial_seed = self.make_seed();
        for _ in 0..self.options.num_puzzles_in_pool {
            self.pool.push(PoolEntry {
                loss: f64::MAX,
                puzzle: initial_seed.clone(),
            });
        }
    }

    /// Seed the pool from a file of existing puzzles.
    ///
    /// Lines starting with `'#'` and blank lines are skipped.  Puzzles are
    /// validated with a fast structural check; structurally invalid ones are
    /// skipped with a count reported to stderr.  If the file provides fewer
    /// puzzles than [`GeneratorOptions::num_puzzles_in_pool`], the pool is
    /// padded by cycling through the loaded puzzles.
    ///
    /// Exits the process with an error message if the file cannot be opened.
    pub fn load(&mut self, filename: &str) {
        let reader = match crate::util::open_regular_file(filename) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("error: {}", e);
                std::process::exit(1);
            }
        };
        let puzzle_size = if self.options.pencilmark { 729 } else { 81 };
        let mut skipped_invalid = 0usize;
        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => continue,
            };
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let puzzle: String = line.chars().take(puzzle_size).collect();
            if puzzle.len() < puzzle_size {
                continue;
            }
            if !self.is_valid_puzzle(puzzle.as_bytes()) {
                skipped_invalid += 1;
                continue;
            }
            let (_, _, loss) = self.evaluate(puzzle.as_bytes());
            self.pool.push(PoolEntry {
                loss,
                puzzle: puzzle.clone(),
            });
            self.pool_set.insert(puzzle);
        }
        if skipped_invalid > 0 {
            eprintln!(
                "skipped {} invalid puzzle{} (duplicate digit in row, column, or box)",
                skipped_invalid,
                if skipped_invalid == 1 { "" } else { "s" }
            );
        }

        // Pad the pool up to num_puzzles_in_pool by cycling through loaded puzzles.
        let target = self.options.num_puzzles_in_pool;
        if self.pool.len() < target {
            let seeds: Vec<String> = self.pool.iter().map(|e| e.puzzle.clone()).collect();
            let mut idx = 0usize;
            while self.pool.len() < target {
                self.pool.push(PoolEntry {
                    loss: f64::MAX,
                    puzzle: seeds[idx % seeds.len()].clone(),
                });
                idx += 1;
            }
        }
    }

    // ── main loop variants ─────────────────────────────────────────────

    /// Run the generator loop, calling `on_accepted` for each puzzle accepted
    /// into the pool.
    ///
    /// `on_accepted` receives ownership of the [`GeneratedPuzzle`] and returns
    /// `true` to continue or `false` to stop.  The loop also stops when the
    /// shared `running` flag is set to `false`.
    pub fn run_accepted<F>(&mut self, mut on_accepted: F)
    where
        F: FnMut(GeneratedPuzzle) -> bool,
    {
        self.run_inner(false, |p| on_accepted(p));
    }

    /// Run the generator loop in "display all" mode: `on_evaluated` is called
    /// for *every* evaluated puzzle regardless of whether it was accepted into
    /// the pool.
    ///
    /// `on_evaluated` receives ownership of the [`GeneratedPuzzle`] and returns
    /// `true` to continue or `false` to stop.  The loop also stops when the
    /// shared `running` flag is set to `false`.
    pub fn run_all<F>(&mut self, mut on_evaluated: F)
    where
        F: FnMut(GeneratedPuzzle) -> bool,
    {
        self.run_inner(true, |p| on_evaluated(p));
    }

    // ── internals ─────────────────────────────────────────────────────

    fn run_inner<F>(&mut self, display_all: bool, mut callback: F)
    where
        F: FnMut(GeneratedPuzzle) -> bool,
    {
        let puzzle_size = if self.options.pencilmark { 729 } else { 81 };

        loop {
            if !self.running.load(Ordering::SeqCst) {
                break;
            }
            if self.pool.is_empty() {
                break;
            }

            // Pick a random puzzle from the pool.
            let which = (self.util.random_uint() as usize) % self.pool.len();
            let pattern_puzzle = self.pool[which].puzzle.clone();
            let mut puzzle: Vec<u8> = pattern_puzzle.bytes().take(puzzle_size).collect();

            // Randomly drop clues to unconstrain the puzzle.
            if self.options.clues_to_drop > 0 {
                let perm = self.util.permutation(puzzle_size);
                let mut dropped = 0;
                for &j in &perm {
                    if dropped == self.options.clues_to_drop {
                        break;
                    }
                    if self.options.pencilmark {
                        if puzzle[j] == b'.' {
                            puzzle[j] = b'1' + (j % 9) as u8;
                            dropped += 1;
                        }
                    } else {
                        if puzzle[j] != b'.' {
                            puzzle[j] = b'.';
                            dropped += 1;
                        }
                    }
                }

                // Re-complete to a unique solution.
                let mut puzzle_str = String::from_utf8_lossy(&puzzle).into_owned();
                if !crate::constrain(self.options.pencilmark, &mut puzzle_str) {
                    continue;
                }
                if self.options.do_minimize {
                    crate::minimize(self.options.pencilmark, false, &mut puzzle_str);
                }
                puzzle = puzzle_str.into_bytes();
            }

            let puzzle_bytes = &puzzle[..puzzle_size.min(puzzle.len())];
            let (num_clues, geo_mean_guesses, loss) = self.evaluate(puzzle_bytes);
            let puzzle_str = String::from_utf8_lossy(puzzle_bytes).into_owned();

            // Skip duplicates.
            if self.options.clues_to_drop > 0 {
                let pattern_cmp = &pattern_puzzle[..puzzle_size.min(pattern_puzzle.len())];
                if puzzle_str == pattern_cmp {
                    continue;
                }
                if self.pool_set.contains(&puzzle_str) {
                    continue;
                }
            }

            let generated = GeneratedPuzzle {
                puzzle: puzzle_str.clone(),
                num_clues,
                geo_mean_guesses,
                loss,
            };

            if display_all {
                if !callback(generated) {
                    break;
                }
            }

            // Skip if loss is worse than the current worst in the pool.
            if loss > self.worst_loss() {
                continue;
            }

            if !display_all {
                let generated = GeneratedPuzzle {
                    puzzle: puzzle_str.clone(),
                    num_clues,
                    geo_mean_guesses,
                    loss,
                };
                if !callback(generated) {
                    break;
                }
            }

            // Accept: replace the worst pool entry.
            if let Some(worst_idx) = self.worst_idx() {
                let old = std::mem::replace(
                    &mut self.pool[worst_idx],
                    PoolEntry {
                        loss,
                        puzzle: puzzle_str.clone(),
                    },
                );
                self.pool_set.remove(&old.puzzle);
                self.pool_set.insert(puzzle_str);
            }
        }
    }

    fn make_seed(&mut self) -> String {
        let (count, sol_bytes, _) = crate::solver_basic::solve(&[b'.'; 81], 1, 0);
        if count == 0 {
            return if self.options.pencilmark {
                "123456789".repeat(81)
            } else {
                ".".repeat(81)
            };
        }

        if self.options.pencilmark {
            let mut pm = Vec::with_capacity(729);
            #[allow(clippy::needless_range_loop)]
            for cell in 0..81 {
                let digit = sol_bytes[cell];
                for d in 0u8..9 {
                    pm.push(if b'1' + d == digit { digit } else { b'.' });
                }
            }
            let mut pm_str = String::from_utf8(pm).expect("valid ascii");
            crate::minimize(true, false, &mut pm_str);
            pm_str
        } else {
            let mut sol_str = String::from_utf8(sol_bytes.to_vec()).expect("valid ascii");
            crate::minimize(false, false, &mut sol_str);
            sol_str
        }
    }

    fn is_valid_puzzle(&self, puzzle: &[u8]) -> bool {
        if self.options.pencilmark {
            return true;
        }
        if puzzle.len() < 81 {
            return false;
        }
        for row in 0..9 {
            let mut seen = 0u16;
            for col in 0..9 {
                let c = puzzle[row * 9 + col];
                if c != b'.' {
                    let bit = 1u16 << (c - b'1');
                    if seen & bit != 0 {
                        return false;
                    }
                    seen |= bit;
                }
            }
        }
        for col in 0..9 {
            let mut seen = 0u16;
            for row in 0..9 {
                let c = puzzle[row * 9 + col];
                if c != b'.' {
                    let bit = 1u16 << (c - b'1');
                    if seen & bit != 0 {
                        return false;
                    }
                    seen |= bit;
                }
            }
        }
        for box_row in 0..3 {
            for box_col in 0..3 {
                let mut seen = 0u16;
                for r in 0..3 {
                    for c in 0..3 {
                        let idx = (box_row * 3 + r) * 9 + (box_col * 3 + c);
                        let ch = puzzle[idx];
                        if ch != b'.' {
                            let bit = 1u16 << (ch - b'1');
                            if seen & bit != 0 {
                                return false;
                            }
                            seen |= bit;
                        }
                    }
                }
            }
        }
        true
    }

    fn has_unique_solution(&self, puzzle: &[u8]) -> bool {
        if !self.is_valid_puzzle(puzzle) {
            return false;
        }
        let input = match std::str::from_utf8(puzzle) {
            Ok(s) => s,
            Err(_) => return false,
        };
        let (count, _, _) = crate::solve_sudoku(input, 2, 0);
        count == 1
    }

    fn num_clues(&self, puzzle: &[u8]) -> usize {
        if self.options.pencilmark {
            puzzle.iter().filter(|&&c| c == b'.').count()
        } else {
            puzzle.iter().filter(|&&c| c != b'.').count()
        }
    }

    fn evaluate(&mut self, puzzle: &[u8]) -> (usize, f64, f64) {
        let num_clues = self.num_clues(puzzle);

        let mean_log_guesses = if self.options.num_evals > 0 {
            let mut eval = puzzle.to_vec();
            let mut sum = 0.0f64;
            for _ in 0..self.options.num_evals {
                self.util.permute_sudoku(&mut eval, self.options.pencilmark);
                let input = std::str::from_utf8(&eval).unwrap_or("");
                let (_, _, guesses) = crate::solve_sudoku(input, 1, 0);
                sum += (guesses as f64 + 1.0).ln();
            }
            sum / self.options.num_evals as f64
        } else {
            0.0
        };

        let loss = if self.has_unique_solution(puzzle) {
            num_clues as f64 * self.options.clue_weight
                - (mean_log_guesses * self.options.guess_weight).exp()
                + self.util.random_double() * self.options.random_weight
        } else {
            f64::MAX
        };

        (num_clues, mean_log_guesses.exp(), loss)
    }

    fn worst_idx(&self) -> Option<usize> {
        self.pool
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.loss.total_cmp(&b.loss))
            .map(|(i, _)| i)
    }

    fn worst_loss(&self) -> f64 {
        self.pool
            .iter()
            .map(|e| e.loss)
            .reduce(|a, b| if a.total_cmp(&b).is_ge() { a } else { b })
            .unwrap_or(f64::NEG_INFINITY)
    }
}

// ── formatting helpers ────────────────────────────────────────────────────

/// Build a JSON object representing one puzzle output line.
///
/// Always includes `puzzle`, `num_clues`, `geo_mean_guesses`, and `loss`.
/// `pretty` and `solution` are included only when `Some`.
pub fn format_puzzle_json(
    puzzle: &str,
    num_clues: usize,
    geo_mean_guesses: f64,
    loss: f64,
    pretty: Option<String>,
    solution: Option<&str>,
) -> serde_json::Value {
    let mut obj = serde_json::json!({
        "puzzle": puzzle,
        "num_clues": num_clues,
        "geo_mean_guesses": (geo_mean_guesses * 10.0).round() / 10.0,
        "loss": (loss * 100.0).round() / 100.0,
    });
    if let Some(formatted) = pretty {
        obj["pretty"] = serde_json::Value::String(formatted);
    }
    if let Some(sol) = solution {
        obj["solution"] = serde_json::Value::String(sol.to_string());
    }
    obj
}

/// Render a puzzle as a human-readable ASCII art grid.
///
/// For pencilmark format (729 chars), cells with exactly one remaining candidate
/// are shown as that digit; cells with multiple candidates are shown as `'.'`.
/// For vanilla format (81 chars), each character is used directly.
pub fn format_pretty(puzzle: &str, pencilmark: bool) -> String {
    let cells: Vec<u8> = if pencilmark {
        let bytes = puzzle.as_bytes();
        (0..81)
            .map(|cell| {
                let start = cell * 9;
                let end = (start + 9).min(bytes.len());
                let mut found = b'.';
                let mut count = 0u32;
                for &b in &bytes[start..end] {
                    if b != b'.' {
                        found = b;
                        count += 1;
                    }
                }
                if count == 1 { found } else { b'.' }
            })
            .collect()
    } else {
        puzzle.bytes().take(81).collect()
    };

    let sep = "+-------+-------+-------+";
    let mut out = String::with_capacity(14 * 26);
    out.push_str(sep);
    out.push('\n');
    for row in 0..9usize {
        out.push_str("| ");
        for col in 0..9usize {
            let c = *cells.get(row * 9 + col).unwrap_or(&b'.') as char;
            out.push(c);
            match col {
                2 | 5 => out.push_str(" | "),
                8 => {}
                _ => out.push(' '),
            }
        }
        out.push_str(" |\n");
        if row == 2 || row == 5 || row == 8 {
            out.push_str(sep);
            out.push('\n');
        }
    }
    out
}
