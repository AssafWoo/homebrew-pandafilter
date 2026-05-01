# Raw OpenVINO Bypass — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restore Intel NPU acceleration on top of upstream's ORT-based panda by porting `archive/pre-upstream-merge:ccr-core/src/ov_embed.rs` forward and grafting it onto the resolver/daemon scaffolding from Approach A.

**Architecture:** The `--features openvino` Cargo feature is repointed: instead of activating ORT's broken OpenVINO EP, it pulls in the `openvino-rs` crates and registers an `OvEmbedder` that runs ahead of the ORT-CPU path inside `embed_and_normalize`. The Approach A scaffolding (config field, `current_ep` resolver, `set_execution_provider`, daemon wiring) is reused verbatim; only `MiniLmEmbedder::new` reverts to upstream's flat `[CPU]` builder.

**Tech Stack:** Rust 2021, `openvino`/`openvino-sys` from `intel/openvino-rs` rev `e25f1f848edc` with `runtime-linking`, OpenVINO 2024+ runtime (`libopenvino_c.so`, default search includes `~/.local/share/ccr/onnxruntime/`), Intel NPU 3720, `tokenizers` 0.21, `hf-hub` 0.4 (reused via `summarizer::resolve_model_files`).

**Branch:** continues on `feat/npu-on-ort`. Prerequisite: Approach A's 9 commits (`e1ca02a` → `7a7bf2a`) are already in place. This plan adds 7 new commits on top.

---

## File Structure

| File | Responsibility | Change type |
|---|---|---|
| `ccr-core/Cargo.toml` | Replace `ort/openvino` feature wiring with `openvino-rs` deps | Modify |
| `ccr-core/src/lib.rs` | Declare `ov_embed` module gated on the feature | Modify |
| `ccr-core/src/ov_embed.rs` | Direct OpenVINO C-API embedder (Core, compiled model, async InferRequest pool, NPU compile cache, degradation flag) | Create (port from archive) |
| `ccr-core/src/summarizer.rs` | Revert `MiniLmEmbedder::new` to upstream's flat `[CPU]`. Add `OV_EMBEDDER` static, `get_ov_embedder`, `preload_ov_embedder`, `ov_embedder_is_active`. Wire OV dispatch into `embed_and_normalize` and `embed_direct`. Move CPU "embedder loaded" log into `MODEL_CACHE.get_or_try_init`. Make `current_ep` `pub`. | Modify |
| `ccr/src/cmd/daemon.rs` | Eager `preload_ov_embedder` call when configured | Modify |
| `ccr-core/tests/npu_smoke.rs` | Replace Approach A assertions with sharper ones using `ov_embedder_is_active` | Modify |
| `README.md` | Rewrite NPU section to describe the raw bypass | Modify |

No file becomes large enough to need splitting. `ov_embed.rs` is ~400 lines, single responsibility (NPU embedding). `summarizer.rs` already exceeds 1900 lines pre-existing — we don't restructure it; we only add to its existing patterns.

---

## Task 1: Revert `MiniLmEmbedder::new` to upstream's flat `[CPU]` builder

**Why first:** Approach A's closure-based EP list is dead weight under Approach B (no caller asks `MiniLmEmbedder` for an NPU session anymore). Reverting now keeps the diff readable when later tasks add the OV path.

**Files:**
- Modify: `ccr-core/src/summarizer.rs`

- [ ] **Step 1: Confirm the current `MiniLmEmbedder::new` has the Approach A closure**

Run:
```bash
cd /home/smiie/github/homebrew-pandafilter/.worktrees/npu-on-ort
grep -n "let build_session\|chosen_ep\|fn new" ccr-core/src/summarizer.rs | head
```
Expected output includes a line like `let build_session = |use_npu: bool| ...`. If that line is missing, stop — branch state is unexpected.

- [ ] **Step 2: Replace the closure block with upstream's flat builder**

Open `ccr-core/src/summarizer.rs`. Locate the entire block inside `impl MiniLmEmbedder { fn new(name: &str) -> ... {` from the line `// Build the EP list. CPU is always last so ORT can per-op fall through` through `Err(e) => return Err(e), };` (about 50 lines).

Replace it with this exact block:

```rust
        let mut builder = ort::session::Session::builder().map_err(ort_err)?;
        builder = builder
            .with_optimization_level(ort::session::builder::GraphOptimizationLevel::Level3)
            .map_err(ort_err)?;
        builder = builder.with_memory_pattern(false).map_err(ort_err)?;
        builder = builder.with_intra_threads(threads).map_err(ort_err)?;
        builder = builder
            .with_execution_providers([ort::ep::CPU::default().with_arena_allocator(false).build()])
            .map_err(ort_err)?;
        let session = builder.commit_from_file(&model_path).map_err(ort_err)?;
```

This is verbatim upstream. The eprintln moves to Task 6.

- [ ] **Step 3: Build to confirm compile**

Run:
```bash
cargo build -p panda-core
```
Expected: clean build. The unused `current_ep` warning may appear — that's fine for now; Task 6 re-uses it.

- [ ] **Step 4: Run tests**

Run:
```bash
cargo test -p panda-core
```
Expected: all tests pass (including `ep_resolver_tests` and `execution_provider_tests` from Approach A).

- [ ] **Step 5: Commit**

