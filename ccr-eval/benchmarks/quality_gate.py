#!/usr/bin/env python3
"""
PandaFilter Quality Gate

Runs a compression benchmark across 8 repos (~40 commands) and compares
results against a stored baseline. Fails with exit code 1 if hard gates
are violated.

Usage:
  # Normal CI run — compare vs baseline:
  python3 quality_gate.py [--panda target/debug/panda] [--ci]

  # Seed or update baseline (run once on a known-good build):
  python3 quality_gate.py --update-baseline [--panda target/release/panda]

Hard gates (EXIT 1):
  - Overall weighted savings drops > 2pp vs baseline
  - Any single command savings drops > 5pp vs baseline

Soft gate (warn only, never fails CI):
  - Any command latency increases > 200ms vs baseline
  (BERT daemon is absent in CI — absolute latency numbers are unreliable)

Label stability note:
  Command labels are the primary key for baseline matching.
  Renaming a label without also running --update-baseline will
  cause that command to appear as a new entry (no comparison made).
"""

import json
import subprocess
import sys
import time
import argparse
from datetime import datetime, timezone
from pathlib import Path
from typing import Optional

# ── Paths ─────────────────────────────────────────────────────────────────────

SCRIPT_DIR       = Path(__file__).parent
WORKSPACE        = SCRIPT_DIR.parent.parent
REPOS_DIR        = SCRIPT_DIR / "repos"
DEFAULT_REPORTS  = SCRIPT_DIR / "reports"
DEFAULT_BASELINE = SCRIPT_DIR / "baseline.json"
DEFAULT_PANDA    = WORKSPACE / "target" / "debug" / "panda"


# ── Command dataset ───────────────────────────────────────────────────────────
# Each entry: (repo, category, label, shell_command)
#   repo         — subdirectory of REPOS_DIR
#   category     — test | lint | deps | explore | build
#   label        — stable identifier used as the baseline key (never rename without --update-baseline)
#   shell_command— executed in REPOS_DIR/<repo>, stdout+stderr captured
#
# Design rules:
#   - All commands run without language tooling except npm (express only)
#   - find/grep always include head -N for deterministic output sizes
#   - New repos can be added freely; they are ignored until baseline is regenerated

