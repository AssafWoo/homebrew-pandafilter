<p align="center">
  <img src="assets/logo.png" alt="PandaFilter" width="160" />
</p>

<h1 align="center">PandaFilter</h1>

<p align="center"><strong>Cut your AI agent's token bill by 60–95% — transparently, locally, automatically.</strong></p>

<p align="center">
  Built by Assaf Petronio — building PandaFilter in public, with the open-source community.
</p>

<p align="center">
  <a href="https://x.com/AssafPetronio">@AssafPetronio</a>&nbsp; • &nbsp;<a href="https://github.com/AssafWoo">github.com/AssafWoo</a>
</p>

<p align="center">
  <a href="https://github.com/AssafWoo/PandaFilter/stargazers">
    <img src="https://img.shields.io/github/stars/AssafWoo/PandaFilter?style=for-the-badge&logo=github&logoColor=white&label=Star%20the%20panda%20%F0%9F%90%BC%E2%AD%90&labelColor=4b4b4b&color=7c3aed" alt="Star PandaFilter on GitHub">
  </a>
</p>

<p align="center">
  <a href="https://discord.com/invite/FFQC3bxYQ"><img src="https://img.shields.io/badge/Discord-Join-5865F2?style=for-the-badge&logo=discord&logoColor=white" alt="Discord"></a>
  &nbsp;
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-yellow.svg?style=for-the-badge" alt="License: MIT"></a>
  &nbsp;
  <a href="https://github.com/AssafWoo/PandaFilter/releases/latest"><img src="https://img.shields.io/github/v/release/AssafWoo/PandaFilter?style=for-the-badge" alt="Latest Release"></a>
</p>

---

## Quick start

```bash
brew tap AssafWoo/pandafilter
brew install pandafilter
```

**Linux / any platform:**

```bash
curl -fsSL https://raw.githubusercontent.com/AssafWoo/homebrew-pandafilter/main/install.sh | bash
```

> **First run:** PandaFilter downloads the BERT model (~90 MB, `all-MiniLM-L6-v2`) from HuggingFace and caches it at `~/.cache/huggingface/`. Subsequent runs are instant.

---

## Why PandaFilter?

AI coding agents are expensive to run — not because of what you ask them, but because of what they read back. Every `cargo build`, `git log`, or `npm install` dumps thousands of tokens of noise into the context window. I built PandaFilter to fix that transparently: it sits between your agent and your shell, compresses the output, and hands back only what the model needs. No config changes, no workflow changes — just less waste.

---

## What it does

- Hooks into Claude Code, Cursor, Gemini CLI, Cline, and VS Code Copilot automatically after `panda init`.
- Filters build logs, test noise, and progress bars before the model ever sees them.
- Uses BERT embeddings to match unknown commands to the closest handler — nothing falls through silently.
- Caches repeated commands (git, kubectl, docker, terraform) so the model isn't re-reading stale output.
- Runs 100% locally — no data leaves your machine.

---

## Token savings

Numbers from `ccr/tests/handler_benchmarks.rs`. Run `cargo test -p panda benchmark -- --nocapture` to reproduce, or `panda gain` to see your own live data.

| Operation | Without PandaFilter | With PandaFilter | Savings |
|-----------|------------:|---------:|:-------:|
| `pip install` | 1,787 | 9 | **−99%** |
| `uv sync` | 1,574 | 15 | **−99%** |
| `playwright test` | 1,367 | 19 | **−99%** |
| `docker build` | 1,801 | 24 | **−99%** |
| `swift build` | 1,218 | 9 | **−99%** |
| `dotnet build` | 438 | 3 | **−99%** |
| `cmake` | 850 | 5 | **−99%** |
| `gradle build` | 803 | 17 | **−98%** |
| `git clone` | 139 | 8 | **−94%** |
| `bazel build` | 150 | 12 | **−92%** |
| `go test` | 4,507 | 148 | **−97%** |
| `pytest` | 3,818 | 162 | **−96%** |
| `terraform plan` | 3,926 | 163 | **−96%** |
| `npm install` | 648 | 25 | **−96%** |
| `ember build` | 3,377 | 139 | **−96%** |
| `cargo build` | 1,923 | 93 | **−95%** |
| `cargo test` | 2,782 | 174 | **−94%** |
| `next build` | 549 | 53 | **−90%** |
| `cargo clippy` | 786 | 93 | **−88%** |
| `git merge` | 164 | 5 | **−97%** |
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
| **Total** | **81,882** | **16,062** | **−80%** |

