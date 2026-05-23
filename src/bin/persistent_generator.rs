//! Persistent generator pool — controller and worker modes.
//!
//! # Controller mode (default)
//!
//! Reads a JSON config file that defines one or more difficulty *profiles*. Each
//! profile spawns `count` long-running worker processes.  Workers warm up by
//! running the hill-climbing generator for `warmup` accepted puzzles before
//! announcing readiness.  Incoming client connections are held until a worker of
//! the requested profile becomes available (or the timeout expires).
//!
//! ```
//! persistent_generator [--config pg_config.json] [--socket /tmp/rdoku-pg.sock]
//!                       [--max-timeout 60]
//! ```
//!
//! # Worker mode (internal)
//!
//! Invoked automatically by the controller using `--internal-worker <json>`.
//! Not intended for direct use.
//!
//! # Configuration file (JSON)
//!
//! ```json
//! {
//!   "profiles": [
//!     {
//!       "name": "default",
//!       "count": 2,
//!       "warmup": 10000,
//!       "pool_size": 500,
//!       "clue_weight": 1.0,
//!       "guess_weight": 0.5,
//!       "random_weight": 1.0,
//!       "clues_to_drop": 3,
//!       "num_evals": 10
//!     }
//!   ]
//! }
//! ```

use rdoku::generator::{format_pretty, format_puzzle_json, GeneratedPuzzle, GeneratorOptions};
use rdoku::pg_protocol::{
    read_json_line, write_json_line, ClientError, ClientRequest, ClientResponse, PuzzleOptions,
    WorkerReady, WorkerRegister, WorkerRequest, WorkerResponse,
};
use serde::Deserialize;
use std::collections::HashMap;
use std::io::{BufReader, BufWriter};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ── config ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ProfileConfig {
    name: String,
    #[serde(default = "default_count")]
    count: usize,
    #[serde(default = "default_warmup")]
    warmup: u64,
    #[serde(default = "default_pool_size")]
    pool_size: usize,
    #[serde(default = "default_clue_weight")]
    clue_weight: f64,
    #[serde(default = "default_guess_weight")]
    guess_weight: f64,
    #[serde(default = "default_random_weight")]
    random_weight: f64,
    #[serde(default = "default_clues_to_drop")]
    clues_to_drop: usize,
    #[serde(default = "default_num_evals")]
    num_evals: usize,
    #[serde(default)]
    pencilmark: bool,
}

fn default_count() -> usize { 2 }
fn default_warmup() -> u64 { 10_000 }
fn default_pool_size() -> usize { 500 }
fn default_clue_weight() -> f64 { 1.0 }
fn default_guess_weight() -> f64 { 0.5 }
fn default_random_weight() -> f64 { 1.0 }
fn default_clues_to_drop() -> usize { 3 }
fn default_num_evals() -> usize { 10 }

#[derive(Debug, Deserialize)]
struct Config {
    profiles: Vec<ProfileConfig>,
}

// ── worker config passed via --internal-worker ────────────────────────────

#[derive(Debug, serde::Serialize, Deserialize, Clone)]
struct WorkerConfig {
    profile: String,
    socket_path: String,
    warmup: u64,
    gen: WorkerGenConfig,
}

#[derive(Debug, serde::Serialize, Deserialize, Clone)]
struct WorkerGenConfig {
    clue_weight: f64,
    guess_weight: f64,
    random_weight: f64,
    clues_to_drop: usize,
    num_evals: usize,
    num_puzzles_in_pool: usize,
    do_minimize: bool,
    pencilmark: bool,
}

// ── controller state ───────────────────────────────────────────────────────

/// Per-profile pool of idle worker channels.
///
/// Each entry is a `Sender` through which the controller can dispatch a request
/// to the worker's dedicated handler thread.  When the worker finishes, it
/// re-inserts its sender into the pool.
type WorkerSender = mpsc::Sender<(WorkerRequest, mpsc::Sender<Result<WorkerResponse, String>>)>;

struct ProfilePool {
    idle: Mutex<std::collections::VecDeque<WorkerSender>>,
    condvar: std::sync::Condvar,
}

impl ProfilePool {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            idle: Mutex::new(std::collections::VecDeque::new()),
            condvar: std::sync::Condvar::new(),
        })
    }
}

struct ControllerState {
    profiles: HashMap<String, Arc<ProfilePool>>,
    max_timeout: Duration,
}

// ── output formatting helper ───────────────────────────────────────────────

