//! Client for the persistent generator pool.
//!
//! Connects to a running `persistent_generator` controller over a Unix domain
//! socket and requests one or more puzzles.
//!
//! ```
//! pg_client [OPTIONS]
//! ```
//!
//! ## Options
//! * `--socket <path>`   — socket path (default `/tmp/rdoku-pg.sock`)
//! * `--profile <name>`  — difficulty profile (default `default`)
//! * `-t <secs>`         — timeout in seconds (default `30`)
//! * `-l <count>`        — number of puzzles to request (default `1`)
//! * `--json`            — output as JSON
//! * `-s` / `--solution` — append the solution
//! * `--pretty`          — include ASCII art grid
//! * `--pencilmark`      — use pencilmark (729-char) puzzle format
//! * `-h` / `--help`     — show help

use rdoku::pg_protocol::{read_json_line, write_json_line, ClientRequest, PuzzleOptions};
use std::io::{BufReader, BufWriter};
use std::os::unix::net::UnixStream;

fn print_usage() {
    eprintln!("usage: pg_client [OPTIONS]");
    eprintln!();
    eprintln!("Request puzzles from a running persistent_generator pool.");
    eprintln!();
    eprintln!("OPTIONS:");
    eprintln!("  --socket <path>    Unix socket path.  Default: /tmp/rdoku-pg.sock");
    eprintln!("  --profile <name>   Difficulty profile.  Default: default");
    eprintln!("  -t <secs>          Timeout in seconds.  Default: 30");
    eprintln!("  -l <count>         Number of puzzles to request.  Default: 1");
    eprintln!("  --json             Output puzzles as JSON objects");
    eprintln!("  -s, --solution     Include the unique solution");
    eprintln!("  --pretty           Include an ASCII art grid");
    eprintln!("  --pencilmark       Use pencilmark (729-char) puzzle format");
    eprintln!("  -h, --help         Show this help");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let mut socket_path = "/tmp/rdoku-pg.sock".to_string();
    let mut profile = "default".to_string();
    let mut timeout_secs = 30.0f64;
    let mut count = 1usize;
    let mut opts = PuzzleOptions::default();

    let mut i = 1usize;
    while i < args.len() {
        match args[i].as_str() {
            "--socket" => {
                i += 1;
                socket_path = args.get(i).cloned().unwrap_or_else(|| {
                    eprintln!("Error: --socket requires a path.");
                    std::process::exit(1);
                });
            }
            "--profile" => {
                i += 1;
                profile = args.get(i).cloned().unwrap_or_else(|| {
                    eprintln!("Error: --profile requires a name.");
                    std::process::exit(1);
                });
            }
            "-t" => {
                i += 1;
                timeout_secs = args
                    .get(i)
                    .and_then(|v| v.parse::<f64>().ok())
                    .unwrap_or_else(|| {
                        eprintln!("Error: -t requires a number.");
                        std::process::exit(1);
                    });
            }
            "-l" => {
                i += 1;
                count = args
                    .get(i)
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or_else(|| {
                        eprintln!("Error: -l requires a positive integer.");
                        std::process::exit(1);
                    });
            }
            "--json" => opts.json = true,
            "-s" | "--solution" => opts.solution = true,
            "--pretty" => opts.pretty = true,
            "--pencilmark" => opts.pencilmark = true,
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

    let mut exit_code = 0;

    for _ in 0..count {
        let stream = match UnixStream::connect(&socket_path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Error: cannot connect to {}: {}", socket_path, e);
                std::process::exit(1);
            }
        };

        let read_stream = match stream.try_clone() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Error: cannot clone stream: {}", e);
                std::process::exit(1);
            }
        };
        let mut reader = BufReader::new(read_stream);
        let mut writer = BufWriter::new(&stream);

        let request = ClientRequest::new(&profile, opts.clone(), timeout_secs);

        if let Err(e) = write_json_line(&mut writer, &request) {
            eprintln!("Error: failed to send request: {}", e);
            std::process::exit(1);
        }

        // Read response — could be ClientResponse or ClientError.
        let resp: serde_json::Value = match read_json_line(&mut reader) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("Error: failed to read response: {}", e);
                std::process::exit(1);
            }
        };

        let msg_type = resp.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match msg_type {
            "ClientResponse" => {
                if let Some(formatted) = resp.get("formatted").and_then(|v| v.as_str()) {
                    println!("{}", formatted);
                } else {
                    eprintln!("Error: malformed ClientResponse (no 'formatted' field)");
                    exit_code = 1;
                }
            }
            "ClientError" => {
                let msg = resp.get("message").and_then(|v| v.as_str()).unwrap_or("unknown error");
                eprintln!("Error: {}", msg);
                exit_code = 1;
            }
            _ => {
                eprintln!("Error: unexpected response type '{}'", msg_type);
                exit_code = 1;
            }
        }
    }

    std::process::exit(exit_code);
}