COMMANDS = [
    # ── EXPRESS (Node.js) ─────────────────────────────────────────────────────
    ("express", "test",    "npm test",
     "npm test"),

    ("express", "lint",    "npm audit",
     "npm audit"),

    ("express", "deps",    "npm ls --depth=1",
     "npm ls --depth=1"),

    ("express", "deps",    "cat package.json",
     "cat package.json"),

    ("express", "explore", "find js files",
     "find . -name '*.js' -not -path '*/node_modules/*' | sort | head -60"),

    ("express", "explore", "grep function lib/",
     "grep -rn 'function' lib/ --include='*.js'"),

    ("express", "explore", "git log -20",
     "git log --format='%h %ad %s' --date=short -20"),

    ("express", "explore", "git show HEAD stat",
     "git show HEAD --stat"),

    # ── FLASK (Python) ────────────────────────────────────────────────────────
    ("flask", "explore", "grep def src/",
     "grep -rn 'def ' src/ --include='*.py'"),

    ("flask", "explore", "find py files",
     "find . -name '*.py' -not -path '*/__pycache__/*' | sort | head -80"),

    ("flask", "deps",    "cat pyproject.toml",
     "cat pyproject.toml"),

    ("flask", "explore", "git log -20",
     "git log --format='%h %ad %s' --date=short -20"),

    ("flask", "explore", "git show HEAD stat",
     "git show HEAD --stat"),

    # ── FASTAPI (Python) ──────────────────────────────────────────────────────
    ("fastapi", "explore", "grep def fastapi/",
     "grep -rn 'def ' fastapi/ --include='*.py'"),

    ("fastapi", "explore", "grep class fastapi/",
     "grep -rn 'class ' fastapi/ --include='*.py'"),

    ("fastapi", "explore", "find py files",
     "find . -name '*.py' -not -path '*/__pycache__/*' | sort | head -60"),

    ("fastapi", "deps",    "cat pyproject.toml",
     "cat pyproject.toml"),

    ("fastapi", "explore", "git log -20",
     "git log --format='%h %ad %s' --date=short -20"),

    # ── GIN (Go) ──────────────────────────────────────────────────────────────
    ("gin", "explore", "grep func *.go",
     "grep -rn 'func ' . --include='*.go' | grep -v '_test.go'"),

    ("gin", "explore", "grep func Test",
     "grep -rn 'func Test' . --include='*.go'"),

    ("gin", "deps",    "cat go.mod",
     "cat go.mod"),

    ("gin", "explore", "cat README.md",
     "cat README.md"),

    ("gin", "explore", "git log -20",
     "git log --format='%h %ad %s' --date=short -20"),

    ("gin", "explore", "git show HEAD stat",
     "git show HEAD --stat"),

    # ── RUST-ANALYZER (Rust) ─────────────────────────────────────────────────
    ("rust-analyzer", "explore", "find rs files",
     "find . -name '*.rs' -not -path '*/target/*' | sort | head -80"),

    ("rust-analyzer", "explore", "grep pub fn",
     "grep -rn '^pub fn ' . --include='*.rs' | grep -v '/target/' | head -60"),

    ("rust-analyzer", "deps",    "cat Cargo.toml",
     "cat Cargo.toml"),

    ("rust-analyzer", "explore", "git log -20",
     "git log --format='%h %ad %s' --date=short -20"),

    # ── RAILS (Ruby) ──────────────────────────────────────────────────────────
    ("rails", "explore", "find rb files",
     "find activerecord/lib -name '*.rb' | sort | head -60"),

    ("rails", "explore", "grep def activerecord/",
     "grep -rn '^  def \\|^    def ' activerecord/lib/ --include='*.rb' | head -50"),

    ("rails", "explore", "git log -20",
     "git log --format='%h %ad %s' --date=short -20"),

    ("rails", "explore", "git show HEAD stat",
     "git show HEAD --stat"),

    # ── VUE-CORE (TypeScript) ─────────────────────────────────────────────────
    ("vue-core", "explore", "find ts files",
     "find packages -name '*.ts' | grep -v 'node_modules\\|dist\\|__tests__' | sort | head -80"),

    ("vue-core", "explore", "grep export function",
     "grep -rn '^export function\\|^export const' packages/ --include='*.ts' 2>/dev/null | grep -v 'dist/' | head -50"),

    ("vue-core", "deps",    "cat package.json",
     "cat package.json"),

    ("vue-core", "explore", "git log -20",
     "git log --format='%h %ad %s' --date=short -20"),

    # ── SVELTE (TypeScript / Svelte) ──────────────────────────────────────────
    ("svelte", "explore", "find svelte files",
     "find packages -name '*.svelte' | sort | head -60"),

    ("svelte", "explore", "grep export function",
     "grep -rn '^export function\\|^export const' packages/svelte/src/ --include='*.js' 2>/dev/null | head -50"),

    ("svelte", "explore", "git log -20",
     "git log --format='%h %ad %s' --date=short -20"),

    ("svelte", "explore", "git show HEAD stat",
     "git show HEAD --stat"),
]


# ── Core benchmark helpers ────────────────────────────────────────────────────

def run_command(cmd: str, cwd: Path, timeout: int = 30) -> str:
    try:
        r = subprocess.run(
            cmd, shell=True, cwd=cwd,
            capture_output=True, text=True, timeout=timeout,
        )
        return (r.stdout + r.stderr).rstrip()
    except subprocess.TimeoutExpired:
        return f"[TIMEOUT after {timeout}s]"
    except Exception as e:
        return f"[ERROR: {e}]"


def run_panda_filter(panda_bin: Path, raw_output: str, cmd: str) -> tuple[str, float]:
    """Pipe raw_output through `panda filter --command <cmd>`. Returns (filtered, elapsed_ms)."""
    start = time.perf_counter()
    try:
        r = subprocess.run(
            [str(panda_bin), "filter", "--command", cmd],
            input=raw_output, capture_output=True, text=True, timeout=15,
        )
        elapsed_ms = (time.perf_counter() - start) * 1000
        return (r.stdout + r.stderr).rstrip(), elapsed_ms
    except subprocess.TimeoutExpired:
        elapsed_ms = (time.perf_counter() - start) * 1000
        return "[TIMEOUT]", elapsed_ms
    except Exception as e:
        elapsed_ms = (time.perf_counter() - start) * 1000
        return f"[ERROR: {e}]", elapsed_ms


