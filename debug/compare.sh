#!/usr/bin/env bash
# compare.sh — build both debug containers and diff their DT: trace output.
#
# Usage (from any directory):
#   ./compare.sh [puzzle] [limit]
#
#   puzzle  81-char vanilla or 729-char pencilmark string
#           default: reference test puzzle
#   limit   max solutions to count (default: 2)
#
# Requirements: Docker with the colima/amd64 context active.
# The script captures stderr (DT: lines) from each container, strips the
# non-DT lines, and diffs the traces side-by-side.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(dirname "$SCRIPT_DIR")"
TDOKU_DIR="$REPO_DIR/tdoku"
ARTIFACTS_DIR="$SCRIPT_DIR/artifacts"

PUZZLE="${1:-.5..83.17...1..4..3.4..56.8....3...9.9.8245....6....7...9....5...729..861.36.72.4}"
LIMIT="${2:-2}"

mkdir -p "$ARTIFACTS_DIR"

echo "=== Building tdoku-debug ==="
docker build \
    --platform linux/amd64 \
    -f "$SCRIPT_DIR/Dockerfile.tdoku" \
    -t tdoku-debug \
    "$TDOKU_DIR"

echo ""
echo "=== Building rdoku-debug ==="
docker build \
    --platform linux/amd64 \
    -f "$SCRIPT_DIR/Dockerfile.rdoku" \
    -t rdoku-debug \
    "$REPO_DIR"

echo ""
echo "=== Running tdoku-debug (puzzle first $((${#PUZZLE} < 30 ? ${#PUZZLE} : 30)) chars: ${PUZZLE:0:30}...) ==="
docker run --rm --platform linux/amd64 \
    -v "$TDOKU_DIR:/tdoku" \
    -v tdoku-build:/build \
    tdoku-debug "$PUZZLE" "$LIMIT" \
    2>"$ARTIFACTS_DIR/trace_tdoku.txt" \
    | tee "$ARTIFACTS_DIR/result_tdoku.txt"

echo ""
echo "=== Running rdoku-debug ==="
docker run --rm --platform linux/amd64 \
    -v "$REPO_DIR:/rdoku" \
    -v rdoku-target:/rdoku/target \
    -v cargo-registry:/usr/local/cargo/registry \
    rdoku-debug "$PUZZLE" "$LIMIT" \
    2>"$ARTIFACTS_DIR/trace_rdoku.txt" \
    | tee "$ARTIFACTS_DIR/result_rdoku.txt"

echo ""
echo "=== Filtering DT: lines only ==="
grep '^DT:' "$ARTIFACTS_DIR/trace_tdoku.txt" > "$ARTIFACTS_DIR/dt_tdoku.txt" || true
grep '^DT:' "$ARTIFACTS_DIR/trace_rdoku.txt" > "$ARTIFACTS_DIR/dt_rdoku.txt" || true

echo "tdoku trace events: $(wc -l < "$ARTIFACTS_DIR/dt_tdoku.txt")"
echo "rdoku trace events: $(wc -l < "$ARTIFACTS_DIR/dt_rdoku.txt")"

echo ""
echo "=== Diff (first divergence) ==="
if diff -u "$ARTIFACTS_DIR/dt_tdoku.txt" "$ARTIFACTS_DIR/dt_rdoku.txt" > "$ARTIFACTS_DIR/trace_diff.txt" 2>&1; then
    echo "Traces are IDENTICAL — no divergence found at this depth."
else
    echo "Traces DIVERGE. First 40 diff lines:"
    head -40 "$ARTIFACTS_DIR/trace_diff.txt"
    echo ""
    echo "Full diff saved to: $ARTIFACTS_DIR/trace_diff.txt"
fi

echo ""
echo "=== Result comparison ==="
echo "tdoku: $(cat "$ARTIFACTS_DIR/result_tdoku.txt")"
echo "rdoku: $(cat "$ARTIFACTS_DIR/result_rdoku.txt")"
