# CCR — Cool Cost Reduction

> **60–95% token savings on Claude Code tool outputs.** CCR sits between Claude and your tools, compressing what Claude reads without changing what you ask it to do.

---

## Token Savings

Numbers from `ccr/tests/handler_benchmarks.rs` — each handler fed a realistic large-project fixture, tokens counted before and after. Run `cargo test -p ccr benchmark -- --nocapture` to reproduce, or `ccr gain` to see your own live data.

| Operation | Without CCR | With CCR | Savings |
|-----------|------------:|---------:|:-------:|
| `pip install` | 1,787 | 9 | **−99%** |
| `uv sync` | 1,574 | 15 | **−99%** |
| `playwright test` | 1,367 | 19 | **−99%** |
| `gradle build` | 803 | 17 | **−98%** |
| `go test` | 4,507 | 148 | **−97%** |
| `pytest` | 3,818 | 162 | **−96%** |
| `terraform plan` | 3,926 | 163 | **−96%** |
| `npm install` | 648 | 25 | **−96%** |
| `cargo build` | 1,923 | 93 | **−95%** |
| `cargo test` | 2,782 | 174 | **−94%** |
| `next build` | 549 | 53 | **−90%** |
| `cargo clippy` | 786 | 93 | **−88%** |
| `make` | 545 | 72 | **−87%** |
| `git push` | 173 | 24 | **−86%** |
| `ls` | 691 | 102 | **−85%** |
| `webpack` | 882 | 143 | **−84%** |
| `vitest` | 625 | 103 | **−84%** |
| `nx run-many` | 1,541 | 273 | **−82%** |
| `turbo run build` | 597 | 115 | **−81%** |
| `ruff check` | 2,035 | 435 | −79% |
| `eslint` | 4,393 | 974 | −78% |
| `git log` | 1,573 | 353 | −78% |
| `grep` | 2,925 | 691 | −76% |
| `helm install` | 224 | 54 | −76% |
| `docker ps` | 1,057 | 266 | −75% |
| `golangci-lint` | 3,678 | 960 | −74% |
| `git status` | 650 | 184 | −72% |
| `kubectl get pods` | 2,306 | 689 | −70% |
| `vite build` | 526 | 182 | −65% |
| `jest` | 330 | 114 | −65% |
| `env` | 1,155 | 399 | −65% |
| `mvn install` | 4,585 | 1,613 | −65% |
| `brew install` | 368 | 148 | −60% |
| `gh pr list` | 774 | 321 | −59% |
| `git diff` | 6,370 | 2,654 | −58% |
| `biome lint` | 1,503 | 753 | −50% |
| `tsc` | 2,598 | 1,320 | −49% |
| `mypy` | 2,053 | 1,088 | −47% |
| `stylelint` | 1,100 | 845 | −23% |
| **Total** | **69,727** | **15,846** | **−77%** |

**Notes:**
- `cargo build` / `cargo test`: CCR injects `--message-format json` to extract structured errors.
- `git status` / `git log`: CCR injects `--porcelain` / `--oneline` before running.
- `git diff`: 10-file refactoring fixture; context lines trimmed to 2 per side, total capped at 200.
- `gradle build`: UP-TO-DATE tasks collapsed — savings scale with subproject count.
- Run `ccr gain` after any session to see your real numbers.

---

## Contents

