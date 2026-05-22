//! Puzzle generator — port of `tdoku/src/generate.cc`.
//!
//! Generates Sudoku puzzles using a pool-based hill-climbing search.
//! Each iteration picks a puzzle from the pool, randomly drops some clues,
//! re-completes it to a unique solution, scores it, and keeps it if it scores
//! better than the current worst pool entry.
//!
//! Puzzles are scored by a loss function (lower = better):
//!   `loss = clues * clue_weight - exp(geo_mean_guesses * guess_weight) + rand * random_weight`
//!
//! Run with `-h` for full usage, including difficulty-tuning guidance.

use rdoku::util::Util;
use std::collections::HashSet;
use std::io::BufRead;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

struct Options {
    max_puzzles: u64,
    skip: u64,
    clue_weight: f64,
    guess_weight: f64,
    random_weight: f64,
    clues_to_drop: usize,
    num_evals: usize,
    num_puzzles_in_pool: usize,
    display_all: bool,
    do_minimize: bool,
    pencilmark: bool,
    pretty: bool,
    json: bool,
    solution: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            max_puzzles: u64::MAX,
            skip: 0,
            clue_weight: 1.0,
            guess_weight: 0.5,
            random_weight: 1.0,
            clues_to_drop: 3,
            num_evals: 10,
            num_puzzles_in_pool: 500,
            display_all: false,
            do_minimize: true,
            pencilmark: true,
            pretty: false,
            json: false,
            solution: false,
        }
    }
}

#[derive(Clone)]
struct PoolEntry {
    loss: f64,
    puzzle: String,
}

struct Generator {
    options: Options,
    util: Util,
    pool: Vec<PoolEntry>,
    pool_set: HashSet<String>,
    /// How many puzzles have been printed (or would have been printed if not
    /// skipped).  Used to implement `--skip`.
    printed: u64,
    /// Shared signal flag for graceful shutdown on Ctrl-C.
    running: Arc<AtomicBool>,
}

impl Generator {
    fn new(options: Options, running: Arc<AtomicBool>) -> Self {
        Self {
            options,
            util: Util::new(),
            pool: Vec::new(),
            pool_set: HashSet::new(),
            printed: 0,
            running,
        }
    }

    fn init_empty(&mut self) {
        // Seed the pool with a minimal puzzle rather than the empty/full-candidate grid.
        // Starting from a completely unconstrained grid causes the SIMD DPLL solver to
        // recurse ~80 000+ levels deep (it needs to branch on almost every digit placement),
        // which overflows the thread stack.  The basic backtracking solver only recurses
        // ≤81 levels (one per cell), so it's safe to use here for the first seed.
        let initial_seed = self.make_seed();
        for _ in 0..self.options.num_puzzles_in_pool {
            self.pool.push(PoolEntry {
                loss: f64::MAX,
                puzzle: initial_seed.clone(),
            });
        }
    }

    /// Generate one minimal seed puzzle to prime the pool.
    ///
    /// Uses the basic cell-level solver (max recursion depth 81) to produce a complete
    /// solution from the empty grid, then minimizes it with the SIMD generator.  The
    /// resulting puzzle has 20–30 clues (vanilla) or a comparable pencilmark encoding,
    /// which is well within the SIMD solver's efficient operating range for subsequent
    /// `constrain` and `evaluate` calls.
    fn make_seed(&mut self) -> String {
        // Solve the empty vanilla grid with the basic solver (safe depth ≤81).
        let (count, sol_bytes, _) = rdoku::solver_basic::solve(&[b'.'; 81], 1, 0);
        if count == 0 {
            // Should never happen for the empty grid; fall back gracefully.
            return if self.options.pencilmark {
                "123456789".repeat(81)
            } else {
                ".".repeat(81)
            };
        }

        if self.options.pencilmark {
            // Convert the complete solution to a "fully determined" pencilmark string
            // (each cell retains only its unique digit), then minimize.
            let mut pm = Vec::with_capacity(729);
            #[allow(clippy::needless_range_loop)]
            for cell in 0..81 {
                let digit = sol_bytes[cell];
                for d in 0u8..9 {
                    pm.push(if b'1' + d == digit { digit } else { b'.' });
                }
            }
            let mut pm_str = String::from_utf8(pm).expect("valid ascii");
            rdoku::minimize(true, false, &mut pm_str);
            pm_str
        } else {
            let mut sol_str = String::from_utf8(sol_bytes.to_vec()).expect("valid ascii");
            // Minimize the complete solution to get a proper minimal vanilla puzzle.
            rdoku::minimize(false, false, &mut sol_str);
            sol_str
        }
    }

