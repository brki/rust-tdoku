//! Benchmark runner — port of `tdoku/src/run_benchmark.cc`.
//!
//! Loads a puzzle file, replicates/samples it to a target dataset size,
//! runs a configurable warmup phase, then benchmarks all registered solvers
//! and reports puzzles/sec, usec/puzzle, %no_guess, and guesses/puzzle.
//!
//! Usage: benchmark [options] puzzle_file_1 [puzzle_file_2 ...]
//!
//! Options:
//!   -a                  // do rating (per-puzzle timing/backtracks)
//!   -b                  // rate by backtracks instead of time
//!   -c [0|1]            // output csv instead of table [default 0]
//!   -e <seed>           // random seed [default random_device{}()]
//!   -f                  // stop at first solution (don't validate uniqueness)
//!   -h                  // display this help message
//!   -n <size>           // test set size [default 100000]
//!   -p                  // expect 729-character pencilmark sudoku
//!   -r [0|1]            // randomly permute puzzles [default 1]
//!   -s solver_1,...     // which solvers to run [default all]
//!   -t <secs>           // target test time in seconds [default 10]
//!   -v [0|1]            // validate during warmup [default 1]
//!   -w <secs>           // target warmup time in seconds [default 4]

use rdoku::util::Util;
use std::io::BufRead;
use std::time::Instant;

// ─── Solver abstraction ───────────────────────────────────────────────────

type SolveFn = fn(&[u8], usize, u32) -> (usize, [u8; 81], usize);

struct Solver {
    solve_fn: SolveFn,
    config: u32,
    id: &'static str,
    desc: &'static str,
    returns_solution: bool,
    returns_guess_count: bool,
}

impl Solver {
    fn solve(&self, puzzle: &[u8], limit: usize) -> (usize, [u8; 81], usize) {
        (self.solve_fn)(puzzle, limit, self.config)
    }
}

fn all_solvers() -> Vec<Solver> {
    vec![
        Solver {
            solve_fn: rdoku::solver_dpll_triad_scc::solve,
            config: 0,
            id: "_tdev_dpll_triad",
            desc: "S/shrc+./m.",
            returns_solution: true,
            returns_guess_count: true,
        },
        Solver {
            solve_fn: rdoku::solver_dpll_triad_scc::solve,
            config: 1,
            id: "_tdev_dpll_triad_scc_i",
            desc: "S/shrc++/m.",
            returns_solution: true,
            returns_guess_count: true,
        },
        Solver {
            solve_fn: rdoku::solver_dpll_triad_scc::solve,
            config: 2,
            id: "_tdev_dpll_triad_scc_h",
            desc: "S/shrc+./m+",
            returns_solution: true,
            returns_guess_count: true,
        },
        Solver {
            solve_fn: rdoku::solver_dpll_triad_scc::solve,
            config: 3,
            id: "_tdev_dpll_triad_scc_ih",
            desc: "S/shrc++/m+",
            returns_solution: true,
            returns_guess_count: true,
        },
        Solver {
            solve_fn: rdoku::solver_basic::solve,
            config: 0,
            id: "_tdev_basic",
            desc: "G/....../..",
            returns_solution: true,
            returns_guess_count: true,
        },
        Solver {
            solve_fn: rdoku::solver_basic::solve,
            config: 1,
            id: "_tdev_basic_heuristic",
            desc: "G/s...../m.",
            returns_solution: true,
            returns_guess_count: true,
        },
        Solver {
            solve_fn: rdoku::solver_dpll_triad_simd::solve,
            config: 0,
            id: "tdoku",
            desc: "T/shrc+./m+",
            returns_solution: true,
            returns_guess_count: true,
        },
    ]
}

// ─── Options ──────────────────────────────────────────────────────────────

struct Options {
    /// Expect 729-char pencilmark input instead of 81-char sudoku.
    pencilmark: bool,
    /// Rate puzzles by backtracks instead of time.
    rate_by_backtracks: bool,
    /// Size of the dataset to create from input (via replication / sampling).
    test_dataset_size: usize,
    /// Target warmup time in seconds.
    min_seconds_warmup: f64,
    /// Target benchmark duration in seconds.
    min_seconds_test: f64,
    /// Randomly permute puzzles when building the test dataset.
    randomize: bool,
    /// Fixed random seed (0 = use OS entropy).
    random_seed: u64,
    /// Stop at the first solution instead of checking for uniqueness.
    first_solution: bool,
    /// Validate puzzle solutions during warmup.
    validate: bool,
    /// Emit CSV instead of a Markdown table.
    csv_output: bool,
    /// Solver IDs to run (None = all).
    solver_ids: Option<Vec<String>>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            pencilmark: false,
            rate_by_backtracks: false,
            test_dataset_size: 100_000,
            min_seconds_warmup: 4.0,
            min_seconds_test: 10.0,
            randomize: true,
            random_seed: 0,
            first_solution: false,
            validate: true,
            csv_output: false,
            solver_ids: None,
        }
    }
}