---

## Compared to doing nothing

| Scenario | Without PandaFilter | With PandaFilter |
|---|---|---|
| `cargo build` (errors) | 1,923 tokens | 93 tokens |
| `pytest` (all passing) | 3,818 tokens | 162 tokens |
| `npm install` | 648 tokens | 25 tokens |
| **Typical session** | **71k tokens** | **15k tokens** |

---

## Commands

**`panda init`** — wire PandaFilter into your agent's hooks:

```bash
panda init                              # Claude Code (default)
panda init --agent cursor               # Cursor
panda init --agent gemini               # Gemini CLI
panda init --agent cline                # Cline (.clinerules in project dir)
panda init --agent copilot              # VS Code Copilot
panda init --uninstall                  # remove (add --agent <x> for specific agent)
```

**`panda gain`** — see your token savings:

```bash
panda gain                    # overall summary
panda gain --breakdown        # per-command table
panda gain --history          # last 14 days
panda gain --insight          # categorized savings + top saves
```

**`panda doctor`** — diagnose the full installation in one command (run this first when something seems off).

**Other commands:**

```bash
panda verify                            # check hook integrity for all installed agents
panda discover                          # scan Claude history for commands that ran without PandaFilter
panda noise                             # show learned noise patterns; --reset to clear
panda expand ZI_3                       # print original lines from a collapsed block
panda read-file src/main.rs --level auto  # apply read-level filter and print savings
panda compress --scan-session           # compress current conversation context
panda filter --command cargo            # filter stdin as if it were cargo output
panda run git status                    # run through PandaFilter handler manually
panda proxy git status                  # run raw (no filtering), record analytics baseline
```

---

<details>
<summary><strong>Handlers (59 handlers)</strong></summary>

59 handlers (70+ command aliases) in `ccr/src/handlers/`. Lookup cascade:

1. **User filters** — `.panda/filters.toml` or `~/.config/panda/filters.toml`
2. **Exact match** — direct command name
3. **Static alias table** — versioned binaries, wrappers, common aliases
4. **BERT routing** — unknown commands matched by embedding similarity