fn format_output(p: &GeneratedPuzzle, opts: &PuzzleOptions) -> String {
    if opts.json {
        let solution = if opts.solution {
            let (_, sol, _) = rdoku::solve_sudoku(&p.puzzle, 1, 0);
            Some(sol)
        } else {
            None
        };
        let obj = format_puzzle_json(
            &p.puzzle,
            p.num_clues,
            p.geo_mean_guesses,
            p.loss,
            if opts.pretty { Some(format_pretty(&p.puzzle, opts.pencilmark)) } else { None },
            solution.as_deref(),
        );
        serde_json::to_string(&obj).unwrap_or_default()
    } else {
        let mut out = String::new();
        if opts.pretty {
            out.push_str(&format_pretty(&p.puzzle, opts.pencilmark));
        }
        if opts.solution {
            let (_, sol, _) = rdoku::solve_sudoku(&p.puzzle, 1, 0);
            out.push_str(&format!(
                "{} {} {:.1} {:.2} {}",
                p.puzzle, p.num_clues, p.geo_mean_guesses, p.loss, sol
            ));
        } else {
            out.push_str(&format!(
                "{} {} {:.1} {:.2}",
                p.puzzle, p.num_clues, p.geo_mean_guesses, p.loss
            ));
        }
        out
    }
}

// ── controller ─────────────────────────────────────────────────────────────

fn run_controller(socket_path: &str, config: Config, max_timeout_secs: f64) {
    // Build profile pool map.
    let mut profiles: HashMap<String, Arc<ProfilePool>> = HashMap::new();
    for p in &config.profiles {
        profiles.insert(p.name.clone(), ProfilePool::new());
    }

    let running = Arc::new(AtomicBool::new(true));
    let state = Arc::new(ControllerState {
        profiles,
        max_timeout: Duration::from_secs_f64(max_timeout_secs),
    });

    // Set up Ctrl-C handler.
    {
        let running = Arc::clone(&running);
        let path = socket_path.to_string();
        ctrlc::set_handler(move || {
            eprintln!("[controller] shutting down...");
            running.store(false, Ordering::SeqCst);
            // Wake up the accept loop by connecting briefly.
            let _ = UnixStream::connect(&path);
        })
        .expect("Error setting Ctrl-C handler");
    }

    // Remove stale socket file if it exists.
    let _ = std::fs::remove_file(socket_path);

    let listener = UnixListener::bind(socket_path).unwrap_or_else(|e| {
        eprintln!("[controller] failed to bind socket {}: {}", socket_path, e);
        std::process::exit(1);
    });

    eprintln!("[controller] listening on {}", socket_path);

    // Spawn worker processes for each profile.
    let exe = std::env::current_exe().expect("cannot find current executable");
    let mut child_handles: Vec<(std::process::Child, String)> = Vec::new();

    for profile_cfg in &config.profiles {
        let worker_cfg = WorkerConfig {
            profile: profile_cfg.name.clone(),
            socket_path: socket_path.to_string(),
            warmup: profile_cfg.warmup,
            gen: WorkerGenConfig {
                clue_weight: profile_cfg.clue_weight,
                guess_weight: profile_cfg.guess_weight,
                random_weight: profile_cfg.random_weight,
                clues_to_drop: profile_cfg.clues_to_drop,
                num_evals: profile_cfg.num_evals,
                num_puzzles_in_pool: profile_cfg.pool_size,
                do_minimize: true,
                pencilmark: profile_cfg.pencilmark,
            },
        };
        let worker_json = serde_json::to_string(&worker_cfg).expect("serialize worker config");

        for _ in 0..profile_cfg.count {
            let child = std::process::Command::new(&exe)
                .arg("--internal-worker")
                .arg(&worker_json)
                .spawn()
                .unwrap_or_else(|e| {
                    eprintln!("[controller] failed to spawn worker: {}", e);
                    std::process::exit(1);
                });
            eprintln!(
                "[controller] spawned worker {} (pid {})",
                profile_cfg.name,
                child.id()
            );
            child_handles.push((child, profile_cfg.name.clone()));
        }
    }

    // Background thread: monitor child processes and log exits.
    {
        let handles = Arc::new(Mutex::new(child_handles));
        let running = Arc::clone(&running);
        std::thread::spawn(move || {
            while running.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_secs(1));
                let mut guard = handles.lock().unwrap();
                for (child, profile) in guard.iter_mut() {
                    match child.try_wait() {
                        Ok(Some(status)) => {
                            if !status.success() {
                                eprintln!(
                                    "[controller] worker for profile '{}' (pid {}) exited: {}",
                                    profile,
                                    child.id(),
                                    status
                                );
                            }
                        }
                        Ok(None) => {} // still running
                        Err(e) => eprintln!("[controller] wait error: {}", e),
                    }
                }
            }
        });
    }

    // Accept loop.
    for stream in listener.incoming() {
        if !running.load(Ordering::SeqCst) {
            break;
        }
        let stream = match stream {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[controller] accept error: {}", e);
                continue;
            }
        };
        let state = Arc::clone(&state);
        std::thread::spawn(move || {
            handle_connection(stream, state);
        });
    }

    eprintln!("[controller] stopped accepting connections");
    let _ = std::fs::remove_file(socket_path);
}