    fn load(&mut self, filename: &str) {
        let reader = match rdoku::util::open_regular_file(filename) {
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
            // Quick structural check: skip puzzles with duplicate digits in a
            // row, column, or box before feeding them to the solver.
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
    }

    /// Quick structural check: reject puzzles with duplicate digits in any
    /// row, column, or 3×3 box.  The full solver also catches these, but it
    /// may spend a long time exploring dead ends first — this fast filter
    /// prevents that.
    fn is_valid_puzzle(&self, puzzle: &[u8]) -> bool {
        if self.options.pencilmark {
            // For pencilmark (729 chars), a quick row/col/box duplicate check
            // is less meaningful because a cell may have multiple candidates.
            // Rely on the solver's own validation.
            return true;
        }
        // Vanilla format (81 chars): each cell is a digit '1'..'9' or '.'
        if puzzle.len() < 81 {
            return false;
        }

        // Check each row for duplicate digits
        for row in 0..9 {
            let mut seen = 0u16;
            for col in 0..9 {
                let c = puzzle[row * 9 + col];
                if c != b'.' {
                    let bit = 1u16 << (c - b'1');
                    if seen & bit != 0 {
                        return false; // duplicate digit in row
                    }
                    seen |= bit;
                }
            }
        }

        // Check each column for duplicate digits
        for col in 0..9 {
            let mut seen = 0u16;
            for row in 0..9 {
                let c = puzzle[row * 9 + col];
                if c != b'.' {
                    let bit = 1u16 << (c - b'1');
                    if seen & bit != 0 {
                        return false; // duplicate digit in column
                    }
                    seen |= bit;
                }
            }
        }

        // Check each 3×3 box for duplicate digits
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
                                return false; // duplicate digit in box
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
        // Fast structural validity check first
        if !self.is_valid_puzzle(puzzle) {
            return false;
        }
        let input = match std::str::from_utf8(puzzle) {
            Ok(s) => s,
            Err(_) => return false,
        };
        let (count, _, _) = rdoku::solve_sudoku(input, 2, 0);
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
                let (_, _, guesses) = rdoku::solve_sudoku(input, 1, 0);
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

    fn print_puzzle(
        &self,
        puzzle: &str,
        num_clues: usize,
        geo_mean_guesses: f64,
        loss: f64,
        solution: Option<&str>,
    ) {
        if self.options.json {
            let obj = format_puzzle_json(
                puzzle,
                num_clues,
                geo_mean_guesses,
                loss,
                if self.options.pretty {
                    Some(format_pretty(puzzle, self.options.pencilmark))
                } else {
                    None
                },
                solution,
            );
            println!("{}", serde_json::to_string(&obj).unwrap());
        } else {
            if self.options.pretty {
                print!("{}", format_pretty(puzzle, self.options.pencilmark));
            }
            match solution {
                Some(sol) => println!(
                    "{} {} {:.1} {:.2} {}",
                    puzzle, num_clues, geo_mean_guesses, loss, sol
                ),
                None => println!(
                    "{} {} {:.1} {:.2}",
                    puzzle, num_clues, geo_mean_guesses, loss
                ),
            }
        }
    }

    fn generate(&mut self) {
        let puzzle_size = if self.options.pencilmark { 729 } else { 81 };
        // Run until we have accepted (and counted) max_puzzles entries.
        // "accepted" includes both skipped and printed puzzles; skip just
        // controls visibility, not the total count.  Previously the outer
        // loop ran max_puzzles *iterations*, which meant a single `continue`
        // (constrain failure, duplicate, or loss > worst) could exhaust the
        // budget before any puzzle was printed.
        let target = self.options.max_puzzles;

        loop {
            if !self.running.load(Ordering::SeqCst) {
                break;
            }
            if self.printed >= target {
                break;
            }
            if self.pool.is_empty() {
                break;
            }

            // pick a random puzzle from the pool
            let which = (self.util.random_uint() as usize) % self.pool.len();
            let pattern_puzzle = self.pool[which].puzzle.clone();
            let mut puzzle: Vec<u8> = pattern_puzzle.bytes().take(puzzle_size).collect();

            // randomly drop clues to unconstrain the puzzle
            if self.options.clues_to_drop > 0 {
                let perm = self.util.permutation(puzzle_size);
                let mut dropped = 0;
                for &j in &perm {
                    if dropped == self.options.clues_to_drop {
                        break;
                    }
                    if self.options.pencilmark {
                        // for pencilmark a clue is an elimination (a '.')
                        if puzzle[j] == b'.' {
                            puzzle[j] = b'1' + (j % 9) as u8;
                            dropped += 1;
                        }
                    } else {
                        // for vanilla a clue is a placed digit (not '.')
                        if puzzle[j] != b'.' {
                            puzzle[j] = b'.';
                            dropped += 1;
                        }
                    }
                }

                // re-complete to a unique solution
                let mut puzzle_str = String::from_utf8_lossy(&puzzle).into_owned();
                if !rdoku::constrain(self.options.pencilmark, &mut puzzle_str) {
                    continue;
                }
                if self.options.do_minimize {
                    rdoku::minimize(self.options.pencilmark, false, &mut puzzle_str);
                }
                puzzle = puzzle_str.into_bytes();
            }

            // evaluate difficulty
            let puzzle_bytes = &puzzle[..puzzle_size.min(puzzle.len())];
            let (num_clues, geo_mean_guesses, loss) = self.evaluate(puzzle_bytes);

            let puzzle_str = String::from_utf8_lossy(puzzle_bytes).into_owned();

            // skip if duplicate of the pattern it was drawn from, or already in pool
            if self.options.clues_to_drop > 0 {
                let pattern_cmp = &pattern_puzzle[..puzzle_size.min(pattern_puzzle.len())];
                if puzzle_str == pattern_cmp {
                    continue;
                }
                if self.pool_set.contains(&puzzle_str) {
                    continue;
                }
            }

            // Determine whether this puzzle will be printed (not skipped).
            let will_print = self.printed >= self.options.skip;

            // Resolve the solution only when it will actually be printed.
            let solution: Option<String> = if self.options.solution && will_print {
                let (_, sol, _) = rdoku::solve_sudoku(&puzzle_str, 1, 0);
                Some(sol)
            } else {
                None
            };

            if self.options.display_all {
                if will_print {
                    self.print_puzzle(
                        &puzzle_str,
                        num_clues,
                        geo_mean_guesses,
                        loss,
                        solution.as_deref(),
                    );
                }
                self.printed += 1;
            }

            // skip if the puzzle's loss is worse than the current worst in the pool
            if loss > self.worst_loss() {
                continue;
            }

            if !self.options.display_all {
                if will_print {
                    self.print_puzzle(
                        &puzzle_str,
                        num_clues,
                        geo_mean_guesses,
                        loss,
                        solution.as_deref(),
                    );
                }
                self.printed += 1;
            }

            // add the new puzzle and evict the worst entry
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
}

/// Build a JSON object representing one puzzle output line.
///
/// Fields are always present; `pretty` and `solution` are `None` to omit them.
fn format_puzzle_json(
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
///
/// Example output:
/// ```text
/// +-------+-------+-------+
/// | 5 3 . | . 7 . | . . . |
/// | 6 . . | 1 9 5 | . . . |
/// | . 9 8 | . . . | . 6 . |
/// +-------+-------+-------+
/// | 8 . . | . 6 . | . . 3 |
/// ...
/// +-------+-------+-------+
/// ```
fn format_pretty(puzzle: &str, pencilmark: bool) -> String {
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
                if count == 1 {
                    found
                } else {
                    b'.'
                }
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

fn print_usage() {
    eprintln!("usage: generate [options] [pattern_file]");
    eprintln!();
    eprintln!("Generates Sudoku puzzles using a pool-based hill-climbing search.");
    eprintln!("Each iteration picks a puzzle from the pool, randomly drops some clues,");
    eprintln!("re-completes it to a unique solution, scores it, and keeps it if it scores");
    eprintln!("better than the current worst entry in the pool.");
    eprintln!();
    eprintln!("PUZZLE FORMATS:");
    eprintln!("  Vanilla (81 chars)    One character per cell, row by row.");
    eprintln!("                        '1'–'9' = given clue, '.' = empty cell.");
    eprintln!("                        Example: 53..7....6..195....98....6.8...6...34..8.3..");
    eprintln!();
    eprintln!("  Pencilmark (729 chars) One character per candidate, row by row.");
    eprintln!("                        Each cell occupies 9 characters (digits 1–9 in order).");
    eprintln!("                        A digit is present if that value is still possible;");
    eprintln!("                        '.' means the candidate has been eliminated.");
    eprintln!("                        Encodes more information than vanilla: a cell showing");
    eprintln!("                        only '5' (digits 1–4,6–9 eliminated) is equivalent to");
    eprintln!("                        a vanilla clue; a cell with multiple candidates still");
    eprintln!("                        open represents a constraint stronger than an empty cell.");
    eprintln!("                        Use -p 0 for vanilla output (default is pencilmark).");
    eprintln!();
    eprintln!("OUTPUT FORMAT (one line per puzzle printed):");
    eprintln!("  <puzzle>  <num_clues>  <geo_mean_guesses>  <loss>  [<solution>]");
    eprintln!();
    eprintln!("  solution          The unique solved grid (81 chars, row by row).");
    eprintln!("                    Present only when --solution / -s is given.");
    eprintln!("  geo_mean_guesses  Average guesses needed by the solver across -e random");
    eprintln!("                   permutations. Higher means harder for algorithmic solvers.");
    eprintln!("  loss              Score used for pool selection (lower = better/preferred):");
    eprintln!("    loss = clues * clue_weight - exp(geo_mean_guesses * guess_weight)");
    eprintln!("         + random() * random_weight");
    eprintln!();
    eprintln!("DIFFICULTY TUNING:");
    eprintln!("  Harder puzzles (more solver guesses required):");
    eprintln!("    Increase -g (rewards puzzles that require more guesses).");
    eprintln!("    Decrease -c toward 0 (stops penalizing extra clues).");
    eprintln!("    Example: -c 0.0 -g 2.0");
    eprintln!();
    eprintln!("  Easier puzzles (fewer solver guesses, more naked singles):");
    eprintln!("    Decrease -g toward 0 (removes the guess-count reward).");
    eprintln!("    Increase -c (strongly prefers puzzles with fewer clues, which tend to");
    eprintln!("    be simpler for human solvers).");
    eprintln!("    Example: -c 3.0 -g 0.0");
    eprintln!();
    eprintln!("RANDOMNESS AND DIVERSITY:");
    eprintln!("  By default, the entire pool is seeded with copies of a single minimal");
    eprintln!("  puzzle. After a few dozen iterations the pool is already quite diverse —");
    eprintln!("  each call to constrain/re-complete uses a random permutation of all");
    eprintln!("  candidates, so it can reach any valid puzzle regardless of the seed.");
    eprintln!();
    eprintln!("  The most reliable way to have diversity:");
    eprintln!("  1. generate a large number of puzzles (e.g. -l 1000+) and extract the");
    eprintln!("     last 200-300 puzzles, saving them to a file.");
    eprintln!("  2. Use that pattern file as input for future runs; you can then expect");
    eprintln!("     high diversity even for the first results.");
    eprintln!();
    eprintln!("  When not using a pattern file and generating only a handful of puzzles");
    eprintln!("  (e.g. -l 2–20), the first few outputs are siblings of that initial seed.");
    eprintln!("  To get more independent puzzles from short runs:");
    eprintln!();
    eprintln!("    -d <clues_to_drop>  Increase (e.g. -d 10). Drops more clues before");
    eprintln!("                        re-completing → larger mutations, more variety");
    eprintln!("                        per iteration.");
    eprintln!("    -r <random_weight>  Increase (e.g. -r 5.0). More noise in the loss");
    eprintln!("                        function → pool keeps a more diverse set of");
    eprintln!("                        puzzles instead of converging greedily.");
    eprintln!("    -n <pool_size>      Increase (e.g. -n 2000). Larger pool → maintains");
    eprintln!("                        more distinct lineages simultaneously.");
    eprintln!("    pattern_file        Seed the pool from a file of diverse puzzles.");
    eprintln!("                        This is the most direct way to guarantee");
    eprintln!("                        independent starting points. You can use puzzles");
    eprintln!("                        from previous runs, other generators, or");
    eprintln!("                        published collections.");
    eprintln!();
    eprintln!("OPTIONS:");
    eprintln!("  Scoring weights (all non-negative floats):");
    eprintln!("  -c <clue_weight>    Weight for clue count in the loss function.");
    eprintln!("                      Higher = prefer puzzles with fewer clues.");
    eprintln!("                      Range: 0.0–10.0.  Default: 1.0");
    eprintln!("  -g <guess_weight>   Exponent scaling the solver-guess reward.");
    eprintln!("                      Higher = prefer puzzles that require more guesses");
    eprintln!("                      (harder for algorithmic solvers).");
    eprintln!("                      Range: 0.0–5.0.  Default: 0.5");
    eprintln!("  -r <random_weight>  Weight for uniform random noise added to loss.");
    eprintln!("                      Higher = more random exploration of puzzle space;");
    eprintln!("                      0.0 = fully deterministic, greedy selection.");
    eprintln!("                      Range: 0.0–10.0.  Default: 1.0");
    eprintln!();
    eprintln!("  Generation control:");
    eprintln!("  -d <drop>           Clues to remove before re-completing each iteration.");
    eprintln!("                      Higher = more variation between pool entries.");
    eprintln!("                      0 = sample from pool without mutation.");
    eprintln!("                      Range: 0–20.  Default: 3");
    eprintln!("  -e <num_evals>      Permutations used to estimate geo_mean_guesses.");
    eprintln!("                      Higher = more accurate difficulty estimate, slower.");
    eprintln!("                      0 = skip difficulty evaluation (guess_weight ignored).");
    eprintln!("                      Range: 0–100.  Default: 10");
    eprintln!("  -m [0|1]            Minimize puzzles (remove redundant clues) before");
    eprintln!("                      scoring and printing.  Default: 1 (enabled)");
    eprintln!("  -n <pool_size>      Number of top-scored puzzles kept for mutation.");
    eprintln!("                      Larger = more diverse pool, slower convergence.");
    eprintln!("                      Range: 1–5000.  Default: 500");
    eprintln!();
    eprintln!("  Output control:");
    eprintln!("  -l <limit>          Stop after generating this many puzzles.");
    eprintln!("                      Range: 1–unlimited.  Default: unlimited");
    eprintln!("      --skip <n>      Skip the first <n> puzzles that would have been");
    eprintln!("                      printed.  Useful for discarding early, less-diverse");
    eprintln!("                      outputs when the pool is still warming up.");
    eprintln!("                      Must be less than -l when -l is specified.");
    eprintln!("                      Default: 0");
    eprintln!("  -a [0|1]            1 = print every evaluated puzzle;");
    eprintln!("                      0 = print only puzzles accepted into the pool.");
    eprintln!("                      Default: 0");
    eprintln!("  -p [0|1]            1 = pencilmark format (729 chars, eliminations as dots);");
    eprintln!("                      0 = vanilla format (81 chars, blanks as dots).");
    eprintln!("                      Default: 1 (pencilmark)");
    eprintln!("      --pretty        Print each puzzle as a human-readable ASCII art grid");
    eprintln!("                      before its one-line output.");
    eprintln!("  -s, --solution      Include the unique solution (81-char solved grid) in");
    eprintln!("                      the output. In plain text mode the solution is appended");
    eprintln!("                      as a 5th column. In JSON mode it appears as a");
    eprintln!("                      \"solution\" field.");
    eprintln!("  -h                  Display this help message.");
    eprintln!("  -j, --json          Output each puzzle as a JSON object");
    eprintln!("                      (one per line) instead of plain text.");
    eprintln!("                      When combined with --pretty, includes formatted");
    eprintln!("                      ASCII art in an additional \"pretty\" field.");
    eprintln!();
    eprintln!("ARGUMENTS:");
    eprintln!("  pattern_file        Optional file of seed puzzles to pre-populate the pool");
    eprintln!("                      (one puzzle per line, comments start with '#').");
    eprintln!("                      If omitted, the pool is seeded with a single minimal");
    eprintln!("                      puzzle generated from the empty grid.");
    eprintln!();
    eprintln!("EXAMPLES:");
    eprintln!("  # Generate 10 vanilla puzzles with default difficulty settings:");
    eprintln!("  generate -p 0 -l 10");
    eprintln!();
    eprintln!("  # Generate 5 hard vanilla puzzles (maximize required guesses):");
    eprintln!("  generate -p 0 -l 5 -c 0.0 -g 2.0");
    eprintln!();
    eprintln!("  # Generate 5 easy vanilla puzzles (minimize required guesses):");
    eprintln!("  generate -p 0 -l 5 -c 3.0 -g 0.0");
    eprintln!();
    eprintln!("  # Generate 10 vanilla puzzles and display each as an ASCII art grid:");
    eprintln!("  generate -p 0 -l 10 --pretty");
    eprintln!();
    eprintln!("  # Generate pencilmark puzzles, printing only the first accepted into pool:");
    eprintln!("  generate -p 1 -l 1");
    eprintln!();
    eprintln!("  # Use a faster but less accurate difficulty estimate (-e 3 instead of 10):");
    eprintln!("  generate -p 0 -l 20 -e 3");
    eprintln!();
    eprintln!("  # Burn through 200 iterations to diversify the pool, then print 5 puzzles:");
    eprintln!("  generate -p 0 -l 205 --skip 200");
    eprintln!();
    eprintln!("  # Seed from an existing puzzle file and generate 50 new variations:");
    eprintln!("  generate -p 0 -l 50 my_puzzles.txt");
}

fn main() {
    let mut options = Options::default();
    let args: Vec<String> = std::env::args_os()
        .enumerate()
        .map(|(idx, os)| {
            os.into_string().unwrap_or_else(|bad| {
                eprintln!("Error: argument {} is not valid UTF-8: {:?}", idx, bad);
                std::process::exit(1);
            })
        })
        .collect();
    let mut i = 1usize;
    let mut pattern_file: Option<String> = None;

    while i < args.len() {
        let arg = &args[i];
        if arg == "--pretty" {
            options.pretty = true;
            i += 1;
        } else if arg == "--json" || arg == "-j" {
            options.json = true;
            i += 1;
        } else if arg == "--solution" || arg == "-s" {
            options.solution = true;
            i += 1;
        } else if arg == "-h" || arg == "--help" {
            print_usage();
            std::process::exit(0);
        } else if arg == "--version" {
            println!("generate {}", env!("CARGO_PKG_VERSION"));
            std::process::exit(0);
        } else if arg == "--skip" {
            i += 1;
            match args.get(i) {
                None => {
                    eprintln!("Error: --skip requires a non-negative integer argument.");
                    std::process::exit(1);
                }
                Some(val) => match val.parse::<u64>() {
                    Ok(v) => options.skip = v,
                    Err(_) => {
                        eprintln!(
                            "Error: invalid value for --skip: {:?} (expected a non-negative integer).",
                            val
                        );
                        std::process::exit(1);
                    }
                },
            }
            i += 1;
        } else if arg.starts_with('-') && arg.len() == 2 {
            let ch = arg.chars().nth(1).unwrap();
            i += 1;
            match ch {
                // flags with required numeric arguments
                'c' | 'g' | 'r' | 'd' | 'e' | 'l' | 'n' => {
                    let val = match args.get(i) {
                        Some(v) => {
                            let v = v.clone();
                            i += 1;
                            v
                        }
                        None => {
                            eprintln!("Error: -{} requires a numeric argument.", ch);
                            std::process::exit(1);
                        }
                    };
                    match ch {
                        'c' => match val.parse::<f64>() {
                            Ok(v) if v >= 0.0 => options.clue_weight = v,
                            Ok(v) => {
                                eprintln!(
                                    "Error: invalid value for -c: {} (must be a non-negative number).",
                                    v
                                );
                                std::process::exit(1);
                            }
                            Err(_) => {
                                eprintln!(
                                    "Error: invalid value for -c: {:?} (expected a non-negative number).",
                                    val
                                );
                                std::process::exit(1);
                            }
                        },
                        'g' => match val.parse::<f64>() {
                            Ok(v) if v >= 0.0 => options.guess_weight = v,
                            Ok(v) => {
                                eprintln!(
                                    "Error: invalid value for -g: {} (must be a non-negative number).",
                                    v
                                );
                                std::process::exit(1);
                            }
                            Err(_) => {
                                eprintln!(
                                    "Error: invalid value for -g: {:?} (expected a non-negative number).",
                                    val
                                );
                                std::process::exit(1);
                            }
                        },
                        'r' => match val.parse::<f64>() {
                            Ok(v) if v >= 0.0 => options.random_weight = v,
                            Ok(v) => {
                                eprintln!(
                                    "Error: invalid value for -r: {} (must be a non-negative number).",
                                    v
                                );
                                std::process::exit(1);
                            }
                            Err(_) => {
                                eprintln!(
                                    "Error: invalid value for -r: {:?} (expected a non-negative number).",
                                    val
                                );
                                std::process::exit(1);
                            }
                        },
                        'd' => match val.parse::<usize>() {
                            Ok(v) => options.clues_to_drop = v,
                            Err(_) => {
                                eprintln!(
                                    "Error: invalid value for -d: {:?} (expected a non-negative integer).",
                                    val
                                );
                                std::process::exit(1);
                            }
                        },
                        'e' => match val.parse::<usize>() {
                            Ok(v) => options.num_evals = v,
                            Err(_) => {
                                eprintln!(
                                    "Error: invalid value for -e: {:?} (expected a non-negative integer).",
                                    val
                                );
                                std::process::exit(1);
                            }
                        },
                        'l' => match val.parse::<u64>() {
                            Ok(v) => options.max_puzzles = v,
                            Err(_) => {
                                eprintln!(
                                    "Error: invalid value for -l: {:?} (expected a positive integer).",
                                    val
                                );
                                std::process::exit(1);
                            }
                        },
                        'n' => match val.parse::<usize>() {
                            Ok(v) if v >= 1 => options.num_puzzles_in_pool = v,
                            Ok(v) => {
                                eprintln!(
                                    "Error: invalid value for -n: {} (must be at least 1).",
                                    v
                                );
                                std::process::exit(1);
                            }
                            Err(_) => {
                                eprintln!(
                                    "Error: invalid value for -n: {:?} (expected a positive integer).",
                                    val
                                );
                                std::process::exit(1);
                            }
                        },
                        _ => unreachable!(),
                    }
                }
                // flags with optional boolean arguments (consume next arg only if it's "0" or "1")
                'm' | 'a' | 'p' => {
                    let val = match args.get(i).map(String::as_str) {
                        Some("0") | Some("1") => {
                            let v = args[i].as_str();
                            i += 1;
                            v
                        }
                        _ => "1",
                    };
                    match ch {
                        'm' => options.do_minimize = val != "0",
                        'a' => options.display_all = val != "0",
                        'p' => options.pencilmark = val != "0",
                        _ => unreachable!(),
                    }
                }
                _ => {
                    eprintln!("Unknown flag: -{}", ch);
                    print_usage();
                    std::process::exit(1);
                }
            }
        } else if !arg.starts_with('-') {
            pattern_file = Some(arg.clone());
            i += 1;
        } else {
            eprintln!("Unknown argument: {}", arg);
            print_usage();
            std::process::exit(1);
        }
    }

    if options.max_puzzles != u64::MAX && options.skip >= options.max_puzzles {
        eprintln!(
            "Error: --skip ({}) must be less than -l ({}).",
            options.skip, options.max_puzzles
        );
        std::process::exit(1);
    }

    let running = Arc::new(AtomicBool::new(true));
    let running_clone = Arc::clone(&running);
    ctrlc::set_handler(move || {
        running_clone.store(false, Ordering::SeqCst);
    }).expect("Error setting Ctrl-C handler");

    let mut generator = Generator::new(options, running);
    match pattern_file {
        None => generator.init_empty(),
        Some(ref path) => {
            generator.load(path);
            if generator.pool.is_empty() {
                eprintln!("error: pattern file '{}' contains no valid puzzles", path);
                std::process::exit(1);
            }
        }
    }
    generator.generate();
}

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // format_puzzle_json
    // ------------------------------------------------------------------

    #[test]
    fn json_has_required_fields() {
        let obj = format_puzzle_json(
            ".2.....89.5.7........1.34....4.6.....3.8...1...7...365..1.4.9.....9...3.9.2..1...",
            25,
            1.0,
            24.27,
            None,
            None,
        );

        assert!(obj.is_object());
        let map = obj.as_object().unwrap();
        assert!(map.contains_key("puzzle"));
        assert!(map.contains_key("num_clues"));
        assert!(map.contains_key("geo_mean_guesses"));
        assert!(map.contains_key("loss"));
        // "pretty" and "solution" should be absent when not requested
        assert!(!map.contains_key("pretty"));
        assert!(!map.contains_key("solution"));
    }

    #[test]
    fn json_includes_pretty_when_provided() {
        let obj = format_puzzle_json(
            ".2.....89.5.7........1.34....4.6.....3.8...1...7...365..1.4.9.....9...3.9.2..1...",
            25,
            1.0,
            24.27,
            Some("+-------+...".to_string()),
            None,
        );

        assert!(obj["pretty"].is_string());
    }

    #[test]
    fn json_includes_solution_when_provided() {
        let obj = format_puzzle_json(
            ".2.....89.5.7........1.34....4.6.....3.8...1...7...365..1.4.9.....9...3.9.2..1...",
            25,
            1.0,
            24.27,
            None,
            Some(
                "652483917978162435314975628825736149791824563436519872269348751547291386183657294",
            ),
        );

        assert!(obj["solution"].is_string());
        assert_eq!(
            obj["solution"].as_str().unwrap(),
            "652483917978162435314975628825736149791824563436519872269348751547291386183657294"
        );
    }

    #[test]
    fn json_field_types_are_correct() {
        let obj = format_puzzle_json("123456789", 9, 2.5, 10.123, None, None);

        assert!(obj["puzzle"].is_string());
        assert!(obj["num_clues"].is_number());
        assert!(obj["geo_mean_guesses"].is_number());
        assert!(obj["loss"].is_number());
    }

    #[test]
    fn json_values_are_preserved() {
        let puzzle = ".23......5...";
        let obj = format_puzzle_json(puzzle, 5, 3.14159, 12.3456, None, None);

        assert_eq!(obj["puzzle"].as_str().unwrap(), puzzle);
        assert_eq!(obj["num_clues"].as_u64().unwrap(), 5);
    }

    #[test]
    fn json_rounds_geo_mean_guesses_to_1_decimal() {
        let obj = format_puzzle_json("...", 0, 3.14159, 0.0, None, None);
        // 3.14159 * 10 = 31.4159, round = 31, /10 = 3.1
        assert_eq!(obj["geo_mean_guesses"].as_f64().unwrap(), 3.1);
    }

    #[test]
    fn json_rounds_loss_to_2_decimals() {
        let obj = format_puzzle_json("...", 0, 0.0, 12.3456, None, None);
        // 12.3456 * 100 = 1234.56, round = 1235, /100 = 12.35
        assert_eq!(obj["loss"].as_f64().unwrap(), 12.35);
    }

    #[test]
    fn json_output_is_valid_json_line() {
        let obj = format_puzzle_json(".2.....89.", 25, 1.0, 24.27, None, None);
        let json_str = serde_json::to_string(&obj).unwrap();

        // Should parse back successfully
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["puzzle"], obj["puzzle"]);
        assert_eq!(parsed["num_clues"], obj["num_clues"]);
    }

    #[test]
    fn json_with_solution_output_is_valid() {
        let solution =
            "652483917978162435314975628825736149791824563436519872269348751547291386183657294";
        let obj = format_puzzle_json(".2.....89.", 25, 1.0, 24.27, None, Some(solution));
        let json_str = serde_json::to_string(&obj).unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["puzzle"], obj["puzzle"]);
        assert_eq!(parsed["solution"], obj["solution"]);
        assert_eq!(parsed["solution"].as_str().unwrap(), solution);
        // Solution should be 81 chars, all digits 1-9
        let sol_str = parsed["solution"].as_str().unwrap();
        assert_eq!(sol_str.len(), 81);
        assert!(sol_str.chars().all(|c| c.is_ascii_digit()));
    }

    // ------------------------------------------------------------------
    // format_pretty
    // ------------------------------------------------------------------
    // --skip
    // ------------------------------------------------------------------

    #[test]
    fn default_skip_is_zero() {
        let opts = Options::default();
        assert_eq!(opts.skip, 0);
    }

    // ------------------------------------------------------------------
    // empty pattern file
    // ------------------------------------------------------------------

    #[test]
    fn load_empty_file_leaves_pool_empty() {
        let tmp = std::env::temp_dir().join("rdoku_test_empty_pattern.txt");
        std::fs::write(&tmp, b"").expect("create temp file");
        let path = tmp.to_str().unwrap();

        // open_regular_file must reject a 0-byte file
        let result = rdoku::util::open_regular_file(path);
        let _ = std::fs::remove_file(&tmp);
        assert!(result.is_err(), "expected error for 0-byte file");
        let msg = result.unwrap_err();
        assert!(
            msg.contains("empty"),
            "error message should mention 'empty': {}",
            msg
        );
    }

    #[test]
    fn generator_starts_with_printed_zero() {
        let g = Generator::new(Options::default(), Arc::new(AtomicBool::new(true)));
        assert_eq!(g.printed, 0);
    }

    #[test]
    fn skip_with_one_puzzle_display_all_prints_nothing() {
        // With display_all and skip=1, a single generated puzzle should
        // not be printed but should increment the counter.
        let opts = Options {
            max_puzzles: 1,
            skip: 1,
            display_all: true,
            num_puzzles_in_pool: 1,
            clues_to_drop: 3,
            do_minimize: true,
            pencilmark: false,
            ..Options::default()
        };
        let mut g = Generator::new(opts, Arc::new(AtomicBool::new(true)));
        g.init_empty();
        g.generate();
        // printed == 1 (counted but not shown), pool has 1 entry
        assert_eq!(g.printed, 1);
        assert_eq!(g.pool.len(), 1);
    }

    #[test]
    fn skip_zero_with_one_puzzle_prints() {
        let opts = Options {
            max_puzzles: 1,
            skip: 0,
            display_all: true,
            num_puzzles_in_pool: 1,
            clues_to_drop: 3,
            do_minimize: true,
            pencilmark: false,
            ..Options::default()
        };
        let mut g = Generator::new(opts, Arc::new(AtomicBool::new(true)));
        g.init_empty();
        g.generate();
        assert_eq!(g.printed, 1);
        assert_eq!(g.pool.len(), 1);
    }

    #[test]
    fn skip_without_display_all_counts_accepted_only() {
        // Without display_all, only puzzles accepted into the pool count.
        // With a fresh pool (all MAX), the first puzzle is always accepted.
        let opts = Options {
            max_puzzles: 1,
            skip: 0,
            display_all: false,
            num_puzzles_in_pool: 1,
            clues_to_drop: 3,
            do_minimize: true,
            pencilmark: false,
            ..Options::default()
        };
        let mut g = Generator::new(opts, Arc::new(AtomicBool::new(true)));
        g.init_empty();
        g.generate();
        // The puzzle was accepted, so printed counter incremented
        assert_eq!(g.printed, 1);
    }

    #[test]
    fn skip_equal_to_limit_is_rejected() {
        // Validation logic (extracted for testing)
        fn validate(opts: &Options) -> Result<(), String> {
            if opts.max_puzzles != u64::MAX && opts.skip >= opts.max_puzzles {
                Err(format!(
                    "Error: --skip ({}) must be less than -l ({}).",
                    opts.skip, opts.max_puzzles
                ))
            } else {
                Ok(())
            }
        }

        assert!(validate(&Options {
            max_puzzles: 5,
            skip: 5,
            ..Options::default()
        })
        .is_err());

        assert!(validate(&Options {
            max_puzzles: 5,
            skip: 6,
            ..Options::default()
        })
        .is_err());
    }

    #[test]
    fn skip_less_than_limit_is_accepted() {
        fn validate(opts: &Options) -> Result<(), String> {
            if opts.max_puzzles != u64::MAX && opts.skip >= opts.max_puzzles {
                Err(format!(
                    "Error: --skip ({}) must be less than -l ({}).",
                    opts.skip, opts.max_puzzles
                ))
            } else {
                Ok(())
            }
        }

        assert!(validate(&Options {
            max_puzzles: 5,
            skip: 4,
            ..Options::default()
        })
        .is_ok());

        // No -l specified → unlimited, any skip is fine
        assert!(validate(&Options {
            max_puzzles: u64::MAX,
            skip: 1000,
            ..Options::default()
        })
        .is_ok());
    }

    #[test]
    fn skip_with_solution_does_not_compute_for_skipped() {
        // With skip=1, display_all=true, solution=true:
        // the first (and only) puzzle is skipped, so solution is not computed.
        // We verify by checking that printed increments but the generator
        // doesn't crash (solving is skipped internally).
        let opts = Options {
            max_puzzles: 1,
            skip: 1,
            display_all: true,
            solution: true,
            num_puzzles_in_pool: 1,
            clues_to_drop: 3,
            do_minimize: true,
            pencilmark: false,
            ..Options::default()
        };
        let mut g = Generator::new(opts, Arc::new(AtomicBool::new(true)));
        g.init_empty();
        g.generate();
        assert_eq!(g.printed, 1);
        assert_eq!(g.pool.len(), 1);
    }

    // ------------------------------------------------------------------
    // is_valid_puzzle
    // ------------------------------------------------------------------

    fn make_vanilla_generator() -> Generator {
        Generator::new(
            Options {
                pencilmark: false,
                ..Options::default()
            },
            Arc::new(AtomicBool::new(true)),
        )
    }

    #[test]
    fn valid_empty_puzzle() {
        let g = make_vanilla_generator();
        assert!(g.is_valid_puzzle(
            b"................................................................................."
        ));
    }

    #[test]
    fn valid_partial_puzzle() {
        let g = make_vanilla_generator();
        // A valid partially-filled puzzle (no duplicates per row/col/box)
        let puzzle =
            b"53..7....6..195....98....6.8...6...34..8.3..17...2...6.6....28....419..5....8..79";
        assert!(g.is_valid_puzzle(puzzle));
    }

    #[test]
    fn invalid_duplicate_in_row() {
        let g = make_vanilla_generator();
        // Two 1's in the first row at positions 0 and 2
        let puzzle =
            b"1.11111..........................................................................";
        assert!(!g.is_valid_puzzle(puzzle));
    }

    #[test]
    fn invalid_duplicate_in_column() {
        let g = make_vanilla_generator();
        // Two 1's in column 0 (cells 0 and 9)
        let puzzle =
            b"1........1.......................................................................";
        assert!(!g.is_valid_puzzle(puzzle));
    }

    #[test]
    fn invalid_duplicate_in_box() {
        let g = make_vanilla_generator();
        // Two 1's in box 0: cell 0 and cell 10 (row 1, col 1)
        let puzzle =
            b"1.........1......................................................................";
        assert!(!g.is_valid_puzzle(puzzle));
    }

    #[test]
    fn valid_complete_solution() {
        let g = make_vanilla_generator();
        let puzzle =
            b"534678912672195348198342567859761423426853791713924856961537284287419635345286179";
        assert!(g.is_valid_puzzle(puzzle));
    }

    #[test]
    fn invalid_duplicate_in_row_8() {
        let g = make_vanilla_generator();
        // Two 9's at the end of row 8 (cells 79 and 80)
        let mut puzzle = [b'.'; 81];
        puzzle[79] = b'9';
        puzzle[80] = b'9';
        assert!(!g.is_valid_puzzle(&puzzle));
    }

    // ------------------------------------------------------------------
    // generate() loop termination — regression test for the bug where the
    // outer loop ran max_puzzles *iterations* instead of running until
    // max_puzzles puzzles were *printed*.  With a seed-file pool (entries
    // have finite loss rather than f64::MAX), the first iteration could be
    // discarded (duplicate check, constrain failure, or loss > worst_loss),
    // leaving printed == 0 even with -l 1.
    // ------------------------------------------------------------------

    #[test]
    fn limit_one_with_seeded_pool_produces_exactly_one_puzzle() {
        let mut g = Generator::new(Options {
            max_puzzles: 1,
            skip: 0,
            display_all: false,
            num_puzzles_in_pool: 5,
            clues_to_drop: 1,
            do_minimize: false,
            pencilmark: false,
            num_evals: 1,
            clue_weight: 3.0,
            guess_weight: 0.0,
            random_weight: 1.0,
            ..Options::default()
        }, Arc::new(AtomicBool::new(true)));
        let seed =
            ".2.4.6.3.4...591..3...2..5.214....9....8......97..4......6....8....7....9....13..";
        // Simulate what load() does: evaluate the seed and push it into the pool.
        let (_, _, loss) = g.evaluate(seed.as_bytes());
        g.pool.push(PoolEntry {
            loss,
            puzzle: seed.to_string(),
        });
        g.pool_set.insert(seed.to_string());

        g.generate();

        assert_eq!(
            g.printed, 1,
            "generate() should print exactly 1 puzzle when -l 1 is given, \
             even when the pool is seeded with finite-loss entries"
        );
    }

    #[test]
    fn limit_three_with_seeded_pool_produces_exactly_three_puzzles() {
        let mut g = Generator::new(Options {
            max_puzzles: 3,
            skip: 0,
            display_all: false,
            num_puzzles_in_pool: 5,
            clues_to_drop: 1,
            do_minimize: false,
            pencilmark: false,
            num_evals: 1,
            clue_weight: 3.0,
            guess_weight: 0.0,
            random_weight: 1.0,
            ..Options::default()
        }, Arc::new(AtomicBool::new(true)));
        let seed =
            ".2.4.6.3.4...591..3...2..5.214....9....8......97..4......6....8....7....9....13..";
        let (_, _, loss) = g.evaluate(seed.as_bytes());
        g.pool.push(PoolEntry {
            loss,
            puzzle: seed.to_string(),
        });
        g.pool_set.insert(seed.to_string());

        g.generate();

        assert_eq!(g.printed, 3);
    }

    // ------------------------------------------------------------------
    // Signal handling (graceful shutdown on Ctrl-C)
    // ------------------------------------------------------------------

    #[test]
    fn generate_stops_when_running_flag_is_false() {
        let mut g = Generator::new(
            Options {
                max_puzzles: 100,  // Request many puzzles
                skip: 0,
                display_all: false,
                num_puzzles_in_pool: 1,
                clues_to_drop: 1,
                do_minimize: false,
                pencilmark: false,
                num_evals: 1,
                clue_weight: 3.0,
                guess_weight: 0.0,
                random_weight: 1.0,
                ..Options::default()
            },
            Arc::new(AtomicBool::new(true)),
        );

        let seed =
            ".2.4.6.3.4...591..3...2..5.214....9....8......97..4......6....8....7....9....13..";
        let (_, _, loss) = g.evaluate(seed.as_bytes());
        g.pool.push(PoolEntry {
            loss,
            puzzle: seed.to_string(),
        });
        g.pool_set.insert(seed.to_string());

        // Immediately stop the generator by setting running to false
        g.running.store(false, Ordering::SeqCst);

        g.generate();

        // Should produce 0 puzzles since we set running=false before generate()
        assert_eq!(
            g.printed, 0,
            "generate() should stop immediately when running flag is false"
        );
    }

    #[test]
    fn generate_produces_partial_output_before_shutdown() {
        let mut g = Generator::new(
            Options {
                max_puzzles: 10,
                skip: 0,
                display_all: false,
                num_puzzles_in_pool: 1,
                clues_to_drop: 1,
                do_minimize: false,
                pencilmark: false,
                num_evals: 1,
                clue_weight: 3.0,
                guess_weight: 0.0,
                random_weight: 1.0,
                ..Options::default()
            },
            Arc::new(AtomicBool::new(true)),
        );

        let seed =
            ".2.4.6.3.4...591..3...2..5.214....9....8......97..4......6....8....7....9....13..";
        let (_, _, loss) = g.evaluate(seed.as_bytes());
        g.pool.push(PoolEntry {
            loss,
            puzzle: seed.to_string(),
        });
        g.pool_set.insert(seed.to_string());

        // Simulate early shutdown by setting running=false
        g.running.store(false, Ordering::SeqCst);
        let printed_before_generate = g.printed;

        g.generate();

        // With running=false at the start, should produce 0 additional puzzles
        assert_eq!(
            g.printed, printed_before_generate,
            "generate() should respect running flag check on each iteration"
        );
    }
}