def benchmark_one(
    panda_bin: Path, repo: str, category: str, label: str, cmd: str
) -> Optional[tuple]:
    """Run one command and return (repo, category, label, raw_chars, filtered_chars, savings_pct, elapsed_ms)."""
    repo_dir = REPOS_DIR / repo
    if not repo_dir.exists():
        return None

    raw = run_command(cmd, repo_dir)
    raw_chars = len(raw)
    if raw_chars == 0:
        return (repo, category, label, 0, 0, 0.0, 0.0)

    filtered, elapsed_ms = run_panda_filter(panda_bin, raw, cmd)
    filtered_chars = len(filtered)
    savings = (1.0 - filtered_chars / raw_chars) * 100.0
    return (repo, category, label, raw_chars, filtered_chars, savings, elapsed_ms)


def run_all(panda_bin: Path, verbose: bool = False) -> list[tuple]:
    print(f"\n  Running {len(COMMANDS)} commands across {len(set(c[0] for c in COMMANDS))} repos ...", flush=True)
    results = []
    for i, (repo, cat, lbl, cmd) in enumerate(COMMANDS, 1):
        r = benchmark_one(panda_bin, repo, cat, lbl, cmd)
        if r is None:
            print(f"  [{i:2d}/{len(COMMANDS)}] {repo:<16} — MISSING REPO, skipped", flush=True)
            continue
        results.append(r)
        if verbose:
            _, _, _, raw_c, filt_c, savings, elapsed = r
            bkt = size_bucket(raw_c)
            print(
                f"  [{i:2d}/{len(COMMANDS)}] {repo}/{lbl:<32} "
                f"{raw_c:>7,}c  {savings:>6.1f}%  {elapsed:>5.0f}ms  {bkt}",
                flush=True,
            )
        else:
            print(".", end="", flush=True)
    if not verbose:
        print()
    return results


# ── Metrics ───────────────────────────────────────────────────────────────────

def tokens(chars: int) -> float:
    return chars / 4.0


def weighted_savings(results: list[tuple]) -> float:
    total = sum(tokens(r[3]) for r in results)
    if total == 0:
        return 0.0
    return sum(tokens(r[3]) * r[5] for r in results) / total


def by_category(results: list[tuple]) -> dict:
    cats: dict[str, list] = {}
    for r in results:
        cats.setdefault(r[1], []).append(r)
    return {cat: weighted_savings(rs) for cat, rs in sorted(cats.items())}


def size_bucket(chars: int) -> str:
    if chars < 800:   return "<800"
    if chars < 2000:  return "800-2k"
    if chars < 5000:  return "2k-5k"
    return ">5k"


# ── Baseline I/O ──────────────────────────────────────────────────────────────

DEFAULT_THRESHOLDS = {
    "overall_regression_pct":     -2.0,
    "per_command_regression_pct": -5.0,
    "latency_warn_ms":           200.0,
}


def load_baseline(path: Path) -> Optional[dict]:
    """Return parsed baseline dict, or None if missing / has no command data (first-run mode)."""
    if not path.exists():
        return None
    try:
        with open(path) as f:
            data = json.load(f)
    except Exception as e:
        print(f"  Warning: could not parse baseline ({e}), running in first-run mode.", file=sys.stderr)
        return None
    if not data.get("commands"):
        return None
    return data


