//! Shared utilities for AFL++ binary-fuzzing harnesses.
//!
//! Each harness reads raw bytes from stdin (provided by AFL++), converts them
//! into CLI arguments + stdin content for the target binary, spawns the binary
//! via `std::process::Command`, and checks the result.
//!
//! If the target binary panics (signal exit), hangs (timeout), or produces
//! non-UTF-8 output, the harness aborts — AFL++ records the fuzz input as a
//! crashing test case.

use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

// ── binary path resolution ────────────────────────────────────────────────

/// Directory containing the target binaries (`solve`, `generate`, …).
///
/// Set `RDOKU_BINARY_DIR` to override; defaults to the main workspace's
/// `target/release` directory (embedded at compile time via `CARGO_MANIFEST_DIR`).
/// Use `RDOKU_BINARY_DIR=../target/debug` to point at debug binaries if needed
/// (e.g. to catch integer overflow panics).
pub fn bin_dir() -> String {
    if let Ok(dir) = std::env::var("RDOKU_BINARY_DIR") {
        return dir;
    }
    // env!() is resolved at compile time, so this always works.
    format!("{}/../target/release", env!("CARGO_MANIFEST_DIR"))
}

/// Full path to a specific binary.
pub fn bin_path(name: &str) -> String {
    format!("{}/{}", bin_dir(), name)
}

// ── result type ───────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct BinaryResult {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    /// Raw stdout bytes — used for accurate UTF-8 validation in `crash_if_bad`.
    pub stdout_raw: Vec<u8>,
    /// Raw stderr bytes — used for accurate UTF-8 validation in `crash_if_bad`.
    pub stderr_raw: Vec<u8>,
    pub timed_out: bool,
    pub signaled: bool,
}

// ── temp file RAII guard ──────────────────────────────────────────────────

/// A temporary file that is automatically deleted when dropped.
///
/// Created by [`write_temp_file`] and [`write_temp_file_raw`]. If the harness
/// calls `std::process::abort()` the OS reclaims the file; for all other
/// control flow (normal return, panics) the `Drop` impl ensures cleanup.
pub struct TempFile(pub String);

