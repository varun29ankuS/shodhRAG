use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tauri::{AppHandle, Manager, Emitter};
use tokio::sync::mpsc;

pub struct FileWatcherManager {
    watchers: Arc<Mutex<HashMap<PathBuf, WatcherState>>>,
    app_handle: AppHandle,
}

struct WatcherState {
    watcher: RecommendedWatcher,
    space_id: Option<String>,
    last_events: HashMap<PathBuf, SystemTime>,
}

#[derive(Clone, serde::Serialize)]
struct FileChangeEvent {
    path: String,
    change_type: String,
    space_id: Option<String>,
    timestamp: String,
}

impl FileWatcherManager {
    pub fn new(app_handle: AppHandle) -> Self {
        Self {
            watchers: Arc::new(Mutex::new(HashMap::new())),
            app_handle,
        }
    }

    pub async fn watch_folder(
        &self,
        path: PathBuf,
        space_id: Option<String>,
    ) -> Result<(), String> {
        let (tx, mut rx) = mpsc::channel(100);
        let app_handle = self.app_handle.clone();
        let space_id_clone = space_id.clone();
        
        // Create watcher with debouncing
        let watcher = RecommendedWatcher::new(
            move |res: Result<Event, notify::Error>| {
                if let Ok(event) = res {
                    let _ = tx.blocking_send(event);
                }
            },
            Config::default().with_poll_interval(Duration::from_secs(2)),
        ).map_err(|e| format!("Failed to create watcher: {}", e))?;

        // Start watching
        let mut watcher = watcher;
        watcher
            .watch(&path, RecursiveMode::Recursive)
            .map_err(|e| format!("Failed to watch path: {}", e))?;

        // Store watcher
        let mut watchers = self.watchers.lock().unwrap();
        watchers.insert(
            path.clone(),
            WatcherState {
                watcher,
                space_id: space_id.clone(),
                last_events: HashMap::new(),
            },
        );

        // Spawn event handler
        let watchers_clone = self.watchers.clone();
        let path_clone = path.clone();
        
        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                Self::handle_event(
                    event,
                    &app_handle,
                    &space_id_clone,
                    &watchers_clone,
                    &path_clone,
                ).await;
            }
        });

        tracing::info!("Started watching: {:?}", path);
        Ok(())
    }

    pub fn stop_watching(&self, path: &Path) -> Result<(), String> {
        let mut watchers = self.watchers.lock().unwrap();
        if watchers.remove(path).is_some() {
            tracing::info!("Stopped watching: {:?}", path);
            Ok(())
        } else {
            Err(format!("No watcher found for path: {:?}", path))
        }
    }

    async fn handle_event(
        event: Event,
        app_handle: &AppHandle,
        space_id: &Option<String>,
        watchers: &Arc<Mutex<HashMap<PathBuf, WatcherState>>>,
        watch_path: &Path,
    ) {
        // Debounce events (ignore if same file changed within 2 seconds)
        let should_process = {
            let mut watchers = watchers.lock().unwrap();
            if let Some(state) = watchers.get_mut(watch_path) {
                let now = SystemTime::now();
                let mut process = true;
                
                for path in &event.paths {
                    if let Some(last_time) = state.last_events.get(path) {
                        if now.duration_since(*last_time).unwrap_or_default() < Duration::from_secs(2) {
                            process = false;
                            break;
                        }
                    }
                    state.last_events.insert(path.clone(), now);
                }
                process
            } else {
                false
            }
        };

        if !should_process {
            return;
        }

        // Process different event types
        let change_type = match event.kind {
            EventKind::Create(_) => "created",
            EventKind::Modify(_) => "modified",
            EventKind::Remove(_) => "deleted",
            _ => return, // Ignore other events
        };

        for path in event.paths {
            // Skip directories and non-supported files
            if path.is_dir() {
                continue;
            }

            // Check if file type is supported
            if !Self::is_supported_file(&path) {
                continue;
            }

            // Emit event to frontend
            let file_event = FileChangeEvent {
                path: path.to_string_lossy().to_string(),
                change_type: change_type.to_string(),
                space_id: space_id.clone(),
                timestamp: chrono::Local::now().to_rfc3339(),
            };

            let _ = app_handle.emit("file-change", &file_event);

            // Auto-index new or modified files
            if change_type == "created" || change_type == "modified" {
                Self::auto_index_file(&app_handle, &path, space_id).await;
            } else if change_type == "deleted" {
                Self::remove_from_index(&app_handle, &path, space_id).await;
            }
        }
    }

    fn is_supported_file(path: &Path) -> bool {
        if let Some(ext) = path.extension() {
            let ext_str = ext.to_string_lossy().to_lowercase();
            matches!(
                ext_str.as_str(),
                "txt" | "md" | "pdf" | "html" | "json" | "csv" | "docx" | 
                "rs" | "py" | "js" | "ts" | "jsx" | "tsx" | "java" | "cpp" | 
                "c" | "h" | "go" | "rb" | "php" | "swift" | "kt"
            )
        } else {
            false
        }
    }

    async fn auto_index_file(app_handle: &AppHandle, path: &Path, space_id: &Option<String>) {
        // Call the RAG indexing command
        if let Ok(content) = std::fs::read_to_string(path) {
            let metadata = HashMap::from([
                ("source".to_string(), path.to_string_lossy().to_string()),
                ("auto_indexed".to_string(), "true".to_string()),
                ("space_id".to_string(), space_id.clone().unwrap_or_default()),
            ]);

            // Use invoke to call the add_document command
            if let Err(e) = app_handle
                .emit("auto-index-file", serde_json::json!({
                    "path": path.to_string_lossy(),
                    "content": content,
                    "metadata": metadata,
                }))
            {
                tracing::error!("Failed to auto-index file: {}", e);
            }
        }
    }

    async fn remove_from_index(app_handle: &AppHandle, path: &Path, space_id: &Option<String>) {
        // Emit event to remove from index
        let _ = app_handle.emit("remove-from-index", serde_json::json!({
            "path": path.to_string_lossy(),
            "space_id": space_id,
        }));
    }

    pub fn get_watching_paths(&self) -> Vec<PathBuf> {
        let watchers = self.watchers.lock().unwrap();
        watchers.keys().cloned().collect()
    }
}

