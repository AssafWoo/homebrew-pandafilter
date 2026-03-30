# CCR — Cool Cost Reduction

> **60–95% token savings on Claude Code tool outputs.** CCR sits between Claude and your tools, compressing what Claude reads without changing what you ask it to do.

---

## Token Savings

Numbers from `ccr/tests/handler_benchmarks.rs` — each handler fed a realistic large-project fixture, tokens counted before and after. Run `cargo test -p ccr benchmark -- --nocapture` to reproduce, or `ccr gain` to see your own live data.

| Operation | Without CCR | With CCR | Savings |
|-----------|------------:|---------:|:-------:|
| `pip install` | 1,787 | 9 | **−99%** |
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
| `turbo run build` | 597 | 115 | **−81%** |
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
| `stylelint` | 1,100 | 845 | −23% |
| **Total** | **62,524** | **14,035** | **−78%** |

**Notes:**
- For `cargo build` / `cargo test`: "without CCR" is standard human-readable output; CCR injects `--message-format json` to extract structured errors.
- For `git status` / `git log`: "without CCR" is the full verbose format; CCR injects `--porcelain` / `--oneline` before running.
- `git diff` fixture is a 10-file refactoring diff; context lines trimmed to 2 per side, total capped at 200.
- `gradle build` collapses UP-TO-DATE task lines into a single count — savings scale with subproject count.
- `tsc` groups errors by file and truncates verbose type messages; savings scale with error count.
- Run `ccr gain` after any session to see your real numbers.

---

## Contents