| Handler | Keys | Key behavior |
|---------|------|-------------|
| **cargo** | `cargo` | `build`/`clippy`: errors (capped at 15) + warning count. `test`: failures + summary. `nextest run`: FAIL lines + Summary. |
| **git** | `git` | `status`: counts. `log`: `--oneline`, cap 50 with total. `diff`: 2 context lines, 200-line cap. `clone`/`merge`/`checkout`/`rebase`: compressed success or full conflict output. |
| **go** | `go` | `test`: NDJSON streaming, FAIL blocks + summary. `build`: errors only. |
| **ember** | `ember` | `build`: errors + summary; drops fingerprint/asset spam. `test`: failures + summary. `serve`: serving URL only. |
| **tsc** | `tsc` | Errors grouped by file; deduplicates repeated TS codes. `Build OK` on clean. Injects `--noEmit`. |
| **vitest** | `vitest` | FAIL blocks + summary; drops `✓` lines. |
| **jest** | `jest`, `bun`, `deno` | `●` failure blocks + summary; drops `PASS` lines. |
| **pytest** | `pytest` | FAILED node IDs + AssertionError + short summary. Injects `--tb=short`. |
| **rspec** | `rspec` | Injects `--format json`; example-level failures with message + location. |
| **rubocop** | `rubocop` | Injects `--format json`; offenses grouped by severity, capped. |
| **rake** | `rake`, `bundle` | Failure/error blocks + summary; drops passing test lines. |
| **mypy** | `mypy` | Errors grouped by file, capped at 10 per file. Injects `--no-color`. |
| **ruff** | `ruff` | Violations grouped by error code. `format`: summary line only. |
| **uv** | `uv`, `uvx` | Strips Downloading/Fetching/Preparing noise; keeps errors + summary. |
| **pip** | `pip`, `poetry`, `pdm`, `conda` | `install`: `[complete — N packages]` or already-satisfied short-circuit. |
| **python** | `python` | Traceback: keep block + final error. Detects and compresses tabular/CSV, pandas DataFrames, Word (.docx), Excel (.xlsx), and PowerPoint (.pptx) output. Long output: BERT. |
| **eslint** | `eslint` | Errors grouped by file, caps at 20 + `[+N more]`. |
| **next** | `next` | `build`: route table collapsed. `dev`: errors + ready line. |
| **playwright** | `playwright` | Failing test names + error messages; passing tests dropped. Injects `--reporter=list`. |
| **prettier** | `prettier` | `--check`: files needing formatting + count. |
| **vite** | `vite` | Asset chunk table collapsed, HMR deduplication. |
| **webpack** | `webpack` | Module resolution graph dropped; keeps assets, errors, build result. |
| **turbo** | `turbo` | Inner task output stripped; cache hit/miss per package + final summary. |
| **nx** | `nx`, `npx nx` | Passing tasks collapsed to `[N tasks passed]`; failing task output kept. |
| **stylelint** | `stylelint` | Issues grouped by file, caps at 40 + `[+N more]`. |
| **biome** | `biome` | Code context snippets stripped; keeps file:line, rule, message. |
| **kubectl** | `kubectl`, `k` | `get pods`: aggregates to `[N pods, all running]` or problem-pods table with counts. Smart column selection, log anomaly scoring, describe key sections. `events`: warning-only, capped at 20. |
| **terraform** | `terraform`, `tofu` | `plan`: `+`/`-`/`~` + summary. `validate`: short-circuits on success. `output`: compact key=value. `state list`: capped at 50. |
| **aws** | `aws`, `gcloud`, `az` | Resource extraction; `--output json` injected for read-only actions. |
| **gh** | `gh` | Compact tables for list commands; strips HTML from `pr view`. |
| **helm** | `helm` | `list`: compact table. `status`/`diff`/`template`: structured. |
| **docker** | `docker` | `logs`: ANSI strip + BERT. `ps`/`images`: formatted tables + total size. `build`: errors + final image ID. |
| **make** | `make`, `ninja` | "Nothing to be done" short-circuit; keeps errors. Injects `--no-print-directory`. |
| **golangci-lint** | `golangci-lint` | Diagnostics grouped by file; runner noise dropped. Detects v1 text and v2 JSON formats. |
| **prisma** | `prisma` | `generate`/`migrate`/`db push` structured summaries. |
| **mvn** | `mvn` | Drops `[INFO]` noise; keeps errors + reactor summary. |
| **gradle** | `gradle` | UP-TO-DATE tasks collapsed; FAILED tasks and errors kept. |
| **npm/yarn** | `npm`, `yarn` | `install`: package count; strips boilerplate. |
| **pnpm** | `pnpm` | `install`: summary; drops progress bars. |
| **brew** | `brew` | `install`/`update`: status lines + Caveats. |
| **curl** | `curl` | JSON → type schema. Non-JSON: cap 30 lines. |
| **grep / rg** | `grep`, `rg` | Compact paths, per-file 100-match cap, line numbers preserved, `[N matches in M files]` summary. Injects `--no-heading --with-filename`. Match-centered line truncation. |
| **find** | `find` | Groups by directory, caps at 50. Injects `-maxdepth 8` if unset. |
| **journalctl** | `journalctl` | Injects `--no-pager -n 200`. BERT anomaly scoring. |
| **psql** | `psql` | Strips borders, caps at 20 rows. |
| **tree** | `tree` | Auto-injects `-I "node_modules\|.git\|target\|..."`. |
| **diff** | `diff` | `+`/`-`/`@@` + 2 context lines, max 5 hunks. |
| **jq** | `jq` | Array: schema of first element + `[N items]`. |
| **env** | `env` | Categorized sections; sensitive values redacted. |
| **ls** | `ls` | Drops noise dirs; top-3 extension summary. |
| **log** | `log` | Timestamp/UUID normalization, dedup `[×N]`, error summary block. |
| **rsync** | `rsync` | Drops per-file transfer progress lines (`to-chk=`, `MB/s`); keeps file list and final summary. |
| **ffmpeg** | `ffmpeg`, `ffprobe` | Drops `frame=` and `size=` real-time progress lines; keeps input/output codec info and final size line. |
| **wget** | `wget` | Injects `--quiet` if no verbosity flag set. |
| **swift** | `swift`, `swift-build`, `swift-test` | `build`: errors/warnings + `Build complete`. `test`: failures + summary. `package resolve`: strips progress. |
| **dotnet** | `dotnet`, `dotnet-cli` | `build`: errors grouped by CS code + summary. Short-circuits on clean build. `test`: failures + summary. `restore`: package count. |
| **cmake** | `cmake`, `cmake3` | `configure`: errors + final written-to line. `--build`: errors + `[N targets built]`. Auto-detects mode from args/output. |
| **bazel** | `bazel`, `bazelisk`, `bzl` | `build`: errors + completion summary `[N actions, build OK (Xs)]`. `test`: failures + `[N passed, N failed]`. `query`: cap at 30 targets. |