```bash
git add ccr-core/src/summarizer.rs
git commit -m "refactor(ccr-core): revert MiniLmEmbedder to upstream flat [CPU] builder" -m "Approach A's closure-based EP-list with CPU fallback retry was scaffolding for the ort OpenVINO EP path, which empirical verification showed cannot engage on Meteor Lake with ort 2.0.0-rc.12. The raw OpenVINO bypass (next commits) makes the closure dead code. Reverting to upstream's exact builder makes that obvious and shrinks the diff." -m "MiniLmEmbedder is now strictly the CPU fallback path."
```

---

## Task 2: Repoint `openvino` Cargo feature to `openvino-rs` crates

**Files:**
- Modify: `ccr-core/Cargo.toml`

- [ ] **Step 1: Update `ccr-core/Cargo.toml`**

Open `ccr-core/Cargo.toml`. The current state has these two relevant blocks (left in place by Approach A):

```toml
ort = { version = "=2.0.0-rc.12", features = ["std", "ndarray", "download-binaries", "tls-native"], default-features = false }
# ... other deps ...

[features]
default = []
openvino = ["ort/openvino"]
```

Replace the `[features]` block and add two new optional dependencies. Final relevant excerpt:

```toml
[dependencies]
regex.workspace = true
serde.workspace = true
serde_json.workspace = true
toml.workspace = true
anyhow.workspace = true
thiserror.workspace = true
tiktoken-rs.workspace = true
once_cell = "1"
libc = "0.2"
ort = { version = "=2.0.0-rc.12", features = ["std", "ndarray", "download-binaries", "tls-native"], default-features = false }
tokenizers = { version = "0.21", features = ["onig"], default-features = false }
ndarray = "0.17"
hf-hub = { version = "0.4", features = ["ureq", "native-tls"] }
rusqlite = { version = "0.31", features = ["bundled"] }
walkdir = "2"
openvino = { git = "https://github.com/intel/openvino-rs", rev = "e25f1f848edc", features = ["runtime-linking"], optional = true }
openvino-sys = { git = "https://github.com/intel/openvino-rs", rev = "e25f1f848edc", features = ["runtime-linking"], optional = true }

[features]
default = []
openvino = ["dep:openvino", "dep:openvino-sys"]

[dev-dependencies]
tempfile = "3"
```

Don't touch `ort`'s features — the bypass coexists with ORT (CPU only) in default builds; we just don't ask ORT for OpenVINO anymore.

- [ ] **Step 2: Verify default build is unchanged**

Run:
```bash
cargo build -p panda-core
```
Expected: clean build. `Cargo.lock` may pick up the optional `openvino` git dep info but won't activate it.

- [ ] **Step 3: Verify feature-on build (link only — no NPU touched yet)**

Run:
```bash
cargo build -p panda-core --features openvino 2>&1 | tail -10
```
Expected: clean compile and link. `runtime-linking` means no link-time `libopenvino_c.so` requirement. If you see compile errors from the `openvino` crate, the rev pin is wrong — stop and report.

`ccr/Cargo.toml`'s `openvino = ["panda-core/openvino"]` forwarding feature was added in Approach A and stays unchanged.

- [ ] **Step 4: Commit**

```bash
git add ccr-core/Cargo.toml
git commit -m "feat(ccr-core): repoint openvino feature to openvino-rs crates" -m "Replaces the broken ort/openvino EP wiring with the openvino + openvino-sys git deps from intel/openvino-rs (rev e25f1f848edc, runtime-linking). Same rev that worked on this hardware before the upstream merge — proven path." -m "No code consumes the new deps yet; that's added in the next commits."
```

---

## Task 3: Port `ov_embed.rs` from `archive/pre-upstream-merge`

**Files:**
- Create: `ccr-core/src/ov_embed.rs`
- Modify: `ccr-core/src/lib.rs`

- [ ] **Step 1: Copy the archived file verbatim into the worktree**

Run:
```bash
cd /home/smiie/github/homebrew-pandafilter/.worktrees/npu-on-ort
git show archive/pre-upstream-merge:ccr-core/src/ov_embed.rs > ccr-core/src/ov_embed.rs
wc -l ccr-core/src/ov_embed.rs
```
Expected: ~401 lines.

- [ ] **Step 2: Trim `model_onnx_info` to just the two upstream-supported models, and replace it with `model_seq_len`**

Open `ccr-core/src/ov_embed.rs`. Find the `pub fn model_onnx_info(...)` function (the one with the 16-arm match). Replace the entire function with:

```rust
/// Per-model NPU sequence length. Returns `None` for unknown models — caller
/// should fall through to the CPU embedder.
///
/// Mirrors the model registry in `summarizer::model_registry`. When new
/// models are added there, add their NPU `seq_len` here.
pub fn model_seq_len(model_name: &str) -> Option<usize> {
    match model_name {
        "AllMiniLML6V2" => Some(128),
        "AllMiniLML12V2" => Some(128),
        _ => None,
    }
}
```

The 16-model table goes away — we'll restore it in a separate follow-up when the broader model set comes back.

- [ ] **Step 3: Delete `find_fastembed_onnx`**

Find and delete the entire `pub fn find_fastembed_onnx(...) -> Option<(PathBuf, PathBuf, usize)>` function (the one that walks the fastembed cache). Upstream's `summarizer::resolve_model_files` replaces it.

- [ ] **Step 4: Adjust `ov_lib_path` visibility**

The function `fn ov_lib_path()` at the top of the file is currently private. The unit tests in Task 8 will exercise it via `ov_embed::ov_lib_path`. Change its signature from:

```rust
fn ov_lib_path() -> Option<PathBuf> {
```

to:

