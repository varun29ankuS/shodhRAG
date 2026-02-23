//! Google Drive Integration Commands
//!
//! Provides OAuth2 authentication and file sync from Google Drive to Shodh spaces.
//! Allows lawyers to automatically index case files stored in Google Drive.

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use parking_lot::RwLock;
use anyhow::{Result, Context};
use tauri::{State, Manager, Emitter};
use reqwest::Client;
use chrono::{DateTime, Utc};
use crate::rag_commands::RagState;

/// Google Drive OAuth2 configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleDriveConfig {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: String,
}

/// Google Drive OAuth2 tokens
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleDriveTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: DateTime<Utc>,
}

/// Google Drive file metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriveFile {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub size: Option<i64>,
    pub modified_time: String,
    pub web_view_link: Option<String>,
    pub is_folder: bool,
}

/// Folder sync configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolderSyncConfig {
    pub folder_id: String,
    pub folder_name: String,
    pub space_id: String,  // Target Shodh space
    pub auto_sync: bool,
    pub sync_subdirectories: bool,
}

/// Sync status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncStatus {
    pub is_syncing: bool,
    pub total_files: usize,
    pub synced_files: usize,
    pub failed_files: usize,
    pub last_sync: Option<DateTime<Utc>>,
    pub error: Option<String>,
}

/// Global state for Google Drive integration
pub struct GoogleDriveState {
    pub tokens: RwLock<Option<GoogleDriveTokens>>,
    pub config: RwLock<Option<GoogleDriveConfig>>,
    pub sync_configs: RwLock<Vec<FolderSyncConfig>>,
    pub sync_status: RwLock<SyncStatus>,
    pub http_client: Client,
}

impl GoogleDriveState {
    pub fn new() -> Self {
        Self {
            tokens: RwLock::new(None),
            config: RwLock::new(None),
            sync_configs: RwLock::new(Vec::new()),
            sync_status: RwLock::new(SyncStatus {
                is_syncing: false,
                total_files: 0,
                synced_files: 0,
                failed_files: 0,
                last_sync: None,
                error: None,
            }),
            http_client: Client::new(),
        }
    }
}

impl Default for GoogleDriveState {
    fn default() -> Self {
        Self::new()
    }
}

impl GoogleDriveState {
    /// Refresh access token if it expires within the next 60 seconds.
    /// Returns the current (possibly refreshed) access token.
    pub async fn get_valid_token(&self) -> Result<String, String> {
        let needs_refresh = {
            let tokens = self.tokens.read();
            match tokens.as_ref() {
                Some(t) => t.expires_at < Utc::now() + chrono::Duration::seconds(60),
                None => return Err("Not authenticated with Google Drive".to_string()),
            }
        };

        if needs_refresh {
            let refresh_token = {
                let tokens = self.tokens.read();
                tokens.as_ref()
                    .and_then(|t| t.refresh_token.clone())
            };
            let config = self.config.read().clone();

            match (refresh_token, config) {
                (Some(rt), Some(cfg)) => {
                    tracing::info!("Refreshing Google Drive access token...");

                    #[derive(Deserialize)]
                    struct RefreshResponse {
                        access_token: String,
                        expires_in: i64,
                    }

                    let params = [
                        ("client_id", cfg.client_id.as_str()),
                        ("client_secret", cfg.client_secret.as_str()),
                        ("refresh_token", rt.as_str()),
                        ("grant_type", "refresh_token"),
                    ];

                    let resp = self.http_client
                        .post("https://oauth2.googleapis.com/token")
                        .form(&params)
                        .send()
                        .await
                        .map_err(|e| format!("Token refresh request failed: {}", e))?;

                    let data: RefreshResponse = resp.json().await
                        .map_err(|e| format!("Token refresh parse failed: {}", e))?;

                    let new_token = data.access_token.clone();
                    let mut tokens = self.tokens.write();
                    if let Some(t) = tokens.as_mut() {
                        t.access_token = data.access_token;
                        t.expires_at = Utc::now() + chrono::Duration::seconds(data.expires_in);
                    }
                    tracing::info!("Access token refreshed");
                    Ok(new_token)
                }
                _ => Err("Cannot refresh: missing refresh token or config".to_string()),
            }
        } else {
            let tokens = self.tokens.read();
            tokens.as_ref()
                .map(|t| t.access_token.clone())
                .ok_or_else(|| "Not authenticated: no access token available".to_string())
        }
    }
}

