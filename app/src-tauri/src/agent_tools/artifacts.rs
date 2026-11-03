//! The Artifacts tab's agent tools: `list_artifacts` (read) and
//! `open_artifact` (a UI action), so the assistant can find a creation and
//! show the user a version or a pane of it, visibly.
//!
//! What stays the user's alone has no tool: Keep, Export and Delete it
//! (`creation_keep`, `creation_export`, `creation_delete`), and the preview's
//! network switch.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use shodh_rag::harness::events::NavigationTarget;
use shodh_rag::harness::profile::app_tools;
use shodh_rag::harness::tools::creation::{FinishCreationTool, FINISH_CREATION};
use shodh_rag::harness::tools::navigate::emit_navigation;
use shodh_rag::harness::tools::{
    HostTool, RegistryError, ToolContext, ToolError, ToolOutput, ToolRegistry,
};
use shodh_rag::harness::RiskTier;

use super::{invalid, limit_arg, str_arg, AgentHost};
use crate::creations::{valid_id, Creation, CreationStatus, CreationStore};

/// The panes of an artifact the agent may show.
pub const PANES: [&str; 3] = ["preview", "files", "versions"];

const DEFAULT_RESULTS: usize = 20;
const MAX_RESULTS: usize = 50;

/// The tools a Creator session gets on top of its coding tools.
pub const CREATOR_TOOL_NAMES: [&str; 3] = [
    FINISH_CREATION,
    app_tools::OPEN_ARTIFACT,
    app_tools::LIST_ARTIFACTS,
];

/// A Creator session's own tools ([`CREATOR_TOOL_NAMES`]): its finish card
/// and the Artifacts tab's tools.
pub fn creator_tools(data_dir: &std::path::Path) -> Vec<Arc<dyn HostTool>> {
    vec![
        Arc::new(FinishCreationTool),
        Arc::new(OpenArtifactTool {
            data_dir: data_dir.to_path_buf(),
        }),
        Arc::new(ListArtifactsTool {
            data_dir: data_dir.to_path_buf(),
        }),
    ]
}

pub(super) fn register(
    registry: &mut ToolRegistry,
    host: &Arc<AgentHost>,
) -> Result<(), RegistryError> {
    registry.register(Arc::new(ListArtifactsTool {
        data_dir: host.data_dir.clone(),
    }))?;
    registry.register(Arc::new(OpenArtifactTool {
        data_dir: host.data_dir.clone(),
    }))?;
    Ok(())
}

fn status_word(status: CreationStatus) -> &'static str {
    match status {
        CreationStatus::Building => "building",
        CreationStatus::Ready => "ready",
        CreationStatus::Kept => "kept",
    }
}

fn summary_json(creation: &Creation) -> Value {
    json!({
        "id": creation.id,
        "title": creation.title,
        "status": status_word(creation.status),
        "versions": creation.versions.len(),
        "latestVersion": creation.latest_version(),
        "conversationId": creation.conversation_id,
        "summary": creation.summary,
        "updatedAt": creation.updated_at,
    })
}

async fn load(data_dir: PathBuf) -> Result<Vec<Creation>, ToolError> {
    tokio::task::spawn_blocking(move || CreationStore::in_dir(&data_dir).list())
        .await
        .map_err(|e| ToolError::Failed(format!("reading the creations did not finish: {e}")))?
        .map_err(ToolError::Failed)
}

pub struct ListArtifactsTool {
    data_dir: PathBuf,
}