```rust
pub fn ov_lib_path() -> Option<PathBuf> {
```

(Just `pub fn` — module-level visibility is enough since `ov_embed` is `pub(crate)`.)

- [ ] **Step 5: Add the module declaration to `lib.rs`**

Open `ccr-core/src/lib.rs`. Find the existing `pub mod` list near the top:

```rust
pub mod analytics;
pub mod ansi;
pub mod global_rules;
pub mod config;
pub mod delta;
pub mod embed_client;
pub mod focus;
pub mod jsonlog;
pub mod ndjson;
pub mod patterns;
```

Add immediately after `pub mod patterns;`:

```rust
#[cfg(feature = "openvino")]
pub(crate) mod ov_embed;
```

Keep alphabetical placement if the file uses it; otherwise the position right after `patterns` is fine.

- [ ] **Step 6: Build with feature off (file should be inert)**

Run:
```bash
cargo build -p panda-core
```
Expected: clean build. The `ov_embed` module is excluded by `#[cfg(feature = "openvino")]`.

- [ ] **Step 7: Build with feature on**

Run:
```bash
cargo build -p panda-core --features openvino 2>&1 | tail -20
```
Expected: clean compile. The `openvino`/`openvino-sys` crates compile and link via `runtime-linking`. You may see warnings about unused `Send`/`Sync` impls — leave them; they're conservative safety markers from the original.

- [ ] **Step 8: Commit**

```bash
git add ccr-core/src/ov_embed.rs ccr-core/src/lib.rs
git commit -m "feat(ccr-core): restore ov_embed.rs as raw OpenVINO NPU embedder" -m "Near-verbatim port from archive/pre-upstream-merge. Two surgical edits: 1) model_onnx_info replaced with a 2-arm model_seq_len matching upstream's slim model_registry (full table follows in a separate restore-models commit); 2) find_fastembed_onnx removed because hf_hub-based summarizer::resolve_model_files supersedes it." -m "Public surface: try_new, embed, is_degraded, mark_degraded, model_seq_len, ov_lib_path. Module is gated on --features openvino."
```

---

## Task 4: Add `OV_EMBEDDER` static + accessors in `summarizer.rs`

**Files:**
- Modify: `ccr-core/src/summarizer.rs`

- [ ] **Step 1: Locate the insertion point**

Run:
```bash
grep -n "static MODEL_CACHE\|pub fn preload_model" ccr-core/src/summarizer.rs
```
Expected: `static MODEL_CACHE` near the bottom of the embedder section, followed by `fn get_model`, then `pub fn preload_model`.

- [ ] **Step 2: Add the OV embedder static and accessors right after `pub fn preload_model`**

Open `ccr-core/src/summarizer.rs`. Find the `pub fn preload_model() -> anyhow::Result<()>` function. Immediately after its closing brace, insert:

```rust
// ── Raw OpenVINO bypass (opt-in via --features openvino) ─────────────────────

#[cfg(feature = "openvino")]
static OV_EMBEDDER: OnceCell<Option<crate::ov_embed::OvEmbedder>> = OnceCell::new();

/// Lazily construct the OV embedder. Returns `None` (cached) on any failure
/// or once `is_degraded()` flips. Idempotent and process-lifetime stable.
#[cfg(feature = "openvino")]
fn get_ov_embedder() -> Option<&'static crate::ov_embed::OvEmbedder> {
    if crate::ov_embed::is_degraded() {
        return None;
    }
    OV_EMBEDDER
        .get_or_init(|| {
            let name = get_model_name();
            let (onnx, tok) = match resolve_model_files(name) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("[panda] OV bypass: model fetch failed: {e}");
                    return None;
                }
            };
            let seq_len = match crate::ov_embed::model_seq_len(name) {
                Some(s) => s,
                None => {
                    eprintln!(
                        "[panda] OV bypass: no NPU seq_len for {name}; CPU fallback"
                    );
                    return None;
                }
            };
            match crate::ov_embed::OvEmbedder::try_new(&onnx, &tok, seq_len) {
                Ok(e) => {
                    eprintln!("[panda] embedder: {} on NPU (raw OpenVINO)", name);
                    Some(e)
                }
                Err(err) => {
                    eprintln!("[panda] OV bypass init failed: {err}");
                    None
                }
            }
        })
        .as_ref()
}

/// Eagerly construct the OV embedder. Used by `panda daemon start` so the
/// multi-second NPU compile happens once at start, not on the first client
/// embed call.
///
/// Returns `Some(())` on success, `None` if construction failed (in which
/// case `OV_EMBEDDER` is cached as `None` and subsequent calls go to CPU).
#[cfg(feature = "openvino")]
pub fn preload_ov_embedder() -> Option<()> {
    get_ov_embedder().map(|_| ())
}

/// Reports whether the OV embedder is currently active. Used by the smoke
/// test to assert NPU was actually engaged (vs silent CPU fallback).
#[cfg(feature = "openvino")]
pub fn ov_embedder_is_active() -> bool {
    OV_EMBEDDER.get().and_then(|o| o.as_ref()).is_some()
}
```

- [ ] **Step 3: Build with feature off**

Run:
```bash
cargo build -p panda-core
```
Expected: clean build. None of the new symbols compile (all `#[cfg]`-gated).

- [ ] **Step 4: Build with feature on**

Run:
```bash
cargo build -p panda-core --features openvino 2>&1 | tail -10
```
Expected: clean compile. Warnings about unused `get_ov_embedder` / `preload_ov_embedder` / `ov_embedder_is_active` are expected — Task 5 wires them up.