// ─── Benchmark ────────────────────────────────────────────────────────────

struct Benchmark {
    options: Options,
    puzzle_size: usize,
    /// Flat buffer: slot i occupies bytes [i*puzzle_size .. (i+1)*puzzle_size].
    dataset: Vec<u8>,
    /// True if the dataset file contains unsolvable puzzles (ALLOWZERO comment).
    allow_zero: bool,
    util: Util,
}

impl Benchmark {
    fn new(options: Options) -> Self {
        let puzzle_size = if options.pencilmark { 729 } else { 81 };
        Self {
            options,
            puzzle_size,
            dataset: Vec::new(),
            allow_zero: false,
            util: Util::new(),
        }
    }

    // ── Permutation helpers ────────────────────────────────────────────────

    /// Permute the puzzle at slot `slot` in place.
    /// Uses destructuring to avoid simultaneous mutable borrow conflicts
    /// between `dataset` and `util`.
    fn permute_slot(&mut self, slot: usize) {
        let Benchmark {
            dataset,
            puzzle_size,
            options,
            util,
            ..
        } = self;
        let start = slot * *puzzle_size;
        util.permute_sudoku(
            &mut dataset[start..start + *puzzle_size],
            options.pencilmark,
        );
    }

    // ── Dataset loading ────────────────────────────────────────────────────

