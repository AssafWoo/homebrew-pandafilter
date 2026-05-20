//! Feature-gated CPU-fallback test for the OpenVINO bypass.
//!
//! Lives in its own integration-test file so it runs in a separate process
//! from `npu_smoke.rs`. Both tests touch the `OV_EMBEDDER` `OnceCell`, and
//! once one of them caches `None` the other can't recover within the same
//! process.

#![cfg(feature = "openvino")]

use panda_core::summarizer;

fn npu_opted_in() -> bool {
    std::env::var("OPENVINO_NPU_AVAILABLE").ok().as_deref() == Some("1")
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