fn handle_connection(stream: UnixStream, state: Arc<ControllerState>) {
    let read_stream = match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    let mut reader = BufReader::new(read_stream);

    // Peek at first message to determine peer type.
    let mut line = String::new();
    if std::io::BufRead::read_line(&mut reader, &mut line).unwrap_or(0) == 0 {
        return;
    }

    // Determine message type from the `type` field.
    let type_val: serde_json::Value = match serde_json::from_str(line.trim_end()) {
        Ok(v) => v,
        Err(_) => return,
    };
    let msg_type = type_val.get("type").and_then(|v| v.as_str()).unwrap_or("");

    match msg_type {
        "WorkerRegister" => {
            if let Ok(reg) = serde_json::from_str::<WorkerRegister>(line.trim_end()) {
                handle_worker(stream, reader, reg.profile, state);
            }
        }
        "ClientRequest" => {
            if let Ok(req) = serde_json::from_str::<ClientRequest>(line.trim_end()) {
                handle_client(stream, req, state);
            }
        }
        _ => {
            eprintln!("[controller] unknown message type: {}", msg_type);
        }
    }
}

fn handle_worker(
    stream: UnixStream,
    mut reader: BufReader<UnixStream>,
    profile: String,
    state: Arc<ControllerState>,
) {
    eprintln!("[controller] worker registered for profile '{}'", profile);

    // Wait for WorkerReady.
    let ready: WorkerReady = match read_json_line(&mut reader) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[controller] worker did not send WorkerReady: {}", e);
            return;
        }
    };
    if ready.msg_type != "WorkerReady" {
        eprintln!("[controller] expected WorkerReady, got '{}'", ready.msg_type);
        return;
    }
    eprintln!("[controller] worker for profile '{}' is ready", profile);

    let pool = match state.profiles.get(&profile) {
        Some(p) => Arc::clone(p),
        None => {
            eprintln!("[controller] unknown profile '{}' from worker", profile);
            return;
        }
    };

    // Create channel for this worker.
    let (tx, rx) = mpsc::channel::<(WorkerRequest, mpsc::Sender<Result<WorkerResponse, String>>)>();

    let mut writer = BufWriter::new(stream);

    loop {
        // Re-add this worker to the idle pool.
        {
            let mut guard = pool.idle.lock().unwrap();
            guard.push_back(tx.clone());
        }
        pool.condvar.notify_all();

        // Wait for a dispatched request.
        let (request, reply_tx) = match rx.recv() {
            Ok(r) => r,
            Err(_) => break, // controller shutting down, channel closed
        };

        // Send request to worker process.
        if let Err(e) = write_json_line(&mut writer, &request) {
            eprintln!("[controller] error writing to worker: {}", e);
            let _ = reply_tx.send(Err(format!("worker write error: {}", e)));
            break;
        }

        // Read response from worker process.
        match read_json_line::<_, WorkerResponse>(&mut reader) {
            Ok(resp) => {
                let _ = reply_tx.send(Ok(resp));
            }
            Err(e) => {
                eprintln!("[controller] error reading from worker: {}", e);
                let _ = reply_tx.send(Err(format!("worker read error: {}", e)));
                break;
            }
        }
    }

    eprintln!("[controller] worker for profile '{}' disconnected", profile);
}