    /// Load puzzles from `filename` into `self.dataset`, replicating / sampling
    /// to reach exactly `test_dataset_size` slots.
    fn load(&mut self, filename: &str) {
        let file = match std::fs::File::open(filename) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("Error opening {}: {}", filename, e);
                std::process::exit(1);
            }
        };

        self.allow_zero = false;
        let n = self.options.test_dataset_size;
        let ps = self.puzzle_size;
        self.dataset.resize(n * ps, 0);

        let reader = std::io::BufReader::new(file);
        let mut num_loaded: usize = 0;
        let mut num_processed: usize = 0;

        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => continue,
            };
            let line = line.trim_end_matches('\r');

            if line.is_empty() {
                continue;
            }
            if line.starts_with('#') {
                if line.contains("ALLOWZERO") {
                    self.allow_zero = true;
                }
                continue;
            }
            if line.len() < ps {
                continue;
            }

            num_processed += 1;

            if num_loaded < n {
                let dest = &mut self.dataset[num_loaded * ps..(num_loaded + 1) * ps];
                dest.copy_from_slice(&line.as_bytes()[..ps]);
                if self.options.randomize {
                    // Copy out → permute → copy back to avoid borrow conflict with self.util.
                    let slot = num_loaded;
                    let mut buf: Vec<u8> = self.dataset[slot * ps..(slot + 1) * ps].to_vec();
                    self.util.permute_sudoku(&mut buf, self.options.pencilmark);
                    self.dataset[slot * ps..(slot + 1) * ps].copy_from_slice(&buf);
                }
                num_loaded += 1;
            } else {
                // Reservoir sampling: replace a random existing slot.
                if self.util.random_double() < n as f64 / num_processed as f64 {
                    let replace = self.util.random_uint() as usize % n;
                    let dest = &mut self.dataset[replace * ps..(replace + 1) * ps];
                    dest.copy_from_slice(&line.as_bytes()[..ps]);
                    if self.options.randomize {
                        let mut buf: Vec<u8> =
                            self.dataset[replace * ps..(replace + 1) * ps].to_vec();
                        self.util.permute_sudoku(&mut buf, self.options.pencilmark);
                        self.dataset[replace * ps..(replace + 1) * ps].copy_from_slice(&buf);
                    }
                }
            }
        }

        // If the input file had fewer puzzles than the dataset size and we loaded
        // all of them, duplicate the input in full copies (with fresh permutations).
        if num_loaded == num_processed && num_loaded > 0 {
            while num_loaded + num_processed <= n {
                for j in 0..num_processed {
                    let src_start = j * ps;
                    let dst_start = (num_loaded + j) * ps;
                    self.dataset
                        .copy_within(src_start..src_start + ps, dst_start);
                    if self.options.randomize {
                        let slot = num_loaded + j;
                        self.permute_slot(slot);
                    }
                }
                num_loaded += num_processed;
            }
        }

        // Fill remaining slots by uniform sampling from already-loaded puzzles.
        let num_source = num_loaded.max(1); // guard against empty file
        for i in num_loaded..n {
            let which = self.util.random_uint() as usize % num_source;
            let src_start = which * ps;
            let dst_start = i * ps;
            self.dataset
                .copy_within(src_start..src_start + ps, dst_start);
            if self.options.randomize {
                self.permute_slot(i);
            }
        }
    }

    // ── Validation ────────────────────────────────────────────────────────

    fn validate_solution(solution: &[u8; 81]) -> bool {
        let mut covered = [0u32; 27]; // [0..9]=rows, [9..18]=cols, [18..27]=boxes
        for row in 0..9 {
            for col in 0..9 {
                let b = solution[row * 9 + col];
                if !(b'1'..=b'9').contains(&b) {
                    return false;
                }
                let bit = 1u32 << (b - b'1');
                covered[row] ^= bit;
                covered[9 + col] ^= bit;
                covered[18 + 3 * (row / 3) + (col / 3)] ^= bit;
            }
        }
        covered.iter().all(|&x| x == 0x1ff)
    }

    // ── Output helpers ────────────────────────────────────────────────────

    fn output_header(&self, filename: &str) {
        if !self.options.csv_output {
            println!();
            println!(
                "|{:<37} |  puzzles/sec|  usec/puzzle|   %no_guess|  guesses/puzzle|",
                filename
            );
            println!(
                "|--------------------------------------|------------:|------------:|-----------:|---------------:|"
            );
        }
    }

    fn output_result(
        &self,
        solver: &Solver,
        dataset_filename: &str,
        num_solved: usize,
        usec_total: f64,
        total_guesses: usize,
        total_no_guess: usize,
    ) {
        let puzzles_per_second = 1_000_000.0 * num_solved as f64 / usec_total;
        let usec_per_puzzle = usec_total / num_solved as f64;
        let percent_no_guess = 100.0 * total_no_guess as f64 / num_solved as f64;
        let guesses_per_puzzle = total_guesses as f64 / num_solved as f64;

        if self.options.csv_output {
            let build_flags = if cfg!(debug_assertions) { "-g" } else { "-O3" };
            if solver.returns_guess_count {
                println!(
                    "rustc,{},{},{},{},{},{},{},{}",
                    env!("CARGO_PKG_VERSION"),
                    build_flags,
                    dataset_filename,
                    solver.id,
                    puzzles_per_second,
                    usec_per_puzzle,
                    percent_no_guess,
                    guesses_per_puzzle
                );
            } else {
                println!(
                    "rustc,{},{},{},{},{},{},N/A,N/A",
                    env!("CARGO_PKG_VERSION"),
                    build_flags,
                    dataset_filename,
                    solver.id,
                    puzzles_per_second,
                    usec_per_puzzle,
                );
            }
        } else if solver.returns_guess_count {
            println!(
                "|{:<27}{:<11}|{:>12.1} |{:>12.1} |{:>10.1}% |{:>15.2} |",
                solver.id,
                solver.desc,
                puzzles_per_second,
                usec_per_puzzle,
                percent_no_guess,
                guesses_per_puzzle
            );
        } else {
            println!(
                "|{:<27}{:<11}|{:>12.1} |{:>12.1} |        N/A |            N/A |",
                solver.id, solver.desc, puzzles_per_second, usec_per_puzzle,
            );
        }
    }

    // ── Warmup ────────────────────────────────────────────────────────────

    /// Run the solver for `min_seconds_warmup` seconds on random puzzles to warm
    /// caches and branch predictors.  Returns the estimated puzzles/second rate.
    fn warmup_and_estimate_rate(&mut self, solver: &Solver) -> f64 {
        let n = self.options.test_dataset_size;
        let ps = self.puzzle_size;
        let warmup_secs = self.options.min_seconds_warmup;
        let validate = self.options.validate;
        let allow_zero = self.allow_zero;

        let start = Instant::now();
        let mut warmup_count: usize = 0;

        loop {
            let idx = self.util.random_uint() as usize % n;
            let puzzle = &self.dataset[idx * ps..(idx + 1) * ps];
            let (count, sol, _guesses) = solver.solve(puzzle, 1);
            if !allow_zero
                && (count == 0
                    || (validate && solver.returns_solution && !Self::validate_solution(&sol)))
            {
                eprintln!("Error during warmup");
                eprintln!("{}", String::from_utf8_lossy(puzzle));
                std::process::exit(1);
            }
            warmup_count += 1;
            let elapsed = start.elapsed().as_secs_f64();
            if elapsed >= warmup_secs {
                return warmup_count as f64 / elapsed;
            }
        }
    }

    // ── Test (main benchmark loop) ────────────────────────────────────────

    fn test(&mut self, filename: &str, solvers: &[Solver]) {
        if self.options.random_seed > 0 {
            self.util.random_seed(self.options.random_seed);
        }
        self.load(filename);

        // Generate a permutation for use by slow solvers (to avoid difficulty bias).
        let perm = self.util.permutation(self.options.test_dataset_size);

        self.output_header(filename);

        let n = self.options.test_dataset_size;
        let ps = self.puzzle_size;
        let min_secs_test = self.options.min_seconds_test;
        let first_solution = self.options.first_solution;
        let allow_zero = self.allow_zero;

        for solver in solvers {
            let pps = self.warmup_and_estimate_rate(solver);

            // Fast path: expected to complete a full pass in ≤ 2× test time.
            let fast = pps * min_secs_test * 2.0 > n as f64;

            let mut total_guesses: usize = 0;
            let mut total_no_guess: usize = 0;
            let mut total_solved: usize = 0;

            let limit: usize = if first_solution { 1 } else { 2 };
            let start = Instant::now();

            if fast {
                // Full passes over the entire dataset.
                while start.elapsed().as_secs_f64() < min_secs_test {
                    for i in 0..n {
                        let puzzle = &self.dataset[i * ps..(i + 1) * ps];
                        let (count, _sol, guesses) = solver.solve(puzzle, limit);
                        if !allow_zero && count == 0 {
                            eprintln!("Error during benchmark");
                            eprintln!("{}", String::from_utf8_lossy(puzzle));
                            std::process::exit(1);
                        }
                        total_guesses += guesses;
                        total_no_guess += (guesses == 0) as usize;
                    }
                    total_solved += n;
                }
            } else {
                // Slow solver: iterate over permuted dataset order.
                while start.elapsed().as_secs_f64() < min_secs_test * 2.0 {
                    let idx = perm[total_solved % n];
                    let puzzle = &self.dataset[idx * ps..(idx + 1) * ps];
                    let (count, _sol, guesses) = solver.solve(puzzle, limit);
                    if !allow_zero && count == 0 {
                        eprintln!("Error during benchmark");
                        eprintln!("{}", String::from_utf8_lossy(puzzle));
                        std::process::exit(1);
                    }
                    total_guesses += guesses;
                    total_no_guess += (guesses == 0) as usize;
                    total_solved += 1;
                }
            }

            let elapsed_usec = start.elapsed().as_micros() as f64;
            self.output_result(
                solver,
                filename,
                total_solved,
                elapsed_usec,
                total_guesses,
                total_no_guess,
            );
        }
    }

    // ── Rating mode ───────────────────────────────────────────────────────

    /// For each puzzle in `filename`, time all solvers solving `test_dataset_size`
    /// permuted copies, and print a tab-separated row of costs per solver.
    fn rate(&mut self, filename: &str, solvers: &[Solver]) {
        let file = match std::fs::File::open(filename) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("Error opening {}: {}", filename, e);
                std::process::exit(1);
            }
        };

        let n = self.options.test_dataset_size;
        let ps = self.puzzle_size;
        self.dataset.resize(n * ps, 0);

        let reader = std::io::BufReader::new(file);
        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => continue,
            };
            let line = line.trim_end_matches('\r');
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.len() < ps {
                continue;
            }

            let puzzle_bytes = &line.as_bytes()[..ps];

            // Fill all slots with (possibly permuted) copies of this puzzle.
            for i in 0..n {
                let dst = i * ps;
                self.dataset[dst..dst + ps].copy_from_slice(puzzle_bytes);
                if self.options.randomize {
                    self.permute_slot(i);
                }
            }

            for solver in solvers {
                let start = Instant::now();
                let mut total_guesses = 0.0f64;
                for i in 0..n {
                    let puzzle = &self.dataset[i * ps..(i + 1) * ps];
                    let (_count, _sol, guesses) = solver.solve(puzzle, 1);
                    total_guesses += guesses as f64;
                }
                let elapsed_usec = start.elapsed().as_micros() as f64;
                let cost = if self.options.rate_by_backtracks {
                    total_guesses / n as f64
                } else {
                    elapsed_usec / n as f64
                };
                print!("{:>12.1}\t", cost);
            }
            println!();
        }
    }
}

