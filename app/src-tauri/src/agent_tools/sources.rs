//! Folder source tools: `add_folder`, `reindex_source`, `remove_source`.
//!
//! Folder indexing holds the RAG engine's write lock for the whole job, so
//! `add_folder` and `reindex_source` start a background job (through
//! [`super::HostEffects::start_indexing`]) and return at once.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use shodh_rag::audit::payload::{source_change, ChangeOrigin};
use shodh_rag::audit::AuditEventType;
use shodh_rag::harness::profile::app_tools;
use shodh_rag::harness::tools::sources::{find_folder_source, SourceSummary};
use shodh_rag::harness::tools::{
    ApprovalPreview, HostTool, RegistryError, ToolContext, ToolError, ToolOutput, ToolRegistry,
};
use shodh_rag::harness::RiskTier;

use super::{invalid, str_arg, AgentHost, IndexJob};

pub(super) fn register(
    registry: &mut ToolRegistry,
    host: &Arc<AgentHost>,
) -> Result<(), RegistryError> {
    registry.register(Arc::new(AddFolderTool { host: host.clone() }))?;
    registry.register(Arc::new(ReindexSourceTool { host: host.clone() }))?;
    registry.register(Arc::new(RemoveSourceTool { host: host.clone() }))?;
    Ok(())
}

/// Strip the Windows verbatim prefix so paths match the index.
pub(crate) fn strip_verbatim(path: &Path) -> PathBuf {
    let text = path.display().to_string();
    PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text))
}

fn validate_folder(tool: &str, raw: &str) -> Result<PathBuf, ToolError> {
    let path = Path::new(raw);
    if !path.is_absolute() {
        return Err(invalid(tool, "`path` must be an absolute folder path"));
    }
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(ToolError::Forbidden(
            "Paths containing '..' are not allowed".to_string(),
        ));
    }
    if !path.is_dir() {
        return Err(ToolError::NotFound(format!(
            "{raw} is not an existing folder"
        )));
    }
    std::fs::canonicalize(path)
        .map(|p| strip_verbatim(&p))
        .map_err(|e| ToolError::Unavailable(format!("Cannot open {raw}: {e}")))
}

/// Resolve the `source_id` argument to an indexed folder source.
async fn resolve_source(
    host: &AgentHost,
    tool: &str,
    args: &Value,
) -> Result<SourceSummary, ToolError> {
    let source_id =
        str_arg(args, "source_id").ok_or_else(|| invalid(tool, "`source_id` is required"))?;
    let engine = host.rag.read().await;
    find_folder_source(&engine, source_id).await
}

fn source_preview(source: &SourceSummary) -> Value {
    json!({
        "sourceId": source.source_id,
        "folder": source.folder,
        "files": source.files,
        "chunks": source.chunks,
    })
}

fn source_id_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "source_id": {"type": "string", "minLength": 1, "maxLength": 200}
        },
        "required": ["source_id"],
        "additionalProperties": false
    })
}

pub struct AddFolderTool {
    host: Arc<AgentHost>,
}

#[async_trait]
impl HostTool for AddFolderTool {
    fn name(&self) -> &'static str {
        app_tools::ADD_FOLDER
    }
    fn label(&self) -> &'static str {
        "Add folder"
    }
    fn label_template(&self) -> &'static str {
        "Adding folder {path}"
    }
    fn description(&self) -> &'static str {
        "Index a folder on this computer so its documents become searchable. Indexing runs in the \
         background; the call returns once it has started."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "minLength": 1, "maxLength": 1024}
            },
            "required": ["path"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Write
    }
    async fn preview(&self, args: &Value) -> Result<ApprovalPreview, ToolError> {
        let raw = str_arg(args, "path")
            .ok_or_else(|| invalid(app_tools::ADD_FOLDER, "`path` is required"))?;
        let folder = validate_folder(app_tools::ADD_FOLDER, raw)?;
        let folder = folder.display().to_string();
        Ok(ApprovalPreview {
            label: Some(format!("Add {folder} to the index")),
            details: json!({ "path": folder }),
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let tool = app_tools::ADD_FOLDER;
        let raw = str_arg(&args, "path").ok_or_else(|| invalid(tool, "`path` is required"))?;
        let folder = validate_folder(tool, raw)?;
        let folder_text = folder.display().to_string();
        let source_id = uuid::Uuid::new_v4().to_string();
        self.host.effects.start_indexing(
            ctx,
            IndexJob {
                action: "add",
                folder: folder_text.clone(),
                source_id: source_id.clone(),
            },
        );
        Ok(ToolOutput {
            text_for_model: format!(
                "Started indexing {folder_text} as source {source_id}. It runs in the background; \
                 its documents become searchable as files finish."
            ),
            summary_for_ui: format!("Indexing {folder_text}"),
            detail: Some(json!({ "sourceId": source_id, "path": folder_text })),
        })
    }
}

pub struct ReindexSourceTool {
    host: Arc<AgentHost>,
}

#[async_trait]
impl HostTool for ReindexSourceTool {
    fn name(&self) -> &'static str {
        app_tools::REINDEX_SOURCE
    }
    fn label(&self) -> &'static str {
        "Re-index source"
    }
    fn label_template(&self) -> &'static str {
        "Re-indexing source {source_id}"
    }
    async fn preview(&self, args: &Value) -> Result<ApprovalPreview, ToolError> {
        let source = resolve_source(&self.host, app_tools::REINDEX_SOURCE, args).await?;
        let folder = source
            .folder
            .clone()
            .unwrap_or_else(|| source.source_id.clone());
        Ok(ApprovalPreview {
            label: Some(format!("Re-index {folder}")),
            details: source_preview(&source),
        })
    }
    fn description(&self) -> &'static str {
        "Re-index a folder source (id from list_sources) to pick up new and changed files. Runs \
         in the background."
    }
    fn schema(&self) -> Value {
        source_id_schema()
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Write
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let source = resolve_source(&self.host, app_tools::REINDEX_SOURCE, &args).await?;
        let source_id = source.source_id.as_str();
        let folder = source.folder.ok_or_else(|| {
            ToolError::NotFound(format!("The folder of source {source_id} is unknown"))
        })?;
        if !Path::new(&folder).is_dir() {
            return Err(ToolError::NotFound(format!(
                "{folder} no longer exists on disk"
            )));
        }
        self.host.effects.start_indexing(
            ctx,
            IndexJob {
                action: "reindex",
                folder: folder.clone(),
                source_id: source_id.to_string(),
            },
        );
        Ok(ToolOutput {
            text_for_model: format!(
                "Started re-indexing {folder} (source {source_id}) in the background."
            ),
            summary_for_ui: format!("Re-indexing {folder}"),
            detail: Some(json!({ "sourceId": source_id, "path": folder })),
        })
    }
}

