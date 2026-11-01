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