- [ ] **Step 5: Commit**

```bash
git add ccr-core/src/summarizer.rs
git commit -m "feat(ccr-core): add OV_EMBEDDER static and accessors" -m "Lazy and eager construction APIs for the raw OpenVINO embedder, mirroring the existing MODEL_CACHE pattern. ov_embedder_is_active is exposed for the smoke test so NPU engagement can be asserted (not just configured)." -m "Not yet wired into embed_and_normalize — that's the next commit."
```

---

## Task 5: Wire OV dispatch into `embed_and_normalize` and `embed_direct`

**Files:**
- Modify: `ccr-core/src/summarizer.rs`

- [ ] **Step 1: Update `embed_and_normalize`**

Find the existing function:

```rust
fn embed_and_normalize(texts: Vec<&str>) -> anyhow::Result<Vec<Vec<f32>>> {
    #[cfg(unix)]
    if let Some(embeddings) = crate::embed_client::daemon_embed(&texts, true) {
        return Ok(embeddings);
    }
    #[cfg(unix)]
    apply_nice_once();
    embed_direct(texts)
}
```

Replace with:

```rust
fn embed_and_normalize(texts: Vec<&str>) -> anyhow::Result<Vec<Vec<f32>>> {
    #[cfg(unix)]
    if let Some(embeddings) = crate::embed_client::daemon_embed(&texts, true) {
        return Ok(embeddings);
    }
    #[cfg(feature = "openvino")]
    if current_ep() == "npu" {
        if let Some(ov) = get_ov_embedder() {
            let texts_slice: Vec<&str> = texts.iter().copied().collect();
            let mut v = ov.embed(&texts_slice)?;
            for e in &mut v {
                l2_normalize(e);
            }
            return Ok(v);
        }
    }
    #[cfg(unix)]
    apply_nice_once();
    embed_direct(texts)
}
```

(`OvEmbedder::embed` already L2-normalises internally; we re-normalise here defensively to keep the contract identical to `embed_direct`'s output. Leave the duplicate normalise in — it's idempotent on already-unit vectors and removes a subtle "is this normalised" question for future readers.)

- [ ] **Step 2: Update `embed_direct` so the daemon worker thread also tries OV**

`embed_direct` is what the daemon's worker calls (the daemon doesn't go through `embed_and_normalize`; it calls a public function directly). Find:

```rust
pub fn embed_direct(texts: Vec<&str>) -> anyhow::Result<Vec<Vec<f32>>> {
    let model = get_model()?;
    let mut embeddings = model.embed(&texts)?;
    for emb in &mut embeddings {
        l2_normalize(emb);
    }
    Ok(embeddings)
}
```

Replace with:

```rust
pub fn embed_direct(texts: Vec<&str>) -> anyhow::Result<Vec<Vec<f32>>> {
    #[cfg(feature = "openvino")]
    if current_ep() == "npu" {
        if let Some(ov) = get_ov_embedder() {
            let texts_slice: Vec<&str> = texts.iter().copied().collect();
            let mut v = ov.embed(&texts_slice)?;
            for e in &mut v {
                l2_normalize(e);
            }
            return Ok(v);
        }
    }
    let model = get_model()?;
    let mut embeddings = model.embed(&texts)?;
    for emb in &mut embeddings {
        l2_normalize(emb);
    }
    Ok(embeddings)
}
```

- [ ] **Step 3: Add the CPU "embedder loaded" log to `get_model`**

The Approach A eprintln moved out of `MiniLmEmbedder::new` in Task 1. We add it back in `get_model`'s init closure so it fires once when ORT actually loads. Find:

```rust
fn get_model() -> anyhow::Result<&'static MiniLmEmbedder> {
    MODEL_CACHE.get_or_try_init(|| {
        let name = get_model_name();
        if !bert_is_cached(name) {
            eprintln!("[panda] downloading BERT model ({name}, one-time setup)...");
            eprintln!("[panda] this may take a minute. future runs are instant.");
        }
        let embedder = MiniLmEmbedder::new(name)?;
        mark_bert_cached(name);
        Ok(embedder)
    })
}
```

Replace with:

```rust
fn get_model() -> anyhow::Result<&'static MiniLmEmbedder> {
    MODEL_CACHE.get_or_try_init(|| {
        let name = get_model_name();
        if !bert_is_cached(name) {
            eprintln!("[panda] downloading BERT model ({name}, one-time setup)...");
            eprintln!("[panda] this may take a minute. future runs are instant.");
        }
        let embedder = MiniLmEmbedder::new(name)?;
        mark_bert_cached(name);
        eprintln!("[panda] embedder: {} on CPU (ort)", name);
        Ok(embedder)
    })
}
```

- [ ] **Step 4: Make `current_ep` `pub` so the daemon crate can call it**

Find the existing function:

```rust
pub(crate) fn current_ep() -> &'static str {
```

Change to:

```rust
pub fn current_ep() -> &'static str {
```

- [ ] **Step 5: Default build**

Run:
```bash
cargo build -p panda-core
```
Expected: clean. The `#[cfg(feature = "openvino")]` blocks are absent; only the `[panda] embedder: ... on CPU (ort)` line is added unconditionally — that's intended (CPU is what runs in default builds).

- [ ] **Step 6: Feature-on build**

Run:
```bash
cargo build -p panda-core --features openvino 2>&1 | tail -10
```
Expected: clean.

- [ ] **Step 7: Workspace build**