// Commands for Tauri
#[tauri::command]
pub async fn start_watching_folder(
    app_handle: AppHandle,
    path: String,
    space_id: Option<String>,
) -> Result<String, String> {
    let path_buf = PathBuf::from(&path);
    if !path_buf.exists() {
        return Err(format!("Path does not exist: {}", path));
    }
    
    // Create a new FileWatcherManager instance for this specific watch
    let file_watcher = FileWatcherManager::new(app_handle.clone());
    file_watcher.watch_folder(path_buf.clone(), space_id).await?;
    
    // Store it in the app state
    let watcher_manager = app_handle.state::<Arc<Mutex<FileWatcherManager>>>();
    let mut manager = watcher_manager.lock().unwrap();
    *manager = file_watcher;
    
    Ok(format!("Started watching: {}", path))
}

#[tauri::command]
pub async fn stop_watching_folder(
    app_handle: AppHandle,
    path: String,
) -> Result<String, String> {
    let watcher_manager = app_handle.state::<Arc<Mutex<FileWatcherManager>>>();
    let path_buf = PathBuf::from(&path);
    
    let result = {
        let manager = watcher_manager.lock().unwrap();
        manager.stop_watching(&path_buf)
    };
    
    result?;
    Ok(format!("Stopped watching: {}", path))
}

#[tauri::command]
pub async fn get_watched_folders(app_handle: AppHandle) -> Result<Vec<String>, String> {
    let watcher_manager = app_handle.state::<Arc<Mutex<FileWatcherManager>>>();
    
    let paths = {
        let manager = watcher_manager.lock().unwrap();
        manager
            .get_watching_paths()
            .into_iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect()
    };
    
    Ok(paths)
}