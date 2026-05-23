//! Wire protocol for the persistent generator pool system.
//!
//! All messages are newline-delimited JSON sent over a Unix domain socket.
//! The first message on any new connection identifies the peer type:
//! - [`WorkerRegister`] → this is a worker process connecting to the controller.
//! - [`ClientRequest`] → this is a client requesting a puzzle.
//!
//! ## Worker lifecycle
//! 1. Worker connects, sends [`WorkerRegister`].
//! 2. Worker completes warmup, sends [`WorkerReady`].
//! 3. Controller sends [`WorkerRequest`] when a client arrives.
//! 4. Worker sends [`WorkerResponse`] with the generated puzzle.
//! 5. Repeat from 3.
//! 6. On shutdown, controller sends [`WorkerShutdown`]; worker exits cleanly.
//!
//! ## Client lifecycle
//! 1. Client connects, sends [`ClientRequest`].
//! 2. Controller waits for an idle worker for the requested profile.
//! 3. Controller replies with [`ClientResponse`] (success) or [`ClientError`] (timeout).

use serde::{Deserialize, Serialize};

// ── per-request output options ─────────────────────────────────────────────

/// Output format requested by the client.
///
/// These affect how the puzzle is rendered in the response — they do not change
/// the generation algorithm or pool state.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PuzzleOptions {
    /// `true` = pencilmark (729-char) output; `false` = vanilla (81-char).
    pub pencilmark: bool,
    /// Return a JSON object instead of a plain puzzle string.
    pub json: bool,
    /// Append the unique solution to the output.
    pub solution: bool,
    /// Include an ASCII art rendering.
    pub pretty: bool,
}

// ── worker ↔ controller messages ──────────────────────────────────────────

/// Sent by a worker to the controller immediately after connecting, before warmup.
#[derive(Debug, Serialize, Deserialize)]
pub struct WorkerRegister {
    #[serde(rename = "type")]
    pub msg_type: String, // always "WorkerRegister"
    /// The profile name this worker serves (e.g. `"default"`, `"hard"`).
    pub profile: String,
}

impl WorkerRegister {
    pub fn new(profile: &str) -> Self {
        Self {
            msg_type: "WorkerRegister".into(),
            profile: profile.to_string(),
        }
    }
}

/// Sent by a worker after warmup completes — signals it is ready to serve requests.
#[derive(Debug, Serialize, Deserialize)]
pub struct WorkerReady {
    #[serde(rename = "type")]
    pub msg_type: String, // always "WorkerReady"
}

impl WorkerReady {
    pub fn new() -> Self {
        Self { msg_type: "WorkerReady".into() }
    }
}

impl Default for WorkerReady {
    fn default() -> Self { Self::new() }
}

/// Sent by the controller to a worker when a client request arrives.
#[derive(Debug, Serialize, Deserialize)]
pub struct WorkerRequest {
    #[serde(rename = "type")]
    pub msg_type: String, // always "WorkerRequest"
    /// Opaque request ID echoed back in the response.
    pub id: String,
    /// Output format options for this specific request.
    pub options: PuzzleOptions,
}

impl WorkerRequest {
    pub fn new(id: &str, options: PuzzleOptions) -> Self {
        Self {
            msg_type: "WorkerRequest".into(),
            id: id.to_string(),
            options,
        }
    }
}

/// Sent by a worker to the controller with the generated puzzle.
#[derive(Debug, Serialize, Deserialize)]
pub struct WorkerResponse {
    #[serde(rename = "type")]
    pub msg_type: String, // always "WorkerResponse"
    /// Echoed request ID.
    pub id: String,
    /// The raw puzzle string (81-char vanilla or 729-char pencilmark).
    pub puzzle: String,
    /// Number of clues.
    pub num_clues: usize,
    /// Geometric mean of solver guesses.
    pub geo_mean_guesses: f64,
    /// Loss score.
    pub loss: f64,
    /// Fully formatted output string (ready for the client to print).
    pub formatted: String,
}

