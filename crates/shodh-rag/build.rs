//! Build-time configuration for shodh-rag.
//!
//! Model-backed tests (tokenizer loading, ONNX reranking) need model files that are
//! not checked in. They are `#[ignore]`d unless `SHODH_TEST_MODELS` is set when the
//! crate is compiled; with the variable set they run as ordinary tests, so a missing
//! or broken model directory fails the run instead of being silently skipped.

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-env-changed=SHODH_TEST_MODELS");
    println!("cargo::rustc-check-cfg=cfg(shodh_test_models)");

    if std::env::var_os("SHODH_TEST_MODELS").is_some_and(|v| !v.is_empty()) {
        println!("cargo::rustc-cfg=shodh_test_models");
    }
}