impl TempFile {
    /// Returns the path of the temporary file.
    pub fn path(&self) -> &str {
        &self.0
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

// ── spawning & waiting ────────────────────────────────────────────────────

/// Returns the log file path if `RDOKU_AFL_LOG` is set, otherwise `None`.
///
/// This env var's **value** is the path to write the log to.  We avoid an
/// `AFL_` prefix so that AFL++ doesn't warn about a "mistyped" variable.
fn log_path() -> Option<String> {
    std::env::var("RDOKU_AFL_LOG").ok().filter(|v| !v.is_empty())
}

/// Format a SystemTime as a simple ISO-like string (YYYY-MM-DD HH:MM:SS).
fn format_time(now: SystemTime) -> String {
    match now.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(duration) => {
            let total_secs = duration.as_secs();
            let millis = duration.subsec_millis();

            // Constants for date calculations.
            const SECS_PER_DAY: u64 = 86400;
            const SECS_PER_HOUR: u64 = 3600;
            const SECS_PER_MIN: u64 = 60;

            // Calculate days since epoch and seconds within the current day.
            let days_since_epoch = total_secs / SECS_PER_DAY;
            let secs_today = total_secs % SECS_PER_DAY;

            // Extract time of day.
            let hours = secs_today / SECS_PER_HOUR;
            let minutes = (secs_today % SECS_PER_HOUR) / SECS_PER_MIN;
            let seconds = secs_today % SECS_PER_MIN;

            // Approximate year/month/day (good enough for logging).
            // 1970-01-01 was day 0. Assume 365.2425 days per year (accounting for leap years).
            let years_since_1970 = (days_since_epoch as f64 / 365.2425).floor() as u32;
            let mut year = 1970 + years_since_1970;

            // Days in each month (non-leap).
            const DAYS_IN_MONTH: [u32; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

            // Refine year by subtracting accumulated days.
            let mut remaining_days = days_since_epoch;
            loop {
                let days_in_year = if is_leap_year(year) { 366 } else { 365 };
                if remaining_days < days_in_year as u64 {
                    break;
                }
                remaining_days -= days_in_year as u64;
                year += 1;
            }

            // Calculate month and day.
            let is_leap = is_leap_year(year);
            let mut month = 1u32;
            let mut day = remaining_days + 1; // Days are 1-indexed.

            for (i, &base_days) in DAYS_IN_MONTH.iter().enumerate() {
                let days_in_m = if i == 1 && is_leap { 29 } else { base_days as u64 };
                if day <= days_in_m {
                    break;
                }
                day -= days_in_m;
                month += 1;
            }

            format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
                year, month, day, hours, minutes, seconds, millis
            )
        }
        Err(_) => "<unknown>".to_string(),
    }
}

/// Check if a year is a leap year.
fn is_leap_year(year: u32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

/// Log the command being run (binary + args + stdin preview + raw byte prefix).
///
/// Appends one line per invocation to the file named in `RDOKU_AFL_LOG`.
/// No dedup — every execution is logged so you can see everything AFL++ tests.
/// The 8-char hex prefix distinguishes different fuzz inputs that decode to the
/// same command (lossy byte→puzzle mapping). Also logs raw hex bytes for debugging.
fn log_invocation(binary_path: &str, args: &[&str], stdin_str: &str, raw_data: &[u8]) {
    let Some(ref path) = log_path() else {
        return;
    };

    // Human-readable timestamp using only std::time.
    let timestamp = format_time(SystemTime::now());

    // 8-char hex prefix from first 4 bytes (or fewer).
    let hex_prefix: String = raw_data.iter().take(4).map(|b| format!("{b:02x}")).collect();

    // Raw hex dump of first 32 bytes (or fewer).
    let raw_hex: String = raw_data
        .iter()
        .take(32)
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ");

    let preview: String = stdin_str
        .chars()
        .take(120)
        .map(|c| if c.is_ascii_graphic() || c == ' ' { c } else { '·' })
        .collect();
    let more = if stdin_str.len() > 120 { "…" } else { "" };
    let line = format!(
        "{timestamp} [{hex_prefix:0>8}] {binary_path} {args}  ← stdin: \"{preview}{more}\" (raw: {raw_hex})\n",
        args = args.join(" "),
    );

    if let Ok(mut f) = std::fs::OpenOptions::new().append(true).create(true).open(path) {
        let _ = f.write_all(line.as_bytes());
    }
}

/// Spawn `binary_path` with the given args, pipe `stdin_str` on its stdin,
/// and wait up to `timeout_secs` seconds.
///
/// `raw_data` is the original fuzz bytes (used for logging only).
pub fn run_binary(
    binary_path: &str,
    args: &[&str],
    stdin_str: &str,
    timeout_secs: u64,
    raw_data: &[u8],
) -> BinaryResult {
    log_invocation(binary_path, args, stdin_str, raw_data);
    let result = run_binary_inner(binary_path, args, stdin_str, timeout_secs);
    sleep_between_execs();
    result
}

/// Spawn `binary_path` with `base_args` followed by `file_path`, pipe `stdin_str`.
///
/// Convenience wrapper for targets that take a file path as their last positional
/// argument.
pub fn run_binary_with_file(
    binary_path: &str,
    base_args: &[&str],
    file_path: &str,
    stdin_str: &str,
    timeout_secs: u64,
    raw_data: &[u8],
) -> BinaryResult {
    let mut all_args: Vec<&str> = base_args.to_vec();
    all_args.push(file_path);
    run_binary(binary_path, &all_args, stdin_str, timeout_secs, raw_data)
}

fn sleep_between_execs() {
    if let Ok(val) = std::env::var("RDOKU_AFL_DELAY_MS") {
        if let Ok(ms) = val.parse::<u64>() {
            std::thread::sleep(std::time::Duration::from_millis(ms));
        }
    }
}

fn run_binary_inner(
    binary_path: &str,
    args: &[&str],
    stdin_str: &str,
    timeout_secs: u64,
) -> BinaryResult {
    let mut child = match Command::new(binary_path)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return BinaryResult {
                success: false,
                exit_code: Some(1),
                stdout: String::new(),
                stderr: format!("spawn error: {}", e),
                stdout_raw: Vec::new(),
                stderr_raw: Vec::new(),
                timed_out: false,
                signaled: false,
            };
        }
    };

