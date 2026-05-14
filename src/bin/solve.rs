//! Solver binary — solve Sudoku puzzles from stdin or a file.
//!
//! Reads puzzles one per line and writes results to stdout.
//! Run with `-h` for full usage.

use std::io::{self, BufRead, Write};
use std::time::Instant;

struct Options {
    limit: usize,
    count_only: bool,
    pencilmark: bool,
    pretty: bool,
    stats: bool,
    solver: Solver,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Solver {
    Simd,
    Scc,
    Basic,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            limit: 1,
            count_only: false,
            pencilmark: false,
            pretty: false,
            stats: false,
            solver: Solver::Simd,
        }
    }
}

// ---------------------------------------------------------------------------
// Pretty-print helper
// ---------------------------------------------------------------------------

/// Render an 81-char solution as an ASCII art grid.
fn format_pretty(solution: &str) -> String {
    let cells: Vec<char> = solution.chars().take(81).collect();
    let sep = "+-------+-------+-------+";
    let mut out = String::with_capacity(14 * 26);
    out.push_str(sep);
    out.push('\n');
    for row in 0..9usize {
        out.push_str("| ");
        for col in 0..9usize {
            let c = cells.get(row * 9 + col).copied().unwrap_or('.');
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

// ---------------------------------------------------------------------------
// Solve one puzzle
// ---------------------------------------------------------------------------

fn solve_line(puzzle: &str, options: &Options) -> (usize, String, usize) {
    match options.solver {
        Solver::Simd => rdoku::solve_sudoku(puzzle, options.limit, 0),
        Solver::Scc => {
            let (count, sol, guesses) =
                rdoku::solver_dpll_triad_scc::solve(puzzle.as_bytes(), options.limit, 0);
            (count, String::from_utf8_lossy(&sol).into_owned(), guesses)
        }
        Solver::Basic => {
            let (count, sol, guesses) =
                rdoku::solver_basic::solve(puzzle.as_bytes(), options.limit, 0);
            (count, String::from_utf8_lossy(&sol).into_owned(), guesses)
        }
    }
}

// ---------------------------------------------------------------------------
// Help text
// ---------------------------------------------------------------------------

fn print_usage() {
    eprintln!("usage: solve [options] [puzzle_file ...]");
    eprintln!();
    eprintln!("Reads Sudoku puzzles one per line and prints their solutions to stdout.");
    eprintln!("Puzzles can be read from files given as arguments, or from stdin if no");
    eprintln!("files are specified (or '-' is given as a filename).");
    eprintln!("Lines beginning with '#' and blank lines are skipped.");
    eprintln!();
    eprintln!("PUZZLE FORMATS:");
    eprintln!("  Vanilla (81 chars)     One character per cell, row by row.");
    eprintln!("                         '1'–'9' = given clue, '.' = empty cell.");
    eprintln!("                         Example: 53..7....6..195....98....6.8...6...3");
    eprintln!();
    eprintln!("  Pencilmark (729 chars) One character per candidate, row by row.");
    eprintln!("                         Each cell occupies 9 characters (digits 1–9 in order).");
    eprintln!("                         A digit is present if that value is still possible;");
    eprintln!("                         '.' means the candidate has been eliminated.");
    eprintln!("                         Pass -p when reading pencilmark input.");
    eprintln!();
    eprintln!("OUTPUT FORMAT:");
    eprintln!("  Default (one line per puzzle):");
    eprintln!("    <solution>  <count>  <guesses>");
    eprintln!();
    eprintln!("    solution  81-char solution string when count >= 1 and limit = 1,");
    eprintln!("              otherwise the empty string '.'*81.");
    eprintln!("    count     Number of solutions found (capped at limit).");
    eprintln!("    guesses   Number of search tree nodes visited.");
    eprintln!();
    eprintln!("  With -c (count-only):");
    eprintln!("    <count>  <guesses>");
    eprintln!();
    eprintln!("  With --pretty (each puzzle preceded by an ASCII art grid).");
    eprintln!();
    eprintln!("SOLVERS:");
    eprintln!("  simd   DPLL with triad constraints and SIMD propagation (fastest). Default.");
    eprintln!("  scc    DPLL with triad constraints and SCC variable-selection heuristic.");
    eprintln!("  basic  Simple DPLL with minimum-candidates heuristic. Reference solver.");
    eprintln!();
    eprintln!("  All three solvers always return the correct solution count and string.");
    eprintln!("  Use 'simd' for maximum throughput; 'basic' when debugging.");
    eprintln!();
    eprintln!("OPTIONS:");
    eprintln!("  -l <limit>    Stop counting after this many solutions per puzzle.");
    eprintln!("                 limit=1  Find the first solution and stop (default).");
    eprintln!("                 limit=2  Detect uniqueness: count will be 1 or 2+.");
    eprintln!("                 limit=0  Count all solutions (may be very slow for");
    eprintln!("                          puzzles with many solutions).");
    eprintln!("                 Default: 1");
    eprintln!("  -c            Count-only mode. Output only solution count and guesses,");
    eprintln!("                 not the solution string.");
    eprintln!("  -p            Input is pencilmark format (729 chars per puzzle).");
    eprintln!("  --pretty      Print each solution as a human-readable ASCII art grid");
    eprintln!("                 before the one-line output.");
    eprintln!("  --stats       Print a summary line to stderr after all puzzles:");
    eprintln!("                 total puzzles, total solved, total guesses, elapsed time,");
    eprintln!("                 and puzzles/second.");
    eprintln!("  -s <solver>   Solver to use: simd | scc | basic.  Default: simd");
    eprintln!("  -h            Display this help message.");
    eprintln!();
    eprintln!("EXAMPLES:");
    eprintln!("  # Solve a single puzzle from the command line:");
    eprintln!("  echo '53..7....6..195....98....6.8...6...34..8.3..17...2...6.6....28....419..5....8..79' \\");
    eprintln!("    | solve");
    eprintln!();
    eprintln!("  # Check uniqueness of all puzzles in a file:");
    eprintln!("  solve -l 2 -c puzzles.txt");
    eprintln!();
    eprintln!("  # Solve and display each solution as an ASCII art grid:");
    eprintln!("  solve --pretty puzzles.txt");
    eprintln!();
    eprintln!("  # Solve and count solutions for a batch, with a performance summary:");
    eprintln!("  solve --stats -l 1 tdoku/test/test_puzzles");
    eprintln!();
    eprintln!("  # Benchmark all three solvers on the same file:");
    eprintln!("  for s in simd scc basic; do");
    eprintln!("    echo \"--- $s ---\"");
    eprintln!("    solve -s $s --stats puzzles.txt");
    eprintln!("  done");
    eprintln!();
    eprintln!("  # Pipe output from the generator:");
    eprintln!("  generate -p 0 -l 20 | solve --pretty --stats");
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() {
    let mut options = Options::default();
    let mut files: Vec<String> = Vec::new();
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1usize;

    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--pretty" => {
                options.pretty = true;
                i += 1;
            }
            "--stats" => {
                options.stats = true;
                i += 1;
            }
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            "-c" => {
                options.count_only = true;
                i += 1;
            }
            "-p" => {
                options.pencilmark = true;
                i += 1;
            }
            _ if arg.starts_with('-') && arg.len() == 2 => {
                let ch = arg.chars().nth(1).unwrap();
                i += 1;
                if i >= args.len() {
                    eprintln!("error: -{ch} requires an argument");
                    std::process::exit(1);
                }
                let val = &args[i];
                match ch {
                    'l' => {
                        options.limit = val.parse().unwrap_or_else(|_| {
                            eprintln!("error: -l requires a non-negative integer, got '{val}'");
                            std::process::exit(1);
                        });
                    }
                    's' => {
                        options.solver = match val.as_str() {
                            "simd" => Solver::Simd,
                            "scc" => Solver::Scc,
                            "basic" => Solver::Basic,
                            _ => {
                                eprintln!("error: -s must be one of: simd, scc, basic");
                                std::process::exit(1);
                            }
                        };
                    }
                    _ => {
                        eprintln!("error: unknown option '-{ch}'. Run with -h for usage.");
                        std::process::exit(1);
                    }
                }
                i += 1;
            }
            _ if arg.starts_with('-') && arg != "-" => {
                eprintln!("error: unknown option '{}'. Run with -h for usage.", arg);
                std::process::exit(1);
            }
            _ => {
                files.push(arg.clone());
                i += 1;
            }
        }
    }

    if files.is_empty() {
        files.push("-".to_string());
    }

    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());

    let mut total_puzzles: u64 = 0;
    let mut total_solved: u64 = 0;
    let mut total_guesses: u64 = 0;
    let start = Instant::now();

    for filename in &files {
        let input: Box<dyn BufRead> = if filename == "-" {
            Box::new(io::BufReader::new(io::stdin()))
        } else {
            match std::fs::File::open(filename) {
                Ok(f) => Box::new(io::BufReader::new(f)),
                Err(e) => {
                    eprintln!("error: cannot open '{}': {}", filename, e);
                    std::process::exit(1);
                }
            }
        };

        for line in input.lines() {
            let line = line.unwrap_or_default();
            let puzzle = line.trim();
            if puzzle.is_empty() || puzzle.starts_with('#') {
                continue;
            }

            // For puzzle files with extra fields (e.g. "puzzle:count:solution"),
            // use only the first colon-separated field.
            let puzzle = puzzle.split(':').next().unwrap_or(puzzle);

            let expected_len = if options.pencilmark { 729 } else { 81 };
            if puzzle.len() < expected_len {
                eprintln!(
                    "warning: skipping short puzzle ({} chars, expected {}): {}",
                    puzzle.len(),
                    expected_len,
                    &puzzle[..puzzle.len().min(30)]
                );
                continue;
            }

            let (count, solution, guesses) = solve_line(puzzle, &options);

            total_puzzles += 1;
            if count > 0 {
                total_solved += 1;
            }
            total_guesses += guesses as u64;

            if options.pretty && count > 0 {
                let sol_for_pretty = if solution.trim_matches('\0').is_empty() {
                    puzzle
                } else {
                    &solution
                };
                write!(out, "{}", format_pretty(sol_for_pretty)).unwrap();
            }

            if options.count_only {
                writeln!(out, "{}  {}", count, guesses).unwrap();
            } else {
                let sol_out = if solution.bytes().all(|b| b == 0) {
                    ".".repeat(81)
                } else {
                    solution.clone()
                };
                writeln!(out, "{}  {}  {}", sol_out, count, guesses).unwrap();
            }
        }
    }

    out.flush().unwrap();

    if options.stats {
        let elapsed = start.elapsed().as_secs_f64();
        let pps = if elapsed > 0.0 {
            total_puzzles as f64 / elapsed
        } else {
            f64::INFINITY
        };
        eprintln!(
            "puzzles: {}  solved: {}  guesses: {}  elapsed: {:.3}s  rate: {:.0}/s",
            total_puzzles, total_solved, total_guesses, elapsed, pps
        );
    }
}
