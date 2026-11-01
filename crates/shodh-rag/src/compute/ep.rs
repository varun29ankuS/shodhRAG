//! The GPU execution providers, appended to a session's options through
//! ONNX Runtime's C API.
//!
//! The ONNX Runtime library ort-sys links already contains them: its
//! prebuilt Windows library is the DirectML build and its Apple silicon
//! library the Core ML build (ort-sys's `dist.tsv` has no other for those
//! targets). The `ort` crate wraps them only behind its `directml` and
//! `coreml` features, which would also have to be enabled on every
//! platform's dependency graph that CI resolves the archive from; calling
//! the two C entry points directly keeps the archive, its cache key and the
//! feature set exactly as they are.
//!
//! Each call reports ONNX Runtime's own error: a provider that does not
//! start is an error here, never a silent fallback.

use std::ffi::{c_char, c_void, CStr, CString};

use ort::session::builder::SessionBuilder;
use ort::AsPointer;

/// `Ok` for a null status, else its message (the status is released).
fn check(status: ort::sys::OrtStatusPtr) -> Result<(), String> {
    if status.0.is_null() {
        return Ok(());
    }
    let api = ort::api();
    // SAFETY: a non-null status from ONNX Runtime; its message lives until
    // the status is released, which happens after the copy.
    let message = unsafe {
        let text = (api.GetErrorMessage)(status.0);
        let message = if text.is_null() {
            "unknown error".to_string()
        } else {
            CStr::from_ptr(text).to_string_lossy().into_owned()
        };
        (api.ReleaseStatus)(status.0);
        message
    };
    Err(message)
}

/// Appends DirectML on the DXGI adapter `device_id` to `builder`.
pub fn append_directml(builder: &mut SessionBuilder, device_id: i32) -> Result<(), String> {
    let api = ort::api();
    let mut dml: *const c_void = std::ptr::null();
    // SAFETY: the provider name is NUL-terminated and `dml` receives a
    // pointer to a static table owned by ONNX Runtime.
    check(unsafe { (api.GetExecutionProviderApi)(c"DML".as_ptr(), 0, &mut dml) })
        .map_err(|e| format!("this ONNX Runtime has no DirectML provider: {e}"))?;
    if dml.is_null() {
        return Err("this ONNX Runtime has no DirectML provider".to_string());
    }
    // SAFETY: GetExecutionProviderApi("DML") returns an `OrtDmlApi` table.
    let dml = unsafe { &*dml.cast::<ort::sys::OrtDmlApi>() };
    // SAFETY: the session options are valid for the builder's life.
    check(unsafe { (dml.SessionOptionsAppendExecutionProvider_DML)(builder.ptr_mut(), device_id) })
}

/// Appends Core ML with `options` (its provider option keys and values) to
/// `builder`.
pub fn append_coreml(
    builder: &mut SessionBuilder,
    options: &[(&str, String)],
) -> Result<(), String> {
    let mut keys = Vec::with_capacity(options.len());
    let mut values = Vec::with_capacity(options.len());
    for (key, value) in options {
        keys.push(CString::new(*key).map_err(|e| format!("Core ML option {key}: {e}"))?);
        values
            .push(CString::new(value.as_str()).map_err(|e| format!("Core ML option {key}: {e}"))?);
    }
    let key_ptrs: Vec<*const c_char> = keys.iter().map(|k| k.as_ptr()).collect();
    let value_ptrs: Vec<*const c_char> = values.iter().map(|v| v.as_ptr()).collect();
    // SAFETY: the name and every key and value are NUL-terminated and live
    // until the call returns; ONNX Runtime copies them.
    check(unsafe {
        (ort::api().SessionOptionsAppendExecutionProvider)(
            builder.ptr_mut(),
            c"CoreML".as_ptr(),
            key_ptrs.as_ptr(),
            value_ptrs.as_ptr(),
            key_ptrs.len(),
        )
    })
}

/// The execution providers the linked ONNX Runtime was built with
/// ("DmlExecutionProvider", "CoreMLExecutionProvider", "CPUExecutionProvider", ...).
pub fn available_providers() -> Result<Vec<String>, String> {
    let api = ort::api();
    let mut list: *mut *mut c_char = std::ptr::null_mut();
    let mut count: std::ffi::c_int = 0;
    // SAFETY: both out-pointers are valid; the list is released below.
    check(unsafe { (api.GetAvailableProviders)(&mut list, &mut count) })?;
    if list.is_null() {
        return Ok(Vec::new());
    }
    let names = (0..usize::try_from(count).unwrap_or(0))
        .map(|i| {
            // SAFETY: `list` holds `count` NUL-terminated names.
            unsafe { CStr::from_ptr(*list.add(i)) }
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    // SAFETY: the list and count are the ones GetAvailableProviders gave.
    check(unsafe { (api.ReleaseAvailableProviders)(list, count) })?;
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ONNX Runtime this build links has the GPU provider of its
    /// platform: DirectML on Windows, Core ML on Apple silicon.
    #[test]
    fn the_linked_onnx_runtime_has_this_platforms_gpu_provider() {
        let providers = available_providers().expect("providers listed");
        assert!(
            providers.iter().any(|p| p == "CPUExecutionProvider"),
            "{providers:?}"
        );
        #[cfg(windows)]
        assert!(
            providers.iter().any(|p| p == "DmlExecutionProvider"),
            "{providers:?}"
        );
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        assert!(
            providers.iter().any(|p| p == "CoreMLExecutionProvider"),
            "{providers:?}"
        );
    }

    /// Core ML takes the options Shodh appends it with (on Apple silicon;
    /// no model is needed to register the provider).
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn core_ml_registers_with_shodhs_options() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut builder = ort::session::Session::builder().expect("session builder");
        append_coreml(
            &mut builder,
            &[
                ("ModelFormat", "MLProgram".to_string()),
                ("MLComputeUnits", "ALL".to_string()),
                ("ModelCacheDirectory", dir.path().display().to_string()),
            ],
        )
        .expect("Core ML registered");
    }
}
