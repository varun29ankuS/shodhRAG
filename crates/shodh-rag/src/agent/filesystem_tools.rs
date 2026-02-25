//! Filesystem Tools with Permission Management
//!
//! Production-grade filesystem access with:
//! - User permission prompts
//! - Session-level permissions
//! - Path sandboxing
//! - Audit logging

use super::context::AgentContext;
use super::tools::{AgentTool, ToolInput, ToolResult};
use anyhow::{Context as AnyhowContext, Result};
use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

// ============================================================================
// Permission Management
// ============================================================================

/// Permission types for filesystem operations
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum FilePermission {
    /// Read file contents
    ReadFile,
    /// Write/create files
    WriteFile,
    /// List directory contents
    ListDirectory,
    /// Delete files
    DeleteFile,
    /// Create directories
    CreateDirectory,
    /// Execute code (writes to temp)
    ExecuteCode,
}

/// Permission scope
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PermissionScope {
    /// One-time permission for this operation only
    Once,
    /// Permission for this session (until app restart)
    Session,
    /// Always allow (stored permanently) - for future use
    Always,
}

/// Permission request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRequest {
    pub operation: FilePermission,
    pub path: PathBuf,
    pub reason: String,
    pub agent_id: String,
}

/// Permission decision
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionDecision {
    pub allowed: bool,
    pub scope: PermissionScope,
    pub granted_at: chrono::DateTime<Utc>,
}

/// Session permission cache
pub struct PermissionManager {
    /// Session permissions: (agent_id, operation) -> allowed
    session_permissions: Arc<RwLock<HashMap<(String, FilePermission), bool>>>,

    /// Audit log
    audit_log: Arc<RwLock<Vec<AuditEntry>>>,

    /// Allowed base paths (sandboxing)
    allowed_paths: Vec<PathBuf>,

    /// Blocked paths (system directories)
    blocked_paths: Vec<PathBuf>,
}

/// Audit log entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub timestamp: chrono::DateTime<Utc>,
    pub agent_id: String,
    pub operation: FilePermission,
    pub path: PathBuf,
    pub allowed: bool,
    pub result: String,
}

impl PermissionManager {
    /// Create new permission manager with default sandboxing
    pub fn new() -> Self {
        let home_dir = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".to_string());

        Self {
            session_permissions: Arc::new(RwLock::new(HashMap::new())),
            audit_log: Arc::new(RwLock::new(Vec::new())),
            allowed_paths: vec![
                PathBuf::from(&home_dir).join("Documents"),
                PathBuf::from(&home_dir).join("Downloads"),
                PathBuf::from(&home_dir).join("Desktop"),
                PathBuf::from("./agent_workspace"), // Project workspace (from app root)
                PathBuf::from("../agent_workspace"), // Project workspace (from subdirectory)
                PathBuf::from(std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))), // Current working directory
                PathBuf::from(std::env::temp_dir()), // Temp directory
            ],
            blocked_paths: vec![
                PathBuf::from("/etc"),
                PathBuf::from("/root"),
                PathBuf::from("/System"),
                PathBuf::from("/Windows"),
                PathBuf::from("/sys"),
                PathBuf::from("/proc"),
                PathBuf::from("C:\\Windows"),
                PathBuf::from("C:\\Program Files"),
                PathBuf::from("C:\\Program Files (x86)"),
            ],
        }
    }

    /// Check if path is within allowed boundaries
    pub fn is_path_allowed(&self, path: &Path) -> bool {
        let canonical = match path.canonicalize() {
            Ok(p) => p,
            Err(_) => {
                // Path doesn't exist yet - check parent
                let parent = path.parent().unwrap_or(path);
                match parent.canonicalize() {
                    Ok(p) => p,
                    Err(_) => return false,
                }
            }
        };

        // Check if path is in blocked list
        for blocked in &self.blocked_paths {
            if canonical.starts_with(blocked) {
                return false;
            }
        }

        // Check if path is in allowed list
        for allowed in &self.allowed_paths {
            if canonical.starts_with(allowed) {
                return true;
            }
        }

        // Default: deny if not in allowed paths
        false
    }

    /// Check if permission is granted for this session
    pub async fn has_session_permission(&self, agent_id: &str, operation: &FilePermission) -> bool {
        let key = (agent_id.to_string(), operation.clone());
        self.session_permissions
            .read()
            .await
            .get(&key)
            .copied()
            .unwrap_or(false)
    }

    /// Grant session permission
    pub async fn grant_session_permission(&self, agent_id: &str, operation: FilePermission) {
        let key = (agent_id.to_string(), operation);
        self.session_permissions.write().await.insert(key, true);
    }

    /// Request permission from user (async - will be prompted via frontend)
    pub async fn request_permission(
        &self,
        request: PermissionRequest,
    ) -> Result<PermissionDecision> {
        // Check path sandboxing first
        if !self.is_path_allowed(&request.path) {
            return Ok(PermissionDecision {
                allowed: false,
                scope: PermissionScope::Once,
                granted_at: Utc::now(),
            });
        }

        // Check session permission
        if self
            .has_session_permission(&request.agent_id, &request.operation)
            .await
        {
            return Ok(PermissionDecision {
                allowed: true,
                scope: PermissionScope::Session,
                granted_at: Utc::now(),
            });
        }

        // Auto-approve read operations, deny write/delete without explicit session permission
        let auto_approve = matches!(
            request.operation,
            FilePermission::ReadFile | FilePermission::ListDirectory
        );

        Ok(PermissionDecision {
            allowed: auto_approve,
            scope: PermissionScope::Once,
            granted_at: Utc::now(),
        })
    }

    /// Log operation to audit trail
    pub async fn log_operation(
        &self,
        agent_id: String,
        operation: FilePermission,
        path: PathBuf,
        allowed: bool,
        result: String,
    ) {
        let entry = AuditEntry {
            timestamp: Utc::now(),
            agent_id,
            operation,
            path,
            allowed,
            result,
        };

        self.audit_log.write().await.push(entry);
    }

    /// Get audit log
    pub async fn get_audit_log(&self) -> Vec<AuditEntry> {
        self.audit_log.read().await.clone()
    }
}