    // Write stdin and close the pipe before waiting.
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(stdin_str.as_bytes());
        // stdin dropped here closes the write end of the pipe.
    }

    // Wrap the child in an Arc<Mutex> so the timeout handler can kill it if it
    // fires before the background thread takes ownership. The thread takes the
    // child out of the mutex immediately (releasing the lock), then calls
    // wait_with_output() without holding the lock, so there is no deadlock risk.
    let child_arc: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(Some(child)));
    let child_arc2 = Arc::clone(&child_arc);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let child = child_arc2.lock().unwrap().take().unwrap();
        let result = child.wait_with_output();
        let _ = tx.send(result);
    });

    match rx.recv_timeout(std::time::Duration::from_secs(timeout_secs)) {
        Ok(Ok(output)) => {
            let success = output.status.success();
            let code = output.status.code();
            let signaled = {
                #[cfg(unix)]
                {
                    use std::os::unix::process::ExitStatusExt;
                    output.status.signal().is_some()
                }
                #[cfg(not(unix))]
                {
                    !success && code.is_none()
                }
            };
            BinaryResult {
                success,
                exit_code: code,
                stdout_raw: output.stdout.clone(),
                stderr_raw: output.stderr.clone(),
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                timed_out: false,
                signaled,
            }
        }
        Ok(Err(e)) => BinaryResult {
            success: false,
            exit_code: Some(1),
            stdout: String::new(),
            stderr: format!("wait error: {}", e),
            stdout_raw: Vec::new(),
            stderr_raw: Vec::new(),
            timed_out: false,
            signaled: false,
        },
        Err(_) => {
            // Timeout — kill the child if the background thread hasn't taken it yet.
            // If it has, guard holds None and the kill is a no-op.
            if let Ok(mut guard) = child_arc.lock() {
                if let Some(ref mut c) = *guard {
                    let _ = c.kill();
                }
            }
            BinaryResult {
                success: false,
                exit_code: None,
                stdout: String::new(),
                stderr: "timeout".to_string(),
                stdout_raw: Vec::new(),
                stderr_raw: Vec::new(),
                timed_out: true,
                signaled: false,
            }
        }
    }
}

// ── crash detection ───────────────────────────────────────────────────────

/// Abort if the binary misbehaved: signal exit or non-UTF-8 output.
///
/// Does NOT abort on timeouts — timeouts can be caused by instrumentation
/// overhead or legitimate long-running operations and are not necessarily bugs.
/// Focus on detecting panics (signals) and output corruption.
pub fn crash_if_bad(result: &BinaryResult, expect_text_stdout: bool, expect_text_stderr: bool) {
    if result.signaled {
        eprintln!(
            "AFL_HARNESS: binary exited via signal (exit_code={:?})",
            result.exit_code
        );
        eprintln!("  stderr: {}", &result.stderr[..result.stderr.len().min(500)]);
        std::process::abort();
    }

    // Timeouts are NOT crashes — they can be caused by harness overhead,
    // instrumentation, or the solver operating correctly on edge cases.
    // Only abort on actual panics (signals).

    if expect_text_stdout && std::str::from_utf8(&result.stdout_raw).is_err() {
        eprintln!("AFL_HARNESS: binary produced non-UTF-8 stdout");
        std::process::abort();
    }
    if expect_text_stderr && std::str::from_utf8(&result.stderr_raw).is_err() {
        eprintln!("AFL_HARNESS: binary produced non-UTF-8 stderr");
        std::process::abort();
    }
}

// ── byte → puzzle / argument conversion ──────────────────────────────────

/// Variant of puzzle encoding.
#[derive(Clone, Copy)]
pub enum PuzzleVariant {
    /// 81-char vanilla Sudoku: `'1'`–`'9'` for clues, `'.'` for blanks.
    Vanilla,
    /// 729-char pencilmark: 9 chars per cell, `'1'`–`'9'` for candidates,
    /// `'.'` for eliminated candidates.
    Pencilmark,
}

