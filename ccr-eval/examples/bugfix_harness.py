#!/usr/bin/env python3
"""
PandaFilter × Claude SDK — bugfix dataset harness.

Shows how to wire `panda filter --json` into an Anthropic SDK agentic loop
so every tool output is compressed before it reaches the model, and per-call
savings are tracked with zero DB plumbing.

Usage
-----
    # Install dependencies
    pip install anthropic

    # Set API key
    export ANTHROPIC_API_KEY=sk-ant-...

    # Run on the built-in stub dataset
    python3 bugfix_harness.py

    # Run on a real dataset (JSON lines: {repo_path, broken_file, description})
    python3 bugfix_harness.py --dataset my_bugs.jsonl --max-turns 20

How it works
------------
1. For each bugfixing task in the dataset, start a fresh Claude conversation.
2. Claude has one tool: `bash` — it can run any shell command in the repo.
3. Every tool output is piped through:
       panda filter --command <cmd> --json
   which returns:
       {"output": "<filtered text>", "tokens_in": N, "tokens_out": N, "savings_pct": F}
4. The "output" field is returned to Claude as the tool_result content.
   The savings fields are accumulated per-task.
5. After Claude signals the fix is done (or turn limit reached), we run the
   repo's test suite to check correctness.
6. A report table is printed: task | fix_ok | raw_tok | filtered_tok | savings%
"""

import argparse
import json
import os
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Optional

try:
    import anthropic
except ImportError:
    print("ERROR: anthropic package not installed. Run: pip install anthropic", file=sys.stderr)
    sys.exit(1)

# ─── Types ────────────────────────────────────────────────────────────────────

@dataclass
class BugTask:
    """One bug to fix."""
    name: str
    repo_path: str          # absolute path to the repo
    broken_file: str        # relative path of the file with the bug
    description: str        # human-readable description of the bug
    test_cmd: str           # command to verify the fix (exit 0 = fixed)

@dataclass
class TaskResult:
    name: str
    fix_ok: bool
    raw_tokens: int         # sum of tokens_in across all panda filter calls
    filtered_tokens: int    # sum of tokens_out across all panda filter calls
    api_input_tokens: int   # actual input_tokens from Anthropic usage (includes history)
    api_output_tokens: int  # actual output_tokens from Anthropic usage
    turns: int
    error: Optional[str] = None

    @property
    def panda_savings_pct(self) -> float:
        if self.raw_tokens == 0:
            return 0.0
        return (self.raw_tokens - self.filtered_tokens) / self.raw_tokens * 100.0

    @property
    def panda_tokens_saved(self) -> int:
        return self.raw_tokens - self.filtered_tokens

# ─── Stub dataset (used when --dataset is not provided) ───────────────────────

def stub_dataset() -> list[BugTask]:
    """
    Minimal stub dataset that works without any real repos.
    Replace with your actual bugfixing dataset.
    """
    return [
        BugTask(
            name="off_by_one",
            repo_path="/tmp/panda_bugfix_demo",
            broken_file="main.py",
            description="Function `count_items` returns len(items) - 1 instead of len(items).",
            test_cmd="python3 -m pytest test_main.py -q",
        ),
    ]

def setup_stub_repo():
    """Create a minimal repo for the stub dataset."""
    repo = Path("/tmp/panda_bugfix_demo")
    repo.mkdir(exist_ok=True)
    (repo / "main.py").write_text(
        "def count_items(items):\n    return len(items) - 1  # BUG: off-by-one\n"
    )
    (repo / "test_main.py").write_text(
        "from main import count_items\n"
        "def test_count():\n    assert count_items([1, 2, 3]) == 3\n"
    )

# ─── Panda integration ────────────────────────────────────────────────────────

def filter_output(raw: str, cmd: str) -> dict:
    """
    Pipe `raw` through `panda filter --command <cmd> --json`.

    Returns dict with keys: output, tokens_in, tokens_out, savings_pct.
    Falls back to passthrough on any error so the harness never blocks.
    """
    try:
        proc = subprocess.run(
            ["panda", "filter", "--command", cmd, "--json"],
            input=raw,
            capture_output=True,
            text=True,
            timeout=15,
        )
        if proc.returncode == 0 and proc.stdout.strip().startswith("{"):
            return json.loads(proc.stdout)
    except Exception:
        pass

    # Passthrough fallback — savings = 0
    tok = len(raw) // 4
    return {"output": raw, "tokens_in": tok, "tokens_out": tok, "savings_pct": 0.0}

# ─── Agentic loop ─────────────────────────────────────────────────────────────

SYSTEM_PROMPT = """\
You are a precise software engineer. You will be given a buggy repository and a
description of the bug. Your job is to find the bug and fix it by editing the
source file directly using the bash tool.

Rules:
- Use bash to read files, run tests, and apply fixes (sed, awk, or Python inline).
- Fix only what is described — do not refactor or add features.
- When you believe the fix is complete, say "DONE" as the final word in your response.
- If you cannot find the bug after 5 attempts, say "GIVE_UP".
"""