- [How It Works](#how-it-works)
- [Installation](#installation)
- [Commands](#commands)
- [Handlers](#handlers)
- [Pipeline Architecture](#pipeline-architecture)
- [BERT Routing](#bert-routing)
- [Configuration](#configuration)
- [User-Defined Filters](#user-defined-filters)
- [Session Intelligence](#session-intelligence)
- [Hook Architecture](#hook-architecture)
- [CCR vs RTK](#ccr-vs-rtk)
- [Crate Overview](#crate-overview)

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
| Conversation files (`ccr discover` only) | Local JSONL files Claude Code writes to `~/.claude/` | Find which commands ran without a handler — **opt-in, never automatic** |

The hook **never reads your Claude conversation content, prompts, or chat history.** It only sees the output of shell commands — the same bytes you'd see in your terminal. `ccr discover` is an optional analytics command; you can ignore it entirely and CCR works the same.

**What makes CCR different from rule-based proxies:**

- **46 handlers (60+ aliases)** — purpose-built filters for common dev tools (cargo, git, kubectl, gh, terraform, pytest, tsc, vite, webpack, turbo, biome, …)
- **Global regex pre-filter** — strips progress bars, spinners, download lines, and decorators before BERT even loads
- **BERT semantic routing** — unknown commands matched to nearest handler via sentence embeddings, with confidence tiers and margin gating
- **Intent-aware compression** — uses Claude's last message as the BERT query so output relevant to the current task scores highest
- **Noise learning** — learns which lines are boilerplate in your project and pre-filters them before BERT runs
- **Pre-run cache** — git commands with identical repo state return cached output instantly
- **Read/Glob compression** — file reads ≥50 lines and large glob listings go through BERT compression too
- **Session dedup** — identical outputs across turns collapse to a single reference line
- **Elastic context** — pipeline tightens automatically as the session fills up
- **User-defined filters** — declarative TOML rules per command, no code needed

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

Per-Command Breakdown
─────────────────────────────────────────────────────────────
COMMAND      RUNS       SAVED   SAVINGS   AVG ms  IMPACT
─────────────────────────────────────────────────────────────
(pipeline)    112       25.8k     65.2%       —  █████████████
rustfmt         2        2.3k     56.8%       —  ███████████
...

Unoptimized Commands
  Run `ccr discover` for full details · ~18.3k tokens potential
  cargo         ~8.2k saveable
  git           ~6.1k saveable
```

If unoptimized commands are detected in your Claude Code history, they appear at the bottom with estimated savings. Pricing uses `cost_per_million_tokens` from `ccr.toml` if set, otherwise `ANTHROPIC_MODEL` env var (Opus 4.6: $15, Sonnet 4.6: $3, Haiku 4.5: $0.80), otherwise $3.00.

### ccr discover

```bash
ccr discover
```

Scans `~/.claude/projects/*/` JSONL history for Bash commands that ran without CCR. Reports estimated missed savings sorted by impact. Commands already measured through CCR show actual savings ratios; others use handler-specific estimates.

### ccr compress

```bash
ccr compress --scan-session --dry-run   # estimate savings for current conversation
ccr compress --scan-session             # compress and write to {file}.compressed.json
ccr compress conversation.json -o out.json
cat conversation.json | ccr compress -
```

Finds the most recently modified conversation JSONL under `~/.claude/projects/`, runs tiered compression (recent turns preserved verbatim, older turns compressed), and reports `tokens_in → tokens_out`.

`--dry-run` estimates savings without writing output. `--scan-session` auto-locates the current conversation file. When context pressure is high, the hook suggests: `ccr compress --scan-session --dry-run`.

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

### ccr update

```bash
ccr update
```

Checks the latest release on GitHub and replaces the binary in-place if a newer version is available. Also re-runs `ccr init`.

### ccr filter / ccr run / ccr proxy

```bash
cargo clippy 2>&1 | ccr filter --command cargo
ccr run git status    # run through CCR handler
ccr proxy git status  # run raw (no filtering), record analytics baseline
```

---

## Handlers

40 handlers (50+ command aliases) in `ccr/src/handlers/`. Lookup cascade:

1. **Level 0 — User filters** — `.ccr/filters.toml` or `~/.config/ccr/filters.toml` (overrides built-in)
2. **Level 1 — Exact match** — direct command name
3. **Level 2 — Static alias table** — versioned binaries, wrappers, common aliases
4. **Level 3 — BERT routing** — unknown commands matched with confidence tiers (see [BERT Routing](#bert-routing))

**TypeScript / JavaScript**

| Handler | Keys | Savings | Key behavior |
|---------|------|---------|-------------|
| **tsc** | `tsc` | ~50% | Groups errors by file; deduplicates repeated TS codes; truncates verbose type messages. `Build OK` on clean. |
| **vitest** | `vitest` | ~84% | FAIL blocks + summary; drops `✓` lines. |
| **jest** | `jest`, `bun`, `deno`, `nx` | ~65% | `●` failure blocks + summary; drops `PASS` lines. |
| **eslint** | `eslint` | ~78% | Errors grouped by file, caps at 20 + `[+N more]`. |
| **next** | `next` | ~90% | `build`: route table collapsed, errors + page count. `dev`: errors + ready line. |
| **playwright** | `playwright` | ~99% | Failing test names + error messages; passing tests dropped. |
| **prettier** | `prettier` | ~80% | `--check`: files needing formatting + count. `--write`: file count. |
| **vite** | `vite` | ~65% | `build`: asset chunk table collapsed, module noise dropped. `dev`: HMR deduplication. |
| **webpack** | `webpack`, `webpack-cli` | ~84% | Module resolution graph dropped; keeps assets, errors, warnings, and build result. |
| **turbo** | `turbo`, `npx turbo` | ~81% | Inner task output stripped; keeps cache hit/miss per package + final summary. |
| **stylelint** | `stylelint` | ~23% | Issues grouped by file, caps at 40 + `[+N more]`, summary count kept. |
| **biome** | `biome`, `@biomejs/biome` | ~50% | Code context snippets (│/^^^) stripped; keeps file:line, rule name, and message. |

**Python**

| Handler | Keys | Savings | Key behavior |
|---------|------|---------|-------------|
| **pytest** | `pytest`, `py.test` | ~87% | FAILED node IDs + AssertionError + short summary. |
| **pip** | `pip`, `pip3`, `uv`, `poetry`, `pdm`, `conda` | ~80% | `install`: `[complete — N packages]` or already-satisfied short-circuit. |
| **python** | `python`, `python3`, `python3.X` | ~60% | Traceback: keep block + final error. Long output: BERT. |

**DevOps / Cloud**

| Handler | Keys | Savings | Key behavior |
|---------|------|---------|-------------|
| **kubectl** | `kubectl`, `k`, `minikube`, `kind` | ~85% | `get`: smart column selection (NAME+STATUS+READY, drops AGE/RESTARTS). `logs`: BERT anomaly. `describe`: key sections. |
| **gh** | `gh` | ~90% | `pr list`/`issue list`: compact tables. `pr view`: strips HTML noise. Passthrough for `--json`/`--jq`. |
| **terraform** | `terraform`, `tofu` | ~88% | `plan`: `+`/`-`/`~` + summary. `validate`: short-circuits on success. |
| **aws** | `aws`, `gcloud`, `az` | ~85% | Action-specific resource extraction (ec2, lambda, iam, s3api). JSON → schema fallback. |
| **make** | `make`, `gmake`, `ninja` | ~75% | "Nothing to be done" short-circuit. Keeps errors + recipe failures. |
| **go** | `go` | ~82% | `build`/`vet`: errors only. `test`: FAIL blocks + `[N tests passed]` summary. Drops `=== RUN`/`--- PASS`/`coverage:` lines. |
| **golangci-lint** | `golangci-lint`, `golangci_lint` | ~88% | Diagnostics grouped by file; INFO/DEBUG runner noise dropped. |
| **prisma** | `prisma` | ~85% | `generate`: client summary. `migrate`: migration names. `db push`: sync status. |
| **mvn** | `mvn`, `mvnw`, `./mvnw` | ~80% | Drops `[INFO]` noise; keeps errors + reactor summary. |
| **gradle** | `gradle`, `gradlew`, `./gradlew` | ~98% | UP-TO-DATE tasks collapsed to `[N tasks UP-TO-DATE]`. FAILED tasks, Kotlin errors, failure blocks kept. |
| **helm** | `helm`, `helm3` | ~85% | `list`: compact table. `status`/`diff`/`template`: structured. |

**System / Utility**

| Handler | Keys | Savings | Key behavior |
|---------|------|---------|-------------|
| **cargo** | `cargo` | ~87% | `build`/`check`/`clippy`: JSON format, errors + warning count. `test`: failures + summary. Repeated Clippy rules grouped `[rule ×N]`. |
| **git** | `git` | ~80% | `status`: Staged/Modified/Untracked counts. `log` injects `--oneline`, caps 20. `diff`: 2 context lines per side, 200-line total cap, per-file `[+N -M]` tally. Push/pull success short-circuits. |
| **curl** | `curl` | ~96% | JSON → type schema. Non-JSON: cap 30 lines. |
| **docker** | `docker`, `docker-compose` | ~85% | `logs`: ANSI strip + timestamp normalization before BERT. `ps`/`images`: compact table. |
| **npm/yarn** | `npm`, `yarn` | ~85% | `install`: package count. Strips boilerplate (`> project@...`, `npm WARN`, spinners). |
| **pnpm** | `pnpm`, `pnpx` | ~87% | `install`: summary; drops progress bars. `run`/`exec`: errors + tail. |
| **clippy** | `clippy`, `cargo-clippy` | ~85% | Rustc-style diagnostics filtered; duplicate warnings collapsed. |
| **journalctl** | `journalctl` | ~80% | Injects `--no-pager -n 200`. BERT anomaly scoring. |
| **psql** | `psql`, `pgcli` | ~88% | Strips borders, pipe-separated columns, caps at 20 rows. |
| **brew** | `brew` | ~75% | `install`/`update`: status lines + Caveats. |
| **tree** | `tree` | ~70% | Injects `-I "node_modules\|.git\|target\|..."` unless user set `-I`. |
| **diff** | `diff` | ~75% | `+`/`-`/`@@` + 2 context lines per hunk. Max 5 hunks + `[+N more hunks]`. |
| **jq** | `jq` | ~80% | ≤20 lines pass through. Array: schema of first element + `[N items]`. |
| **env** | `env`, `printenv` | ~65% | Categorized sections: [PATH]/[Language]/[Cloud]/[Tools]/[Other]. Long PATH values summarized as `[N entries — bin1, bin2, …]`. Sensitive values redacted. |
| **ls** | `ls` | ~80% | Drops noise dirs (node_modules, .git, target, …). Top-3 extension summary. |
| **cat** | `cat` | ~70% | ≤100 lines: pass through. 101–500: head/tail. >500: BERT. |
| **grep / rg** | `grep`, `rg` | ~80% | Compact paths (>50 chars), per-file 25-match cap. |
| **find** | `find` | ~78% | Strips common prefix, groups by directory, caps at 50. |
| **json** | `json` | ~70% | Parses output as JSON, returns depth-limited type schema if smaller. |
| **log** | `log` | ~75% | Timestamp/UUID/hex normalization, dedup `[×N]`, error/warning summary block. |

---

## Pipeline Architecture

Every output goes through these steps in order:

```
1. Strip ANSI codes
2. Normalize whitespace (trailing spaces, blank-line collapse, consecutive-line dedup)
2.5 ── Global regex pre-filter (NEW, zero BERT cost, always runs) ──────────────────
        • Strip progress bars: [=======>   ], [####  56%], bare ====== (8+ chars)
        • Strip download/transfer lines: "Downloading 45 MB", "Fetching index..."
        • Strip spinner lines: ⠙⠹⠸ / - \ |
        • Strip standalone percentage lines: "34%", "100% done"
        • Strip pure decorator lines ≥10 chars: ─────────, ═════════
3. Command-specific pattern filter (regex rules from config/handlers)
4. ── Only if over summarize_threshold_lines ─────────────────────────────────────
   4a. BERT noise pre-filter (semantic: removes boilerplate via embedding distance)
   4b. Entropy-adaptive BERT summarization (7 passes, see below)
```

**Minimum token gate (hook level):** Outputs under 15 tokens (`which`, `mkdir`, `wc`, `source`) skip the entire pipeline — no BERT, no analytics recording. This keeps efficiency metrics clean and avoids latency overhead on trivial outputs.

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

This makes BERT routing reliable for unknown wrappers that follow standard subcommand conventions.

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
- **`match_output`** — short-circuit: if `pattern` found and `unless_pattern` absent, return `message` immediately (no further filtering)

---

## Session Intelligence

CCR tracks state across turns within a session (identified by `CCR_SESSION_ID=$PPID`). State lives at `~/.local/share/ccr/sessions/<id>.json`.

**Cross-turn output cache** — Identical outputs (cosine > 0.92) across turns collapse to `[same output as turn 4 (3m ago) — 1.2k tokens saved]`.

**Semantic delta** — Repeated commands emit only new/changed lines: `[Δ from turn N: +M new, K repeated — ~T tokens saved]`. Subcommand-aware so `git status` and `git log` histories don't cross-contaminate.

**Elastic context** — As cumulative session tokens grow (25k → 80k), pipeline pressure scales 0 → 1, shrinking BERT budgets automatically. At >80% pressure: `[⚠ context near full — run ccr compress --scan-session --dry-run to estimate savings]`.

**Pre-run cache** — git commands with identical HEAD+staged+unstaged state are served from cache (TTL 1h), skipping execution entirely.

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

- **Bash** — min-token gate → noise pre-filter → global regex rules → EC pressure → IX intent query → BERT pipeline → ZI blocks → delta compression → sentence dedup → session cache → analytics
- **Read** — files < 50 lines pass through; larger files go through BERT pipeline with intent query; session dedup by file path
- **Glob** — results ≤ 20 pass through; larger lists grouped by directory (max 60), session dedup by path-list hash
- **Grep** — results ≤ 10 lines pass through; larger result sets routed through GrepHandler (compact paths, per-file 25-match cap)

Never fails — returns nothing on error so Claude Code always sees a result.

---

## CCR vs RTK

| Feature | CCR | RTK |
|---------|-----|-----|
| Handler count | **40 (50+ aliases)** | 40+ |
| Global regex pre-filter | **Yes** (progress bars, spinners, decorators, download lines) | Partial |
| Minimum token gate | **Yes** (skip pipeline for <15-token outputs) | No |
| Unknown commands | **BERT routing + confidence tiers** (~40%) | Passthrough (0%) |
| BERT routing confidence | **Tier system + margin gate + subcommand hints** | N/A |
| Handler routing | Exact → alias → BERT similarity | Exact match only |
| Read tool compression | Yes (BERT pipeline ≥50 lines) | — |
| Glob tool compression | Yes (dir grouping + session dedup) | — |
| Intent-aware query | Yes (reads live session JSONL) | — |
| Project noise learning | Yes (auto-promotes at ≥90% suppression) | — |
| Pre-run structural cache | Yes (git by HEAD+staged+unstaged) | — |
| Cross-turn output cache | Yes (cosine > 0.92) | — |
| Elastic context | Yes (scales with session size) | — |
| User-defined TOML filters | Yes (Level 0, project + global) | — |
| Missed savings surfaced | Yes (ccr gain + ccr discover) | — |
| Conversation compression | ccr-sdk: tiered + Ollama + dedup | — |
| Hooks preserved on init | Yes (merges arrays) | Overwrites |

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

## Uninstall

```bash
rm ~/.local/bin/ccr
rm ~/.claude/hooks/ccr-rewrite.sh
rm -rf ~/.local/share/ccr          # optional: cached data + analytics
# Remove CCR entries from ~/.claude/settings.json
```

---

## Contributing

Open an issue or PR on [GitHub](https://github.com/AssafWoo/homebrew-ccr). To add a handler: implement the `Handler` trait and register it in `ccr/src/handlers/mod.rs` — see `git.rs` as a template.

---

## License

MIT — see [LICENSE](LICENSE).