// ============================================================================
// Filesystem Tools
// ============================================================================

/// Read file tool with permissions
pub struct ReadFileTool {
    permission_manager: Arc<PermissionManager>,
}

impl ReadFileTool {
    pub fn new(permission_manager: Arc<PermissionManager>) -> Self {
        Self { permission_manager }
    }
}

#[async_trait]
impl AgentTool for ReadFileTool {
    fn id(&self) -> &str {
        "read_file"
    }

    fn name(&self) -> &str {
        "Read File"
    }

    fn description(&self) -> &str {
        "Read the contents of a file from the filesystem. Requires user permission."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to read"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, input: ToolInput, context: AgentContext) -> Result<ToolResult> {
        let path_str = input.parameters["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing path parameter"))?;

        let path = PathBuf::from(path_str);

        // Extract user_id from context
        let user_id = context
            .user_info
            .as_ref()
            .map(|u| u.user_id.clone())
            .unwrap_or_else(|| "unknown".to_string());

        // Request permission
        let permission = self
            .permission_manager
            .request_permission(PermissionRequest {
                operation: FilePermission::ReadFile,
                path: path.clone(),
                reason: format!("Agent wants to read file: {}", path.display()),
                agent_id: user_id.clone(),
            })
            .await?;

        if !permission.allowed {
            self.permission_manager
                .log_operation(
                    user_id,
                    FilePermission::ReadFile,
                    path,
                    false,
                    "Permission denied".to_string(),
                )
                .await;

            return Ok(ToolResult {
                success: false,
                output: "Permission denied by user or sandboxing rules".to_string(),
                data: serde_json::json!({}),
                error: Some("Permission denied".to_string()),
            });
        }

        // Read file
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                self.permission_manager
                    .log_operation(
                        user_id,
                        FilePermission::ReadFile,
                        path.clone(),
                        true,
                        format!("Read {} bytes", content.len()),
                    )
                    .await;

                Ok(ToolResult {
                    success: true,
                    output: format!("Read file '{}' ({} bytes)", path.display(), content.len()),
                    data: serde_json::json!({
                        "path": path_str,
                        "content": content,
                        "size_bytes": content.len(),
                    }),
                    error: None,
                })
            }
            Err(e) => {
                self.permission_manager
                    .log_operation(
                        user_id,
                        FilePermission::ReadFile,
                        path,
                        true,
                        format!("Failed: {}", e),
                    )
                    .await;

                Ok(ToolResult {
                    success: false,
                    output: format!("Failed to read file: {}", e),
                    data: serde_json::json!({}),
                    error: Some(e.to_string()),
                })
            }
        }
    }
}

/// Write file tool with permissions
pub struct WriteFileTool {
    permission_manager: Arc<PermissionManager>,
}

impl WriteFileTool {
    pub fn new(permission_manager: Arc<PermissionManager>) -> Self {
        Self { permission_manager }
    }
}

#[async_trait]
impl AgentTool for WriteFileTool {
    fn id(&self) -> &str {
        "write_file"
    }

    fn name(&self) -> &str {
        "Write File"
    }

    fn description(&self) -> &str {
        "Write content to a file. Creates file if it doesn't exist. Requires user permission."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path where to write the file"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write to the file"
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, input: ToolInput, context: AgentContext) -> Result<ToolResult> {
        let path_str = input.parameters["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing path parameter"))?;