#[async_trait]
impl HostTool for ListArtifactsTool {
    fn name(&self) -> &'static str {
        app_tools::LIST_ARTIFACTS
    }
    fn label(&self) -> &'static str {
        "List artifacts"
    }
    fn label_template(&self) -> &'static str {
        "Looking through what Shodh made"
    }
    fn description(&self) -> &'static str {
        "List the creations Creator mode built (newest first): each with its id, title, \
         status (building, ready or kept), how many versions are saved and the conversation \
         that made it. Optionally only those whose title or summary contains `text`. Use an \
         id with open_artifact. Diagrams, charts and tables are in list_visuals."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "text": {"type": "string", "minLength": 1, "maxLength": 200},
                "limit": {"type": "integer", "minimum": 1, "maximum": MAX_RESULTS}
            },
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let limit = limit_arg(&args, DEFAULT_RESULTS, MAX_RESULTS);
        let needle = str_arg(&args, "text").map(str::to_lowercase);
        let all = load(self.data_dir.clone()).await?;
        let matching: Vec<&Creation> = all
            .iter()
            .filter(|c| {
                needle.as_deref().is_none_or(|needle| {
                    c.title.to_lowercase().contains(needle)
                        || c.summary
                            .as_deref()
                            .is_some_and(|s| s.to_lowercase().contains(needle))
                })
            })
            .collect();
        let shown: Vec<&Creation> = matching.iter().take(limit).copied().collect();
        let text = if shown.is_empty() {
            "No creations match.".to_string()
        } else {
            let mut text = format!("{} creation(s):", matching.len());
            for c in &shown {
                text.push_str(&format!(
                    "\n- {} \"{}\" ({}, {} version(s))",
                    c.id,
                    c.title,
                    status_word(c.status),
                    c.versions.len()
                ));
            }
            text
        };
        Ok(ToolOutput {
            text_for_model: text,
            summary_for_ui: format!("{} creation(s)", matching.len()),
            detail: Some(json!({
                "total": matching.len(),
                "creations": shown.iter().map(|c| summary_json(c)).collect::<Vec<_>>(),
            })),
        })
    }
}

pub struct OpenArtifactTool {
    data_dir: PathBuf,
}

