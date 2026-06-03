# Run full test suite

Run all PandaFilter verification steps in order. Stop and report on first failure.
This is the required pre-commit gate.

## Steps

### Step 1 — Workspace unit + integration tests
```bash
cargo test --workspace 2>&1
```
**Pass:** zero failures (exit code 0).

### Step 2 — Handler benchmarks
```bash
cargo test -p panda handler_benchmarks -- --nocapture 2>&1
```
**Pass:** zero failures. Output should show savings > 0% for all known handlers.

### Step 3 — Fixture savings check (no API key needed)
```bash
cargo run -p panda-eval -- --savings-only 2>&1
```
**Pass:** exit code 0, recall ≥ 80% across fixtures.

### Step 4 — Repo benchmark (29 commands, 4 repos)
```bash
python3 ccr-eval/benchmarks/run_benchmark.py --after-only 2>&1
```
**Pass:** `Weighted savings` line in the output shows ≥ 40%.

## How to run

Execute each step above, capturing output. After each step:
- If exit code ≠ 0 → print the failing output and stop with "BLOCKED: Step N failed."
- If Step 4's weighted savings drops below 40% → stop with "BLOCKED: savings regression."

After all steps pass, print a summary table:

```
Step 1 — cargo test --workspace          PASS
Step 2 — handler_benchmarks              PASS
Step 3 — panda-eval --savings-only       PASS
Step 4 — repo benchmark                  PASS  (weighted savings: XX.X%)

All checks passed. Safe to commit.
```

## Notes
- Step 4 requires the benchmark repos to exist under `ccr-eval/benchmarks/repos/`.
  If repos are missing, skip Step 4 and warn the user.
- All steps should be run from the workspace root (`/Users/assafpetronio/Desktop/ccr`).
- Do not run with `--release`; debug builds are the pre-commit target.
