#!/usr/bin/env bash
# rdoku benchmark suite — runs criterion micro-benchmarks and the legacy
# tdoku-style benchmark binary.  Results are saved to benchmark-results/ with
# a timestamp.
set -euo pipefail

GREEN='\033[0;32m'
NC='\033[0m'

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$PROJECT_DIR"

TIMESTAMP="$(date +%Y%m%d_%H%M%S)"
OUTDIR="$PROJECT_DIR/benchmark-results/$TIMESTAMP"
mkdir -p "$OUTDIR"

echo "==> rdoku benchmarks"
echo "    results: $OUTDIR"
echo ""

# ── criterion benchmarks ──────────────────────────────────────────────────────
echo "── cargo bench (criterion)"
cargo bench 2>&1 | tee "$OUTDIR/criterion.txt"
echo ""

# ── legacy tdoku-style benchmark ───────────────────────────────────────────────
echo "── cargo run --release --bin benchmark (tdoku-style)"
cargo run --release --bin benchmark \
    -w 1 -t 5 -n 1000 \
    -s tdoku,_tdev_basic,tdoku_dpll_triad_scc_ih \
    < "$PROJECT_DIR/tests/test_puzzles" \
    2>&1 | tee "$OUTDIR/legacy_benchmark.txt"
echo ""

echo -e "${GREEN}==> Benchmarks complete — results in $OUTDIR${NC}"
