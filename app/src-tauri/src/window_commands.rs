use tauri::{Manager, WebviewWindow, WebviewUrl, LogicalSize, LogicalPosition};
use crate::file_watcher::FileWatcherManager;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[tauri::command]
pub async fn create_floating_widget(app: tauri::AppHandle) -> Result<(), String> {
    // Check if widget window already exists
    if let Some(widget_window) = app.get_webview_window("widget") {
        widget_window.show().map_err(|e| e.to_string())?;
        widget_window.set_focus().map_err(|e| e.to_string())?;
        return Ok(());
    }

    // Create the floating widget window using Tauri v2 API
    let _widget_window = tauri::WebviewWindowBuilder::new(
        &app,
        "widget",
        WebviewUrl::App("widget.html".into())
    )
    .title("Vectora Widget")
    .inner_size(80.0, 80.0)
    .resizable(false)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .transparent(true)
    .position(100.0, 100.0)
    .build()
    .map_err(|e| e.to_string())?;

    // Position in top-right corner
    if let Ok(monitor) = _widget_window.current_monitor() {
        if let Some(monitor) = monitor {
            let size = monitor.size();
            let scale = monitor.scale_factor();
            let logical_size = LogicalSize::new(
                size.width as f64 / scale,
                size.height as f64 / scale
            );

            _widget_window.set_position(tauri::Position::Logical(LogicalPosition::new(
                logical_size.width - 100.0,
                20.0
            ))).map_err(|e| e.to_string())?;
        }
    }

    Ok(())
}

#[tauri::command]
pub async fn show_main_window(window: WebviewWindow) -> Result<(), String> {
    window.show().map_err(|e| e.to_string())?;
    window.set_focus().map_err(|e| e.to_string())?;

    // Hide widget window if it exists
    if let Some(widget) = window.app_handle().get_webview_window("widget") {
        widget.hide().map_err(|e| e.to_string())?;
    }

    Ok(())
}

#[tauri::command]
pub async fn watch_folder(
    app: tauri::AppHandle,
    path: String,
    space_id: String,
) -> Result<(), String> {
    let path_buf = PathBuf::from(&path);
    if !path_buf.exists() || !path_buf.is_dir() {
        return Err(format!("Invalid folder path: {}", path));
    }

    let watcher_state = app.state::<Arc<Mutex<FileWatcherManager>>>();
    let manager = watcher_state.lock().unwrap_or_else(|e| e.into_inner());
    manager.watch_folder(path_buf, Some(space_id)).await?;

    tracing::info!("Watching folder: {}", path);
    Ok(())
}

#[tauri::command]
pub async fn unwatch_folder(
    app: tauri::AppHandle,
    path: String,
) -> Result<(), String> {
    let path_buf = PathBuf::from(&path);

    let watcher_state = app.state::<Arc<Mutex<FileWatcherManager>>>();
    let manager = watcher_state.lock().unwrap_or_else(|e| e.into_inner());
    manager.stop_watching(&path_buf)?;

    tracing::info!("Stopped watching folder: {}", path);
    Ok(())
}

#[tauri::command]
pub async fn watch_global_folder(
    app: tauri::AppHandle,
    path: String,
) -> Result<(), String> {
    let path_buf = PathBuf::from(&path);
    if !path_buf.exists() || !path_buf.is_dir() {
        return Err(format!("Invalid folder path: {}", path));
    }

    let watcher_state = app.state::<Arc<Mutex<FileWatcherManager>>>();
    let manager = watcher_state.lock().unwrap_or_else(|e| e.into_inner());
    manager.watch_folder(path_buf, None).await?;

    tracing::info!("Watching global folder: {}", path);
    Ok(())
}

#[tauri::command]
pub async fn scan_global_folder(
    app: tauri::AppHandle,
    path: String,
) -> Result<(), String> {
    let path_buf = PathBuf::from(&path);
    if !path_buf.exists() || !path_buf.is_dir() {
        return Err(format!("Invalid folder path: {}", path));
    }

    // Emit an event to the frontend with the list of supported files found
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(&path_buf)
        .max_depth(5)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        if let Some(ext) = entry.path().extension() {
            let ext_str = ext.to_string_lossy().to_lowercase();
            if shodh_rag::indexing::is_supported_file_type(&ext_str) {
                files.push(entry.path().to_string_lossy().to_string());
            }
        }
        if files.len() >= 1000 {
            break;
        }
    }

    tracing::info!("Scanned global folder {}: found {} supported files", path, files.len());
    use tauri::Emitter;
    let _ = app.emit("global-folder-scanned", serde_json::json!({
        "path": path,
        "files": files,
        "count": files.len(),
    }));

    Ok(())
}
