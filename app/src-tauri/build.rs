use std::env;
use std::fs;
use std::path::Path;

fn main() {
    // Copy onnxruntime-genai.dll to output directory for runtime loading
    copy_genai_dll();

    tauri_build::build()
}

fn copy_genai_dll() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let out_dir = env::var("OUT_DIR").unwrap();

    // Navigate from OUT_DIR to the actual target/debug or target/release directory
    let target_dir = Path::new(&out_dir)
        .parent().unwrap()
        .parent().unwrap()
        .parent().unwrap();

    let libs_dir = Path::new(&manifest_dir)
        .parent().unwrap()
        .parent().unwrap()
        .join("libs");

    // Copy all required DLLs
    let dlls = vec![
        "onnxruntime-genai.dll",
        "onnxruntime.dll",
        "onnxruntime_providers_shared.dll",
    ];

    for dll_name in dlls {
        let dll_source = libs_dir.join(dll_name);
        let dll_dest = target_dir.join(dll_name);

        if dll_source.exists() {
            match fs::copy(&dll_source, &dll_dest) {
                Ok(_) => println!("cargo:warning=✓ Copied {} to target directory", dll_name),
                Err(e) => println!("cargo:warning=⚠ Failed to copy {}: {}", dll_name, e),
            }
        } else {
            println!("cargo:warning=⚠ {} not found at: {}", dll_name, dll_source.display());
        }

        // Rerun if DLL changes
        println!("cargo:rerun-if-changed={}", dll_source.display());
    }
}