TOOLS = [
    {
        "name": "bash",
        "description": "Run a shell command in the repo. Returns stdout+stderr.",
        "input_schema": {
            "type": "object",
            "properties": {
                "command": {"type": "string", "description": "Shell command to run"},
            },
            "required": ["command"],
        },
    }
]

def run_bash(command: str, cwd: str, timeout: int = 30) -> str:
    """Execute a shell command and return stdout+stderr."""
    try:
        result = subprocess.run(
            command, shell=True, cwd=cwd,
            capture_output=True, text=True, timeout=timeout,
        )
        out = (result.stdout + result.stderr).rstrip()
        return out if out else "(no output)"
    except subprocess.TimeoutExpired:
        return f"[TIMEOUT after {timeout}s]"
    except Exception as e:
        return f"[ERROR: {e}]"

def run_bugfix_task(
    task: BugTask,
    client: "anthropic.Anthropic",
    model: str = "claude-sonnet-4-6",
    max_turns: int = 15,
) -> TaskResult:
    """
    Run one bugfixing task. Returns savings + correctness metrics.
    """
    messages = []
    raw_tokens_total = 0
    filtered_tokens_total = 0
    api_input_tokens = 0
    api_output_tokens = 0
    turns = 0

    # Prime the conversation with the task description
    initial_content = (
        f"Repo path: {task.repo_path}\n"
        f"Broken file: {task.broken_file}\n\n"
        f"Bug description: {task.description}\n\n"
        "Please fix the bug."
    )
    messages.append({"role": "user", "content": initial_content})

    try:
        while turns < max_turns:
            response = client.messages.create(
                model=model,
                max_tokens=2048,
                system=SYSTEM_PROMPT,
                tools=TOOLS,
                messages=messages,
            )

            api_input_tokens += response.usage.input_tokens
            api_output_tokens += response.usage.output_tokens
            turns += 1

            # Append assistant turn
            messages.append({"role": "assistant", "content": response.content})

            # Check stop condition
            if response.stop_reason == "end_turn":
                # Extract text to check for DONE / GIVE_UP
                text = " ".join(
                    b.text for b in response.content if hasattr(b, "text")
                )
                if "DONE" in text or "GIVE_UP" in text:
                    break

            # Process tool calls
            tool_results = []
            for block in response.content:
                if block.type != "tool_use":
                    continue

                cmd_str = block.input.get("command", "")
                raw_output = run_bash(cmd_str, task.repo_path)

                # Guess the command hint from the first word
                cmd_hint = cmd_str.strip().split()[0] if cmd_str.strip() else "bash"

                # Filter through panda
                panda_result = filter_output(raw_output, cmd_hint)
                raw_tokens_total += panda_result["tokens_in"]
                filtered_tokens_total += panda_result["tokens_out"]

                tool_results.append({
                    "type": "tool_result",
                    "tool_use_id": block.id,
                    "content": panda_result["output"],
                })

            if not tool_results:
                break

            messages.append({"role": "user", "content": tool_results})

    except Exception as e:
        return TaskResult(
            name=task.name,
            fix_ok=False,
            raw_tokens=raw_tokens_total,
            filtered_tokens=filtered_tokens_total,
            api_input_tokens=api_input_tokens,
            api_output_tokens=api_output_tokens,
            turns=turns,
            error=str(e),
        )

    # Verify fix
    test_output = run_bash(task.test_cmd, task.repo_path, timeout=60)
    fix_ok = "passed" in test_output.lower() or "ok" in test_output.lower()

    return TaskResult(
        name=task.name,
        fix_ok=fix_ok,
        raw_tokens=raw_tokens_total,
        filtered_tokens=filtered_tokens_total,
        api_input_tokens=api_input_tokens,
        api_output_tokens=api_output_tokens,
        turns=turns,
    )

# ─── Reporting ────────────────────────────────────────────────────────────────

# Anthropic pricing ($/million tokens) — update as needed
SONNET_INPUT_PRICE  = 3.00
SONNET_OUTPUT_PRICE = 15.00

def cost_usd(input_tokens: int, output_tokens: int) -> float:
    return (input_tokens * SONNET_INPUT_PRICE + output_tokens * SONNET_OUTPUT_PRICE) / 1_000_000

