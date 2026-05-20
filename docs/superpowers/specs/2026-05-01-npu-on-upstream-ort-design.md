# NPU support on upstream ORT-based panda — design

**Date:** 2026-05-01
**Status:** Draft for review
**Branch (target):** `feat/npu-on-ort` cut from `upstream/main` @ `9546fb6` (v1.3.9)

## Problem

Upstream landed PR #5 ("ort-migration") which:

- Replaces `fastembed` with direct `ort` 2.0.0-rc.12 for ONNX inference.
- Deletes `ccr-core/src/ov_embed.rs` (the fork's OpenVINO/NPU bypass, ~400 lines).
- Rewrites `ccr-core/src/summarizer.rs` around a new `MiniLmEmbedder` struct.
- Adds an embedding daemon (`panda daemon start/stop/status`) over a Unix socket.

The fork's NPU acceleration on Intel Meteor Lake (NPU 3720) is built on the deleted code. After this work, `main` should be back on upstream and NPU acceleration restored as a small, opt-in graft on top of the new ORT-based code.

## Non-goals

- Restoring the 9 opt-in embedding models (Snowflake, Jina, Nomic, BGE family). Separate follow-up.
- Restoring `embed-bench` and `bench_summarize`. Separate follow-up.
- A raw-OpenVINO bypass alongside ORT (the deleted `ov_embed.rs` design). Only revisited if the ORT OpenVINO EP empirically fails on a model we care about.
- GPU support. The toggle is `cpu`/`npu`/`auto`; GPU is a one-line extension if/when needed.
- Restoring `install.sh`. NPU users install the OpenVINO runtime once out-of-band.

## Approach (chosen — "minimal-conflict rebase + thin NPU graft")

1. Reset the fork's `main` to `upstream/main`. Preserve the fork's pre-merge custom commits on `archive/pre-upstream-merge` for later cherry-picking.
2. Cut `feat/npu-on-ort` from the new `main` in an isolated worktree at `.worktrees/npu-on-ort`.
3. Add NPU support as a small patch series on the new code, using `ort`'s built-in OpenVINO execution provider (`ort` feature `openvino`) — no separate inference engine.

Rejected alternatives:
- **Hand-merge `upstream/main` into the fork's existing `main`.** Conflict surface ≈ 700 lines in `summarizer.rs` plus deleted files; effectively a rewrite, high risk.
- **Restore raw OpenVINO bypass alongside ORT.** Two embedder code paths to maintain through future upstream merges; only justified if the ORT EP path fails empirically.

## Architecture

```
panda CLI (hook/run/filter)
  │
  ├── embed_and_normalize(texts)
  │     ├── 1st: try daemon_embed via /run/user/$UID/panda/embed.sock  ← daemon owns NPU session
  │     └── fallback: in-process MiniLmEmbedder
  │
  └── panda daemon start
        └── daemon_main → preload_model → MiniLmEmbedder::new(name)
                                                          │
                                                          ▼
                                       ort::Session::builder()
                                         .with_execution_providers([
                                            (cfg) OpenVINO::default()
                                                  .with_device_type("NPU").build(),
                                            CPU::default().build(),  // always last
                                         ])
```

Single integration point: the EP list passed to `Session::builder().with_execution_providers(...)` inside `MiniLmEmbedder::new`. CPU is always the final EP so ORT's per-op fallthrough handles ops the OpenVINO EP can't compile.

## File-level changes

### `ccr-core/Cargo.toml`

Add an opt-in feature:

```toml
[features]
default = []
openvino = ["ort/openvino"]
```

No change to default deps. NPU users build with `cargo build --release --features openvino`.

### `ccr-core/src/config.rs`

Restore one field on `GlobalConfig`:

```rust
#[serde(default = "default_execution_provider")]
pub execution_provider: String,  // "auto" | "cpu" | "npu"
```

`default_execution_provider() -> "auto"`. `"auto"` resolves at runtime to `"npu"` if the `openvino` feature is compiled in, else `"cpu"`.

### `ccr-core/src/summarizer.rs`

Three additions, each mirroring an existing pattern:

- `static EXECUTION_PROVIDER: OnceCell<String>` and `pub fn set_execution_provider(s: &str)` — same shape as `set_model_name` / `set_ort_threads`.
- A private `ep_choice() -> &'static str` resolver that reads `EXECUTION_PROVIDER`, applies the `PANDA_NPU` env override, validates the value (unknown → fall back to `"auto"` with a one-time warning), and resolves `"auto"`.
- In `MiniLmEmbedder::new`, build the EP list conditionally:

  ```rust
  let mut eps: Vec<ort::ep::ExecutionProviderDispatch> = Vec::new();
  #[cfg(feature = "openvino")]
  if matches!(ep_choice(), "npu") {
      eps.push(ort::ep::OpenVINO::default()
          .with_device_type("NPU")
          .build().into());
  }
  eps.push(ort::ep::CPU::default().with_arena_allocator(false).build().into());
  builder = builder.with_execution_providers(eps).map_err(ort_err)?;
  ```

  On EP-list failure, retry once with `[CPU]` only (unless `PANDA_NPU_STRICT=1`). The retry must rebuild the `Session::builder()` from scratch because ORT builders are consumed by `with_execution_providers`.

### `ccr/src/cmd/daemon.rs`

In `daemon_main`, next to the existing `set_model_name` / `set_ort_threads` calls:

```rust
panda_core::summarizer::set_execution_provider(&config.global.execution_provider);
```

The daemon now preloads an NPU-compiled session once on start. The OpenVINO compile cost (~3-10s on first run, cached on disk by OpenVINO) is paid by the daemon, not by foreground hooks.

### `ccr/src/main.rs` (and any non-daemon init paths)

Same `set_execution_provider` call after config load, so the in-process fallback path also honours config.

### `README.md`

Short "NPU support (opt-in)" section: `cargo build --release --features openvino`, requires `libopenvino_c.so` discoverable at runtime, set `execution_provider = "npu"` in `panda.toml` or `PANDA_NPU=npu`.

## Data flow

**Cold-start (daemon, NPU):** `panda daemon start` → double-fork → flock pid → load config → `set_*` → `preload_model` → `MiniLmEmbedder::new` → ORT loads `libopenvino_c.so`, OpenVINO compiles MiniLM ONNX for NPU 3720 (~3-10s, disk-cached) → bind socket → accept loop. Subsequent `daemon_embed` calls reuse the warm NPU session.

**Cold-start (in-process, NPU):** Same flow inside the foreground `panda` process. Compile cost paid on the first hook invocation that reaches the BERT stage. Subsequent processes hit the OpenVINO disk cache and warm up in <1s.

## Error handling

| Failure | Where | Behaviour |
|---|---|---|
| Built without `--features openvino`, config says `"npu"` | Compile time — feature-gated block absent; `ep_choice()` returns `"cpu"` | One-time `eprintln!`: `[panda] execution_provider=npu but binary built without openvino feature; using CPU` |
| Built with feature, `libopenvino_c.so` missing at runtime | `Session::builder().with_execution_providers(...)` returns `ort::Error` | Log `[panda] OpenVINO EP unavailable: <err>; falling back to CPU`, rebuild builder, retry with `[CPU]` |
| OpenVINO compiles but NPU device absent / busy | `with_execution_providers` or first `session.run()` | Same fallback path; ORT per-op fallback hides most cases |
| `PANDA_NPU_STRICT=1` set | Disables silent fallbacks above | Surface the error, fail loud |
| Daemon crashes mid-embed | `daemon_embed()` returns `None` | `embed_and_normalize` already falls through to `embed_direct`. Next hook re-runs `try_auto_start`; daemon restarts fresh |

**Design choices flagged:**

- CPU is always the last EP. Free graceful degradation per-op without extra code.
- No multi-device retry logic. `"auto"` means "NPU if compiled in, else CPU" — not "try GPU then NPU then CPU."

**Observability:** one `eprintln!` on session creation: `[panda] embedder: <model> on <NPU|CPU>`. Visible in daemon log / journal, and in foreground runs.

## Testing

### Unit tests (`ccr-core/src/summarizer.rs`)

1. `ep_choice_resolves_auto_to_cpu_without_feature` — feature off → `"cpu"` regardless of config.
2. `ep_choice_honours_panda_npu_env` — `PANDA_NPU=cpu` overrides `"npu"` and vice versa.
3. `ep_choice_validates_unknown_string` — unknown value → `"auto"` with warning, never panics.

These run on every CI machine; they don't touch ORT.

### Feature-gated integration test (`ccr-core/tests/npu_smoke.rs`, `#[cfg(feature = "openvino")]`)

4. `npu_smoke_embeds_three_strings` — `MiniLmEmbedder` with `"npu"`, embed `["error", "warning", "ok"]`, assert shape 3×384 and L2-normalised. Skipped silently unless `OPENVINO_NPU_AVAILABLE=1`.
5. `npu_falls_back_to_cpu_when_libopenvino_missing` — hide OV runtime, expect embedder construction to succeed via CPU-only retry.

### Manual verification checklist

- [ ] `cargo build --release` (no feature) builds clean; `panda run ls` works.
- [ ] `cargo build --release --features openvino` builds clean; `panda run ls` works.
- [ ] `cargo test -p panda-core` passes.
- [ ] `cargo test -p panda-core --features openvino` passes (or skips NPU tests cleanly without `OPENVINO_NPU_AVAILABLE`).
- [ ] `panda daemon start` with `execution_provider="npu"` — daemon comes up, log shows `embedder: AllMiniLML6V2 on NPU`, no errors.
- [ ] First-call latency bounded (<15s); subsequent calls <50ms for ~10 lines.
- [ ] Restart daemon with NPU disabled → log shows `on CPU`. Toggle confirmed.
- [ ] `PANDA_NPU_STRICT=1 panda daemon start` with OV runtime hidden → fails loud.
- [ ] Real workload (`cargo build` on a medium repo, `git status` here) — output reasonable.
- [ ] `panda gain` works, savings comparable to pre-NPU baseline.

### Regression risk areas

- The `with_execution_providers` retry-on-failure path (ORT builders are consumed; rebuild from scratch).
- Daemon `preload_model` now does ~5s extra on NPU; the `daemon start` parent-sleep heuristic of 200ms may print "started" before the daemon is serving. Acceptable (clients retry via `try_auto_start`), confirm end-to-end.

## Out of scope (follow-ups)

1. Re-add the 9 opt-in embedding models (Snowflake-M-v2, Jina-code, Nomic, BGE family) by extending `model_registry()`.
2. Re-add `embed-bench` to score the new models on the in-tree QA fixtures.
3. Promote a different default model if the bench shows it justifies a breaking change.
4. Optional GPU support — one extra arm in `ep_choice()` and one extra `OpenVINO::with_device_type("GPU")` build.