#[async_trait]
impl HostTool for OpenArtifactTool {
    fn name(&self) -> &'static str {
        app_tools::OPEN_ARTIFACT
    }
    fn label(&self) -> &'static str {
        "Open artifact"
    }
    fn label_template(&self) -> &'static str {
        "Showing it in Artifacts"
    }
    fn description(&self) -> &'static str {
        "Show the user a creation in the Artifacts tab (its id from list_artifacts, or the \
         creation this conversation builds): at a saved version (the folder as it is now when \
         absent) and on a pane: preview (the page running), files (its files) or versions \
         (each saved version and what changed). This changes nothing."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "artifact_id": {"type": "string", "minLength": 1, "maxLength": 64},
                "version": {"type": "integer", "minimum": 1, "maximum": 100000},
                "pane": {"type": "string", "enum": PANES}
            },
            "required": ["artifact_id"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let tool = app_tools::OPEN_ARTIFACT;
        let id = str_arg(&args, "artifact_id")
            .ok_or_else(|| invalid(tool, "`artifact_id` is required"))?
            .to_string();
        if !valid_id(&id) {
            return Err(ToolError::NotFound(format!(
                "{id} is not an artifact id. Call list_artifacts for valid ids."
            )));
        }
        let version = args
            .get("version")
            .and_then(Value::as_u64)
            .and_then(|v| u32::try_from(v).ok());
        let pane = match str_arg(&args, "pane") {
            Some(pane) if PANES.contains(&pane) => pane.to_string(),
            Some(pane) => return Err(invalid(tool, format!("unknown pane {pane}"))),
            None => "preview".to_string(),
        };
        let creation = load(self.data_dir.clone())
            .await?
            .into_iter()
            .find(|c| c.id == id)
            .ok_or_else(|| {
                ToolError::NotFound(format!(
                    "No creation has the id {id}. Call list_artifacts for valid ids."
                ))
            })?;
        if let Some(number) = version {
            if !creation.versions.iter().any(|v| v.number == number) {
                return Err(ToolError::NotFound(format!(
                    "\"{}\" has no version {number}; it has {}.",
                    creation.title,
                    creation.versions.len()
                )));
            }
        }
        emit_navigation(
            ctx,
            "artifacts",
            None,
            Some(NavigationTarget::Artifact {
                artifact_id: creation.id.clone(),
                version,
                pane: Some(pane.clone()),
            }),
        );
        let at = match version {
            Some(number) => format!("version {number}"),
            None => "as it is now".to_string(),
        };
        Ok(ToolOutput {
            text_for_model: format!(
                "The Artifacts tab now shows \"{}\" ({at}) on its {pane} pane for the user.",
                creation.title
            ),
            summary_for_ui: format!("Showed “{}” · {pane}", creation.title),
            detail: Some(json!({ "id": creation.id, "version": version, "pane": pane })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shodh_rag::harness::events::AgentEvent;

    fn store_with_one() -> (tempfile::TempDir, PathBuf, Creation) {
        let dir = tempfile::tempdir().unwrap();
        let documents = dir.path().join("Documents");
        std::fs::create_dir_all(&documents).unwrap();
        let root = crate::creations::creations_root(&documents).unwrap();
        let data = dir.path().join("data");
        let store = CreationStore::in_dir(&data);
        let creation = store
            .prepare(&root, "conv-1", "a snake game", "2026-11-03T10:00:00Z")
            .unwrap();
        std::fs::write(
            std::path::Path::new(&creation.folder).join("index.html"),
            "<canvas></canvas>",
        )
        .unwrap();
        store
            .snapshot(&root, "conv-1", Some("run-1"), "2026-11-03T10:05:00Z")
            .unwrap();
        let creation = store.get(&creation.id).unwrap().unwrap();
        (dir, data, creation)
    }

    fn ctx() -> (
        ToolContext,
        tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        (ToolContext::new("run-1", "step-1", tx), rx)
    }

    #[tokio::test]
    async fn open_artifact_shows_a_version_and_pane_visibly() {
        let (_dir, data, creation) = store_with_one();
        let tool = OpenArtifactTool { data_dir: data };
        let (ctx, mut rx) = ctx();
        let out = tool
            .execute(
                json!({"artifact_id": creation.id, "version": 1, "pane": "files"}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(out.text_for_model.contains("files pane"));
        match rx.try_recv().unwrap() {
            AgentEvent::Navigated { view, target, .. } => {
                assert_eq!(view, "artifacts");
                assert_eq!(
                    target,
                    Some(NavigationTarget::Artifact {
                        artifact_id: creation.id.clone(),
                        version: Some(1),
                        pane: Some("files".into()),
                    })
                );
            }
            other => panic!("expected navigation, got {other:?}"),
        }
        assert_eq!(tool.tier(), RiskTier::Read);
    }

    #[tokio::test]
    async fn open_artifact_checks_what_it_opens() {
        let (_dir, data, creation) = store_with_one();
        let tool = OpenArtifactTool { data_dir: data };
        let (ctx, mut rx) = ctx();
        for args in [
            json!({"artifact_id": "cr_0123456789abcdef0123456789abcdef"}),
            json!({"artifact_id": "../../etc"}),
            json!({"artifact_id": creation.id, "version": 9}),
            json!({"artifact_id": creation.id, "pane": "settings"}),
        ] {
            assert!(tool.execute(args.clone(), &ctx).await.is_err(), "{args}");
        }
        // Nothing was shown for a refused call.
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn list_artifacts_names_each_creation_with_its_id() {
        let (_dir, data, creation) = store_with_one();
        let tool = ListArtifactsTool { data_dir: data };
        let (ctx, _rx) = ctx();
        let out = tool.execute(json!({}), &ctx).await.unwrap();
        assert!(out.text_for_model.contains(&creation.id));
        assert!(out.text_for_model.contains("a snake game"));
        assert!(out.text_for_model.contains("ready, 1 version(s)"));
        let none = tool
            .execute(json!({"text": "spreadsheet"}), &ctx)
            .await
            .unwrap();
        assert_eq!(none.text_for_model, "No creations match.");
    }

    #[test]
    fn a_creator_session_gets_its_finish_card_and_the_artifact_tools() {
        let dir = tempfile::tempdir().unwrap();
        let names: Vec<&str> = creator_tools(dir.path()).iter().map(|t| t.name()).collect();
        assert_eq!(names, CREATOR_TOOL_NAMES);
        // None of them changes anything: they run in every mode, Plan included.
        for tool in creator_tools(dir.path()) {
            assert_eq!(tool.tier(), RiskTier::Read, "{}", tool.name());
            assert!(
                !tool.edits_locally() && !tool.changes_state(),
                "{}",
                tool.name()
            );
        }
    }
}