// ─── CLI ──────────────────────────────────────────────────────────────────

fn print_usage() {
    eprintln!("usage: benchmark [options] puzzle_file_1 [...]");
    eprintln!("options:");
    eprintln!("  -a                  // do rating");
    eprintln!("  -b                  // rate by backtracks");
    eprintln!("  -c [0|1]            // output csv instead of table [default 0]");
    eprintln!("  -e <seed>           // random seed [default random_device{{}}()]");
    eprintln!("  -f                  // stop at first solution");
    eprintln!("  -h                  // display this help message");
    eprintln!("  -n <size>           // test set size [default 100000]");
    eprintln!("  -p                  // expect 729 character pencilmark sudoku");
    eprintln!("  -r [0|1]            // randomly permute puzzles [default 1]");
    eprintln!("  -s solver_1,...     // which solvers to run [default all]");
    eprintln!("  -t <secs>           // target test time [default 10]");
    eprintln!("  -v [0|1]            // validate during warmup [default 1]");
    eprintln!("  -w <secs>           // target warmup time [default 4]");
    eprintln!("solvers:");
    for s in all_solvers() {
        eprint!(" {}", s.id);
    }
    eprintln!();
    eprintln!(
        "build info: rustc {} {}",
        env!("CARGO_PKG_VERSION"),
        if cfg!(debug_assertions) { "-g" } else { "-O3" }
    );
}