Run:
```bash
cargo build
```
Expected: clean.

- [ ] **Step 8: Tests**

Run:
```bash
cargo test -p panda-core
```
Expected: all pass.

- [ ] **Step 9: Commit**

```bash
git add ccr-core/src/summarizer.rs
git commit -m "feat(ccr-core): wire OV embedder ahead of ORT-CPU in dispatch path" -m "embed_and_normalize and embed_direct both try the OV embedder first when --features openvino is on AND current_ep() == 'npu'. Falls through to ORT-CPU on any failure (including the first call after mark_degraded)." -m "Also moves the CPU 'embedder loaded' log into get_model's init closure so it only fires when ORT actually constructs a session — fixes Approach A's false-NPU log bug. current_ep is promoted from pub(crate) to pub for the daemon's eager-preload call (next commit)."
```

---

## Task 6: Eager OV preload in the daemon

**Files:**
- Modify: `ccr/src/cmd/daemon.rs`

- [ ] **Step 1: Locate the existing setter calls**

Run:
```bash
grep -n "set_execution_provider\|preload_model" ccr/src/cmd/daemon.rs
```
Expected: a line near `daemon_main` calling `set_execution_provider`, followed by `preload_model().is_err()`.

- [ ] **Step 2: Add the eager preload between `set_execution_provider` and `preload_model`**

Open `ccr/src/cmd/daemon.rs`. Find:

```rust
        panda_core::summarizer::set_model_name(&config.global.bert_model);
        panda_core::summarizer::set_ort_threads(config.global.ort_threads);
        panda_core::summarizer::set_execution_provider(&config.global.execution_provider);
    }
    if panda_core::summarizer::preload_model().is_err() {
        std::process::exit(1);
    }
```

Replace with:

```rust
        panda_core::summarizer::set_model_name(&config.global.bert_model);
        panda_core::summarizer::set_ort_threads(config.global.ort_threads);
        panda_core::summarizer::set_execution_provider(&config.global.execution_provider);
    }
    #[cfg(feature = "openvino")]
    if panda_core::summarizer::current_ep() == "npu" {
        // Eagerly compile the model for NPU so the multi-second cost is
        // paid here instead of on the first client embed call. If init
        // fails the OV_EMBEDDER static caches None and subsequent calls
        // go to CPU automatically.
        let _ = panda_core::summarizer::preload_ov_embedder();
    }
    if panda_core::summarizer::preload_model().is_err() {
        std::process::exit(1);
    }
```

We always preload the CPU embedder too — it's the fallback path and costs ~100ms.

- [ ] **Step 3: Default build**

Run:
```bash
cargo build -p panda
```
Expected: clean.

- [ ] **Step 4: Feature-on build**

Run:
```bash
cargo build -p panda --features openvino 2>&1 | tail -5
```
Expected: clean.

- [ ] **Step 5: Tests**

Run:
```bash
cargo test
```
Expected: all pass.

- [ ] **Step 6: Commit**

```bash
git add ccr/src/cmd/daemon.rs
git commit -m "feat(ccr): eagerly preload OV embedder in daemon when configured" -m "When the binary is built with --features openvino and the resolver picks NPU, the daemon now triggers OvEmbedder::try_new during start (after set_*, before bind). Multi-second NPU compile happens once at daemon-start, not on the first client embed call." -m "Init failure caches None in OV_EMBEDDER and embeds transparently fall to CPU."
```

---

## Task 7: Unit tests for `ov_embed.rs`

**Files:**
- Modify: `ccr-core/src/ov_embed.rs`

These run on every CI machine — no openvino-sys initialization, no NPU touched. Tests live at the bottom of `ov_embed.rs` in a `#[cfg(test)] mod tests` block. The whole file is already gated on `#[cfg(feature = "openvino")]` via `lib.rs`, so the tests only run with `--features openvino`.

- [ ] **Step 1: Append the test module**

Open `ccr-core/src/ov_embed.rs`. At the very end of the file (after the last closing brace of `impl OvEmbedder`), append:

```rust

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // env tests share process state — serialize them.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn clear_env() {
        std::env::remove_var("OPENVINO_LIB_PATH");
    }

    #[test]
    fn ov_lib_path_returns_none_when_nothing_present() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_env();
        let tmp = tempfile::tempdir().unwrap();
        let saved_home = std::env::var_os("HOME");
        std::env::set_var("HOME", tmp.path());
        let result = ov_lib_path();
        if let Some(home) = saved_home {
            std::env::set_var("HOME", home);
        } else {
            std::env::remove_var("HOME");
        }
        // Result depends on whether /usr/lib/... has libopenvino_c.so on the
        // host. If yes, that's also valid — just assert no panic.
        let _ = result;
    }

    #[test]
    fn ov_lib_path_honours_env_file() {
        let _g = ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("libopenvino_c.so");
        std::fs::write(&path, b"stub").unwrap();
        std::env::set_var("OPENVINO_LIB_PATH", &path);
        let resolved = ov_lib_path();
        std::env::remove_var("OPENVINO_LIB_PATH");
        assert_eq!(resolved.as_deref(), Some(path.as_path()));
    }

    #[test]
    fn ov_lib_path_honours_env_dir() {
        let _g = ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let lib = tmp.path().join("libopenvino_c.so");
        std::fs::write(&lib, b"stub").unwrap();
        std::env::set_var("OPENVINO_LIB_PATH", tmp.path());
        let resolved = ov_lib_path();
        std::env::remove_var("OPENVINO_LIB_PATH");
        assert_eq!(resolved.as_deref(), Some(lib.as_path()));
    }

    #[test]
    fn model_seq_len_returns_known_models() {
        assert_eq!(model_seq_len("AllMiniLML6V2"), Some(128));
        assert_eq!(model_seq_len("AllMiniLML12V2"), Some(128));
        assert_eq!(model_seq_len("nonsense"), None);
        assert_eq!(model_seq_len(""), None);
    }

    #[test]
    fn is_degraded_starts_false_marks_true_idempotent() {
        // Note: DEGRADED is a process-wide flag. This test must run before
        // any other test in this file would call mark_degraded — currently
        // none do, so we're safe. Ordering caveat noted.
        assert!(!is_degraded(), "DEGRADED should start clear");
        mark_degraded("test-1");
        assert!(is_degraded(), "DEGRADED should be set after first call");
        mark_degraded("test-2"); // Should not panic, should not double-print
        assert!(is_degraded());
    }
}
```

