# Persistent Generator Pool

> ⚠️ **This feature has not been tested at all yet.** The design is complete and
> the code compiles, but end-to-end runtime behaviour is entirely unverified. Treat
> everything here as aspirational documentation until a test pass is recorded.

A long-running pool of Sudoku puzzle-generator workers, fronted by a controller
process and served to clients over a Unix domain socket.

Workers spend their first `warmup` accepted puzzles building pool diversity
(the hill-climbing search gets better as the pool fills up). After warmup they
sit idle until a client request arrives, at which point the controller dispatches
the request to a free worker. The client blocks until a puzzle is ready or its
timeout expires.

## Architecture

```
pg_client  ──(Unix socket)──▶  persistent_generator (controller)
                                       │
                             ┌─────────┼─────────┐
                             ▼         ▼         ▼
                          worker    worker    worker   ...
                        (profile:  (profile:  (profile:
                         default)   default)    hard)
```

Each worker is a child process running `persistent_generator --internal-worker
<json>`.  Workers connect *back* to the controller socket after startup, register
their profile, run warmup, then enter a request-response loop.

The controller never queues more than one in-flight request per worker. If all
workers for a profile are busy, clients wait on a condition variable until one
becomes free (or they time out).

## Files

| File | Description |
|------|-------------|
| `src/bin/persistent_generator.rs` | Controller + worker (single binary, two modes) |
| `src/bin/pg_client.rs` | Client binary |
| `src/pg_protocol.rs` | Newline-JSON wire protocol types |
| `src/generator.rs` | Reusable generator library (used by both `generate` and the worker) |
| `pg_config.example.json` | Example config with `default` and `hard` profiles |

## Quick Start

### 1. Build

```sh
cargo build --release
```

### 2. Create a config file

Copy and edit the example:

```sh
cp pg_config.example.json pg_config.json
# edit profile counts, warmup, weights, etc.
```

Or use the example directly for a first test.

### 3. Start the controller

```sh
# via just:
just pg-start

# or directly:
./target/release/persistent_generator --config pg_config.example.json
```

The controller prints a line for each worker it spawns and for each warmup
completion. With the default warmup of 10 000 puzzles per worker this takes
a minute or two.

### 4. Request puzzles

```sh
# via just:
just pg-client

# or directly (request 1 puzzle from "default" profile):
./target/release/pg_client --profile default

# request 5 puzzles from the "hard" profile, with solution appended:
./target/release/pg_client --profile hard -l 5 -s

# with a shorter timeout:
./target/release/pg_client -t 10
```

## Config File Format

```json
{
  "profiles": [
    {
      "name":          "default",
      "count":         2,
      "warmup":        10000,
      "pool_size":     500,
      "clue_weight":   1.0,
      "guess_weight":  0.5,
      "random_weight": 1.0,
      "clues_to_drop": 3,
      "num_evals":     10,
      "pencilmark":    false
    }
  ]
}
```

| Field | Default | Description |
|-------|---------|-------------|
| `name` | — | Profile identifier used by clients (`--profile <name>`). |
| `count` | `2` | Number of worker processes to spawn for this profile. |
| `warmup` | `10000` | Accepted puzzles to discard during startup to build pool diversity. |
| `pool_size` | `500` | Hill-climbing pool size (higher = more diversity, slower). |
| `clue_weight` | `1.0` | Loss weight for clue count (higher = fewer clues preferred). |
| `guess_weight` | `0.5` | Loss weight for solver guesses (higher = harder puzzles preferred). |
| `random_weight` | `1.0` | Random jitter in loss function (higher = more variety). |
| `clues_to_drop` | `3` | Clues dropped per hill-climbing iteration. |
| `num_evals` | `10` | Candidate puzzles evaluated per iteration. |
| `pencilmark` | `false` | If `true`, generate pencilmark (729-char) puzzles instead of 81-char. |

All fields except `name` are optional and fall back to the defaults shown.

## Client Options

```
pg_client [OPTIONS]

  --socket <path>    Unix socket path.  Default: /tmp/rdoku-pg.sock
  --profile <name>   Difficulty profile.  Default: default
  -t <secs>          Timeout in seconds.  Default: 30
  -l <count>         Number of puzzles to request.  Default: 1
  --json             Output puzzles as JSON objects
  -s, --solution     Append the unique solution
  --pretty           Include an ASCII art grid
  --pencilmark       Use pencilmark (729-char) puzzle format
  -h, --help         Show this help
```

## Controller Options

```
persistent_generator [OPTIONS]

  --config <path>       Path to JSON config file.  Default: pg_config.json
  --socket <path>       Unix socket path.  Default: /tmp/rdoku-pg.sock
  --max-timeout <secs>  Maximum client wait time.  Default: 60
  -h, --help            Show this help
```

## Wire Protocol

All messages are newline-delimited JSON (one object per line). Each connection
is identified by the first message's `"type"` field:

| Type | Direction | Description |
|------|-----------|-------------|
| `WorkerRegister` | worker → controller | Announces profile name at connect time. |
| `WorkerReady` | worker → controller | Warmup complete; worker is now idle. |
| `WorkerRequest` | controller → worker | Dispatches a client request (request id + output options). |
| `WorkerResponse` | worker → controller | Returns the generated puzzle and formatted output. |
| `WorkerShutdown` | controller → worker | Requests clean exit. |
| `ClientRequest` | client → controller | Requests a puzzle (profile, output options, timeout). |
| `ClientResponse` | controller → client | Returns formatted puzzle output on success. |
| `ClientError` | controller → client | Returns an error message (timeout, unknown profile, etc.). |

## Difficulty Tuning

Profiles map directly onto the generator's loss function:

```
loss = num_clues × clue_weight
     − exp(geo_mean_guesses × guess_weight)
     + rand × random_weight
```

Lower loss = better. Adjust weights to bias toward the puzzles you want:

| Goal | Suggested weights |
|------|-------------------|
| Fewer clues (minimal puzzles) | `clue_weight=2.0, guess_weight=0.0` |
| Harder (more solver guesses) | `clue_weight=0.0, guess_weight=2.0` |
| Mixed difficulty | `clue_weight=1.0, guess_weight=0.5` (default) |

See [generate -h](src/bin/generate.rs) for the full explanation.

## Relationship to `generate`

The standalone `generate` binary is unchanged and has no dependency on the pool
system. The pool workers call the same `rdoku::generator::Generator` library
internally, so both tools produce statistically equivalent puzzles.