fn parse_args() -> (Options, Vec<String>, bool) {
    let args: Vec<String> = std::env::args().collect();
    let mut options = Options::default();
    let mut filenames: Vec<String> = Vec::new();
    let mut do_rating = false;

    let mut i = 1usize;
    while i < args.len() {
        let arg = &args[i];

        if arg == "-h" || arg == "--help" {
            print_usage();
            std::process::exit(0);
        } else if arg == "-a" {
            do_rating = true;
            i += 1;
        } else if arg == "-b" {
            options.rate_by_backtracks = true;
            i += 1;
        } else if arg == "-f" {
            options.first_solution = true;
            i += 1;
        } else if arg == "-p" {
            options.pencilmark = true;
            i += 1;
        } else if arg.starts_with('-') && arg.len() == 2 {
            let ch = arg.chars().nth(1).unwrap();
            i += 1;

            // Flags with required arguments
            if matches!(ch, 'e' | 'n' | 's' | 't' | 'w') {
                let val = args.get(i).cloned().unwrap_or_default();
                i += 1;
                match ch {
                    'e' => {
                        if let Ok(v) = val.parse() {
                            options.random_seed = v;
                        }
                    }
                    'n' => {
                        if let Ok(v) = val.parse() {
                            options.test_dataset_size = v;
                        }
                    }
                    's' => {
                        options.solver_ids = Some(val.split(',').map(str::to_owned).collect());
                    }
                    't' => {
                        if let Ok(v) = val.parse::<f64>() {
                            options.min_seconds_test = v;
                        }
                    }
                    'w' => {
                        if let Ok(v) = val.parse::<f64>() {
                            options.min_seconds_warmup = v;
                        }
                    }
                    _ => unreachable!(),
                }
            // Flags with optional boolean arguments
            } else if matches!(ch, 'c' | 'r' | 'v') {
                let val = match args.get(i).map(String::as_str) {
                    Some("0") | Some("1") => {
                        let v = args[i].as_str();
                        i += 1;
                        v
                    }
                    _ => "1",
                };
                match ch {
                    'c' => options.csv_output = val != "0",
                    'r' => options.randomize = val != "0",
                    'v' => options.validate = val != "0",
                    _ => unreachable!(),
                }
            } else {
                eprintln!("Unknown flag: -{}", ch);
                print_usage();
                std::process::exit(1);
            }
        } else if !arg.starts_with('-') {
            filenames.push(arg.clone());
            i += 1;
        } else {
            eprintln!("Unknown argument: {}", arg);
            print_usage();
            std::process::exit(1);
        }
    }

    (options, filenames, do_rating)
}

fn main() {
    let (options, filenames, do_rating) = parse_args();

    // Build the solver list (filtered by -s option if provided).
    let all = all_solvers();
    let solvers: Vec<Solver> = match &options.solver_ids {
        None => all,
        Some(ids) => {
            let id_set: std::collections::HashSet<&str> = ids.iter().map(String::as_str).collect();
            all.into_iter().filter(|s| id_set.contains(s.id)).collect()
        }
    };

    if solvers.is_empty() {
        eprintln!("No matching solvers found. Use -h to see available solvers.");
        std::process::exit(1);
    }

    let mut benchmark = Benchmark::new(options);

    if filenames.is_empty() {
        // Default dataset when no file is given — match C++ fallback.
        benchmark.test("data/puzzles1_unbiased", &solvers);
    } else {
        for filename in &filenames {
            if do_rating {
                benchmark.rate(filename, &solvers);
            } else {
                benchmark.test(filename, &solvers);
            }
        }
    }
}