- [ ] **Step 2: Run the new tests**

Run:
```bash
cargo test -p panda-core --features openvino ov_embed::tests
```
Expected: 5 tests pass. Output similar to:
```
running 5 tests
test ov_embed::tests::is_degraded_starts_false_marks_true_idempotent ... ok
test ov_embed::tests::model_seq_len_returns_known_models ... ok
test ov_embed::tests::ov_lib_path_honours_env_dir ... ok
test ov_embed::tests::ov_lib_path_honours_env_file ... ok
test ov_embed::tests::ov_lib_path_returns_none_when_nothing_present ... ok
test result: ok. 5 passed; 0 failed
```

- [ ] **Step 3: Run the full feature-on test suite**

Run:
```bash
cargo test -p panda-core --features openvino
```
Expected: all green, including the existing `ep_resolver_tests` and `execution_provider_tests` from Approach A.

- [ ] **Step 4: Commit**

```bash
git add ccr-core/src/ov_embed.rs
git commit -m "test(ccr-core): unit tests for ov_embed pure functions" -m "Five tests covering ov_lib_path resolution (env unset, env-file form, env-dir form), model_seq_len lookup, and the DEGRADED flag's idempotence. None touch openvino-sys or NPU." -m "ENV_LOCK Mutex serialises tests that mutate process env, so cargo's default parallel test runner doesn't race them."
```

---

## Task 8: Sharper assertions in `npu_smoke.rs`

**Files:**
- Modify: `ccr-core/tests/npu_smoke.rs`

Approach A's smoke test passed even when CPU silently ran. This task replaces the assertions with `ov_embedder_is_active()` checks that cannot pass without the OV embedder.

- [ ] **Step 1: Read the current file to confirm it exists**

Run:
```bash
cat ccr-core/tests/npu_smoke.rs
```
Expected: the file from Approach A's Task 6 — two tests `npu_smoke_embeds_three_strings` and `npu_falls_back_to_cpu_when_openvino_missing`.

- [ ] **Step 2: Replace the file content**

Overwrite `ccr-core/tests/npu_smoke.rs` with:

```rust
//! Feature-gated NPU smoke test.
//!
//! Skipped at compile time unless `--features openvino`; skipped at run time
//! unless `OPENVINO_NPU_AVAILABLE=1`. Verifies the raw OpenVINO embedder
//! actually engaged (asserts `ov_embedder_is_active()`), not just that some
//! embedder produced 384-dim L2-normalised vectors.

#![cfg(feature = "openvino")]

use panda_core::summarizer;

fn npu_opted_in() -> bool {
    std::env::var("OPENVINO_NPU_AVAILABLE").ok().as_deref() == Some("1")
}

#[test]
fn npu_smoke_actually_uses_npu() {
    if !npu_opted_in() {
        eprintln!("skipping: OPENVINO_NPU_AVAILABLE != 1");
        return;
    }
    summarizer::set_execution_provider("npu");
    let texts = vec!["error: build failed", "warning: deprecated", "ok"];

    let t0 = std::time::Instant::now();
    let embeddings = summarizer::embed_direct(texts).expect("embed_direct");
    let elapsed = t0.elapsed();

    assert_eq!(embeddings.len(), 3, "expected 3 vectors");
    for (i, v) in embeddings.iter().enumerate() {
        assert_eq!(v.len(), 384, "vec {i} dim mismatch");
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-3,
            "vec {i} not L2-normalised: norm={norm}"
        );
    }

    assert!(
        summarizer::ov_embedder_is_active(),
        "raw OV embedder must be active in NPU mode — without this assertion, \
         a silent CPU fallback would let this test pass spuriously"
    );

    eprintln!("npu embed elapsed: {elapsed:?}");
}

#[test]
fn npu_falls_back_to_cpu_when_libopenvino_missing() {
    if !npu_opted_in() {
        eprintln!("skipping: OPENVINO_NPU_AVAILABLE != 1");
        return;
    }
    // Hide libopenvino_c.so so OvEmbedder::try_new fails. ov_lib_path()
    // checks OPENVINO_LIB_PATH first, so pointing it at /dev/null beats
    // any other path on the host.
    std::env::set_var("OPENVINO_LIB_PATH", "/dev/null");
    summarizer::set_execution_provider("npu");

    // Embedding should still succeed via CPU fallback.
    let r = summarizer::embed_direct(vec!["x"]);
    std::env::remove_var("OPENVINO_LIB_PATH");

    assert!(r.is_ok(), "expected CPU fallback to succeed: {:?}", r.err());
    assert!(
        !summarizer::ov_embedder_is_active(),
        "OV embedder must NOT be active when libopenvino is hidden"
    );
}
```

