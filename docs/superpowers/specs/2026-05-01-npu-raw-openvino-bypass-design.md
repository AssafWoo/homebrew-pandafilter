# NPU support via raw OpenVINO bypass — design

**Date:** 2026-05-01
**Status:** Draft for review
**Branch (target):** continues on `feat/npu-on-ort` (already cut from upstream/main @ 9546fb6 v1.3.9)
**Supersedes:** the runtime semantics of `2026-05-01-npu-on-upstream-ort-design.md`. The earlier spec's branch hygiene, config field, and resolver remain in place; only the embedder construction changes.

## Problem

The earlier "NPU on upstream ORT" approach wired up `ort` 2.0.0-rc.12's OpenVINO execution provider behind a `--features openvino` flag. Empirical verification on the target hardware (Intel Meteor Lake NPU 3720) showed that path cannot engage:

- ORT's `download-binaries` mode bundles its own `libonnxruntime`, which collides with the OpenVINO provider plugins shipped in the user's existing distribution at `~/.local/share/ccr/onnxruntime/` — `std::bad_alloc` SIGABRT during inference.
- Switching to `ort/load-dynamic` triggers a compile error inside ort itself (`SessionOptionsAppendExecutionProvider_VitisAI` field missing on the OrtApi struct).
- `ort/copy-dylibs` makes no difference; the conflict is at runtime, not deployment.
- `2.0.0-rc.12` is the latest published ort; no newer release exists, no older RC has the missing fix back-ported.
- The current branch's session creation silently succeeds without engaging the EP, then logs `[panda] embedder: ... on NPU` while CPU runs — the false-NPU bug.

The fork shipped a working NPU path before the upstream merge: `ccr-core/src/ov_embed.rs` (preserved on `archive/pre-upstream-merge`) used the OpenVINO C API directly via the `openvino-rs` crates, with disk cache, async infer-request pool, and a process-wide degradation flag. It worked on this exact hardware.

## Approach

Restore the raw-OpenVINO bypass on top of the Approach A scaffolding. The current branch's resolver, config field, daemon wiring, and observability hook are kept verbatim. The `--features openvino` flag is repointed: instead of activating ORT's OpenVINO EP, it pulls in the `openvino-rs` crates and wires a direct `OvEmbedder` ahead of the ORT-CPU path.

Concretely:

1. The four `#[cfg(feature = "openvino")]` blocks in `summarizer.rs`'s `MiniLmEmbedder::new` revert to upstream's flat `[CPU]` builder. The Approach A "EP list with CPU fallback retry" code is deleted as dead weight; nothing constructs the ORT OpenVINO EP anymore.
2. `ccr-core/src/ov_embed.rs` is restored from `archive/pre-upstream-merge`, trimmed to the two models in upstream's current registry (`AllMiniLML6V2`, `AllMiniLML12V2`).
3. `summarizer.rs` gains a `#[cfg(feature = "openvino")] static OV_EMBEDDER: OnceCell<Option<ov_embed::OvEmbedder>>` plus a `get_ov_embedder()` accessor.
4. `embed_and_normalize` checks `current_ep() == "npu"` and `get_ov_embedder().is_some()` *before* falling through to ORT-CPU.
5. The daemon's `daemon_main` eagerly preloads the OV embedder when configured, so the multi-second NPU compile happens once at daemon start.

## Non-goals

- Re-adding the 9 opt-in models from `archive/pre-upstream-merge`. Separate follow-up; this spec ships with the same 2-model registry as upstream.
- Re-adding `embed-bench` and `bench_summarize`. Separate follow-up.
- Reactivating the ORT OpenVINO EP path for any future ort release. If/when ort fixes the rc.12 issues, that's a one-line Cargo.toml flip; not in scope here.
- GPU acceleration. The OpenVINO API supports it (`DeviceType::GPU`), but this spec hard-codes `DeviceType::NPU`.
- Multi-process NPU coordination. The OpenVINO blob cache at `~/.cache/panda/openvino` lets parallel `panda` invocations share compile work, but no explicit fan-out/fan-in is added.
- Replacing `MiniLmEmbedder` with a trait/enum hierarchy. Two embedder structs coexist as static singletons; dispatch is a single `if let Some(...)` in `embed_and_normalize`.

## Architecture

