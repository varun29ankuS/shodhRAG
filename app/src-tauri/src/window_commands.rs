use tauri::{Manager, WebviewWindow, WebviewUrl, LogicalSize, LogicalPosition};

#[tauri::command]
pub async fn create_floating_widget(app: tauri::AppHandle) -> Result<(), String> {
    // Check if widget window already exists
    if let Some(widget_window) = app.get_webview_window("widget") {
        widget_window.show().map_err(|e| e.to_string())?;
        widget_window.set_focus().map_err(|e| e.to_string())?;
        return Ok(());
    }

    // Create the floating widget window using Tauri v2 API
    let widget_window = tauri::WebviewWindowBuilder::new(
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
    if let Ok(monitor) = widget_window.current_monitor() {
        if let Some(monitor) = monitor {
            let size = monitor.size();
            let scale = monitor.scale_factor();
            let logical_size = LogicalSize::new(
                size.width as f64 / scale,
                size.height as f64 / scale
            );
            
            widget_window.set_position(tauri::Position::Logical(LogicalPosition::new(
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
pub async fn watch_folder(path: String, space_id: String) -> Result<(), String> {
    // TODO: Implement folder watching logic
    // This would integrate with a file system watcher like notify-rs
    tracing::info!("Watching folder: {} for space: {}", path, space_id);
    Ok(())
}

#[tauri::command]
pub async fn unwatch_folder(path: String) -> Result<(), String> {
    // TODO: Implement folder unwatching logic
    tracing::info!("Stopped watching folder: {}", path);
    Ok(())
}

#[tauri::command]
pub async fn watch_global_folder(path: String) -> Result<(), String> {
    // TODO: Implement global folder watching
    tracing::info!("Watching global folder: {}", path);
    Ok(())
}

#[tauri::command]
pub async fn scan_global_folder(path: String) -> Result<(), String> {
    // TODO: Scan and import existing files from global folder
    tracing::info!("Scanning global folder: {}", path);
    Ok(())
}

#[tauri::command]
pub async fn delete_space(space_id: String) -> Result<(), String> {
    // TODO: Delete space from database
    tracing::info!("Deleting space: {}", space_id);
    Ok(())
}

#[tauri::command]
pub async fn rename_space(space_id: String, new_name: String) -> Result<(), String> {
    // TODO: Rename space in database
    tracing::info!("Renaming space {} to {}", space_id, new_name);
    Ok(())
}