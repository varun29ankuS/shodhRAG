use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    // Copy the ONNX Runtime DLLs (used by the E5 embedder and the reranker)
    // next to the binary when a local copy exists in `<repo>/libs`.
    copy_onnxruntime_dlls();

    tauri_build::build()
}

/// `<target>/<profile>` for this build, derived from OUT_DIR
/// (`<target>/<profile>/build/<pkg>-<hash>/out`).
fn profile_dir() -> Option<PathBuf> {
    let out_dir = PathBuf::from(env::var_os("OUT_DIR")?);
    Some(out_dir.parent()?.parent()?.parent()?.to_path_buf())
}

fn copy_onnxruntime_dlls() {
    let Some(manifest_dir) = env::var_os("CARGO_MANIFEST_DIR").map(PathBuf::from) else {
        return;
    };
    let Some(target_dir) = profile_dir() else {
        println!("cargo:warning=Could not determine the target directory from OUT_DIR");
        return;
    };
    let Some(libs_dir) = manifest_dir
        .parent()
        .and_then(Path::parent)
        .map(|p| p.join("libs"))
    else {
        return;
    };

    for dll_name in ["onnxruntime.dll", "onnxruntime_providers_shared.dll"] {
        let dll_source = libs_dir.join(dll_name);
        println!("cargo:rerun-if-changed={}", dll_source.display());
        if !dll_source.exists() {
            continue;
        }
        if let Err(e) = fs::copy(&dll_source, target_dir.join(dll_name)) {
            println!("cargo:warning=Failed to copy {}: {}", dll_name, e);
        }
    }
}