```
panda CLI (hook/run/filter)
  │
  ├── embed_and_normalize(texts)
  │     1. try daemon_embed via Unix socket  ── owns warm NPU session
  │     2. (feature gated) if current_ep()=="npu" and get_ov_embedder().is_some()
  │            → OvEmbedder::embed (raw OpenVINO C API, NPU device)
  │     3. fall through: embed_direct → MiniLmEmbedder (ort CPU only)
  │
  └── panda daemon start
        └── daemon_main →
              set_model_name / set_ort_threads / set_execution_provider
              (if feature on and ep == "npu") preload_ov_embedder()
                                              ↓
                                              OvEmbedder::try_new
                                                ├ ov_lib_path() finds
                                                │   ~/.local/share/ccr/onnxruntime/libopenvino_c.so
                                                │   (or OPENVINO_LIB_PATH override)
                                                ├ Core::new + cache_dir + THROUGHPUT hint
                                                ├ resolve_model_files (hf-hub, reused from upstream)
                                                ├ static-shape reshape [1, seq_len]
                                                ├ core.compile_model(NPU)   ← cached at ~/.cache/panda/openvino
                                                └ pre-allocate InferRequest pool (size = OPTIMAL_NUMBER_OF_INFER_REQUESTS, clamp 1..=8)
              preload_model() (CPU embedder, cheap fallback insurance)
              bind socket, accept loop
```

Single integration point at runtime: `embed_and_normalize`'s ordered cascade.
Single integration point at config-time: `current_ep()` (unchanged from Approach A).

## File-level changes

### `ccr-core/Cargo.toml`

Replace the `openvino = ["ort/openvino"]` feature wiring with `openvino-rs` deps:

```toml
[dependencies]
# ... existing unchanged ...
openvino = { git = "https://github.com/intel/openvino-rs", rev = "e25f1f848edc",
             features = ["runtime-linking"], optional = true }
openvino-sys = { git = "https://github.com/intel/openvino-rs", rev = "e25f1f848edc",
                 features = ["runtime-linking"], optional = true }

[features]
default = []
openvino = ["dep:openvino", "dep:openvino-sys"]
```

The git rev matches `archive/pre-upstream-merge` — proven working on this hardware. `runtime-linking` means no link-time dependency on `libopenvino_c.so`; the binary builds on CI machines without OpenVINO installed.

### `ccr/Cargo.toml`

`openvino = ["panda-core/openvino"]` forwarding feature stays unchanged.

### `ccr-core/src/lib.rs`

Add module declaration (next to existing module list):

```rust
#[cfg(feature = "openvino")]
pub(crate) mod ov_embed;
```

### `ccr-core/src/ov_embed.rs` (new)

Near-verbatim port of `archive/pre-upstream-merge:ccr-core/src/ov_embed.rs`. Public surface:

- `pub fn is_degraded() -> bool`
- `pub fn mark_degraded(reason: &str)`
- `pub fn ov_lib_path() -> Option<PathBuf>`
- `pub fn model_seq_len(name: &str) -> Option<usize>` — replaces the old `model_onnx_info`. Returns `Some(128)` for `AllMiniLML6V2`/`AllMiniLML12V2`, `None` otherwise.
- `pub struct OvEmbedder { ... }` with `pub fn try_new(onnx_path, tokenizer_path, seq_len) -> Result<Self>` and `pub fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>`.

Drops `find_fastembed_onnx` — replaced by reusing `summarizer::resolve_model_files()` (which already returns `(model_path, tokenizer_path)` via `hf_hub`). `model_onnx_info`'s 16-model registry shrinks to a `model_seq_len` lookup with two arms; the HF repo strings are no longer needed because `hf_hub` is upstream's job.

Keeps the proven NPU bits intact:
- `~/.cache/panda/openvino` blob cache directory.
- `PERFORMANCE_HINT=THROUGHPUT` and the `PANDA_NPU_PRECISION` env override.
- Static shape reshape `[1, seq_len]`.
- `Mutex<Vec<InferRequest>>` async pool, sized by `OPTIMAL_NUMBER_OF_INFER_REQUESTS` clamped to `[1, 8]`.
- `DEGRADED: AtomicBool` process-wide flag, `mark_degraded` is idempotent and prints once.

### `ccr-core/src/summarizer.rs`

Four feature-gated additions:

```rust
#[cfg(feature = "openvino")]
static OV_EMBEDDER: OnceCell<Option<crate::ov_embed::OvEmbedder>> = OnceCell::new();

#[cfg(feature = "openvino")]
fn get_ov_embedder() -> Option<&'static crate::ov_embed::OvEmbedder> {
    if crate::ov_embed::is_degraded() { return None; }
    OV_EMBEDDER.get_or_init(|| {
        let name = get_model_name();
        let (onnx, tok) = match resolve_model_files(name) {
            Ok(v) => v,
            Err(e) => { eprintln!("[panda] OV bypass: model fetch failed: {e}"); return None; }
        };
        let seq_len = match crate::ov_embed::model_seq_len(name) {
            Some(s) => s,
            None => { eprintln!("[panda] OV bypass: no seq_len for {name}; CPU fallback"); return None; }
        };
        match crate::ov_embed::OvEmbedder::try_new(&onnx, &tok, seq_len) {
            Ok(e) => {
                eprintln!("[panda] embedder: {} on NPU (raw OpenVINO)", name);
                Some(e)
            }
            Err(err) => { eprintln!("[panda] OV bypass init failed: {err}"); None }
        }
    }).as_ref()
}

#[cfg(feature = "openvino")]
pub fn preload_ov_embedder() -> Option<()> {
    get_ov_embedder().map(|_| ())
}

#[cfg(feature = "openvino")]
pub fn ov_embedder_is_active() -> bool {
    OV_EMBEDDER.get().and_then(|o| o.as_ref()).is_some()
}
```