/// Initialize Google Drive OAuth2 configuration and start a one-shot callback server.
/// The callback server listens on localhost:3000 and emits `google-drive-auth-code`
/// when Google redirects back with the authorization code.
#[tauri::command]
pub async fn init_google_drive_oauth(
    client_id: String,
    client_secret: String,
    state: State<'_, Arc<GoogleDriveState>>,
    app_handle: tauri::AppHandle,
) -> Result<String, String> {
    tracing::info!("Initializing Google Drive OAuth...");

    let redirect_uri = "http://localhost:3000/oauth/google/callback".to_string();

    let config = GoogleDriveConfig {
        client_id: client_id.clone(),
        client_secret,
        redirect_uri: redirect_uri.clone(),
    };

    *state.config.write() = Some(config.clone());

    // Build OAuth2 authorization URL
    let auth_url = format!(
        "https://accounts.google.com/o/oauth2/v2/auth?client_id={}&redirect_uri={}&response_type=code&scope={}&access_type=offline&prompt=consent",
        client_id,
        urlencoding::encode(&redirect_uri),
        urlencoding::encode("https://www.googleapis.com/auth/drive.readonly")
    );

    // Start a one-shot callback server in the background
    let handle = app_handle.clone();
    tokio::spawn(async move {
        if let Err(e) = run_oauth_callback_server(handle).await {
            tracing::error!("OAuth callback server error: {}", e);
        }
    });

    tracing::info!("OAuth URL generated, callback server started on :3000");
    Ok(auth_url)
}

