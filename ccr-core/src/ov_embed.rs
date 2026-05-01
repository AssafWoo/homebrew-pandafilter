use anyhow::{Context, Result};
use openvino::{
    Core, DeviceType, ElementType, PartialShape, PropertyKey, RwPropertyKey, Shape, Tensor,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::thread;
use tokenizers::Tokenizer;

// Process-wide flag: once an OvEmbedder call hits a hard NPU error after a
// retry, this flips to true and `is_degraded()` makes summarizer.rs skip the
// NPU path for the rest of the process. Reset only by restarting panda.
static DEGRADED: AtomicBool = AtomicBool::new(false);

pub fn is_degraded() -> bool {
    DEGRADED.load(Ordering::Relaxed)
}

fn mark_degraded(reason: &str) {
    if !DEGRADED.swap(true, Ordering::Relaxed) {
        eprintln!("[panda] NPU embedder failed ({reason}); falling back to CPU");
    }
}

// openvino-sys loads `libopenvino_c.so` (the C API shim), not `libopenvino.so`.
const OV_C_LIB_CANDIDATES: &[&str] = &[
    "/usr/lib/x86_64-linux-gnu/libopenvino_c.so",
    "/usr/local/lib/libopenvino_c.so",
    "/opt/intel/openvino/runtime/lib/intel64/libopenvino_c.so",
];

pub fn ov_lib_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("OPENVINO_LIB_PATH") {
        let pb = PathBuf::from(&p);
        let candidate = if pb.is_dir() {
            pb.join("libopenvino_c.so")
        } else {
            pb
        };
        if candidate.exists() {
            return Some(candidate);
        }
    }
    let ccr_path = std::env::var("HOME").ok().map(|h| {
        PathBuf::from(h).join(".local/share/ccr/onnxruntime/libopenvino_c.so")
    });
    if let Some(p) = ccr_path {
        if p.exists() {
            return Some(p);
        }
    }
    for cand in OV_C_LIB_CANDIDATES {
        let p = PathBuf::from(cand);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

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

pub struct OvEmbedder {
    _core: Core,
    // Compiled model is kept alive but the InferRequests are pre-allocated up
    // front; we never call create_infer_request again at runtime.
    _compiled: openvino::CompiledModel,
    // Pool of pre-allocated requests. Drained on each embed() call, refilled
    // with the same requests after parallel work completes. Size matches the
    // NPU's reported OPTIMAL_NUMBER_OF_INFER_REQUESTS (typically 4 on Meteor
    // Lake's 2-tile NPU 3720).
    requests: Mutex<Vec<openvino::InferRequest>>,
    tokenizer: Tokenizer,
    seq_len: usize,
    hidden_size: usize,
    has_token_type: bool,
}

// SAFETY: openvino-rs marks both CompiledModel (Send) and InferRequest
// (Send + Sync) safe; the Mutex around the request pool guards refill semantics.
unsafe impl Send for OvEmbedder {}
unsafe impl Sync for OvEmbedder {}

impl OvEmbedder {
    pub fn try_new(onnx_path: &Path, tokenizer_path: &Path, seq_len: usize) -> Result<Self> {
        let lib = ov_lib_path()
            .context("libopenvino.so not found; install OpenVINO or set OPENVINO_LIB_PATH")?;
        openvino_sys::library::load_from(&lib)
            .map_err(|e| anyhow::anyhow!("OpenVINO load failed: {e}"))?;

        let mut core = Core::new().context("OpenVINO Core::new() failed")?;

        // Persist compiled-model blob between process invocations. NPU
        // compile is multi-second; cache turns subsequent starts into <500 ms.
        // The plugin invalidates the cache automatically on driver/OV upgrade.
        let cache_dir = std::env::var("HOME")
            .map(|h| PathBuf::from(h).join(".cache/panda/openvino"))
            .unwrap_or_else(|_| PathBuf::from(".panda-openvino-cache"));
        if let Err(e) = std::fs::create_dir_all(&cache_dir) {
            eprintln!(
                "[panda] could not create NPU cache dir {}: {} (continuing without cache)",
                cache_dir.display(),
                e
            );
        } else if let Some(s) = cache_dir.to_str() {
            if let Err(e) = core.set_property(&DeviceType::NPU, &RwPropertyKey::CacheDir, s) {
                eprintln!("[panda] could not enable NPU model cache: {e:?}");
            }
        }

        // PERFORMANCE_HINT=THROUGHPUT tells the NPU plugin to optimise for
        // multiple in-flight infer requests rather than single-call latency.
        // Without this hint, OPTIMAL_NUMBER_OF_INFER_REQUESTS often reports 1
        // and the device sits idle between sync calls.
        if let Err(e) =
            core.set_property(&DeviceType::NPU, &RwPropertyKey::HintPerformanceMode, "THROUGHPUT")
        {
            eprintln!("[panda] could not set NPU PERFORMANCE_HINT: {e:?}");
        }

        // INFERENCE_PRECISION_HINT can be used to force FP16/FP32, but the
        // NPU 3720 plugin already runs in FP16 by default — setting the hint
        // explicitly regressed throughput and broke the cache during testing.
        // Allow advanced users to override via PANDA_NPU_PRECISION but leave
        // the plugin default in place when the env var is unset.
        if let Ok(precision) = std::env::var("PANDA_NPU_PRECISION") {
            let precision = precision.trim();
            if !precision.is_empty() {
                if let Err(e) = core.set_property(
                    &DeviceType::NPU,
                    &RwPropertyKey::HintInferencePrecision,
                    precision,
                ) {
                    eprintln!(
                        "[panda] could not set NPU INFERENCE_PRECISION_HINT={precision}: {e:?}"
                    );
                }
            }
        }

        let mut model = core
            .read_model_from_file(onnx_path.to_str().unwrap(), "")
            .context("Failed to read ONNX model into OpenVINO")?;

        // NPU plugin requires static shapes. Batch=1 + parallel async requests
        // is the canonical NPU recipe; batched models on NPU regress.
        let n_inputs = model.get_inputs_len()?;
        let static_shape = PartialShape::new_static(2, &[1, seq_len as i64])?;
        for i in 0..n_inputs {
            let name = model.get_input_by_index(i)?.get_name()?;
            model.reshape_input_by_name(&name, &static_shape)?;
        }

        eprint!("[panda] compiling model for NPU (one-time per session)...");
        let mut compiled = core
            .compile_model(&model, DeviceType::NPU)
            .context("NPU compile_model failed")?;
        eprintln!(" done");

        let out_node = compiled.get_output_by_index(0)?;
        let shape = out_node.get_shape()?;
        let dims = shape.get_dimensions();
        let hidden_size = dims.last().copied().unwrap_or(384) as usize;

        let has_token_type = compiled.get_input_size()? > 2;

        // Query the optimal in-flight infer-request count. The NPU plugin
        // returns 4 on Meteor Lake; clamp to [1, 8] as a sanity bound.
        let pool_size = compiled
            .get_property(&PropertyKey::OptimalNumberOfInferRequests)
            .ok()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .unwrap_or(4)
            .clamp(1, 8);

        let mut requests = Vec::with_capacity(pool_size);
        for _ in 0..pool_size {
            requests.push(
                compiled
                    .create_infer_request()
                    .context("create_infer_request failed")?,
            );
        }
        eprintln!("[panda] NPU infer-request pool size: {}", pool_size);

        let tokenizer = Tokenizer::from_file(tokenizer_path)
            .map_err(|e| anyhow::anyhow!("tokenizer load: {e}"))?;

        Ok(OvEmbedder {
            _core: core,
            _compiled: compiled,
            requests: Mutex::new(requests),
            tokenizer,
            seq_len,
            hidden_size,
            has_token_type,
        })
    }

    /// Embed texts and return L2-normalised vectors (one per input).
    /// Runs the request pool in parallel — N worker threads, each holding one
    /// dedicated InferRequest, processing texts via an atomic work counter.
    pub fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        // Drain the request pool. Threads will return their request at the
        // end; we refill the pool before returning.
        let owned_reqs: Vec<openvino::InferRequest> = {
            let mut guard = self.requests.lock().unwrap();
            std::mem::take(&mut *guard)
        };
        if owned_reqs.is_empty() {
            return Err(anyhow::anyhow!("NPU request pool empty"));
        }

        let next = AtomicUsize::new(0);
        let local_degraded = AtomicBool::new(false);
        let outputs: Vec<Mutex<Option<Vec<f32>>>> =
            (0..texts.len()).map(|_| Mutex::new(None)).collect();

        let returned = thread::scope(|s| -> Vec<openvino::InferRequest> {
            let handles: Vec<_> = owned_reqs
                .into_iter()
                .map(|mut req| {
                    let next_ref = &next;
                    let degraded_ref = &local_degraded;
                    let outputs_ref = &outputs;
                    s.spawn(move || -> openvino::InferRequest {
                        loop {
                            if degraded_ref.load(Ordering::Relaxed) {
                                break;
                            }
                            let idx = next_ref.fetch_add(1, Ordering::Relaxed);
                            if idx >= texts.len() {
                                break;
                            }
                            let result = match self.embed_one_with(&mut req, texts[idx]) {
                                Ok(v) => Some(v),
                                Err(_) => self.embed_one_with(&mut req, texts[idx]).ok(),
                            };
                            match result {
                                Some(v) => *outputs_ref[idx].lock().unwrap() = Some(v),
                                None => {
                                    degraded_ref.store(true, Ordering::Relaxed);
                                    break;
                                }
                            }
                        }
                        req
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });

        // Refill the pool whether we succeeded or not — the requests are still
        // valid even after a single failed infer.
        *self.requests.lock().unwrap() = returned;

        if local_degraded.load(Ordering::Relaxed) {
            mark_degraded("infer error after retry");
            return Err(anyhow::anyhow!("NPU embed failed; using CPU"));
        }

        let mut out = Vec::with_capacity(texts.len());
        for slot in outputs {
            match slot.into_inner().unwrap() {
                Some(v) => out.push(v),
                None => return Err(anyhow::anyhow!("NPU embed produced no output for index")),
            }
        }
        Ok(out)
    }

    /// Run one tokenise → infer → mean-pool → normalise pipeline against the
    /// supplied dedicated request. Used by the worker loop.
    fn embed_one_with(
        &self,
        req: &mut openvino::InferRequest,
        text: &str,
    ) -> Result<Vec<f32>> {
        let enc = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| anyhow::anyhow!("tokenize: {e}"))?;

        let ids = enc.get_ids();
        let mask = enc.get_attention_mask();
        let actual_len = ids.len().min(self.seq_len);

        let mut id_data = vec![0i64; self.seq_len];
        let mut mask_data = vec![0i64; self.seq_len];
        for k in 0..actual_len {
            id_data[k] = ids[k] as i64;
            mask_data[k] = mask[k] as i64;
        }

        let shape = Shape::new(&[1, self.seq_len as i64])?;
        let mut id_tensor = Tensor::new(ElementType::I64, &shape)?;
        id_tensor.get_data_mut::<i64>()?.copy_from_slice(&id_data);
        let mut mask_tensor = Tensor::new(ElementType::I64, &shape)?;
        mask_tensor.get_data_mut::<i64>()?.copy_from_slice(&mask_data);

        req.set_tensor("input_ids", &id_tensor)?;
        req.set_tensor("attention_mask", &mask_tensor)?;
        if self.has_token_type {
            let tt_tensor = Tensor::new(ElementType::I64, &shape)?;
            let _ = req.set_tensor("token_type_ids", &tt_tensor);
        }

        req.infer()?;

        let out = req.get_output_tensor_by_index(0)?;
        let data: &[f32] = out.get_data::<f32>()?;

        let mut pooled = vec![0.0f32; self.hidden_size];
        let mut count = 0usize;
        for token_idx in 0..actual_len {
            if mask_data[token_idx] == 0 {
                continue;
            }
            let offset = token_idx * self.hidden_size;
            for (j, v) in data[offset..offset + self.hidden_size].iter().enumerate() {
                pooled[j] += v;
            }
            count += 1;
        }
        if count > 0 {
            let n = count as f32;
            pooled.iter_mut().for_each(|v| *v /= n);
        }
        let norm: f32 = pooled.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 1e-9 {
            pooled.iter_mut().for_each(|v| *v /= norm);
        }
        Ok(pooled)
    }
}