- [How It Works](#how-it-works)
- [FAQ](#faq)
- [Installation](#installation)
- [Commands](#commands)
- [Handlers](#handlers)
- [Pipeline Architecture](#pipeline-architecture)
- [BERT Routing](#bert-routing)
- [Configuration](#configuration)
- [User-Defined Filters](#user-defined-filters)
- [Session Intelligence](#session-intelligence)
- [Hook Architecture](#hook-architecture)
- [Crate Overview](#crate-overview)
- [Claude Code Source Findings](#claude-code-source-findings)

---

## How It Works

```
Claude runs: cargo build
    ↓ PreToolUse hook rewrites to: ccr run cargo build
    ↓ ccr run executes cargo, filters output through Cargo handler
    ↓ Claude reads: errors + warning count only (~87% fewer tokens)

Claude runs: Read file.rs  (large file)
    ↓ PostToolUse hook: BERT pipeline using current task as query
    ↓ Claude reads: compressed file content focused on what's relevant

Claude runs: git status  (seen recently)
    ↓ PreToolUse hook rewrites to: ccr run git status
    ↓ Pre-run cache hit (same HEAD+staged+unstaged hash)
    ↓ Claude reads: [PC: cached from 2m ago — ~1.8k tokens saved]
```

After `ccr init`, **this is fully automatic** — no changes to how you use Claude Code.

### Privacy model

CCR is a local-only tool. It never sends data anywhere.

| What CCR touches | What it reads | Why |
|-----------------|---------------|-----|
| Tool output (hook) | stdout/stderr of commands you run (`cargo build`, `git status`, …) | Compress it before Claude sees it |
| Claude's last message (BERT only) | The single most-recent message in the active session | Used as a relevance query so compression keeps lines relevant to your current task — read-only, never stored |
| Conversation files (`ccr discover` only) | Local JSONL files Claude Code writes to `~/.claude/` | Find which commands ran without a handler — **opt-in, never automatic** |

The hook **never reads your prompts or full conversation history.** Everything stays on your machine.

---

## FAQ

**Does CCR degrade Claude's output quality?**
No. CCR only removes noise from tool output — build logs, module graphs, passing test lines, progress bars. The signal Claude needs (errors, file paths, summaries) is always kept.

**What happens with a tool CCR doesn't know about?**
It goes through BERT semantic routing — the command name is compared against all known handlers by similarity. If confidence is high enough the closest handler is applied; if nothing matches the output passes through unchanged. CCR never silently drops output.

**How do I verify it's working?**
Run `ccr gain` after a session. To inspect what Claude actually receives from a specific command:
```bash
ccr proxy git log --oneline -20
```

**Does CCR send any data outside my machine?**
Never. All processing is fully local. BERT runs on-device using a small embedded model.

---

## Installation

### Homebrew (macOS — recommended)

```bash
brew tap AssafWoo/ccr
brew install ccr
ccr init
```

### Script (Linux / any platform)

```bash
curl -fsSL https://raw.githubusercontent.com/AssafWoo/homebrew-ccr/main/install.sh | bash
```

The script installs Rust via `rustup` if needed, builds CCR from source with `cargo install`, adds `~/.cargo/bin` to your PATH, and runs `ccr init`. No prebuilt binaries — works on any architecture Rust supports.

> **First run:** CCR downloads the BERT model (~90 MB, `all-MiniLM-L6-v2`) from HuggingFace and caches it at `~/.cache/huggingface/`. Subsequent runs are instant.

---

## Commands

### ccr gain

```bash
ccr gain                    # overall summary
ccr gain --breakdown        # include per-command table
ccr gain --history          # last 14 days
ccr gain --history --days 7
```

```
CCR Token Savings
═════════════════════════════════════════════════
  Runs:           315  (avg 280ms)
  Tokens saved:   32.9k / 71.1k  (46.3%)  ███████████░░░░░░░░░░░░░
  Cost saved:     ~$0.099  (at $3.00/1M)
  Today:          142 runs · 6.8k saved · 23.9%
  Top command:    (pipeline)  65.2%  ·  25.8k saved
  Run `ccr gain --breakdown` for per-command details.
```

`ccr gain --breakdown` adds a per-command table sorted by tokens saved. Pricing uses `cost_per_million_tokens` from `ccr.toml` if set, otherwise `ANTHROPIC_MODEL` env var (Opus 4.6: $15, Sonnet 4.6: $3, Haiku 4.5: $0.80), otherwise $3.00.

### ccr discover

```bash
ccr discover
```

Scans `~/.claude/projects/*/` JSONL history for Bash commands that ran without CCR. Reports estimated missed savings sorted by impact.

### ccr compress

```bash
ccr compress --scan-session --dry-run   # estimate savings for current conversation
ccr compress --scan-session             # compress and write to {file}.compressed.json
ccr compress conversation.json -o out.json
cat conversation.json | ccr compress -
```

Finds the most recently modified conversation JSONL under `~/.claude/projects/`, runs tiered compression (recent turns preserved verbatim, older turns compressed), and reports `tokens_in → tokens_out`. `--dry-run` estimates without writing. When context pressure is high, the hook suggests: `ccr compress --scan-session --dry-run`.

### ccr init

Installs hooks into `~/.claude/settings.json`. Safe to re-run — merges into existing arrays, preserving other tools' hooks. Registers PostToolUse for Bash, Read, and Glob.

### ccr noise

```bash
ccr noise           # show learned noise patterns for this project
ccr noise --reset   # clear all patterns
```

Lines seen ≥10 times with ≥90% suppression rate are promoted to permanent pre-filters. Error/warning/panic lines are never promoted.

### ccr expand

```bash
ccr expand ZI_3       # print original lines from a collapsed block
ccr expand --list     # list all available IDs in this session
```

When CCR collapses output, it embeds an ID: `[5 lines collapsed — ccr expand ZI_3]`

### ccr filter / ccr run / ccr proxy

```bash
cargo clippy 2>&1 | ccr filter --command cargo
ccr run git status    # run through CCR handler
ccr proxy git status  # run raw (no filtering), record analytics baseline
```

---

## Handlers

44 handlers (55+ command aliases) in `ccr/src/handlers/`. Lookup cascade:

1. **Level 0 — User filters** — `.ccr/filters.toml` or `~/.config/ccr/filters.toml` (overrides built-in)
2. **Level 1 — Exact match** — direct command name
3. **Level 2 — Static alias table** — versioned binaries, wrappers, common aliases
4. **Level 3 — BERT routing** — unknown commands matched with confidence tiers (see [BERT Routing](#bert-routing))

**TypeScript / JavaScript**

| Handler | Keys | Key behavior |
|---------|------|-------------|
| **tsc** | `tsc` | Groups errors by file; deduplicates repeated TS codes; truncates verbose type messages. `Build OK` on clean. |
| **vitest** | `vitest` | FAIL blocks + summary; drops `✓` lines. |
| **jest** | `jest`, `bun`, `deno` | `●` failure blocks + summary; drops `PASS` lines. |
| **nx** | `nx`, `npx nx` | `run-many`/`affected`: passing tasks collapsed to `[N tasks passed (N cached)]`; failing task output preserved verbatim. Injects `--output-style=stream`. |
| **eslint** | `eslint` | Errors grouped by file, caps at 20 + `[+N more]`. |
| **next** | `next` | `build`: route table collapsed, errors + page count. `dev`: errors + ready line. |
| **playwright** | `playwright` | Failing test names + error messages; passing tests dropped. |
| **prettier** | `prettier` | `--check`: files needing formatting + count. `--write`: file count. |
| **vite** | `vite` | `build`: asset chunk table collapsed, module noise dropped. `dev`: HMR deduplication. |
| **webpack** | `webpack`, `webpack-cli` | Module resolution graph dropped; keeps assets, errors, warnings, and build result. |
| **turbo** | `turbo`, `npx turbo` | Inner task output stripped; keeps cache hit/miss per package + final summary. |
| **stylelint** | `stylelint` | Issues grouped by file, caps at 40 + `[+N more]`, summary count kept. |
| **biome** | `biome`, `@biomejs/biome` | Code context snippets (│/^^^) stripped; keeps file:line, rule name, and message. |

**Python**

| Handler | Keys | Key behavior |
|---------|------|-------------|
| **pytest** | `pytest`, `py.test` | FAILED node IDs + AssertionError + short summary. |
| **uv** | `uv`, `uvx` | `install`/`sync`/`add`: strips Downloading/Fetching/Preparing noise. Keeps errors, warnings, and final installed summary. |
| **ruff** | `ruff` | `check`: violations grouped by error code, first 3 shown + `[N more]`. `format`: summary line only. Clean run → `ruff: ok`. Injects `--output-format concise`. |
| **mypy** | `mypy`, `mypy3` | Errors grouped by file, capped at 10 per file. Notes and daemon startup lines stripped. Clean run → `mypy: ok`. |
| **pip** | `pip`, `pip3`, `poetry`, `pdm`, `conda` | `install`: `[complete — N packages]` or already-satisfied short-circuit. |
| **python** | `python`, `python3`, `python3.X` | Traceback: keep block + final error. Long output: BERT. |

**DevOps / Cloud**

| Handler | Keys | Key behavior |
|---------|------|-------------|
| **kubectl** | `kubectl`, `k`, `minikube`, `kind` | `get`: smart column selection (NAME+STATUS+READY, drops AGE/RESTARTS). `logs`: BERT anomaly. `describe`: key sections. |
| **gh** | `gh` | `pr list`/`issue list`: compact tables. `pr view`: strips HTML noise. Passthrough for `--json`/`--jq`. |
| **terraform** | `terraform`, `tofu` | `plan`: `+`/`-`/`~` + summary. `validate`: short-circuits on success. |
| **aws** | `aws`, `gcloud`, `az` | Action-specific resource extraction (ec2, lambda, iam, s3api). JSON → schema fallback. |
| **make** | `make`, `gmake`, `ninja` | "Nothing to be done" short-circuit. Keeps errors + recipe failures. |
| **go** | `go` | `build`/`vet`: errors only. `test`: FAIL blocks + `[N tests passed]` summary. Drops `=== RUN`/`--- PASS`/`coverage:` lines. |
| **golangci-lint** | `golangci-lint`, `golangci_lint` | Diagnostics grouped by file; INFO/DEBUG runner noise dropped. |
| **prisma** | `prisma` | `generate`: client summary. `migrate`: migration names. `db push`: sync status. |
| **mvn** | `mvn`, `mvnw`, `./mvnw` | Drops `[INFO]` noise; keeps errors + reactor summary. |
| **gradle** | `gradle`, `gradlew`, `./gradlew` | UP-TO-DATE tasks collapsed to `[N tasks UP-TO-DATE]`. FAILED tasks, Kotlin errors, failure blocks kept. |
| **helm** | `helm`, `helm3` | `list`: compact table. `status`/`diff`/`template`: structured. |

**System / Utility**

| Handler | Keys | Key behavior |
|---------|------|-------------|
| **cargo** | `cargo` | `build`/`check`/`clippy`: JSON format, errors + warning count. `test`: failures + summary. Repeated Clippy rules grouped `[rule ×N]`. |
| **git** | `git` | `status`: Staged/Modified/Untracked counts. `log` injects `--oneline`, caps 20. `diff`: 2 context lines per side, 200-line total cap, per-file `[+N -M]` tally. Push/pull success short-circuits. |
| **curl** | `curl` | JSON → type schema. Non-JSON: cap 30 lines. |
| **docker** | `docker`, `docker-compose` | `logs`: ANSI strip + timestamp normalization before BERT. `ps`/`images`: compact table. |
| **npm/yarn** | `npm`, `yarn` | `install`: package count. Strips boilerplate (`> project@...`, `npm WARN`, spinners). |
| **pnpm** | `pnpm`, `pnpx` | `install`: summary; drops progress bars. `run`/`exec`: errors + tail. |
| **clippy** | `clippy`, `cargo-clippy` | Rustc-style diagnostics filtered; duplicate warnings collapsed. |
| **journalctl** | `journalctl` | Injects `--no-pager -n 200`. BERT anomaly scoring. |
| **psql** | `psql`, `pgcli` | Strips borders, pipe-separated columns, caps at 20 rows. |
| **brew** | `brew` | `install`/`update`: status lines + Caveats. |
| **tree** | `tree` | Injects `-I "node_modules\|.git\|target\|..."` unless user set `-I`. |
| **diff** | `diff` | `+`/`-`/`@@` + 2 context lines per hunk. Max 5 hunks + `[+N more hunks]`. |
| **jq** | `jq` | ≤20 lines pass through. Array: schema of first element + `[N items]`. |
| **env** | `env`, `printenv` | Categorized sections: [PATH]/[Language]/[Cloud]/[Tools]/[Other]. Sensitive values redacted. |
| **ls** | `ls` | Drops noise dirs (node_modules, .git, target, …). Top-3 extension summary. |
| **cat** | `cat` | ≤100 lines: pass through. 101–500: head/tail. >500: BERT. |
| **grep / rg** | `grep`, `rg` | Compact paths (>50 chars), per-file 25-match cap. |
| **find** | `find` | Strips common prefix, groups by directory, caps at 50. |
| **json** | `json` | Parses output as JSON, returns depth-limited type schema if smaller. |
| **log** | `log` | Timestamp/UUID/hex normalization, dedup `[×N]`, error/warning summary block. |

---

## Pipeline Architecture

Every output goes through these steps in order:

```
0. Hard input ceiling (200k chars max — truncates before any stage runs)
1. Strip ANSI codes
2. Normalize whitespace (trailing spaces, blank-line collapse, consecutive-line dedup)
2.5 Global regex pre-filter (zero BERT cost, always runs)
        • Strip progress bars: [=======>   ], [####  56%], bare ====== (8+ chars)
        • Strip download/transfer lines: "Downloading 45 MB", "Fetching index..."
        • Strip spinner lines: ⠙⠹⠸ / - \ |
        • Strip standalone percentage lines: "34%", "100% done"
        • Strip pure decorator lines ≥10 chars: ─────────, ═════════
3. Command-specific pattern filter (regex rules from config/handlers)
4. Only if over summarize_threshold_lines:
   4a. BERT noise pre-filter (semantic: removes boilerplate via embedding distance)
   4b. Entropy-adaptive BERT summarization (7 passes, see below)
5. Hard output cap (50k chars max — applied after all stages)
```

**Minimum token gate (hook level):** Outputs under 15 tokens skip the entire pipeline — no BERT, no analytics recording.

### BERT Passes (step 4b)

| Pass | What it does |
|------|-------------|
| **Noise pre-filter** | Removes project-specific boilerplate promoted by noise learning |
| **Semantic clustering** | Near-identical lines (cosine > 0.85) collapse to one representative |
| **Entropy budget** | Diverse content gets more lines; uniform output gets a tight budget |
| **Anomaly scoring** | Scores each line against centroid + intent query; keeps top-N |
| **Contextual anchors** | Re-adds semantic neighbors of kept lines (e.g. function signature above error) |
| **Historical centroid** | Scores against rolling mean of prior runs — new output stands out more |
| **Delta compression** | Suppresses unchanged lines vs previous run; surfaces new ones with `[Δ from turn N]` |

### Fallback

If BERT is unavailable or output is short, CCR falls back to head + tail. No crash, no empty output.

---

## BERT Routing

Unknown commands (not in the exact/alias table) are matched to the nearest handler via sentence embeddings. **Three confidence tiers:**

| Tier | Condition | Action |
|------|-----------|--------|
| **HIGH** | score ≥ 0.70 AND margin ≥ 0.15 | Full handler — filter output + rewrite args |
| **MEDIUM** | score ≥ 0.55 AND margin ≥ 0.08 | Filter only — no arg injection (safe) |
| **None** | below thresholds | Passthrough — don't risk misrouting |

**Margin gate:** If `top_score - second_score < threshold`, routing is ambiguous and CCR falls back rather than guessing. A command scoring 0.71 for cargo and 0.69 for npm would route to nothing (0.02 margin < 0.08).

**Subcommand hint boost (+0.08):** When an unknown command is run with a recognizable subcommand, matching handlers get a boost:
- `bloop test` → pytest/jest/vitest/go boosted
- `mytool build` → cargo/go/docker/next boosted
- `newtool install` → npm/pnpm/brew/pip boosted
- `x lint` → eslint/golangci-lint/clippy boosted

---

## Configuration

Config is loaded from: `./ccr.toml` → `~/.config/ccr/config.toml` → embedded default.

```toml
[global]
summarize_threshold_lines = 50   # trigger BERT summarization
head_lines = 30                  # head+tail fallback budget
tail_lines = 30
strip_ansi = true
normalize_whitespace = true
deduplicate_lines = true
input_char_ceiling = 200000      # truncate raw input before pipeline (0 = disabled)
output_char_cap = 50000          # cap pipeline output (0 = disabled)
# cost_per_million_tokens = 15.0  # override pricing in ccr gain

[tee]
enabled = true
mode = "aggressive"   # "aggressive" | "always" | "never"
max_files = 20

[commands.git]
patterns = [
  { regex = "^(Counting|Compressing|Receiving|Resolving) objects:.*", action = "Remove" },
]

[commands.cargo]
patterns = [
  { regex = "^\\s+Compiling \\S+ v[\\d.]+", action = "Collapse" },
  { regex = "^\\s+Downloaded \\S+ v[\\d.]+", action = "Remove"   },
]
```

Pattern actions: `Remove` (delete line), `Collapse` (count → `[N lines collapsed]`), `ReplaceWith = "text"`.

---

## User-Defined Filters

Place a `filters.toml` at `.ccr/filters.toml` (project-local) or `~/.config/ccr/filters.toml` (global). Project-local overrides global for the same command key. These run at **Level 0** — before any built-in handler.

```toml
[commands.myapp]
strip_lines_matching = ["DEBUG:", "TRACE:"]
keep_lines_matching  = []          # empty = keep all survivors
max_lines = 50
on_empty  = "(no relevant output)"

[commands.myapp.match_output]
pattern        = "Server started"
message        = "ok — server ready"
unless_pattern = "error"           # optional: block short-circuit if this also matches
```

Fields:
- **`strip_lines_matching`** — remove any line containing these substrings
- **`keep_lines_matching`** — after stripping, keep only lines matching these (empty = keep all)
- **`max_lines`** — hard cap on output line count
- **`on_empty`** — output when all lines are filtered away
- **`match_output`** — short-circuit: if `pattern` found and `unless_pattern` absent, return `message` immediately

---

## Session Intelligence

CCR tracks state across turns within a session (identified by `CCR_SESSION_ID=$PPID`). State lives at `~/.local/share/ccr/sessions/<id>.json`.

**Cross-turn output cache** — Identical outputs (cosine > 0.92) across turns collapse to `[same output as turn 4 (3m ago) — 1.2k tokens saved]`.

**Semantic delta** — Repeated commands emit only new/changed lines: `[Δ from turn N: +M new, K repeated — ~T tokens saved]`. Subcommand-aware so `git status` and `git log` histories don't cross-contaminate.

**Elastic context** — As cumulative session tokens grow (25k → 80k), pipeline pressure scales 0 → 1, shrinking BERT budgets automatically. At >80% pressure: `[⚠ context near full — run ccr compress --scan-session --dry-run to estimate savings]`.

**Pre-run cache** — git commands with identical HEAD+staged+unstaged state are served from cache (TTL 1h), skipping execution entirely.

**Result cache** — post-pipeline output is frozen per input hash, returning byte-identical bytes on repeat calls. Prevents prompt cache busts in Anthropic's API.

**Intent-aware query** — Reads Claude's last assistant message from the live session JSONL and uses it as the BERT query, biasing compression toward what Claude is currently working on.

---

## Hook Architecture

### PreToolUse

`ccr-rewrite.sh` calls `ccr rewrite "<cmd>"` before Bash executes:

- **Known handler** → rewrites to `ccr run <cmd>`, patches `tool_input.command`
- **Unknown** → exits 1, Claude Code uses original command
- **Compound commands** → each segment rewritten independently
- **Already wrapped** → no double-wrap

### PostToolUse

Dispatches by `tool_name` — Bash, Read, Glob, or Grep:

- **Bash** — min-token gate → result cache → noise pre-filter → global regex rules → EC pressure → IX intent query → BERT pipeline → ZI blocks → delta compression → sentence dedup → session cache → analytics
- **Read** — files < 50 lines pass through; larger files go through BERT pipeline with intent query; session dedup by file path
- **Glob** — results ≤ 20 pass through; larger lists grouped by directory (max 60), session dedup by path-list hash
- **Grep** — results ≤ 10 lines pass through; larger result sets routed through GrepHandler (compact paths, per-file 25-match cap)

Never fails — returns nothing on error so Claude Code always sees a result.

---

## Crate Overview

```
ccr/            CLI binary — handlers, hooks, session state, commands
ccr-core/       Core library (no I/O) — pipeline, BERT summarizer, global rules, config, analytics
ccr-sdk/        Conversation compression — tiered compressor, deduplicator, Ollama
ccr-eval/       Evaluation suite — Q&A + conversation fixtures against Claude API
config/         Embedded default filter patterns (git, cargo, npm, docker)
```

---

## Claude Code Source Findings

Claude Code's source was released on 2026-03-31. Reading it revealed three gaps in CCR that were silently costing tokens or causing stalls:

### 1. Prompt Cache Stability (Result Cache)

Claude Code maintains a running conversation history. Anthropic charges for the entire preceding conversation as input tokens on every **prompt cache miss**. A cache miss happens when the bytes sent to the API differ from the previous turn — even by a single character.

**Finding:** CCR's BERT pipeline is non-deterministic across session turns (embeddings can shift slightly based on session state), so the same raw output could produce slightly different compressed bytes on a second call, busting the prompt cache.

**Fix:** New post-pipeline result cache (`~/.local/share/ccr/result_cache/<session>.json`). After the first compression of a given raw input, the output bytes are frozen and returned identically on every subsequent call with the same input. Cache key is `hash(raw_text + "\0" + command_hint)` — deliberately excludes query and session state so the frozen bytes are stable regardless of context changes. TTL 1h, 200-entry cap per session.

### 2. Input Ceiling

**Finding:** Claude Code caps tool output at `DEFAULT_MAX_RESULT_SIZE_CHARS = 50_000` before sending it to the model. CCR processes output *before* Claude Code's own cap, so a `cat large.log` producing 10 MB of text fed every byte into CCR's BERT chunking loop — causing multi-second stalls, high memory use, and defeating the purpose of the cap.

**Fix:** Stage 0 in the pipeline truncates raw input to 200k chars (≈50k tokens) before any stage runs. The head is preserved (keeps context from the top); a marker is appended: `[--- input truncated: kept Nk of Mk chars ---]`. Configurable via `input_char_ceiling` in `ccr.toml`; set to `0` to disable.

### 3. Output Cap

**Finding:** Structured data (JSON blobs, large code files) that doesn't compress well can survive the full BERT pipeline at 100k+ chars, negating CCR's savings. Claude Code's own `DEFAULT_MAX_RESULT_SIZE_CHARS = 50_000` provides the natural ceiling — CCR should match it.

**Fix:** Final pipeline stage caps output at 50k chars before token counting. Appends `[--- output capped at Nk chars ---]`. Configurable via `output_char_cap` in `ccr.toml`; set to `0` to disable.

---

## Uninstall

**Step 1 — remove hooks from Claude Code** (works for all install methods):
```bash
ccr init --uninstall
```

**Step 2 — remove the binary:**
```bash
brew uninstall ccr && brew untap AssafWoo/ccr   # Homebrew
# or
cargo uninstall ccr                              # cargo install
```

**Optional — remove cached data:**
```bash
rm -rf ~/.local/share/ccr
rm -rf ~/.cache/huggingface/hub/models--sentence-transformers--all-MiniLM-L6-v2
```

---

## Contributing

Open an issue or PR on [GitHub](https://github.com/AssafWoo/homebrew-ccr). To add a handler: implement the `Handler` trait and register it in `ccr/src/handlers/mod.rs` — see `git.rs` as a template.

---

## License

MIT — see [LICENSE](LICENSE).