`embed_and_normalize` adds the dispatch:

```rust
fn embed_and_normalize(texts: Vec<&str>) -> anyhow::Result<Vec<Vec<f32>>> {
    #[cfg(unix)]
    if let Some(embeddings) = crate::embed_client::daemon_embed(&texts, true) {
        return Ok(embeddings);
    }
    #[cfg(feature = "openvino")]
    if current_ep() == "npu" {
        if let Some(ov) = get_ov_embedder() {
            let mut v = ov.embed(&texts)?;
            for e in &mut v { l2_normalize(e); }
            return Ok(v);
        }
    }
    #[cfg(unix)] apply_nice_once();
    embed_direct(texts)
}
```

`embed_direct` (called by daemon worker) gets the same OV check at its top, before reaching `MiniLmEmbedder`.

`MiniLmEmbedder::new` reverts to upstream's flat `[CPU]` builder — the Approach A closure-based EP-list retry is deleted (~50 lines). The "on CPU (ort)" log line moves into the `get_or_init` of `MODEL_CACHE` so it fires only when ORT actually constructs the session.

### `ccr/src/cmd/daemon.rs`

Add the eager preload:

```rust
panda_core::summarizer::set_execution_provider(&config.global.execution_provider);
#[cfg(feature = "openvino")]
if panda_core::summarizer::current_ep() == "npu" {
    let _ = panda_core::summarizer::preload_ov_embedder();
}
if panda_core::summarizer::preload_model().is_err() {
    std::process::exit(1);
}
```

`current_ep()` becomes `pub` (was `pub(crate)`) so the daemon crate can call it.

### `ccr/src/main.rs`

No change beyond Approach A's existing `set_execution_provider` call. The foreground path lazy-initialises OV on first embed via `get_ov_embedder` — there's no benefit to eager init in a short-lived process.

### `README.md`

Rewrite the NPU section. Old text described the ORT EP path; new text describes the raw bypass. Key changes:
- Build command unchanged: `cargo build --release --features openvino`.
- Drop the `libopenvino_c.so` discoverability paragraph; mention `OPENVINO_LIB_PATH` env override and the default search list (`~/.local/share/ccr/onnxruntime/libopenvino_c.so`, then `/usr/lib`, etc.).
- Add a one-liner about the disk cache at `~/.cache/panda/openvino`.
- Mention `PANDA_NPU_STRICT=1` for diagnosing degradation.

## Data flow

**Cold start (daemon, NPU):** `panda daemon start` → fork → flock pid → load config → `set_*` → `preload_ov_embedder()` → `ov_lib_path()` resolves `~/.local/share/ccr/onnxruntime/libopenvino_c.so` → `Core::new` → set `THROUGHPUT` hint + cache dir → `resolve_model_files("AllMiniLML6V2")` returns `(onnx_path, tokenizer.json)` via hf_hub → reshape ONNX inputs to `[1, 128]` → `compile_model(NPU)` (3–10s cold; <500ms warm via cache) → query `OPTIMAL_NUMBER_OF_INFER_REQUESTS` → preallocate 4 `InferRequest`s → eprintln embedder line → bind socket. Subsequent `daemon_embed` calls embed via the warm async pool, parallelising across the NPU's two tiles.

**Cold start (in-process, NPU):** Same flow but lazy. First `panda` invocation that hits the BERT stage pays the compile cost; subsequent processes hit the disk cache.

**Degraded state:** `OvEmbedder::embed` retries once on transient failure, calls `mark_degraded("...")` on second failure. `DEGRADED` flips. `get_ov_embedder` returns `None` for the rest of process lifetime. Next call falls through to `embed_direct` → `MiniLmEmbedder` → CPU. One `eprintln!` line marks the transition.

## Error handling

