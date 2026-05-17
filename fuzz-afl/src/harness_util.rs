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
use std::process::{Command, Stdio};

// ── binary path resolution ────────────────────────────────────────────────

/// Directory containing the target binaries (`solve`, `generate`, …).
///
/// Set `RDOKU_BINARY_DIR` to override; defaults to the main workspace's
/// `target/debug` directory (embedded at compile time via `CARGO_MANIFEST_DIR`).
pub fn bin_dir() -> String {
    if let Ok(dir) = std::env::var("RDOKU_BINARY_DIR") {
        return dir;
    }
    // env!() is resolved at compile time, so this always works.
    format!("{}/../target/debug", env!("CARGO_MANIFEST_DIR"))
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
    pub timed_out: bool,
    pub signaled: bool,
}

// ── spawning & waiting ────────────────────────────────────────────────────

/// Returns the log file path if `RDOKU_AFL_LOG` is set, otherwise `None`.
///
/// This env var's **value** is the path to write the log to.  We avoid an
/// `AFL_` prefix so that AFL++ doesn't warn about a "mistyped" variable.
fn log_path() -> Option<String> {
    std::env::var("RDOKU_AFL_LOG").ok().filter(|v| !v.is_empty())
}

/// Log the command being run (binary + args + stdin preview + raw byte prefix).
///
/// Appends one line per invocation to the file named in `RDOKU_AFL_LOG`.
/// No dedup — every execution is logged so you can see everything AFL++ tests.
/// The 8-char hex prefix distinguishes different fuzz inputs that decode to the
/// same command (lossy byte→puzzle mapping).
fn log_invocation(binary_path: &str, args: &[&str], stdin_str: &str, raw_data: &[u8]) {
    let Some(ref path) = log_path() else {
        return;
    };

    // 8-char hex prefix from first 4 bytes (or fewer).
    let hex_prefix: String = raw_data.iter().take(4).map(|b| format!("{b:02x}")).collect();

    let preview: String = stdin_str
        .chars()
        .take(120)
        .map(|c| if c.is_ascii_graphic() || c == ' ' { c } else { '·' })
        .collect();
    let more = if stdin_str.len() > 120 { "…" } else { "" };
    let line = format!(
        "[{hex_prefix:0>8}] {binary_path} {args}  ← stdin: \"{preview}{more}\"\n",
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
                timed_out: false,
                signaled: false,
            };
        }
    };

    // Write stdin, then drop to close the pipe.
    if let Some(ref mut stdin) = child.stdin {
        let _ = stdin.write_all(stdin_str.as_bytes());
    }

    // Wait with timeout via a helper thread (keeps deps minimal).
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = child.wait_with_output();
        let _ = tx.send(result);
    });

    match rx.recv_timeout(std::time::Duration::from_secs(timeout_secs)) {
        Ok(Ok(output)) => {
            let success = output.status.success();
            let code = output.status.code();
            BinaryResult {
                success,
                exit_code: code,
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                timed_out: false,
                // A signal exit has no exit code and is not "success".
                signaled: !success && code.is_none(),
            }
        }
        Ok(Err(e)) => BinaryResult {
            success: false,
            exit_code: Some(1),
            stdout: String::new(),
            stderr: format!("wait error: {}", e),
            timed_out: false,
            signaled: false,
        },
        Err(_) => BinaryResult {
            success: false,
            exit_code: None,
            stdout: String::new(),
            stderr: "timeout".to_string(),
            timed_out: true,
            signaled: false,
        },
    }
}

// ── crash detection ───────────────────────────────────────────────────────

/// Abort if the binary misbehaved: signal exit, hang, or non-UTF-8 output
/// (when text output is expected).
pub fn crash_if_bad(result: &BinaryResult, expect_text_stdout: bool, expect_text_stderr: bool) {
    if result.signaled {
        eprintln!(
            "AFL_HARNESS: binary exited via signal (exit_code={:?})",
            result.exit_code
        );
        eprintln!("  stderr: {}", &result.stderr[..result.stderr.len().min(500)]);
        std::process::abort();
    }

    if result.timed_out {
        eprintln!("AFL_HARNESS: binary timed out");
        std::process::abort();
    }

    if expect_text_stdout {
        if std::str::from_utf8(result.stdout.as_bytes()).is_err() {
            eprintln!("AFL_HARNESS: binary produced non-UTF-8 stdout");
            std::process::abort();
        }
    }
    if expect_text_stderr {
        if std::str::from_utf8(result.stderr.as_bytes()).is_err() {
            eprintln!("AFL_HARNESS: binary produced non-UTF-8 stderr");
            std::process::abort();
        }
    }
}

// ── byte → puzzle conversion ─────────────────────────────────────────────

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

// ── temp file helpers ─────────────────────────────────────────────────────

/// Write `content` to a temporary file and return its path.
///
/// Uses a per-process filename to avoid races between parallel AFL instances.
pub fn write_temp_file(content: &str) -> String {
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
    path
}

// ── selector helpers ──────────────────────────────────────────────────────

/// Return `data[0] % n` if data is non-empty, else 0.
pub fn pick_profile(data: &[u8], n: usize) -> usize {
    if data.is_empty() || n == 0 {
        0
    } else {
        (data[0] as usize) % n
    }
}

/// Return `&data[1..]` if data has at least 1 byte, else `&[]`.
pub fn payload(data: &[u8]) -> &[u8] {
    if data.len() > 1 {
        &data[1..]
    } else {
        &[]
    }
}