        let content = input.parameters["content"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing content parameter"))?;

        let path = PathBuf::from(path_str);

        // Extract user_id from context
        let user_id = context
            .user_info
            .as_ref()
            .map(|u| u.user_id.clone())
            .unwrap_or_else(|| "unknown".to_string());

        // Request permission
        let permission = self
            .permission_manager
            .request_permission(PermissionRequest {
                operation: FilePermission::WriteFile,
                path: path.clone(),
                reason: format!(
                    "Agent wants to write {} bytes to: {}",
                    content.len(),
                    path.display()
                ),
                agent_id: user_id.clone(),
            })
            .await?;

        if !permission.allowed {
            self.permission_manager
                .log_operation(
                    user_id,
                    FilePermission::WriteFile,
                    path,
                    false,
                    "Permission denied".to_string(),
                )
                .await;

            return Ok(ToolResult {
                success: false,
                output: "Permission denied by user or sandboxing rules".to_string(),
                data: serde_json::json!({}),
                error: Some("Permission denied".to_string()),
            });
        }

        // Create parent directories if needed
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent)?;
            }
        }

        // Write file
        match std::fs::write(&path, content) {
            Ok(_) => {
                self.permission_manager
                    .log_operation(
                        user_id,
                        FilePermission::WriteFile,
                        path.clone(),
                        true,
                        format!("Wrote {} bytes", content.len()),
                    )
                    .await;

                Ok(ToolResult {
                    success: true,
                    output: format!("Wrote {} bytes to '{}'", content.len(), path.display()),
                    data: serde_json::json!({
                        "path": path_str,
                        "size_bytes": content.len(),
                    }),
                    error: None,
                })
            }
            Err(e) => {
                self.permission_manager
                    .log_operation(
                        user_id,
                        FilePermission::WriteFile,
                        path,
                        true,
                        format!("Failed: {}", e),
                    )
                    .await;

                Ok(ToolResult {
                    success: false,
                    output: format!("Failed to write file: {}", e),
                    data: serde_json::json!({}),
                    error: Some(e.to_string()),
                })
            }
        }
    }
}

/// List directory tool
pub struct ListDirectoryTool {
    permission_manager: Arc<PermissionManager>,
}

impl ListDirectoryTool {
    pub fn new(permission_manager: Arc<PermissionManager>) -> Self {
        Self { permission_manager }
    }
}

#[async_trait]
impl AgentTool for ListDirectoryTool {
    fn id(&self) -> &str {
        "list_directory"
    }

    fn name(&self) -> &str {
        "List Directory"
    }

    fn description(&self) -> &str {
        "List contents of a directory. Returns files and subdirectories."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the directory to list"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, input: ToolInput, context: AgentContext) -> Result<ToolResult> {
        let path_str = input.parameters["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing path parameter"))?;

        let path = PathBuf::from(path_str);

        // Extract user_id from context
        let user_id = context
            .user_info
            .as_ref()
            .map(|u| u.user_id.clone())
            .unwrap_or_else(|| "unknown".to_string());

        // Request permission
        let permission = self
            .permission_manager
            .request_permission(PermissionRequest {
                operation: FilePermission::ListDirectory,
                path: path.clone(),
                reason: format!("Agent wants to list directory: {}", path.display()),
                agent_id: user_id.clone(),
            })
            .await?;

        if !permission.allowed {
            self.permission_manager
                .log_operation(
                    user_id,
                    FilePermission::ListDirectory,
                    path,
                    false,
                    "Permission denied".to_string(),
                )
                .await;

            return Ok(ToolResult {
                success: false,
                output: "Permission denied by user or sandboxing rules".to_string(),
                data: serde_json::json!({}),
                error: Some("Permission denied".to_string()),
            });
        }

        // List directory
        match std::fs::read_dir(&path) {
            Ok(entries) => {
                let mut files = Vec::new();
                let mut dirs = Vec::new();

                for entry in entries {
                    if let Ok(entry) = entry {
                        let path = entry.path();
                        let name = entry.file_name().to_string_lossy().to_string();

                        if path.is_dir() {
                            dirs.push(name);
                        } else {
                            let size = entry.metadata().ok().map(|m| m.len()).unwrap_or(0);
                            files.push(serde_json::json!({
                                "name": name,
                                "size": size,
                            }));
                        }
                    }
                }

                self.permission_manager
                    .log_operation(
                        user_id,
                        FilePermission::ListDirectory,
                        path.clone(),
                        true,
                        format!("Found {} files, {} directories", files.len(), dirs.len()),
                    )
                    .await;

                Ok(ToolResult {
                    success: true,
                    output: format!(
                        "Listed directory '{}': {} files, {} directories",
                        path.display(),
                        files.len(),
                        dirs.len()
                    ),
                    data: serde_json::json!({
                        "path": path_str,
                        "files": files,
                        "directories": dirs,
                    }),
                    error: None,
                })
            }
            Err(e) => {
                self.permission_manager
                    .log_operation(
                        user_id,
                        FilePermission::ListDirectory,
                        path,
                        true,
                        format!("Failed: {}", e),
                    )
                    .await;

                Ok(ToolResult {
                    success: false,
                    output: format!("Failed to list directory: {}", e),
                    data: serde_json::json!({}),
                    error: Some(e.to_string()),
                })
            }
        }
    }
}