fn handle_client(stream: UnixStream, req: ClientRequest, state: Arc<ControllerState>) {
    let mut writer = BufWriter::new(stream);

    // Cap the requested timeout to the controller's max.
    let timeout = Duration::from_secs_f64(req.timeout_secs.min(state.max_timeout.as_secs_f64()));
    let deadline = Instant::now() + timeout;

    let pool = match state.profiles.get(&req.profile) {
        Some(p) => Arc::clone(p),
        None => {
            let _ = write_json_line(
                &mut writer,
                &ClientError::new(&format!("unknown profile '{}'", req.profile)),
            );
            return;
        }
    };

    // Acquire an idle worker (or wait until one becomes available).
    let worker_tx = {
        let mut guard = pool.idle.lock().unwrap();
        loop {
            if let Some(tx) = guard.pop_front() {
                break Some(tx);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break None;
            }
            let (g, _) = pool.condvar.wait_timeout(guard, remaining).unwrap();
            guard = g;
        }
    };

    let worker_tx = match worker_tx {
        Some(tx) => tx,
        None => {
            let _ = write_json_line(
                &mut writer,
                &ClientError::new(&format!(
                    "timeout: no worker available for profile '{}' within {:.1}s",
                    req.profile,
                    timeout.as_secs_f64()
                )),
            );
            return;
        }
    };

    // Dispatch request to worker.
    let request_id = uuid_v4();
    let worker_req = WorkerRequest::new(&request_id, req.options);
    let (reply_tx, reply_rx) = mpsc::channel();

    if worker_tx.send((worker_req, reply_tx)).is_err() {
        let _ = write_json_line(&mut writer, &ClientError::new("worker unavailable"));
        return;
    }

    // Wait for response (with timeout).
    let remaining = deadline.saturating_duration_since(Instant::now());
    let worker_resp = match reply_rx.recv_timeout(remaining.max(Duration::from_secs(60))) {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            let _ = write_json_line(&mut writer, &ClientError::new(&e));
            return;
        }
        Err(RecvTimeoutError::Timeout) => {
            let _ = write_json_line(&mut writer, &ClientError::new("timeout waiting for worker response"));
            return;
        }
        Err(RecvTimeoutError::Disconnected) => {
            let _ = write_json_line(&mut writer, &ClientError::new("worker disconnected"));
            return;
        }
    };

    let response = ClientResponse::new(
        worker_resp.formatted,
        worker_resp.puzzle,
        worker_resp.num_clues,
        worker_resp.geo_mean_guesses,
        worker_resp.loss,
    );
    let _ = write_json_line(&mut writer, &response);
}

/// Minimal unique-ish request ID (timestamp-based, no external dep).
fn uuid_v4() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let nanos = t.subsec_nanos();
    let secs = t.as_secs();
    format!("{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}", nanos, nanos >> 16, nanos & 0xfff, (nanos ^ 0x8000) & 0xffff, secs * 0x9e3779b9)
}

// ── worker ─────────────────────────────────────────────────────────────────

fn run_worker(cfg: WorkerConfig) {
    eprintln!("[worker:{}] starting, warmup={}", cfg.profile, cfg.warmup);

    // Connect to controller socket.
    let stream = UnixStream::connect(&cfg.socket_path).unwrap_or_else(|e| {
        eprintln!("[worker:{}] cannot connect to {}: {}", cfg.profile, cfg.socket_path, e);
        std::process::exit(1);
    });

    let read_stream = stream.try_clone().expect("clone stream");
    let mut reader = BufReader::new(read_stream);
    let mut writer = BufWriter::new(&stream);

    // Announce registration.
    write_json_line(&mut writer, &WorkerRegister::new(&cfg.profile))
        .expect("write WorkerRegister");

    // Set up graceful shutdown.
    let running = Arc::new(AtomicBool::new(true));
    {
        let running = Arc::clone(&running);
        ctrlc::set_handler(move || {
            running.store(false, Ordering::SeqCst);
        })
        .expect("Error setting Ctrl-C handler");
    }

    // Build generator options.
    let gen_opts = GeneratorOptions {
        clue_weight: cfg.gen.clue_weight,
        guess_weight: cfg.gen.guess_weight,
        random_weight: cfg.gen.random_weight,
        clues_to_drop: cfg.gen.clues_to_drop,
        num_evals: cfg.gen.num_evals,
        num_puzzles_in_pool: cfg.gen.num_puzzles_in_pool,
        do_minimize: cfg.gen.do_minimize,
        pencilmark: cfg.gen.pencilmark,
    };

    let mut generator = rdoku::generator::Generator::new(gen_opts, Arc::clone(&running));
    generator.init_empty();

    // Warmup phase: build pool diversity without outputting anything.
    eprintln!("[worker:{}] warming up ({} puzzles)...", cfg.profile, cfg.warmup);
    let warmup_count = cfg.warmup;
    let mut warmed = 0u64;
    generator.run_accepted(|_| {
        warmed += 1;
        warmed < warmup_count
    });

    if !running.load(Ordering::SeqCst) {
        eprintln!("[worker:{}] interrupted during warmup", cfg.profile);
        return;
    }

    eprintln!("[worker:{}] warmup complete, announcing ready", cfg.profile);
    write_json_line(&mut writer, &WorkerReady::new()).expect("write WorkerReady");

    // Request-response loop.
    loop {
        if !running.load(Ordering::SeqCst) {
            break;
        }

        // Read next request from controller.
        let request: serde_json::Value = match read_json_line(&mut reader) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[worker:{}] controller disconnected: {}", cfg.profile, e);
                break;
            }
        };

        let msg_type = request.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if msg_type == "WorkerShutdown" {
            eprintln!("[worker:{}] received shutdown", cfg.profile);
            break;
        }

        let req: WorkerRequest = match serde_json::from_value(request) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[worker:{}] bad request: {}", cfg.profile, e);
                continue;
            }
        };

        let opts = req.options.clone();

        // Generate one puzzle using the warmed-up pool.
        let mut result: Option<GeneratedPuzzle> = None;
        generator.run_accepted(|p| {
            result = Some(p);
            false // stop after first
        });

        let puzzle = match result {
            Some(p) => p,
            None => {
                eprintln!("[worker:{}] generator interrupted during request", cfg.profile);
                break;
            }
        };

        let formatted = format_output(&puzzle, &opts);

        let resp = WorkerResponse::new(
            &req.id,
            puzzle.puzzle.clone(),
            puzzle.num_clues,
            puzzle.geo_mean_guesses,
            puzzle.loss,
            formatted,
        );

        if let Err(e) = write_json_line(&mut writer, &resp) {
            eprintln!("[worker:{}] error writing response: {}", cfg.profile, e);
            break;
        }
    }

    eprintln!("[worker:{}] exiting", cfg.profile);
}