def save_baseline(path: Path, results: list[tuple], version: str, thresholds: dict) -> None:
    data = {
        "_note": (
            "PandaFilter Quality Gate baseline. "
            "Update with: python3 quality_gate.py --update-baseline [--panda target/release/panda]"
        ),
        "version":    version,
        "generated":  datetime.now(timezone.utc).isoformat(),
        "thresholds": thresholds,
        "commands": [
            {
                "repo":         r[0],
                "category":     r[1],
                "label":        r[2],
                "raw_chars":    r[3],
                "savings_pct":  round(r[5], 3),
                "elapsed_ms":   round(r[6], 1),
            }
            for r in results
        ],
        "aggregate": {
            "overall_weighted_savings_pct": round(weighted_savings(results), 3),
            "by_category": {k: round(v, 3) for k, v in by_category(results).items()},
        },
    }
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "w") as f:
        json.dump(data, f, indent=2)
    print(f"\n  Baseline written → {path}")


# ── Gate comparison ───────────────────────────────────────────────────────────

def compare(current: list[tuple], baseline: dict) -> dict:
    thresholds           = baseline.get("thresholds", DEFAULT_THRESHOLDS)
    overall_thresh       = thresholds["overall_regression_pct"]
    per_cmd_thresh       = thresholds["per_command_regression_pct"]
    latency_warn         = thresholds.get("latency_warn_ms", 200.0)
    latency_improve_ms   = 50.0   # noteworthy latency drop threshold

    by_key               = {(c["repo"], c["label"]): c for c in baseline["commands"]}
    hard_failures        = []
    soft_warnings        = []
    improvements         = []
    regressions          = []
    latency_improvements = []
    latency_regressions  = []
    paired_current       = []
    paired_both          = []   # (current_result, baseline_entry)

    for r in current:
        repo, cat, label, raw_c, filt_c, savings, elapsed = r
        b = by_key.get((repo, label))
        if b is None:
            continue  # new command — not in baseline, skip comparison

        paired_current.append(r)
        paired_both.append((r, b))
        delta     = savings - b["savings_pct"]
        lat_delta = elapsed - b.get("elapsed_ms", 0.0)

        if delta < per_cmd_thresh:
            hard_failures.append(
                f"{repo}/{label}: savings Δ={delta:+.1f}pp "
                f"(baseline {b['savings_pct']:.1f}% → now {savings:.1f}%)"
            )
            regressions.append((r, b, delta))
        elif delta >= 5.0:
            improvements.append((r, b, delta))

        if lat_delta > latency_warn:
            soft_warnings.append(
                f"{repo}/{label}: latency +{lat_delta:.0f}ms "
                f"(baseline {b['elapsed_ms']:.0f}ms → now {elapsed:.0f}ms)"
            )
            latency_regressions.append((r, b, lat_delta))
        elif lat_delta < -latency_improve_ms:
            latency_improvements.append((r, b, lat_delta))

    # Overall weighted savings check — intersection only, so new commands don't skew
    baseline_ws   = baseline["aggregate"]["overall_weighted_savings_pct"]
    current_ws    = weighted_savings(paired_current) if paired_current else 0.0
    overall_delta = current_ws - baseline_ws

    if paired_current and overall_delta < overall_thresh:
        hard_failures.insert(
            0,
            f"Overall weighted savings Δ={overall_delta:+.1f}pp "
            f"(baseline {baseline_ws:.1f}% → now {current_ws:.1f}%)",
        )

    # Latency aggregate stats across paired commands
    latency_stats: dict = {}
    if paired_both:
        now_ms = sorted(r[6] for r, _ in paired_both)
        bl_ms  = sorted(b.get("elapsed_ms", 0.0) for _, b in paired_both)
        n      = len(now_ms)
        latency_stats = {
            "median_now_ms":      now_ms[n // 2],
            "median_baseline_ms": bl_ms[n // 2],
            "median_delta_ms":    now_ms[n // 2] - bl_ms[n // 2],
            "p90_now_ms":         now_ms[int(n * 0.9)],
            "p90_baseline_ms":    bl_ms[int(n * 0.9)],
            "p90_delta_ms":       now_ms[int(n * 0.9)] - bl_ms[int(n * 0.9)],
            "total_now_ms":       sum(now_ms),
            "total_baseline_ms":  sum(bl_ms),
            "total_delta_ms":     sum(now_ms) - sum(bl_ms),
        }

    return {
        "passed":               len(hard_failures) == 0,
        "hard_failures":        hard_failures,
        "soft_warnings":        soft_warnings,
        "improvements":         improvements,
        "regressions":          regressions,
        "latency_improvements": latency_improvements,
        "latency_regressions":  latency_regressions,
        "latency_stats":        latency_stats,
        "overall_delta":        overall_delta,
        "current_ws":           current_ws,
        "baseline_ws":          baseline_ws,
        "n_paired":             len(paired_current),
        "n_new":                len(current) - len(paired_current),
    }


# ── Terminal output ───────────────────────────────────────────────────────────

def print_table(current: list[tuple], baseline_by_key: Optional[dict]) -> None:
    has_bl = baseline_by_key is not None
    if has_bl:
        header = (
            f"  {'Repo':<16} {'Cat':<8} {'Label':<32} {'Raw':>7} {'Sz':>6} | "
            f"{'Base%':>7} {'Now%':>7} {'Δ%':>7} | {'Base ms':>7} {'Now ms':>6} {'Δms':>7}"
        )
        sep = "─" * 120
    else:
        header = (
            f"  {'Repo':<16} {'Cat':<8} {'Label':<32} "
            f"{'Raw':>8} {'Sz':>6} {'Saved%':>8} {'ms':>6}"
        )
        sep = "─" * 92

    print("\n" + "═" * len(sep))
    print(header)
    print(sep)

    for r in current:
        repo, cat, label, raw_c, filt_c, savings, elapsed = r
        bucket = size_bucket(raw_c)

        if has_bl:
            b = baseline_by_key.get((repo, label))
            if b:
                delta     = savings - b["savings_pct"]
                lat_delta = elapsed - b.get("elapsed_ms", 0.0)
                flag = ""
                if delta <= -5.0:  flag = "  ◄ REGRESSION"
                elif delta >= 5.0: flag = "  ► improved"
                print(
                    f"  {repo:<16} {cat:<8} {label:<32} "
                    f"{raw_c:>7,} {bucket:>6} | "
                    f"{b['savings_pct']:>6.1f}% {savings:>6.1f}% {delta:>+6.1f}% | "
                    f"{b['elapsed_ms']:>6.0f}ms {elapsed:>5.0f}ms {lat_delta:>+6.0f}ms"
                    f"{flag}"
                )
            else:
                print(
                    f"  {repo:<16} {cat:<8} {label:<32} "
                    f"{raw_c:>7,} {bucket:>6} | "
                    f"{'—':>7} {savings:>6.1f}% {'(new)':>7} | "
                    f"{'—':>7} {elapsed:>5.0f}ms {'—':>7}"
                )
        else:
            print(
                f"  {repo:<16} {cat:<8} {label:<32} "
                f"{raw_c:>8,} {bucket:>6} {savings:>7.1f}% {elapsed:>5.0f}ms"
            )

    print(sep)


def print_aggregate(current: list[tuple], gate: Optional[dict]) -> None:
    ws           = weighted_savings(current)
    total_tokens = sum(tokens(r[3]) for r in current)
    saved_tokens = total_tokens * ws / 100.0
    cat_ws       = by_category(current)

    print(f"\n  {'Category':<12} {'Savings%':>9}  {'Cmds':>5}")
    print("  " + "─" * 30)
    for cat, pct in sorted(cat_ws.items()):
        n = sum(1 for r in current if r[1] == cat)
        print(f"  {cat:<12} {pct:>8.1f}%  {n:>5}")
    print("  " + "─" * 30)
    print(f"  {'OVERALL':<12} {ws:>8.1f}%  {len(current):>5}")
    print(f"\n  Raw tokens  : {total_tokens:>10,.0f}")
    print(f"  Saved tokens: {saved_tokens:>10,.0f}")

    if gate is not None and gate["n_paired"] > 0:
        delta = gate["overall_delta"]
        trend = " ◄ REGRESSION" if delta < -2.0 else (" ► improvement" if delta > 2.0 else "")
        print(
            f"\n  vs baseline : savings  Δ = {delta:+.1f}pp  "
            f"({gate['baseline_ws']:.1f}% → {gate['current_ws']:.1f}%){trend}"
        )
        ls = gate.get("latency_stats", {})
        if ls:
            med_d = ls["median_delta_ms"]
            p90_d = ls["p90_delta_ms"]
            tot_d = ls["total_delta_ms"]
            med_trend = "  ⚠ slower" if med_d > 50 else ("  ↓ faster" if med_d < -50 else "")
            print(
                f"  vs baseline : latency  median Δ = {med_d:+.0f}ms "
                f"({ls['median_baseline_ms']:.0f}ms → {ls['median_now_ms']:.0f}ms){med_trend}   "
                f"p90 Δ = {p90_d:+.0f}ms   total Δ = {tot_d:+,.0f}ms"
            )


def print_verdict(gate: dict, ci: bool) -> None:
    print()
    if gate["passed"]:
        print("  ✓ QUALITY GATE: PASS")
    else:
        print("  ✗ QUALITY GATE: FAIL")
        for msg in gate["hard_failures"]:
            print(f"    ✗ {msg}")
            if ci:
                print(f"::error::{msg}")

    # Compression improvements
    if gate["improvements"]:
        n = len(gate["improvements"])
        print(f"\n  ↑ compression improved ({n} cmd{'s' if n > 1 else ''}):")
        for r, b, delta in sorted(gate["improvements"], key=lambda x: -x[2]):
            print(f"    + {r[0]}/{r[2]}  Δ=+{delta:.1f}pp")

    # Latency improvements
    lat_wins = gate.get("latency_improvements", [])
    if lat_wins:
        n = len(lat_wins)
        print(f"\n  ↓ latency improved ({n} cmd{'s' if n > 1 else ''}):")
        for r, b, lat_d in sorted(lat_wins, key=lambda x: x[2]):
            print(f"    + {r[0]}/{r[2]}  Δ={lat_d:+.0f}ms  ({b['elapsed_ms']:.0f}ms → {r[6]:.0f}ms)")

    # Latency regressions / soft warnings
    if gate["soft_warnings"]:
        n = len(gate["soft_warnings"])
        print(f"\n  ⚠ latency slower ({n} cmd{'s' if n > 1 else ''}):")
        for msg in gate["soft_warnings"]:
            print(f"    ⚠ {msg}")
            if ci:
                print(f"::warning::{msg}")

    print()


# ── JSON report ───────────────────────────────────────────────────────────────

def save_report(
    report_dir: Path,
    current: list[tuple],
    gate: Optional[dict],
    version: str,
    baseline_version: Optional[str],
) -> Path:
    report_dir.mkdir(parents=True, exist_ok=True)
    ts   = int(time.time())
    path = report_dir / f"quality-gate-{ts}.json"

    payload: dict = {
        "generated":        datetime.now(timezone.utc).isoformat(),
        "panda_version":    version,
        "baseline_version": baseline_version,
        "total_commands":   len(current),
        "commands": [
            {
                "repo":       r[0],
                "category":   r[1],
                "label":      r[2],
                "raw_chars":  r[3],
                "savings_pct": round(r[5], 3),
                "elapsed_ms":  round(r[6], 1),
            }
            for r in current
        ],
        "aggregate": {
            "overall_weighted_savings_pct": round(weighted_savings(current), 3),
            "by_category": {k: round(v, 3) for k, v in by_category(current).items()},
        },
    }

    if gate is not None:
        payload["gate"] = {
            "passed":                  gate["passed"],
            "hard_failures":           gate["hard_failures"],
            "soft_warnings":           gate["soft_warnings"],
            "overall_delta":           round(gate["overall_delta"], 3),
            "n_regressions":           len(gate["regressions"]),
            "n_improvements":          len(gate["improvements"]),
            "n_latency_regressions":   len(gate.get("latency_regressions", [])),
            "n_latency_improvements":  len(gate.get("latency_improvements", [])),
            "latency_stats":           gate.get("latency_stats", {}),
        }
    else:
        payload["gate"] = {"passed": True, "note": "no baseline — first-run mode"}

    with open(path, "w") as f:
        json.dump(payload, f, indent=2)
    print(f"  Report → {path}")
    return path


# ── Main ──────────────────────────────────────────────────────────────────────

def main() -> None:
    parser = argparse.ArgumentParser(
        description="PandaFilter Quality Gate",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    parser.add_argument(
        "--panda", type=Path, default=DEFAULT_PANDA,
        help=f"panda binary to benchmark (default: {DEFAULT_PANDA})",
    )
    parser.add_argument(
        "--baseline", type=Path, default=DEFAULT_BASELINE,
        help=f"baseline JSON (default: {DEFAULT_BASELINE})",
    )
    parser.add_argument(
        "--update-baseline", action="store_true",
        help="run benchmark and overwrite baseline.json, then exit 0",
    )
    parser.add_argument(
        "--ci", action="store_true",
        help="emit GitHub Actions ::error:: / ::warning:: annotations",
    )
    parser.add_argument(
        "--verbose", "-v", action="store_true",
        help="print each command result as it runs",
    )
    parser.add_argument(
        "--report-dir", type=Path, default=DEFAULT_REPORTS,
        help=f"directory for JSON reports (default: {DEFAULT_REPORTS})",
    )
    args = parser.parse_args()

    # Validate binary
    if not args.panda.exists():
        print(f"\nError: panda binary not found: {args.panda}", file=sys.stderr)
        print("  Build with: cargo build -p panda", file=sys.stderr)
        sys.exit(1)

    version_raw = subprocess.run(
        [str(args.panda), "--version"], capture_output=True, text=True
    ).stdout.strip()
    version = version_raw.split()[-1] if version_raw else "unknown"

    # Header
    repos_str = ", ".join(sorted(set(c[0] for c in COMMANDS)))
    print("╔══════════════════════════════════════════════════════════════════════════════╗")
    print("║                    PandaFilter Quality Gate                                ║")
    print("╠══════════════════════════════════════════════════════════════════════════════╣")
    print(f"║  Binary   : {str(args.panda):<67}║")
    print(f"║  Version  : {version:<67}║")
    print(f"║  Commands : {len(COMMANDS):<67}║")
    print(f"║  Repos    : {repos_str:<67}║")
    if args.update_baseline:
        print( "║  Mode     : UPDATE BASELINE                                                 ║")
    else:
        print(f"║  Baseline : {str(args.baseline):<67}║")
    print("╚══════════════════════════════════════════════════════════════════════════════╝")

    # Run benchmark
    current = run_all(args.panda, verbose=args.verbose)
    if not current:
        print("\nError: no results — are repos cloned in ccr-eval/benchmarks/repos/?", file=sys.stderr)
        sys.exit(1)

    # ── Mode: update baseline ─────────────────────────────────────────────────
    if args.update_baseline:
        existing_thresholds = DEFAULT_THRESHOLDS
        if args.baseline.exists():
            try:
                with open(args.baseline) as f:
                    old = json.load(f)
                existing_thresholds = old.get("thresholds", DEFAULT_THRESHOLDS)
            except Exception:
                pass

        print_table(current, baseline_by_key=None)
        print_aggregate(current, gate=None)
        save_baseline(args.baseline, current, version, existing_thresholds)
        save_report(args.report_dir, current, gate=None, version=version, baseline_version=None)
        print(
            "\n  Next steps:\n"
            f"    git add {args.baseline}\n"
            f"    git commit -m 'chore: advance quality gate baseline to {version}'"
        )
        sys.exit(0)

    # ── Mode: compare vs baseline ─────────────────────────────────────────────
    baseline = load_baseline(args.baseline)

    if baseline is None:
        print(
            "\n  ⚠  No baseline data found — running in first-run mode (no gate checks).\n"
            f"     Seed with: python3 {Path(__file__).name} --update-baseline\n"
        )
        print_table(current, baseline_by_key=None)
        print_aggregate(current, gate=None)
        save_report(args.report_dir, current, gate=None, version=version, baseline_version=None)
        sys.exit(0)

    baseline_by_key = {(c["repo"], c["label"]): c for c in baseline["commands"]}
    gate = compare(current, baseline)

    print_table(current, baseline_by_key)
    print_aggregate(current, gate)
    print_verdict(gate, ci=args.ci)
    save_report(
        args.report_dir, current, gate,
        version=version, baseline_version=baseline.get("version"),
    )

    sys.exit(0 if gate["passed"] else 1)


if __name__ == "__main__":
    main()
