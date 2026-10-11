// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Started as the OCR child (see shodh_rag::processing::ocr::worker): read
    // scans for the app over stdin/stdout, without the app's window or state.
    if std::env::args_os().nth(1).as_deref()
        == Some(std::ffi::OsStr::new(
            shodh_rag::processing::ocr::worker::WORKER_ARG,
        ))
    {
        shodh_rag::processing::ocr::worker::serve_stdio();
    }
    shodh_rag_app_lib::run()
}
