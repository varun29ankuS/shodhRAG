use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    // Copy the ONNX Runtime DLLs (used by the E5 embedder and the reranker)
    // next to the binary when a local copy exists in `<repo>/libs`.
    copy_onnxruntime_dlls();

    let attributes = tauri_build::Attributes::new();
    // The tray menu needs Common Controls v6 (TaskDialogIndirect and
    // friends). tauri-build embeds that manifest in the app binary only, so
    // the unit-test binary, which links the same menu code, would fail to
    // start (STATUS_ENTRYPOINT_NOT_FOUND). Embed it through the linker for
    // every target of this package instead.
    #[cfg(windows)]
    let attributes = {
        embed_windows_manifest();
        attributes.windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest())
    };
    if let Err(e) = tauri_build::try_build(attributes) {
        panic!("tauri-build failed: {e:#}");
    }
}

/// Embed `windows-app-manifest.xml` in every binary this package links.
#[cfg(windows)]
fn embed_windows_manifest() {
    let Some(manifest_dir) = env::var_os("CARGO_MANIFEST_DIR").map(PathBuf::from) else {
        println!("cargo:warning=CARGO_MANIFEST_DIR is not set; no Windows manifest embedded");
        return;
    };
    let manifest = manifest_dir.join("windows-app-manifest.xml");
    println!("cargo:rerun-if-changed={}", manifest.display());
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
    }
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