def print_report(results: list[TaskResult], model: str):
    print()
    print("═" * 100)
    print(f"  PandaFilter × Claude SDK — Bugfix Harness Results  (model: {model})")
    print("═" * 100)
    print(f"  {'Task':<25} {'Fix?':>5} {'Turns':>6} {'Raw tok':>9} {'Filt tok':>9} {'Panda%':>8} {'API in':>9} {'API out':>8} {'Cost':>8}")
    print("  " + "─" * 92)

    for r in results:
        status = "PASS" if r.fix_ok else ("ERR " if r.error else "FAIL")
        usd = cost_usd(r.api_input_tokens, r.api_output_tokens)
        err_tag = f"  [{r.error[:30]}]" if r.error else ""
        print(
            f"  {r.name:<25} {status:>5} {r.turns:>6} "
            f"{r.raw_tokens:>9,} {r.filtered_tokens:>9,} {r.panda_savings_pct:>7.1f}% "
            f"{r.api_input_tokens:>9,} {r.api_output_tokens:>8,} ${usd:>6.4f}"
            f"{err_tag}"
        )

    print("  " + "─" * 92)

    total_raw   = sum(r.raw_tokens for r in results)
    total_filt  = sum(r.filtered_tokens for r in results)
    total_api_in  = sum(r.api_input_tokens for r in results)
    total_api_out = sum(r.api_output_tokens for r in results)
    fix_rate    = sum(1 for r in results if r.fix_ok) / len(results) * 100 if results else 0
    panda_pct   = (total_raw - total_filt) / total_raw * 100 if total_raw else 0
    total_cost  = cost_usd(total_api_in, total_api_out)

    print()
    print(f"  Tasks          : {len(results)}")
    print(f"  Fix rate       : {fix_rate:.0f}%  ({sum(1 for r in results if r.fix_ok)}/{len(results)})")
    print()
    print(f"  Panda savings  : {panda_pct:.1f}%  ({total_raw:,} → {total_filt:,} tool-output tokens)")
    print(f"  Tokens saved   : {total_raw - total_filt:,}")
    print()
    print(f"  API input tok  : {total_api_in:,}")
    print(f"  API output tok : {total_api_out:,}")
    print(f"  Estimated cost : ${total_cost:.4f}  (Sonnet: ${SONNET_INPUT_PRICE}/M in, ${SONNET_OUTPUT_PRICE}/M out)")
    print()

    # Cost delta: rough estimate of what it would cost without panda
    # (assumes saved tool-output tokens were also passed as API input)
    hypothetical_api_in = total_api_in + (total_raw - total_filt)
    hypothetical_cost   = cost_usd(hypothetical_api_in, total_api_out)
    saved_cost = hypothetical_cost - total_cost
    print(f"  Hypothetical cost without Panda : ${hypothetical_cost:.4f}")
    print(f"  Saved by Panda                  : ${saved_cost:.4f}  ({saved_cost/hypothetical_cost*100:.1f}%)")
    print("═" * 100)
    print()

# ─── Main ─────────────────────────────────────────────────────────────────────

def load_jsonl_dataset(path: str) -> list[BugTask]:
    """
    Load a bugfixing dataset from a JSON lines file.

    Each line must be a JSON object with:
      name, repo_path, broken_file, description, test_cmd
    """
    tasks = []
    with open(path) as f:
        for i, line in enumerate(f):
            line = line.strip()
            if not line:
                continue
            obj = json.loads(line)
            tasks.append(BugTask(
                name=obj.get("name", f"task_{i}"),
                repo_path=obj["repo_path"],
                broken_file=obj["broken_file"],
                description=obj["description"],
                test_cmd=obj["test_cmd"],
            ))
    return tasks

def main():
    parser = argparse.ArgumentParser(
        description="PandaFilter × Claude SDK bugfix harness",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument(
        "--dataset", type=str, default=None,
        help="Path to a JSONL dataset file (one BugTask per line). Omit for stub demo.",
    )
    parser.add_argument(
        "--model", type=str, default="claude-sonnet-4-6",
        help="Claude model to use (default: claude-sonnet-4-6)",
    )
    parser.add_argument(
        "--max-turns", type=int, default=15,
        help="Max agentic turns per task (default: 15)",
    )
    parser.add_argument(
        "--limit", type=int, default=None,
        help="Run only the first N tasks (useful for quick smoke tests)",
    )
    args = parser.parse_args()

    # Check panda is available
    check = subprocess.run(["panda", "--version"], capture_output=True, text=True)
    if check.returncode != 0:
        print("ERROR: 'panda' binary not found. Install PandaFilter first.", file=sys.stderr)
        sys.exit(1)

    # Check API key
    if not os.environ.get("ANTHROPIC_API_KEY"):
        print("ERROR: ANTHROPIC_API_KEY environment variable not set.", file=sys.stderr)
        sys.exit(1)

    # Load dataset
    if args.dataset:
        tasks = load_jsonl_dataset(args.dataset)
    else:
        print("No --dataset provided. Using built-in stub demo.")
        setup_stub_repo()
        tasks = stub_dataset()

    if args.limit:
        tasks = tasks[: args.limit]

    print(f"\nRunning {len(tasks)} task(s) with model={args.model}, max_turns={args.max_turns}")
    print(f"Panda binary: {check.stdout.strip()}\n")

    client = anthropic.Anthropic()
    results = []

    for i, task in enumerate(tasks, 1):
        print(f"[{i}/{len(tasks)}] {task.name} ...", end=" ", flush=True)
        result = run_bugfix_task(task, client, model=args.model, max_turns=args.max_turns)
        status = "PASS" if result.fix_ok else ("ERROR" if result.error else "FAIL")
        print(f"{status}  panda_savings={result.panda_savings_pct:.1f}%  turns={result.turns}")
        results.append(result)

    print_report(results, args.model)

if __name__ == "__main__":
    main()