</details>

<details>
<summary><strong>Pipeline architecture</strong></summary>

```
0. Hard input ceiling (200k chars — truncates before any stage)
1. Strip ANSI codes
2. Normalize whitespace
3. Global regex pre-filter (progress bars, spinners, download lines, decorators)
4. NDJSON streaming compaction (go test -json, jest JSON reporter)
5. Command-specific pattern filter
6. If over summarize_threshold_lines:
   6a. BERT noise pre-filter
   6b. Entropy-adaptive BERT summarization (up to 7 passes)
7. Hard output cap (50k chars)
```

Outputs under 15 tokens skip the pipeline entirely. Step 6b falls back to head+tail if BERT is unavailable.

**Pre-run cache** (fires before execution): git, kubectl, docker, and terraform commands are hashed against live state. A hit skips execution entirely and returns the cached output with a `[PC: cached from Xm ago]` marker.

</details>

<details>
<summary><strong>Configuration</strong></summary>

Config loaded from: `./panda.toml` → `~/.config/panda/config.toml` → embedded default.

```toml
[global]
summarize_threshold_lines = 50
head_lines = 30
tail_lines = 30
strip_ansi = true
normalize_whitespace = true
deduplicate_lines = true
input_char_ceiling = 200000
output_char_cap = 50000
# cost_per_million_tokens = 15.0

[tee]
enabled = true
mode = "aggressive"   # "aggressive" | "always" | "never"

[read]
mode = "auto"   # "passthrough" | "auto" | "strip" | "aggressive"

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

Pattern actions: `Remove`, `Collapse`, `ReplaceWith = "text"`, `TruncateLinesAt = N`, `HeadLines = N`, `TailLines = N`, `MatchOutput = "msg"`, `OnEmpty = "msg"`.

Pricing uses `cost_per_million_tokens` from `panda.toml` if set, otherwise `ANTHROPIC_MODEL` env var (Opus 4.6: $15, Sonnet 4.6: $3, Haiku 4.5: $0.80), otherwise $3.00.

</details>

<details>
<summary><strong>User-defined filters</strong></summary>

Place `filters.toml` at `.panda/filters.toml` (project-local) or `~/.config/panda/filters.toml` (global). Project-local overrides global for the same key. Runs before any built-in handler.

```toml
[commands.myapp]
patterns = [
  { regex = "^DEBUG:",            action = "Remove" },
  { regex = "^\\S+\\.ts\\(",     action = "TruncateLinesAt", max_chars = 120 },
]
on_empty = "(no relevant output)"

