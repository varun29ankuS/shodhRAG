use tauri::{AppHandle, Manager, Window, WindowBuilder, WindowUrl};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentWindow {
    pub id: String,
    pub window_label: String,
    pub document_path: String,
    pub document_title: String,
    pub document_type: String,
    pub position: (i32, i32),
    pub size: (u32, u32),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComparisonView {
    pub id: String,
    pub left_document: String,
    pub right_document: String,
    pub sync_scroll: bool,
    pub highlight_differences: bool,
}

pub struct MultiWindowManager {
    windows: Mutex<HashMap<String, DocumentWindow>>,
    comparisons: Mutex<HashMap<String, ComparisonView>>,
}

impl MultiWindowManager {
    pub fn new() -> Self {
        Self {
            windows: Mutex::new(HashMap::new()),
            comparisons: Mutex::new(HashMap::new()),
        }
    }

    pub async fn open_document_window(
        &self,
        app: &AppHandle,
        document_path: String,
        document_title: String,
        position: Option<(i32, i32)>,
    ) -> Result<String, String> {
        let window_id = Uuid::new_v4().to_string();
        let window_label = format!("document_{}", window_id);

        // Calculate position for tiling
        let (x, y) = position.unwrap_or_else(|| {
            let windows = self.windows.lock().unwrap_or_else(|e| e.into_inner());
            let count = windows.len() as i32;
            // Tile windows
            (100 + (count * 50), 100 + (count * 30))
        });

        // Create new window
        let window = WindowBuilder::new(
            app,
            &window_label,
            WindowUrl::App(format!("/document?path={}&title={}",
                urlencoding::encode(&document_path),
                urlencoding::encode(&document_title)
            ).into())
        )
        .title(&document_title)
        .position(x as f64, y as f64)
        .resizable(true)
        .minimizable(true)
        .maximizable(true)
        .inner_size(800.0, 600.0)
        .build()
        .map_err(|e| format!("Failed to create window: {}", e))?;

        // Store window info
        let doc_window = DocumentWindow {
            id: window_id.clone(),
            window_label: window_label.clone(),
            document_path: document_path.clone(),
            document_title: document_title.clone(),
            document_type: detect_document_type(&document_path),
            position: (x, y),
            size: (800, 600),
        };

        self.windows.lock().unwrap_or_else(|e| e.into_inner()).insert(window_id.clone(), doc_window);

        // Emit window opened event
        app.emit_all("window:opened", &window_id)
            .map_err(|e| format!("Failed to emit event: {}", e))?;

        Ok(window_id)
    }

    pub async fn open_comparison_view(
        &self,
        app: &AppHandle,
        left_doc: String,
        right_doc: String,
    ) -> Result<String, String> {
        let comparison_id = Uuid::new_v4().to_string();
        let window_label = format!("comparison_{}", comparison_id);

        // Create split view window
        let window = WindowBuilder::new(
            app,
            &window_label,
            WindowUrl::App(format!("/compare?left={}&right={}",
                urlencoding::encode(&left_doc),
                urlencoding::encode(&right_doc)
            ).into())
        )
        .title("Document Comparison")
        .resizable(true)
        .maximizable(true)
        .inner_size(1400.0, 800.0)
        .center()
        .build()
        .map_err(|e| format!("Failed to create comparison window: {}", e))?;

        // Store comparison info
        let comparison = ComparisonView {
            id: comparison_id.clone(),
            left_document: left_doc,
            right_document: right_doc,
            sync_scroll: true,
            highlight_differences: true,
        };

        self.comparisons.lock().unwrap_or_else(|e| e.into_inner()).insert(comparison_id.clone(), comparison);

        Ok(comparison_id)
    }

    pub fn tile_windows(&self, app: &AppHandle, layout: &str) -> Result<(), String> {
        let windows = self.windows.lock().unwrap_or_else(|e| e.into_inner());
        let window_count = windows.len();

        if window_count == 0 {
            return Ok(());
        }

        // Get screen dimensions (simplified - in production use proper screen API)
        let screen_width = 1920;
        let screen_height = 1080;
        let taskbar_height = 40;
        let usable_height = screen_height - taskbar_height;

        match layout {
            "vertical" => {
                // Split screen vertically
                let window_width = screen_width / window_count as i32;
                for (index, (_, doc_window)) in windows.iter().enumerate() {
                    if let Some(window) = app.get_window(&doc_window.window_label) {
                        let x = (index as i32) * window_width;
                        window.set_position(tauri::Position::Physical((x, 0).into()))
                            .map_err(|e| format!("Failed to position window: {}", e))?;
                        window.set_size(tauri::Size::Physical((window_width as u32, usable_height as u32).into()))
                            .map_err(|e| format!("Failed to resize window: {}", e))?;
                    }
                }
            },
            "horizontal" => {
                // Split screen horizontally
                let window_height = usable_height / window_count as i32;
                for (index, (_, doc_window)) in windows.iter().enumerate() {
                    if let Some(window) = app.get_window(&doc_window.window_label) {
                        let y = (index as i32) * window_height;
                        window.set_position(tauri::Position::Physical((0, y).into()))
                            .map_err(|e| format!("Failed to position window: {}", e))?;
                        window.set_size(tauri::Size::Physical((screen_width as u32, window_height as u32).into()))
                            .map_err(|e| format!("Failed to resize window: {}", e))?;
                    }
                }
            },
            "grid" => {
                // 2x2 grid layout
                let cols = ((window_count as f32).sqrt().ceil()) as usize;
                let rows = (window_count + cols - 1) / cols;
                let window_width = screen_width / cols as i32;
                let window_height = usable_height / rows as i32;

                for (index, (_, doc_window)) in windows.iter().enumerate() {
                    if let Some(window) = app.get_window(&doc_window.window_label) {
                        let col = index % cols;
                        let row = index / cols;
                        let x = (col as i32) * window_width;
                        let y = (row as i32) * window_height;

                        window.set_position(tauri::Position::Physical((x, y).into()))
                            .map_err(|e| format!("Failed to position window: {}", e))?;
                        window.set_size(tauri::Size::Physical((window_width as u32, window_height as u32).into()))
                            .map_err(|e| format!("Failed to resize window: {}", e))?;
                    }
                }
            },
            _ => return Err("Unknown layout".to_string()),
        }

        Ok(())
    }

    pub fn sync_scroll(&self, source_window_id: &str, scroll_position: f32) -> Result<(), String> {
        let comparisons = self.comparisons.lock().unwrap_or_else(|e| e.into_inner());

        // Find comparisons involving this window
        for (_, comparison) in comparisons.iter() {
            if comparison.sync_scroll {
                // Emit scroll sync event to comparison windows
                // This would be handled by the frontend
            }
        }

        Ok(())
    }

    pub fn get_all_windows(&self) -> Vec<DocumentWindow> {
        self.windows.lock().unwrap_or_else(|e| e.into_inner()).values().cloned().collect()
    }

    pub fn close_window(&self, window_id: &str) -> Result<(), String> {
        self.windows.lock().unwrap_or_else(|e| e.into_inner()).remove(window_id);
        Ok(())
    }
}

fn detect_document_type(path: &str) -> String {
    let extension = path.split('.').last().unwrap_or("").to_lowercase();
    match extension.as_str() {
        "pdf" => "PDF Document".to_string(),
        "docx" | "doc" => "Word Document".to_string(),
        "txt" => "Text File".to_string(),
        "md" => "Markdown".to_string(),
        "rs" | "py" | "js" | "ts" => "Source Code".to_string(),
        _ => "Document".to_string(),
    }
}

// Tauri Commands
#[tauri::command]
pub async fn open_in_new_window(
    app: AppHandle,
    manager: tauri::State<'_, MultiWindowManager>,
    document_path: String,
    document_title: String,
) -> Result<String, String> {
    manager.open_document_window(&app, document_path, document_title, None).await
}

#[tauri::command]
pub async fn open_comparison(
    app: AppHandle,
    manager: tauri::State<'_, MultiWindowManager>,
    left_document: String,
    right_document: String,
) -> Result<String, String> {
    manager.open_comparison_view(&app, left_document, right_document).await
}

#[tauri::command]
pub fn arrange_windows(
    app: AppHandle,
    manager: tauri::State<'_, MultiWindowManager>,
    layout: String,
) -> Result<(), String> {
    manager.tile_windows(&app, &layout)
}

#[tauri::command]
pub fn get_open_windows(
    manager: tauri::State<'_, MultiWindowManager>,
) -> Result<Vec<DocumentWindow>, String> {
    Ok(manager.get_all_windows())
}