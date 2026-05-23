//! Puzzle generator CLI — thin wrapper around [`rdoku::generator::Generator`].
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

use rdoku::generator::{format_pretty, format_puzzle_json, GeneratedPuzzle, GeneratorOptions};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

// ── CLI options ────────────────────────────────────────────────────────────

struct CliOptions {
    /// Stop after this many puzzles have been accepted (and counted, even if skipped).
    max_puzzles: u64,
    /// Skip the first N puzzles that would have been printed.
    skip: u64,
    /// Print every evaluated puzzle, not just those accepted into the pool.
    display_all: bool,
    /// Print each puzzle as an ASCII art grid.
    pretty: bool,
    /// Output as JSON objects.
    json: bool,
    /// Append the unique solution to each output line.
    solution: bool,
    /// Generator tuning parameters.
    gen: GeneratorOptions,
    /// Optional seed file.
    pattern_file: Option<String>,
}

impl Default for CliOptions {
    fn default() -> Self {
        Self {
            max_puzzles: u64::MAX,
            skip: 0,
            display_all: false,
            pretty: false,
            json: false,
            solution: false,
            gen: GeneratorOptions::default(),
            pattern_file: None,
        }
    }
}

// ── output helpers ─────────────────────────────────────────────────────────

fn print_puzzle(p: &GeneratedPuzzle, pretty: bool, json: bool, pencilmark: bool, solution: Option<&str>) {
    if json {
        let obj = format_puzzle_json(
            &p.puzzle,
            p.num_clues,
            p.geo_mean_guesses,
            p.loss,
            if pretty { Some(format_pretty(&p.puzzle, pencilmark)) } else { None },
            solution,
        );
        println!("{}", serde_json::to_string(&obj).unwrap());
    } else {
        if pretty {
            print!("{}", format_pretty(&p.puzzle, pencilmark));
        }
        match solution {
            Some(sol) => println!(
                "{} {} {:.1} {:.2} {}",
                p.puzzle, p.num_clues, p.geo_mean_guesses, p.loss, sol
            ),
            None => println!(
                "{} {} {:.1} {:.2}",
                p.puzzle, p.num_clues, p.geo_mean_guesses, p.loss
            ),
        }
    }
}

// ── usage text ────────────────────────────────────────────────────────────

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
    eprintln!(r"                        '1'–'9' = given clue, '.' = empty cell.");
    eprintln!(r"                        Example: 53..7....6..195....98....6.8...6...34..8.3..");
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
    eprintln!(r#"                      "solution" field."#);
    eprintln!("  -h                  Display this help message.");
    eprintln!("  -j, --json          Output each puzzle as a JSON object");
    eprintln!("                      (one per line) instead of plain text.");
    eprintln!("                      When combined with --pretty, includes formatted");
    eprintln!(r#"                      ASCII art in an additional "pretty" field."#);
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

// ── main ──────────────────────────────────────────────────────────────────

fn main() {
    let mut opts = CliOptions::default();
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

    while i < args.len() {
        let arg = &args[i];
        if arg == "--pretty" {
            opts.pretty = true;
            i += 1;
        } else if arg == "--json" || arg == "-j" {
            opts.json = true;
            i += 1;
        } else if arg == "--solution" || arg == "-s" {
            opts.solution = true;
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
                    Ok(v) => opts.skip = v,
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
                            Ok(v) if v >= 0.0 => opts.gen.clue_weight = v,
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
                            Ok(v) if v >= 0.0 => opts.gen.guess_weight = v,
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
                            Ok(v) if v >= 0.0 => opts.gen.random_weight = v,
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
                            Ok(v) => opts.gen.clues_to_drop = v,
                            Err(_) => {
                                eprintln!(
                                    "Error: invalid value for -d: {:?} (expected a non-negative integer).",
                                    val
                                );
                                std::process::exit(1);
                            }
                        },
                        'e' => match val.parse::<usize>() {
                            Ok(v) => opts.gen.num_evals = v,
                            Err(_) => {
                                eprintln!(
                                    "Error: invalid value for -e: {:?} (expected a non-negative integer).",
                                    val
                                );
                                std::process::exit(1);
                            }
                        },
                        'l' => match val.parse::<u64>() {
                            Ok(v) => opts.max_puzzles = v,
                            Err(_) => {
                                eprintln!(
                                    "Error: invalid value for -l: {:?} (expected a positive integer).",
                                    val
                                );
                                std::process::exit(1);
                            }
                        },
                        'n' => match val.parse::<usize>() {
                            Ok(v) if v >= 1 => opts.gen.num_puzzles_in_pool = v,
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
                        'm' => opts.gen.do_minimize = val != "0",
                        'a' => opts.display_all = val != "0",
                        'p' => opts.gen.pencilmark = val != "0",
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
            opts.pattern_file = Some(arg.clone());
            i += 1;
        } else {
            eprintln!("Unknown argument: {}", arg);
            print_usage();
            std::process::exit(1);
        }
    }

    if opts.max_puzzles != u64::MAX && opts.skip >= opts.max_puzzles {
        eprintln!(
            "Error: --skip ({}) must be less than -l ({}).",
            opts.skip, opts.max_puzzles
        );
        std::process::exit(1);
    }

    let running = Arc::new(AtomicBool::new(true));
    let running_clone = Arc::clone(&running);
    ctrlc::set_handler(move || {
        running_clone.store(false, Ordering::SeqCst);
    })
    .expect("Error setting Ctrl-C handler");

    let mut generator = rdoku::generator::Generator::new(opts.gen.clone(), Arc::clone(&running));
    match opts.pattern_file.as_deref() {
        None => generator.init_empty(),
        Some(path) => {
            generator.load(path);
        }
    }

    let max_puzzles = opts.max_puzzles;
    let skip = opts.skip;
    let display_all = opts.display_all;
    let need_solution = opts.solution;
    let pretty = opts.pretty;
    let json = opts.json;
    let pencilmark = opts.gen.pencilmark;

    let mut printed = 0u64;
    let cb = |p: GeneratedPuzzle| -> bool {
        let will_print = printed >= skip;
        if will_print {
            let solution = if need_solution {
                let (_, sol, _) = rdoku::solve_sudoku(&p.puzzle, 1, 0);
                Some(sol)
            } else {
                None
            };
            print_puzzle(&p, pretty, json, pencilmark, solution.as_deref());
        }
        printed += 1;
        printed < max_puzzles
    };

    if display_all {
        generator.run_all(cb);
    } else {
        generator.run_accepted(cb);
    }
}