| Failure | Detection | Behaviour |
|---|---|---|
| Built without `--features openvino`, config says `"npu"` | `current_ep()` returns "cpu" (compile-time gate) | One-time stderr warning. CPU runs. |
| `libopenvino_c.so` missing | `ov_lib_path()` returns `None` → `try_new` errors | Log, cache `None` in `OV_EMBEDDER`, CPU runs. |
| OpenVINO present, NPU device absent | `compile_model(NPU)` errors | Log, cache `None`, CPU runs. |
| NPU runtime failure mid-embed | `OvEmbedder::embed` retry, then `mark_degraded` | One log line, `DEGRADED=true`, all subsequent embeds → CPU. |
| `PANDA_NPU_STRICT=1` set | New env check inside `get_ov_embedder` and on `mark_degraded` | Surface errors as `Err` from `embed_and_normalize` rather than degrading silently. |
| Daemon crashes | Existing `daemon_embed` returns None | Existing fallthrough to in-process; `try_auto_start` restarts. |
| `~/.cache/panda/openvino` write fails | `set_property(NPU, CacheDir, ...)` errors | Log warning; continue without persistent cache (compile every cold start). |
| OpenVINO driver upgrade invalidates cache | OV plugin handles transparently | First post-upgrade run pays cold compile; subsequent warm. |

**Observability** — the false-NPU bug is fixed: only the OV path prints `on NPU`; the ORT path prints `on CPU`. Each fires from inside its own constructor (or `get_or_init` closure), so the line reflects what actually loaded.

## Testing

### Unit tests

Existing (kept): `ep_resolver_tests::*` (4) and `execution_provider_tests::*` (2) — resolver layer is unchanged.

New in `ccr-core/src/ov_embed.rs`:

3. `ov_lib_path_returns_none_when_nothing_present` — clear `OPENVINO_LIB_PATH`, set `HOME` to a temp dir, assert `None`.
4. `ov_lib_path_honours_env_var` — set `OPENVINO_LIB_PATH=<tempfile>`, assert it's found. Also test the directory form.
5. `model_seq_len_returns_known_models` — `Some(128)` for the two registry entries, `None` otherwise.
6. `is_degraded_starts_false_marks_true_idempotent` — flips, then second call no-ops.

These run on every CI machine — no openvino-sys, no NPU.

### Feature-gated integration tests (`ccr-core/tests/npu_smoke.rs`)

Replace Approach A's tests with sharper assertions:

7. `npu_smoke_actually_uses_npu` — embed three strings, assert shape 3×384 and L2-normalised, **plus** `summarizer::ov_embedder_is_active() == true`. The Approach A version of this test passed even when CPU silently ran; this version cannot.
8. `npu_falls_back_to_cpu_when_libopenvino_missing` — `OPENVINO_LIB_PATH=/dev/null`, embed succeeds, `ov_embedder_is_active() == false`.

Both gated `#[cfg(feature = "openvino")]`, both skip silently unless `OPENVINO_NPU_AVAILABLE=1`.

### Manual verification

- [ ] `cargo build --release -p panda` (no feature) — clean.
- [ ] `cargo build --release -p panda --features openvino` — clean compile + link (runtime-linking means no OV lib required to build).
- [ ] `cargo test -p panda-core` — all tests pass, no NPU touched.
- [ ] `OPENVINO_NPU_AVAILABLE=1 cargo test -p panda-core --features openvino --test npu_smoke -- --nocapture` — both tests pass; first run prints multi-second cold time, second run sub-second (proves disk cache).
- [ ] `panda daemon start` with `execution_provider="npu"` — daemon log shows `[panda] embedder: AllMiniLML6V2 on NPU (raw OpenVINO)` and `[panda] NPU infer-request pool size: 4`.
- [ ] `panda run cargo build` — filtered output, no degradation messages.
- [ ] `panda gain` — non-zero savings, comparable to CPU baseline.
- [ ] **Latency probe:** time `panda run cargo build` 3× back-to-back. Cold pays compile (or hits cache); runs 2-3 should be visibly faster than CPU baseline.
- [ ] `PANDA_NPU_STRICT=1 OPENVINO_LIB_PATH=/dev/null panda daemon start` — daemon fails or logs hard error, doesn't silently degrade.
- [ ] Force degradation (kill NPU device or load a corrupt model) — `[panda] NPU embedder failed (...); falling back to CPU` appears once; subsequent embeds quiet on CPU.

### Regression risk

- `#[cfg(feature = "openvino")]` proliferation across `summarizer.rs` — concentrate in 4 marked spots only.
- `Mutex<Vec<InferRequest>>` lock contention if daemon's worker threadpool > pool size.
- The `~/.cache/panda/openvino` invalidation behaviour on driver upgrade — confirm with one manual test cycle if practical.

## Out of scope (follow-ups)

1. Restore the 9 opt-in models. `model_seq_len` grows from 2 to 16 entries; nothing else changes structurally.
2. Restore `embed-bench` against the 9-model registry on NPU.
3. Optional GPU device selection (one new arm in `ep_choice` plus a `DeviceType::GPU` variant in `OvEmbedder`).
4. Re-attempt the ORT OpenVINO EP path when ort releases a fix for `load-dynamic` — at that point the choice is whether to keep both or pick one.