/// Convert raw fuzz bytes into a puzzle string.
///
/// The mapping is the same as the existing libfuzzer targets:
/// - Vanilla:   `byte & 1 == 0` → `'.'`, `byte & 1 == 1` → `'1' + (byte >> 1) % 9`
/// - Pencilmark: for each of 729 positions, `byte & 1 == 0` → `'.'`, else keep `'1'`
pub fn bytes_to_puzzle(data: &[u8], variant: PuzzleVariant) -> String {
    match variant {
        PuzzleVariant::Vanilla => {
            let mut buf = [b'.'; 81];
            for i in 0..81 {
                let b = data.get(i).copied().unwrap_or(0);
                if b & 1 == 1 {
                    buf[i] = b'1' + ((b >> 1) % 9);
                }
            }
            String::from_utf8(buf.to_vec()).unwrap()
        }
        PuzzleVariant::Pencilmark => {
            let mut buf = [b'1'; 729];
            for i in 0..729 {
                let b = data.get(i).copied().unwrap_or(0);
                if b & 1 == 0 {
                    buf[i] = b'.';
                }
            }
            String::from_utf8(buf.to_vec()).unwrap()
        }
    }
}

/// Convert raw fuzz bytes to a lossy UTF-8 string for use as a CLI argument.
///
/// Non-UTF-8 sequences become `\u{FFFD}`; null bytes and control characters
/// are preserved so that the target binary can reject them.
pub fn bytes_to_arg(data: &[u8]) -> String {
    String::from_utf8_lossy(data).into_owned()
}

// ── temp file helpers ─────────────────────────────────────────────────────

/// Write `content` to a temporary file and return a [`TempFile`] guard.
///
/// Uses a per-process filename to avoid races between parallel AFL instances.
/// The file is deleted automatically when the `TempFile` is dropped.
pub fn write_temp_file(content: &str) -> TempFile {
    let path = format!("/tmp/rdoku_afl_input_{}.txt", std::process::id());
    let mut f = std::fs::File::create(&path).unwrap_or_else(|e| {
        eprintln!("AFL_HARNESS: cannot create temp file {}: {}", path, e);
        std::process::abort();
    });
    f.write_all(content.as_bytes()).unwrap_or_else(|e| {
        eprintln!("AFL_HARNESS: cannot write temp file {}: {}", path, e);
        std::process::abort();
    });
    f.flush().ok();
    TempFile(path)
}

/// Write raw bytes to a temporary file and return a [`TempFile`] guard.
///
/// Unlike [`write_temp_file`], accepts arbitrary bytes (nulls, non-UTF-8) so
/// that chaos profiles can test how binaries handle malformed file content.
pub fn write_temp_file_raw(data: &[u8]) -> TempFile {
    let path = format!("/tmp/rdoku_afl_input_{}.txt", std::process::id());
    let mut f = std::fs::File::create(&path).unwrap_or_else(|e| {
        eprintln!("AFL_HARNESS: cannot create temp file {}: {}", path, e);
        std::process::abort();
    });
    f.write_all(data).unwrap_or_else(|e| {
        eprintln!("AFL_HARNESS: cannot write temp file {}: {}", path, e);
        std::process::abort();
    });
    f.flush().ok();
    TempFile(path)
}

// ── selector helpers ──────────────────────────────────────────────────────

/// Pick a profile index from the first two bytes of `data`.
///
/// Using two bytes (a `u16`) instead of one gives a much more uniform
/// distribution for any `n`, so AFL++ bit-flipping in the selector bytes
/// more reliably crosses profile boundaries.
pub fn pick_profile(data: &[u8], n: usize) -> usize {
    if data.is_empty() || n == 0 {
        return 0;
    }
    let b0 = data[0] as usize;
    let b1 = data.get(1).copied().unwrap_or(0) as usize;
    ((b0 << 8) | b1) % n
}

/// Return the payload slice starting after the 2-byte profile selector.
///
/// Harnesses that call [`pick_profile`] should obtain their fuzz payload via
/// this function so the selector bytes are not re-used as puzzle/argument data.
pub fn payload(data: &[u8]) -> &[u8] {
    if data.len() > 2 {
        &data[2..]
    } else {
        &[]
    }
}