pub struct RemoveSourceTool {
    host: Arc<AgentHost>,
}

#[async_trait]
impl HostTool for RemoveSourceTool {
    fn name(&self) -> &'static str {
        app_tools::REMOVE_SOURCE
    }
    fn label(&self) -> &'static str {
        "Remove source"
    }
    fn label_template(&self) -> &'static str {
        "Removing source {source_id}"
    }
    async fn preview(&self, args: &Value) -> Result<ApprovalPreview, ToolError> {
        let source = resolve_source(&self.host, app_tools::REMOVE_SOURCE, args).await?;
        let folder = source
            .folder
            .clone()
            .unwrap_or_else(|| source.source_id.clone());
        Ok(ApprovalPreview {
            label: Some(format!(
                "Remove {folder} ({} files) from the index",
                source.files
            )),
            details: source_preview(&source),
        })
    }
    fn description(&self) -> &'static str {
        "Remove a folder source (id from list_sources) from the index. Files on disk are not \
         touched."
    }
    fn schema(&self) -> Value {
        source_id_schema()
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Destructive
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let source = resolve_source(&self.host, app_tools::REMOVE_SOURCE, &args).await?;
        let source_id = source.source_id.as_str();
        let deleted = {
            let mut engine = self.host.rag.write().await;
            engine
                .delete_by_space_id(&source.source_id)
                .await
                .map_err(|e| ToolError::Failed(format!("Removing the source failed: {e}")))?
        };
        let folder = source.folder.unwrap_or_else(|| source.source_id.clone());
        ctx.audit(
            AuditEventType::SourceChange,
            source_change(
                "remove",
                ChangeOrigin::Agent,
                Some(source_id),
                Some(&folder),
                json!({"ok": true, "files": source.files, "chunks": deleted}),
            ),
        );
        Ok(ToolOutput {
            text_for_model: format!(
                "Removed {folder} (source {source_id}, {} files, {deleted} chunks) from the index. Files on disk were not touched.",
                source.files
            ),
            summary_for_ui: format!("Removed {folder} from the index"),
            detail: Some(json!({ "sourceId": source_id, "path": folder, "files": source.files, "chunks": deleted })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing;
    use super::*;

    #[tokio::test]
    async fn add_folder_validates_and_starts_one_background_job() {
        let t = testing::host().await;
        let tool = AddFolderTool {
            host: t.host.clone(),
        };
        let (ctx, _rx) = testing::ctx();
        assert!(matches!(
            tool.execute(json!({"path": "relative/folder"}), &ctx).await,
            Err(ToolError::InvalidArguments { .. })
        ));
        let missing = t.dir.path().join("missing");
        assert!(matches!(
            tool.execute(json!({"path": missing.display().to_string()}), &ctx)
                .await,
            Err(ToolError::NotFound(_))
        ));
        let docs = t.dir.path().join("docs");
        std::fs::create_dir(&docs).unwrap();
        let out = tool
            .execute(json!({"path": docs.display().to_string()}), &ctx)
            .await
            .unwrap();
        assert!(out.summary_for_ui.starts_with("Indexing "));
        let jobs = t.effects.indexing.lock().unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].action, "add");
        assert!(jobs[0].folder.ends_with("docs"));
    }

    #[tokio::test]
    async fn unknown_sources_fail_before_any_change() {
        let t = testing::host().await;
        let (ctx, _rx) = testing::ctx();
        let remove = RemoveSourceTool {
            host: t.host.clone(),
        };
        assert!(matches!(
            remove.preview(&json!({"source_id": "nope"})).await,
            Err(ToolError::NotFound(_))
        ));
        let reindex = ReindexSourceTool {
            host: t.host.clone(),
        };
        assert!(reindex
            .execute(json!({"source_id": "nope"}), &ctx)
            .await
            .is_err());
        assert!(t.effects.indexing.lock().unwrap().is_empty());
    }
}