/// One-shot HTTP server that captures the OAuth redirect and emits a Tauri event.
/// Shuts down immediately after receiving one request.
async fn run_oauth_callback_server(app_handle: tauri::AppHandle) -> Result<(), String> {
    use axum::{Router, routing::get, extract::Query, response::Html};
    use std::collections::HashMap;

    let handle = app_handle.clone();

    let app = Router::new().route("/oauth/google/callback", get(
        |Query(params): Query<HashMap<String, String>>| async move {
            if let Some(code) = params.get("code") {
                let _ = handle.emit("google-drive-auth-code", code.clone());
                Html(r#"<html><body style="font-family:system-ui;display:flex;align-items:center;justify-content:center;height:100vh;margin:0;background:#0c0c0d;color:#f0f0f2">
                    <div style="text-align:center">
                        <h2>Authentication Successful</h2>
                        <p style="color:#888">You can close this tab and return to Shodh.</p>
                        <script>setTimeout(()=>window.close(),2000)</script>
                    </div>
                </body></html>"#.to_string())
            } else {
                let error = params.get("error").cloned().unwrap_or_else(|| "Unknown error".to_string());
                let _ = handle.emit("google-drive-auth-error", error.clone());
                Html(format!(r#"<html><body style="font-family:system-ui;display:flex;align-items:center;justify-content:center;height:100vh;margin:0;background:#0c0c0d;color:#f0f0f2">
                    <div style="text-align:center">
                        <h2>Authentication Failed</h2>
                        <p style="color:#ef4444">{}</p>
                    </div>
                </body></html>"#, error))
            }
        }
    ));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000").await
        .map_err(|e| format!("Failed to bind :3000 for OAuth callback: {}", e))?;

    // Serve exactly one request then shut down
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            // Give it 5 minutes max, then shut down
            tokio::time::sleep(std::time::Duration::from_secs(300)).await;
        })
        .await
        .map_err(|e| format!("OAuth callback server failed: {}", e))?;

    Ok(())
}

/// Exchange authorization code for access token
#[tauri::command]
pub async fn exchange_google_drive_code(
    code: String,
    state: State<'_, Arc<GoogleDriveState>>,
) -> Result<GoogleDriveTokens, String> {
    tracing::info!("🔄 Exchanging authorization code for access token...");

    let config = state.config.read()
        .as_ref()
        .ok_or("OAuth not initialized")?
        .clone();

    // Exchange code for token
    let params = [
        ("client_id", config.client_id.as_str()),
        ("client_secret", config.client_secret.as_str()),
        ("code", code.as_str()),
        ("redirect_uri", config.redirect_uri.as_str()),
        ("grant_type", "authorization_code"),
    ];

    let response = state.http_client
        .post("https://oauth2.googleapis.com/token")
        .form(&params)
        .send()
        .await
        .map_err(|e| format!("Token exchange failed: {}", e))?;

    #[derive(Deserialize)]
    struct TokenResponse {
        access_token: String,
        refresh_token: Option<String>,
        expires_in: i64,
    }

    let token_data: TokenResponse = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse token response: {}", e))?;

    let tokens = GoogleDriveTokens {
        access_token: token_data.access_token,
        refresh_token: token_data.refresh_token,
        expires_at: Utc::now() + chrono::Duration::seconds(token_data.expires_in),
    };

    *state.tokens.write() = Some(tokens.clone());

    tracing::info!("✅ Access token obtained");
    Ok(tokens)
}

/// List files in a Google Drive folder
#[tauri::command]
pub async fn list_google_drive_files(
    folder_id: Option<String>,
    state: State<'_, Arc<GoogleDriveState>>,
) -> Result<Vec<DriveFile>, String> {
    tracing::info!("Listing Google Drive files...");

    let access_token = state.get_valid_token().await?;

    // Build query
    let query = if let Some(fid) = folder_id {
        format!("'{}' in parents and trashed=false", fid)
    } else {
        "trashed=false".to_string()
    };

    let url = format!(
        "https://www.googleapis.com/drive/v3/files?q={}&fields=files(id,name,mimeType,size,modifiedTime,webViewLink)&pageSize=100",
        urlencoding::encode(&query)
    );

    let response = state.http_client
        .get(&url)
        .bearer_auth(&access_token)
        .send()
        .await
        .map_err(|e| format!("Failed to list files: {}", e))?;

    #[derive(Deserialize)]
    struct FileListResponse {
        files: Vec<FileItem>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct FileItem {
        id: String,
        name: String,
        mime_type: String,
        size: Option<String>,
        modified_time: String,
        web_view_link: Option<String>,
    }

    let file_list: FileListResponse = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse file list: {}", e))?;

    let files: Vec<DriveFile> = file_list.files
        .into_iter()
        .map(|f| DriveFile {
            id: f.id,
            name: f.name,
            mime_type: f.mime_type.clone(),
            size: f.size.and_then(|s| s.parse().ok()),
            modified_time: f.modified_time,
            web_view_link: f.web_view_link,
            is_folder: f.mime_type == "application/vnd.google-apps.folder",
        })
        .collect();

    tracing::info!("✅ Found {} files", files.len());
    Ok(files)
}

/// Download a file from Google Drive
#[tauri::command]
pub async fn download_google_drive_file(
    file_id: String,
    save_path: String,
    state: State<'_, Arc<GoogleDriveState>>,
) -> Result<String, String> {
    tracing::info!("Downloading file: {}", file_id);

    let access_token = state.get_valid_token().await?;

    let url = format!(
        "https://www.googleapis.com/drive/v3/files/{}?alt=media",
        file_id
    );

    let response = state.http_client
        .get(&url)
        .bearer_auth(&access_token)
        .send()
        .await
        .map_err(|e| format!("Download failed: {}", e))?;

    let bytes = response
        .bytes()
        .await
        .map_err(|e| format!("Failed to read file bytes: {}", e))?;

    std::fs::write(&save_path, bytes)
        .map_err(|e| format!("Failed to save file: {}", e))?;

    tracing::info!("File downloaded: {}", save_path);
    Ok(save_path)
}

/// Configure folder sync
#[tauri::command]
pub async fn configure_folder_sync(
    config: FolderSyncConfig,
    state: State<'_, Arc<GoogleDriveState>>,
) -> Result<(), String> {
    tracing::info!("⚙️ Configuring folder sync: {} -> space {}", config.folder_name, config.space_id);

    let mut sync_configs = state.sync_configs.write();

    // Remove existing config for this folder if it exists
    sync_configs.retain(|c| c.folder_id != config.folder_id);

    // Add new config
    sync_configs.push(config);

    tracing::info!("✅ Folder sync configured");
    Ok(())
}

/// Sync a folder to a Shodh space
#[tauri::command]
pub async fn sync_google_drive_folder(
    folder_id: String,
    space_id: String,
    drive_state: State<'_, Arc<GoogleDriveState>>,
    rag_state: State<'_, RagState>,
    app: tauri::AppHandle,
) -> Result<SyncStatus, String> {
    tracing::info!("🔄 Starting folder sync: {} -> space {}", folder_id, space_id);

    // Update sync status
    {
        let mut status = drive_state.sync_status.write();
        status.is_syncing = true;
        status.total_files = 0;
        status.synced_files = 0;
        status.failed_files = 0;
        status.error = None;
    }

    // List all files in folder
    let files = list_google_drive_files(Some(folder_id.clone()), drive_state.clone())
        .await
        .map_err(|e| {
            let mut status = drive_state.sync_status.write();
            status.is_syncing = false;
            status.error = Some(e.clone());
            e
        })?;

    // Filter for supported file types (PDF, DOCX, XLSX, TXT)
    let supported_files: Vec<_> = files.into_iter()
        .filter(|f| !f.is_folder)
        .filter(|f| {
            f.mime_type.contains("pdf") ||
            f.mime_type.contains("document") ||
            f.mime_type.contains("spreadsheet") ||
            f.mime_type.contains("text") ||
            f.mime_type.contains("plain")
        })
        .collect();

    {
        let mut status = drive_state.sync_status.write();
        status.total_files = supported_files.len();
    }

    tracing::info!("📦 Found {} supported files to sync", supported_files.len());

    // Get app data directory for temporary downloads
    let app_data_dir = app.path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data dir: {}", e))?;

    let temp_download_dir = app_data_dir.join("google_drive_temp");
    std::fs::create_dir_all(&temp_download_dir)
        .map_err(|e| format!("Failed to create temp dir: {}", e))?;

    // Download and index each file
    let mut synced = 0;
    let mut failed = 0;

    for file in supported_files {
        tracing::info!("⬇️ Downloading: {}", file.name);

        // Determine file extension from mime type
        let extension = if file.mime_type.contains("pdf") {
            "pdf"
        } else if file.mime_type.contains("wordprocessingml") || file.mime_type.contains("msword") {
            "docx"
        } else if file.mime_type.contains("spreadsheetml") || file.mime_type.contains("excel") {
            "xlsx"
        } else {
            "txt"
        };

        let file_name = if file.name.contains('.') {
            file.name.clone()
        } else {
            format!("{}.{}", file.name, extension)
        };

        let save_path = temp_download_dir.join(&file_name);

        // Download file
        match download_google_drive_file(
            file.id.clone(),
            save_path.to_string_lossy().to_string(),
            drive_state.clone()
        ).await {
            Ok(downloaded_path) => {
                tracing::info!("✅ Downloaded to: {}", downloaded_path);

                // Prepare metadata for indexing
                let mut metadata = std::collections::HashMap::new();
                metadata.insert("space_id".to_string(), space_id.clone());
                metadata.insert("title".to_string(), file_name.clone());
                metadata.insert("source".to_string(), "Google Drive".to_string());
                metadata.insert("file_path".to_string(), downloaded_path.clone());
                metadata.insert("original_name".to_string(), file.name.clone());
                metadata.insert("google_drive_id".to_string(), file.id.clone());
                metadata.insert("sync_date".to_string(), Utc::now().to_rfc3339());

                // Use the upload_file command which properly handles PDF/DOCX/XLSX parsing
                // This calls rag.add_document_from_file() in the backend
                match crate::rag_commands::upload_file(
                    downloaded_path.clone(),
                    metadata,
                    rag_state.clone()
                ).await {
                    Ok(result) => {
                        tracing::info!("✅ Indexed: {} - {}", file_name, result);
                        synced += 1;

                        // Update progress
                        let mut status = drive_state.sync_status.write();
                        status.synced_files = synced;
                    }
                    Err(e) => {
                        tracing::warn!("❌ Failed to index {}: {}", file_name, e);
                        failed += 1;
                        let mut status = drive_state.sync_status.write();
                        status.failed_files = failed;
                    }
                }

                // Clean up temp file
                let _ = std::fs::remove_file(save_path);
            }
            Err(e) => {
                tracing::warn!("❌ Failed to download {}: {}", file_name, e);
                failed += 1;
                let mut status = drive_state.sync_status.write();
                status.failed_files = failed;
            }
        }
    }

    {
        let mut status = drive_state.sync_status.write();
        status.is_syncing = false;
        status.last_sync = Some(Utc::now());
    }

    let final_status = drive_state.sync_status.read().clone();
    tracing::info!("✅ Sync complete: {}/{} files synced, {} failed",
        final_status.synced_files,
        final_status.total_files,
        final_status.failed_files
    );

    Ok(final_status)
}

/// Get current sync status
#[tauri::command]
pub async fn get_google_drive_sync_status(
    state: State<'_, Arc<GoogleDriveState>>,
) -> Result<SyncStatus, String> {
    Ok(state.sync_status.read().clone())
}

/// Check if authenticated with Google Drive
#[tauri::command]
pub async fn is_google_drive_authenticated(
    state: State<'_, Arc<GoogleDriveState>>,
) -> Result<bool, String> {
    let is_authed = state.tokens.read().is_some();
    Ok(is_authed)
}

/// Disconnect Google Drive
#[tauri::command]
pub async fn disconnect_google_drive(
    state: State<'_, Arc<GoogleDriveState>>,
) -> Result<(), String> {
    tracing::info!("🔌 Disconnecting Google Drive...");

    *state.tokens.write() = None;
    *state.config.write() = None;
    state.sync_configs.write().clear();

    tracing::info!("✅ Google Drive disconnected");
    Ok(())
}