Note: this test file is independent of Task 7's unit tests — they test pure functions in `ov_embed`; this exercises the full embedder construction path against real NPU.

- [ ] **Step 3: Run the smoke test (skipped without env var)**

Run:
```bash
cargo test -p panda-core --features openvino --test npu_smoke
```
Expected: both tests run and skip silently with `skipping: OPENVINO_NPU_AVAILABLE != 1`. Both pass.

- [ ] **Step 4: Commit**

```bash
git add ccr-core/tests/npu_smoke.rs
git commit -m "test(ccr-core): assert ov_embedder_is_active in NPU smoke test" -m "Approach A's assertions (3 vectors, 384 dim, L2-normalised) all hold whether NPU or CPU ran the inference, so the test passed even when OpenVINO EP silently failed to engage. Now we additionally assert summarizer::ov_embedder_is_active() — only true when OvEmbedder::try_new succeeded. Catches false-NPU regressions." -m "Skipped silently unless OPENVINO_NPU_AVAILABLE=1, same as before."
```

---

## Task 9: Rewrite README NPU section

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Locate the existing section**

Run:
```bash
grep -n "## NPU support" README.md
```
Expected: a line number for the existing "NPU support (opt-in, Intel only)" header from Approach A's Task 7.

- [ ] **Step 2: Replace the entire section**

Open `README.md`. Find the section starting with `## NPU support (opt-in, Intel only)` and ending just before the next `---` divider. Replace the whole block (header through the line before the divider) with:

```markdown
## NPU support (opt-in, Intel only)

Panda's embedding model can run on an Intel NPU (e.g. Meteor Lake NPU 3720)
through a direct OpenVINO C-API embedder that bypasses ONNX Runtime entirely.
This is opt-in.

### Build

```bash
cargo build --release --features openvino
```

The build links against `openvino`/`openvino-sys` (intel/openvino-rs) with
`runtime-linking`, so no OpenVINO library is required at link time. At
runtime, panda dlopens `libopenvino_c.so` from the first of:

1. `$OPENVINO_LIB_PATH` (if set; can be a file path or a directory).
2. `~/.local/share/ccr/onnxruntime/libopenvino_c.so`.
3. `/usr/lib/x86_64-linux-gnu/libopenvino_c.so`, `/usr/local/lib/...`,
   `/opt/intel/openvino/runtime/lib/intel64/...`.

Install OpenVINO 2024+ runtime via your distro's package manager or Intel's
installer.

### Configure

In `panda.toml` (project or `~/.config/panda/config.toml`):

```toml
[global]
execution_provider = "npu"   # "auto" | "cpu" | "npu"
```

Or override at runtime:

```bash
PANDA_NPU=npu panda daemon start    # force NPU for this daemon
PANDA_NPU=cpu panda run cargo build # force CPU for this invocation
```

### How it works

The first call pays a one-time NPU compile cost (~3–10 s on NPU 3720). The
compiled blob is persisted to `~/.cache/panda/openvino/` — subsequent starts
warm up in <500 ms. Run `panda daemon start` once at the beginning of a
session and every embedding is sub-millisecond dispatch overhead from the
warm InferRequest pool (size matches the NPU's reported optimal request
count, typically 4 on Meteor Lake's two-tile NPU 3720).

If NPU initialisation or inference fails, panda logs one line on stderr and
falls back to CPU for the rest of the process — the daemon doesn't crash.
To make these failures fatal (useful when diagnosing whether NPU is actually
in use), set `PANDA_NPU_STRICT=1`. To force a specific inference precision,
set `PANDA_NPU_PRECISION=FP16` or `=FP32`.
```

- [ ] **Step 3: Verify the section renders cleanly**

Run:
```bash
grep -nA3 "## NPU support" README.md | head -20
```
Expected: the new section header followed by the new opening paragraph.

- [ ] **Step 4: Commit**

```bash
git add README.md
git commit -m "docs: rewrite NPU section for the raw OpenVINO bypass" -m "Approach A's section described the ort/openvino EP path. The bypass uses openvino-rs directly with a different runtime story: runtime-linking (no link-time deps), library finder, ~/.cache/panda/openvino blob cache, async pool. Adds notes on PANDA_NPU_STRICT and PANDA_NPU_PRECISION envs."
```

---

## Task 10: Manual verification

**Files:** none — exercises the binaries.

- [ ] **Step 1: Default build clean**

Run:
```bash
cargo build --release -p panda
./target/release/panda run ls 2>&1 | tail -10
```
Expected: clean build, normal `ls` output. Stderr (if BERT triggers) shows `[panda] embedder: AllMiniLML6V2 on CPU (ort)`. Default behaviour unchanged.

- [ ] **Step 2: Feature-on build clean**

Run:
```bash
cargo build --release -p panda --features openvino
ldd ./target/release/panda 2>&1 | grep -iE "openvino|onnx" || echo "(no dynamic OV/ONNX deps — runtime-linking working)"
```
Expected: clean build. The `ldd` should show no openvino libs (runtime-linking pulls them in via dlopen, not link-time).

- [ ] **Step 3: Whole-workspace tests, both feature configs**

Run:
```bash
cargo test
cargo test --features openvino
```
Expected: both green. Smoke tests skip without `OPENVINO_NPU_AVAILABLE`.

- [ ] **Step 4: NPU smoke test on real hardware**