// ── usage ──────────────────────────────────────────────────────────────────

fn print_usage() {
    eprintln!("usage: persistent_generator [OPTIONS]");
    eprintln!();
    eprintln!("Start a pool of long-running Sudoku puzzle generator workers.");
    eprintln!("Workers warm up by building pool diversity, then serve puzzle requests");
    eprintln!("from pg_client via a Unix domain socket.");
    eprintln!();
    eprintln!("OPTIONS:");
    eprintln!("  --config <path>       Path to JSON config file.  Default: pg_config.json");
    eprintln!("  --socket <path>       Unix socket path.  Default: /tmp/rdoku-pg.sock");
    eprintln!("  --max-timeout <secs>  Maximum client wait time.  Default: 60");
    eprintln!("  -h, --help            Show this help.");
    eprintln!();
    eprintln!("CONFIG FILE FORMAT (JSON):");
    eprintln!("  {{\"profiles\": [{{\"name\":\"default\",\"count\":2,\"warmup\":10000,...}}]}}");
    eprintln!();
    eprintln!("See pg_config.example.json for a complete annotated example.");
}

// ── main ──────────────────────────────────────────────────────────────────

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // Check for internal worker mode.
    if let Some(pos) = args.iter().position(|a| a == "--internal-worker") {
        if let Some(json) = args.get(pos + 1) {
            let cfg: WorkerConfig = serde_json::from_str(json).unwrap_or_else(|e| {
                eprintln!("Error parsing worker config: {}", e);
                std::process::exit(1);
            });
            run_worker(cfg);
            return;
        }
    }

    // Controller mode.
    let mut config_path = "pg_config.json".to_string();
    let mut socket_path = "/tmp/rdoku-pg.sock".to_string();
    let mut max_timeout = 60.0f64;

    let mut i = 1usize;
    while i < args.len() {
        match args[i].as_str() {
            "--config" => {
                i += 1;
                config_path = args.get(i).cloned().unwrap_or_else(|| {
                    eprintln!("Error: --config requires a path.");
                    std::process::exit(1);
                });
            }
            "--socket" => {
                i += 1;
                socket_path = args.get(i).cloned().unwrap_or_else(|| {
                    eprintln!("Error: --socket requires a path.");
                    std::process::exit(1);
                });
            }
            "--max-timeout" => {
                i += 1;
                max_timeout = args
                    .get(i)
                    .and_then(|v| v.parse::<f64>().ok())
                    .unwrap_or_else(|| {
                        eprintln!("Error: --max-timeout requires a number.");
                        std::process::exit(1);
                    });
            }
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            other => {
                eprintln!("Unknown argument: {}", other);
                print_usage();
                std::process::exit(1);
            }
        }
        i += 1;
    }

    let config_text = std::fs::read_to_string(&config_path).unwrap_or_else(|e| {
        eprintln!("Error reading config file '{}': {}", config_path, e);
        std::process::exit(1);
    });

    let config: Config = serde_json::from_str(&config_text).unwrap_or_else(|e| {
        eprintln!("Error parsing config file '{}': {}", config_path, e);
        std::process::exit(1);
    });

    if config.profiles.is_empty() {
        eprintln!("Error: config file contains no profiles.");
        std::process::exit(1);
    }

    run_controller(&socket_path, config, max_timeout);
}
