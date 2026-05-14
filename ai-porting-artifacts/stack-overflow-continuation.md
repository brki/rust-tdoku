Project: rdoku — Rust port of the tdoku C++ Sudoku solver
Workspace: tdoku-to-rust/  (tdoku/ = C++ reference, rdoku/ = Rust port)
Active branch: debug/simd-compare  (in rdoku/)
Docker context: colima-amd64 (default — linux/amd64)

Before doing anything, read these two files for full context:
  prompts/plan-generator-stack-overflow-debug.md
  prompts/generator-stack-overflow-debug-log.md

Task: Continue debugging the stack overflow in rdoku's SIMD solver.

The debugging infrastructure is fully set up: Docker images with volume-mount
caching, DT: trace instrumentation in both C++ and Rust, and a comparison script.

Start by rebuilding both images (fast — no compilation in the Dockerfile):

  cd tdoku-to-rust
  docker build --platform linux/amd64 -f rdoku/debug/Dockerfile.tdoku -t tdoku-debug tdoku/
  docker build --platform linux/amd64 -f rdoku/debug/Dockerfile.rdoku -t rdoku-debug rdoku/

Then run a comparison on a puzzle that exercises DPLL branching:

  cd rdoku
  ./debug/compare.sh "8..........3.6.....7..9.2...6..5...3...4..6..5..7...9...3..4..5.5...3...1...8.67.." 2

Find the first DT: line that differs between C++ and Rust, identify the root
cause, and fix it. Follow the step-by-step plan in the prompt file above.