impl WorkerResponse {
    pub fn new(
        id: &str,
        puzzle: String,
        num_clues: usize,
        geo_mean_guesses: f64,
        loss: f64,
        formatted: String,
    ) -> Self {
        Self {
            msg_type: "WorkerResponse".into(),
            id: id.to_string(),
            puzzle,
            num_clues,
            geo_mean_guesses,
            loss,
            formatted,
        }
    }
}

/// Sent by the controller to a worker requesting a clean exit.
#[derive(Debug, Serialize, Deserialize)]
pub struct WorkerShutdown {
    #[serde(rename = "type")]
    pub msg_type: String, // always "WorkerShutdown"
}

impl WorkerShutdown {
    pub fn new() -> Self {
        Self { msg_type: "WorkerShutdown".into() }
    }
}

impl Default for WorkerShutdown {
    fn default() -> Self { Self::new() }
}

// ── client ↔ controller messages ──────────────────────────────────────────

/// Sent by a client to the controller to request a puzzle.
#[derive(Debug, Serialize, Deserialize)]
pub struct ClientRequest {
    #[serde(rename = "type")]
    pub msg_type: String, // always "ClientRequest"
    /// Worker profile to request from.
    pub profile: String,
    /// Output format options.
    pub options: PuzzleOptions,
    /// Maximum seconds to wait for an available worker.
    pub timeout_secs: f64,
}

impl ClientRequest {
    pub fn new(profile: &str, options: PuzzleOptions, timeout_secs: f64) -> Self {
        Self {
            msg_type: "ClientRequest".into(),
            profile: profile.to_string(),
            options,
            timeout_secs,
        }
    }
}

/// Sent by the controller to a client when a puzzle is successfully generated.
#[derive(Debug, Serialize, Deserialize)]
pub struct ClientResponse {
    #[serde(rename = "type")]
    pub msg_type: String, // always "ClientResponse"
    /// Fully formatted puzzle output (plain text or JSON string).
    pub formatted: String,
    /// Raw puzzle string, always present for programmatic use.
    pub puzzle: String,
    /// Number of clues.
    pub num_clues: usize,
    /// Geometric mean guesses.
    pub geo_mean_guesses: f64,
    /// Loss score.
    pub loss: f64,
}

impl ClientResponse {
    pub fn new(
        formatted: String,
        puzzle: String,
        num_clues: usize,
        geo_mean_guesses: f64,
        loss: f64,
    ) -> Self {
        Self {
            msg_type: "ClientResponse".into(),
            formatted,
            puzzle,
            num_clues,
            geo_mean_guesses,
            loss,
        }
    }
}

/// Sent by the controller to a client when the request cannot be fulfilled.
#[derive(Debug, Serialize, Deserialize)]
pub struct ClientError {
    #[serde(rename = "type")]
    pub msg_type: String, // always "ClientError"
    pub message: String,
}

impl ClientError {
    pub fn new(message: &str) -> Self {
        Self {
            msg_type: "ClientError".into(),
            message: message.to_string(),
        }
    }
}

// ── framing helpers ────────────────────────────────────────────────────────

/// Write a serializable message as a JSON line to the given writer.
///
/// The line is terminated with `\n` and the write is flushed.
pub fn write_json_line<W: std::io::Write, T: Serialize>(
    writer: &mut W,
    msg: &T,
) -> std::io::Result<()> {
    let mut line = serde_json::to_string(msg)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    line.push('\n');
    writer.write_all(line.as_bytes())?;
    writer.flush()
}

/// Read one JSON line from a buffered reader and deserialize it.
pub fn read_json_line<R: std::io::BufRead, T: for<'de> Deserialize<'de>>(
    reader: &mut R,
) -> std::io::Result<T> {
    let mut line = String::new();
    let n = reader.read_line(&mut line)?;
    if n == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "connection closed",
        ));
    }
    serde_json::from_str(line.trim_end())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}