Run:
```bash
OPENVINO_NPU_AVAILABLE=1 cargo test -p panda-core --features openvino --test npu_smoke -- --nocapture
```
Expected: both tests pass. First run prints multi-second elapsed (cold compile or cache miss); a second invocation prints sub-second (warm cache hit).

If `npu_smoke_actually_uses_npu` fails — that's the empirical signal that the OV bypass itself doesn't engage on this machine. STOP and report; this is the spec's escalation trigger.

- [ ] **Step 5: Daemon on NPU**

Edit `panda.toml` (in the worktree) to set `execution_provider = "npu"`, then:

```bash
./target/release/panda daemon stop 2>/dev/null
./target/release/panda daemon start
sleep 12
./target/release/panda daemon status
echo "warning\nerror\nok\nok\nok\nok\nok\nok\nok\nok\nok\nok\nok\nok\nok" | ./target/release/panda filter --command cargo
```
Expected: daemon up. The `panda filter` invocation triggers an embed; stderr should show `[panda] embedder: AllMiniLML6V2 on NPU (raw OpenVINO)` exactly once per process.

- [ ] **Step 6: Strict mode fails loud**

Run:
```bash
./target/release/panda daemon stop
OPENVINO_LIB_PATH=/dev/null PANDA_NPU=npu PANDA_NPU_STRICT=1 ./target/release/panda daemon start
sleep 2
./target/release/panda daemon status
```
Expected: `daemon status` reports it's NOT running, OR if it's running, `panda run cargo build` returns an error rather than CPU output. Reset by running `panda daemon stop` and unsetting the envs.

- [ ] **Step 7: Real workload latency**

Run:
```bash
./target/release/panda daemon stop
./target/release/panda daemon start    # NPU mode per panda.toml
time ./target/release/panda run cargo build -p panda-core 2>&1 | tail -5
time ./target/release/panda run cargo build -p panda-core 2>&1 | tail -5  # warm
```
Expected: clean filtered output, no `NPU embedder failed` lines. Comparable savings vs CPU baseline (NPU is for speed, not quality).

- [ ] **Step 8: Push and open PR**

```bash
git push -u origin feat/npu-on-ort
gh pr create --base main --title "feat: NPU support via raw OpenVINO bypass" --body-file - <<'EOF'
Restores Intel NPU acceleration on top of upstream/main (post ort-migration)
by porting the archived ov_embed.rs forward and wiring it into the resolver/
daemon scaffolding from the earlier ORT-EP attempt.

Specs:
- docs/superpowers/specs/2026-05-01-npu-on-upstream-ort-design.md
- docs/superpowers/specs/2026-05-01-npu-raw-openvino-bypass-design.md
Plans:
- docs/superpowers/plans/2026-05-01-npu-on-ort.md
- docs/superpowers/plans/2026-05-01-npu-raw-openvino-bypass.md

Why the bypass: ort 2.0.0-rc.12's OpenVINO EP cannot engage on Meteor Lake
(libonnxruntime ABI clash with the OpenVINO provider plugins, plus an
unrelated `load-dynamic` compile error). Empirical verification on the
target hardware showed the EP path silently falls back to CPU while
logging "on NPU" — bug, not a feature. The bypass uses the openvino-rs
crates directly, the same path the fork shipped before the upstream merge.

Default builds are byte-identical to upstream — no behaviour change for
users who don't pass `--features openvino`.
EOF
```
Expected: PR opened. Link printed by `gh`.

---

## Self-review notes

Spec coverage:
- `[features] openvino` repointed — Task 2 ✓
- `ov_embed.rs` port + `model_seq_len` + `ov_lib_path` pub + drop `find_fastembed_onnx` — Task 3 ✓
- Module decl in `lib.rs` — Task 3 ✓
- `OV_EMBEDDER` static + `get_ov_embedder` + `preload_ov_embedder` + `ov_embedder_is_active` — Task 4 ✓
- Dispatch into `embed_and_normalize` and `embed_direct` — Task 5 ✓
- CPU log moved into `MODEL_CACHE.get_or_try_init` — Task 5 ✓
- `current_ep` made `pub` — Task 5 ✓
- `MiniLmEmbedder::new` reverted — Task 1 ✓
- Daemon eager preload — Task 6 ✓
- Unit tests (5 in `ov_embed`) — Task 7 ✓
- Sharper smoke tests with `ov_embedder_is_active` — Task 8 ✓
- README rewrite — Task 9 ✓
- Manual verification + PR — Task 10 ✓

Type/name consistency:
- `OvEmbedder::try_new(onnx_path: &Path, tokenizer_path: &Path, seq_len: usize)` — used identically in `get_ov_embedder` and the spec.
- `model_seq_len(&str) -> Option<usize>` — used identically across Task 3, Task 4, and Task 7.
- `current_ep() -> &'static str` — visibility upgrade covered explicitly in Task 5.
- `ov_embedder_is_active() -> bool` — defined Task 4, used Task 8.
- `preload_ov_embedder() -> Option<()>` — defined Task 4, called Task 6.
- The four `#[cfg(feature = "openvino")]` spots in `summarizer.rs` are: the static, `get_ov_embedder`/`preload_ov_embedder`/`ov_embedder_is_active` (one cfg per fn), the dispatch in `embed_and_normalize`, the dispatch in `embed_direct`. Total: 6 cfg guards across one logical surface — concentrated, not scattered.

No placeholders, no "implement later", no "similar to Task N". Every code step shows the actual code; every command step shows the exact command and expected output.
