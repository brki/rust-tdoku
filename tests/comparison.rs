//! Phase 13: Docker-based C++/Rust trace comparison tests.
//!
//! Builds `tdoku-debug` (C++) and `rdoku-debug` (Rust) Docker images using the
//! Dockerfiles in `debug/`, runs both on the same puzzle inputs, and asserts that
//! the `DT:` trace lines and result strings are byte-for-byte identical.
//!
//! Artifacts are written to `debug/artifacts/` (gitignored) and are kept after
//! the test run for manual inspection. The artifacts directory is cleared at the
//! start of each run.
//!
//! # Running
//! ```sh
//! cargo test --test comparison -- --nocapture
//! ```
//!
//! Requires Docker with amd64 support (e.g. colima on macOS). The test is
//! silently skipped when Docker is unavailable.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

// ---------------------------------------------------------------------------
// Directory helpers
// ---------------------------------------------------------------------------

fn repo_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn debug_dir() -> PathBuf {
    repo_dir().join("debug")
}

fn artifacts_dir() -> PathBuf {
    debug_dir().join("artifacts")
}

// ---------------------------------------------------------------------------
// Docker helpers
// ---------------------------------------------------------------------------

/// Returns `true` if Docker is available and responsive.
fn docker_available() -> bool {
    Command::new("docker")
        .args(["info", "--format", "{{.ServerVersion}}"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Build both debug images from the Dockerfiles in `debug/`.
/// Panics with a descriptive message if either build fails.
fn build_images() {
    let debug = debug_dir();
    let tdoku = repo_dir().join("tdoku");

    // tdoku-debug: C++ image; build context = tdoku/ submodule
    let out = Command::new("docker")
        .args([
            "build",
            "--platform", "linux/amd64",
            "-f", debug.join("Dockerfile.tdoku").to_str().unwrap(),
            "-t", "tdoku-debug",
            tdoku.to_str().unwrap(),
        ])
        .output()
        .expect("failed to run docker build for tdoku-debug");
    if !out.status.success() {
        panic!(
            "docker build tdoku-debug failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    // rdoku-debug: Rust image; build context = repo root
    let out = Command::new("docker")
        .args([
            "build",
            "--platform", "linux/amd64",
            "-f", debug.join("Dockerfile.rdoku").to_str().unwrap(),
            "-t", "rdoku-debug",
            repo_dir().to_str().unwrap(),
        ])
        .output()
        .expect("failed to run docker build for rdoku-debug");
    if !out.status.success() {
        panic!(
            "docker build rdoku-debug failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

/// Run a container and return `(stdout, stderr)`.
fn run_tdoku(puzzle: &str, limit: u32) -> (String, String) {
    let tdoku = repo_dir().join("tdoku");
    let out = Command::new("docker")
        .args([
            "run", "--rm", "--platform", "linux/amd64",
            "-v", &format!("{}:/tdoku", tdoku.display()),
            "-v", "tdoku-build:/build",
            "tdoku-debug", puzzle, &limit.to_string(),
        ])
        .output()
        .expect("failed to run tdoku-debug container");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn run_rdoku(puzzle: &str, limit: u32) -> (String, String) {
    let repo = repo_dir();
    let out = Command::new("docker")
        .args([
            "run", "--rm", "--platform", "linux/amd64",
            "-v", &format!("{}:/rdoku", repo.display()),
            "-v", "rdoku-target:/rdoku/target",
            "-v", "cargo-registry:/usr/local/cargo/registry",
            "rdoku-debug", puzzle, &limit.to_string(),
        ])
        .output()
        .expect("failed to run rdoku-debug container");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

// ---------------------------------------------------------------------------
// Artifact helpers
// ---------------------------------------------------------------------------

fn save_artifact(name: &str, content: &str) {
    let path = artifacts_dir().join(name);
    fs::write(&path, content)
        .unwrap_or_else(|e| eprintln!("warning: could not write artifact {name}: {e}"));
}

fn reset_artifacts() {
    let dir = artifacts_dir();
    if dir.exists() {
        fs::remove_dir_all(&dir)
            .unwrap_or_else(|e| panic!("failed to clear artifacts dir: {e}"));
    }
    fs::create_dir_all(&dir)
        .unwrap_or_else(|e| panic!("failed to create artifacts dir: {e}"));
}

// ---------------------------------------------------------------------------
// Trace helpers
// ---------------------------------------------------------------------------

/// Keep only lines that start with `DT:`.
fn filter_dt_lines(trace: &str) -> String {
    trace
        .lines()
        .filter(|l| l.starts_with("DT:"))
        .map(|l| format!("{l}\n"))
        .collect()
}

/// Produce a simple unified-style diff of two strings (line-by-line).
/// Returns an empty string when the inputs are identical.
fn diff_lines(label_a: &str, a: &str, label_b: &str, b: &str) -> String {
    if a == b {
        return String::new();
    }
    let lines_a: Vec<&str> = a.lines().collect();
    let lines_b: Vec<&str> = b.lines().collect();
    let mut out = format!("--- {label_a}\n+++ {label_b}\n");
    let max = lines_a.len().max(lines_b.len());
    for i in 0..max {
        match (lines_a.get(i), lines_b.get(i)) {
            (Some(la), Some(lb)) if la == lb => out.push_str(&format!(" {la}\n")),
            (Some(la), Some(lb)) => {
                out.push_str(&format!("-{la}\n"));
                out.push_str(&format!("+{lb}\n"));
            }
            (Some(la), None) => out.push_str(&format!("-{la}\n")),
            (None, Some(lb)) => out.push_str(&format!("+{lb}\n")),
            (None, None) => {}
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Test cases
// ---------------------------------------------------------------------------

struct Case {
    name: &'static str,
    puzzle: &'static str,
    limit: u32,
}

const CASES: &[Case] = &[
    Case {
        name: "medium",
        puzzle: ".5..83.17...1..4..3.4..56.8....3...9.9.8245....6....7...9....5...729..861.36.72.4",
        limit: 2,
    },
    Case {
        name: "al_escargot",
        puzzle: "800000000003600000070090200060005030040100060020000080000070010500200300000000000",
        limit: 2,
    },
    Case {
        name: "multi_solution",
        puzzle: ".................................................................................",
        limit: 2,
    },
    Case {
        name: "unsolvable",
        // Two '1's in the same row — no solution
        puzzle: "11...............................................................................",
        limit: 2,
    },
];

// ---------------------------------------------------------------------------
// Main test
// ---------------------------------------------------------------------------

/// Phase 13: run all comparison cases and assert DT: traces + results match.
///
/// A single #[test] function is used to:
///   - avoid parallel runs writing to the same artifacts directory, and
///   - ensure reset_artifacts() runs exactly once per test invocation.
///
/// Docker image builds are performed once before all cases.
#[test]
fn test_phase13_comparison() {
    if !docker_available() {
        eprintln!("skipping test_phase13_comparison: Docker is not available");
        return;
    }

    reset_artifacts();

    println!("Building debug images...");
    build_images();
    println!("Images built.");

    let mut all_passed = true;

    for case in CASES {
        println!("\n--- Case: {} (limit={}) ---", case.name, case.limit);

        // Run both containers
        let (tdoku_stdout, tdoku_stderr) = run_tdoku(case.puzzle, case.limit);
        let (rdoku_stdout, rdoku_stderr) = run_rdoku(case.puzzle, case.limit);

        // Filter DT: trace lines
        let dt_tdoku = filter_dt_lines(&tdoku_stderr);
        let dt_rdoku = filter_dt_lines(&rdoku_stderr);

        // Produce diff
        let trace_diff = diff_lines(
            &format!("dt_tdoku_{}", case.name),
            &dt_tdoku,
            &format!("dt_rdoku_{}", case.name),
            &dt_rdoku,
        );

        // Save artifacts
        save_artifact(&format!("trace_tdoku_{}.txt", case.name), &tdoku_stderr);
        save_artifact(&format!("trace_rdoku_{}.txt", case.name), &rdoku_stderr);
        save_artifact(&format!("result_tdoku_{}.txt", case.name), &tdoku_stdout);
        save_artifact(&format!("result_rdoku_{}.txt", case.name), &rdoku_stdout);
        save_artifact(&format!("dt_tdoku_{}.txt", case.name), &dt_tdoku);
        save_artifact(&format!("dt_rdoku_{}.txt", case.name), &dt_rdoku);
        save_artifact(&format!("trace_diff_{}.txt", case.name), &trace_diff);

        // Report
        println!(
            "  tdoku DT events: {}",
            dt_tdoku.lines().count()
        );
        println!(
            "  rdoku DT events: {}",
            dt_rdoku.lines().count()
        );
        println!("  tdoku result: {}", tdoku_stdout.trim());
        println!("  rdoku result: {}", rdoku_stdout.trim());

        // Assert DT: traces are identical
        if dt_tdoku != dt_rdoku {
            eprintln!("FAIL [{}]: DT traces differ", case.name);
            eprintln!("First 20 diff lines:\n{}", trace_diff.lines().take(20).collect::<Vec<_>>().join("\n"));
            all_passed = false;
        } else {
            println!("  DT traces: IDENTICAL");
        }

        // Assert result lines are identical (normalize trailing whitespace)
        let tdoku_result = tdoku_stdout.trim();
        let rdoku_result = rdoku_stdout.trim();
        if tdoku_result != rdoku_result {
            eprintln!(
                "FAIL [{}]: results differ\n  tdoku: {tdoku_result}\n  rdoku: {rdoku_result}",
                case.name
            );
            all_passed = false;
        } else {
            println!("  Results: IDENTICAL");
        }
    }

    println!("\nArtifacts saved to: {}", artifacts_dir().display());
    assert!(all_passed, "One or more comparison cases failed; see output above and artifacts in debug/artifacts/");
}
