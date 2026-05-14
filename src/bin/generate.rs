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

struct Options {
    max_puzzles: u64,
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
}

impl Default for Options {
    fn default() -> Self {
        Self {
            max_puzzles: u64::MAX,
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
}

impl Generator {
    fn new(options: Options) -> Self {
        Self {
            options,
            util: Util::new(),
            pool: Vec::new(),
            pool_set: HashSet::new(),
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
        let file = match std::fs::File::open(filename) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("Error opening {}: {}", filename, e);
                std::process::exit(1);
            }
        };
        let puzzle_size = if self.options.pencilmark { 729 } else { 81 };
        let reader = std::io::BufReader::new(file);
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
            let (_, _, loss) = self.evaluate(puzzle.as_bytes());
            self.pool.push(PoolEntry {
                loss,
                puzzle: puzzle.clone(),
            });
            self.pool_set.insert(puzzle);
        }
    }

    fn has_unique_solution(&self, puzzle: &[u8]) -> bool {
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

    fn print_puzzle(&self, puzzle: &str, num_clues: usize, geo_mean_guesses: f64, loss: f64) {
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
            );
            println!("{}", serde_json::to_string(&obj).unwrap());
        } else {
            if self.options.pretty {
                print!("{}", format_pretty(puzzle, self.options.pencilmark));
            }
            println!(
                "{} {} {:.1} {:.2}",
                puzzle, num_clues, geo_mean_guesses, loss
            );
        }
    }

    fn generate(&mut self) {
        let puzzle_size = if self.options.pencilmark { 729 } else { 81 };

        for _ in 0..self.options.max_puzzles {
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

            if self.options.display_all {
                self.print_puzzle(&puzzle_str, num_clues, geo_mean_guesses, loss);
            }

            // skip if the puzzle's loss is worse than the current worst in the pool
            if loss > self.worst_loss() {
                continue;
            }

            if !self.options.display_all {
                self.print_puzzle(&puzzle_str, num_clues, geo_mean_guesses, loss);
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
/// Fields are always present; `pretty` is `None` to omit it.
fn format_puzzle_json(
    puzzle: &str,
    num_clues: usize,
    geo_mean_guesses: f64,
    loss: f64,
    pretty: Option<String>,
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
    eprintln!("  <puzzle>  <num_clues>  <geo_mean_guesses>  <loss>");
    eprintln!();
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
    eprintln!("  -a [0|1]            1 = print every evaluated puzzle;");
    eprintln!("                      0 = print only puzzles accepted into the pool.");
    eprintln!("                      Default: 0");
    eprintln!("  -p [0|1]            1 = pencilmark format (729 chars, eliminations as dots);");
    eprintln!("                      0 = vanilla format (81 chars, blanks as dots).");
    eprintln!("                      Default: 1 (pencilmark)");
    eprintln!("      --pretty        Print each puzzle as a human-readable ASCII art grid");
    eprintln!("                      before its one-line output.");
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
    eprintln!("  # Seed from an existing puzzle file and generate 50 new variations:");
    eprintln!("  generate -p 0 -l 50 my_puzzles.txt");
}

fn main() {
    let mut options = Options::default();
    let args: Vec<String> = std::env::args().collect();
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
        } else if arg == "-h" || arg == "--help" {
            print_usage();
            std::process::exit(0);
        } else if arg.starts_with('-') && arg.len() == 2 {
            let ch = arg.chars().nth(1).unwrap();
            i += 1;
            match ch {
                // flags with required numeric arguments
                'c' | 'g' | 'r' | 'd' | 'e' | 'l' | 'n' | 's' => {
                    let val = args.get(i).cloned().unwrap_or_default();
                    i += 1;
                    match ch {
                        'c' => {
                            if let Ok(v) = val.parse() {
                                options.clue_weight = v;
                            }
                        }
                        'g' => {
                            if let Ok(v) = val.parse() {
                                options.guess_weight = v;
                            }
                        }
                        'r' => {
                            if let Ok(v) = val.parse() {
                                options.random_weight = v;
                            }
                        }
                        'd' => {
                            if let Ok(v) = val.parse() {
                                options.clues_to_drop = v;
                            }
                        }
                        'e' => {
                            if let Ok(v) = val.parse() {
                                options.num_evals = v;
                            }
                        }
                        'l' => {
                            if let Ok(v) = val.parse() {
                                options.max_puzzles = v;
                            }
                        }
                        'n' => {
                            if let Ok(v) = val.parse() {
                                options.num_puzzles_in_pool = v;
                            }
                        }
                        's' => { /* only tdoku solver available; ignore */ }
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

    let mut generator = Generator::new(options);
    match pattern_file {
        None => generator.init_empty(),
        Some(ref path) => generator.load(path),
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
        );

        assert!(obj.is_object());
        let map = obj.as_object().unwrap();
        assert!(map.contains_key("puzzle"));
        assert!(map.contains_key("num_clues"));
        assert!(map.contains_key("geo_mean_guesses"));
        assert!(map.contains_key("loss"));
        // "pretty" should be absent when not requested
        assert!(!map.contains_key("pretty"));
    }

    #[test]
    fn json_includes_pretty_when_provided() {
        let obj = format_puzzle_json(
            ".2.....89.5.7........1.34....4.6.....3.8...1...7...365..1.4.9.....9...3.9.2..1...",
            25,
            1.0,
            24.27,
            Some("+-------+...".to_string()),
        );

        assert!(obj["pretty"].is_string());
    }

    #[test]
    fn json_field_types_are_correct() {
        let obj = format_puzzle_json("123456789", 9, 2.5, 10.123, None);

        assert!(obj["puzzle"].is_string());
        assert!(obj["num_clues"].is_number());
        assert!(obj["geo_mean_guesses"].is_number());
        assert!(obj["loss"].is_number());
    }

    #[test]
    fn json_values_are_preserved() {
        let puzzle = ".23......5...";
        let obj = format_puzzle_json(puzzle, 5, 3.14159, 12.3456, None);

        assert_eq!(obj["puzzle"].as_str().unwrap(), puzzle);
        assert_eq!(obj["num_clues"].as_u64().unwrap(), 5);
    }

    #[test]
    fn json_rounds_geo_mean_guesses_to_1_decimal() {
        let obj = format_puzzle_json("...", 0, 3.14159, 0.0, None);
        // 3.14159 * 10 = 31.4159, round = 31, /10 = 3.1
        assert_eq!(obj["geo_mean_guesses"].as_f64().unwrap(), 3.1);
    }

    #[test]
    fn json_rounds_loss_to_2_decimals() {
        let obj = format_puzzle_json("...", 0, 0.0, 12.3456, None);
        // 12.3456 * 100 = 1234.56, round = 1235, /100 = 12.35
        assert_eq!(obj["loss"].as_f64().unwrap(), 12.35);
    }

    #[test]
    fn json_output_is_valid_json_line() {
        let obj = format_puzzle_json(".2.....89.", 25, 1.0, 24.27, None);
        let json_str = serde_json::to_string(&obj).unwrap();

        // Should parse back successfully
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["puzzle"], obj["puzzle"]);
        assert_eq!(parsed["num_clues"], obj["num_clues"]);
    }

    // ------------------------------------------------------------------
    // format_pretty
    // ------------------------------------------------------------------

    #[test]
    fn pretty_vanilla_starts_with_separator() {
        let puzzle = ".2.....89.5.7........1.34....4.6.....3.8...1...7...365..1.4.9.....9...3.9.2..1...";
        let out = format_pretty(puzzle, false);
        assert!(
            out.starts_with("+-------+-------+-------+"),
            "expected separator line, got: {out}"
        );
    }
}