[commands.myapp.match_output]
pattern        = "Server started"
message        = "ok — server ready"
unless_pattern = "error"
```

</details>

<details>
<summary><strong>Session intelligence</strong></summary>

State tracked via `PANDA_SESSION_ID=$PPID`, stored at `~/.local/share/panda/sessions/<id>.json`.

- **Result cache** — post-pipeline bytes frozen per input hash; returned identically on repeat calls to prevent prompt cache busts.
- **Semantic delta** — repeated commands emit only new/changed lines: `[Δ from turn N: +M new, K repeated — ~T tokens saved]`.
- **Cross-turn dedup** — identical outputs (cosine > 0.92) collapse to `[same output as turn 4 (3m ago) — 1.2k tokens saved]`.
- **Elastic context** — pipeline pressure scales with session size. At >80% pressure: `[⚠ context near full — run panda compress --scan-session]`.
- **Intent-aware query** — reads the agent's last message from the live session JSONL and uses it as the BERT query.

</details>

<details>
<summary><strong>Hook architecture</strong></summary>

| Agent | Config | Script |
|-------|--------|--------|
| Claude Code | `~/.claude/settings.json` | `~/.claude/hooks/panda-rewrite.sh` |
| Cursor | `~/.cursor/hooks.json` | `~/.cursor/hooks/panda-rewrite.sh` |
| Gemini CLI | `~/.gemini/hooks.json` | `~/.gemini/panda-rewrite.sh` |
| Cline | `.clinerules` (project dir) | — (rules-based) |
| VS Code Copilot | `.github/hooks/panda-rewrite.json` | `.github/hooks/panda-rewrite.sh` |

All agents share the same binary and compression pipeline.

**PreToolUse:** known handler → rewrites to `panda run <cmd>`; unknown → no-op; already wrapped → no double-wrap; compound commands → each segment rewritten independently.

**PostToolUse:** Bash → full pipeline; Read → BERT + session dedup; Glob → grouped by directory; Grep → compact paths.

**Hook integrity:** `panda init` writes SHA-256 baselines (chmod 0o444). PandaFilter verifies at every invocation and exits 1 with a warning if tampered. `panda verify` checks all installed agents.

</details>

<details>
<summary><strong>Crate overview</strong></summary>

```
ccr/        CLI binary (panda) — handlers, hooks, session state, commands
ccr-core/   Core library (no I/O) — pipeline, BERT summarizer, config, analytics
ccr-sdk/    Conversation compression — tiered compressor, deduplicator, Ollama
ccr-eval/   Evaluation suite — fixtures against Claude API
config/     Embedded default filter patterns
```

</details>

<details>
<summary><strong>Uninstall</strong></summary>

```bash
panda init --uninstall                        # Claude Code
panda init --agent cursor --uninstall         # Cursor
panda init --agent gemini --uninstall         # Gemini CLI
panda init --agent cline --uninstall          # Cline
panda init --agent copilot --uninstall        # VS Code Copilot

brew uninstall pandafilter && brew untap AssafWoo/pandafilter   # Homebrew
# or: cargo uninstall panda

rm -rf ~/.local/share/panda                   # analytics + sessions
rm -rf ~/.cache/huggingface/hub/models--sentence-transformers--all-MiniLM-L6-v2
```

</details>

---

## FAQ

**Does PandaFilter degrade Claude's output quality?**
No. PandaFilter only removes noise — build logs, module graphs, passing test lines, progress bars. Errors, file paths, and summaries are always kept.

**What about tools PandaFilter doesn't know?**
BERT semantic routing matches against all known handlers. If confidence is high enough the closest handler applies; otherwise output passes through unchanged. PandaFilter never silently drops output.

**How do I verify it's working?**
`panda gain` after a session. To inspect what the model received from a specific command: `panda proxy git log --oneline -20`.

**Does PandaFilter send any data outside my machine?**
Never. All processing is fully local. BERT runs on-device.

---

## Contributing

Open an issue or PR on [GitHub](https://github.com/AssafWoo/PandaFilter). To add a handler: implement the `Handler` trait and register it in `ccr/src/handlers/mod.rs` — see `git.rs` as a template.

---

## License

MIT — see [LICENSE](LICENSE).
