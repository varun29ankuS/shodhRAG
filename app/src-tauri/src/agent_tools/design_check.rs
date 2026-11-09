//! `design_check` (read): Shodh's deterministic design check of UI files
//! ([`shodh_rag::design_check`]) on a path of the session's folder or on an
//! artifact, shown to the user in the code pane, its findings returned with
//! their file:line receipts.
//!
//! It reads files and changes none; no model is called. A finding's
//! mechanical fix is returned as a proposal only: the assistant applies one
//! with its normal edit tool, which asks as the conversation's mode decides,
//! and the user's own "Send fix to the conversation" asks in every mode.
//!
//! Where it is registered: in a Code session on its workspace's code folder,
//! in a Creator session on its creation folder, and in a Research chat on
//! artifacts only (a Research chat has no code folder of its own).

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use shodh_rag::harness::events::NavigationTarget;
use shodh_rag::harness::profile::app_tools;
use shodh_rag::harness::tools::navigate::{emit_shown, SHOWN_TO_USER};
use shodh_rag::harness::tools::{HostTool, ToolContext, ToolError, ToolOutput};
use shodh_rag::harness::RiskTier;

use super::{str_arg, HostEffects};
use crate::agent_session_commands::workspace_code_folder;
use crate::design_check_commands::{
    checked_relative, creation_subject, run_detached, Scope, StoredDesignCheck, Subject,
};
use crate::workspace_commands::WorkspaceState;

/// Findings listed for the model at most (all are in the pane).
pub const MAX_LISTED: usize = 60;

/// Which folder a `design_check` call reads.
#[derive(Clone)]
pub enum Place {
    /// A Research chat: artifacts only.
    Chat,
    /// A Code session: its workspace's code folder.
    Code {
        workspaces: WorkspaceState,
        workspace_id: Option<String>,
    },
    /// A Creator session: its creation.
    Creator { creation_id: Option<String> },
}

pub struct DesignCheckTool {
    pub effects: Arc<dyn HostEffects>,
    pub data_dir: PathBuf,
    pub place: Place,
}

fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> impl std::future::Future<Output = Result<T, ToolError>> {
    async move {
        tokio::task::spawn_blocking(work)
            .await
            .map_err(|e| ToolError::Failed(format!("the design check did not finish: {e}")))?
            .map_err(ToolError::NotFound)
    }
}

impl DesignCheckTool {
    async fn artifact(&self, id: &str, path: &str) -> Result<(Subject, Option<String>), ToolError> {
        let documents = self.effects.documents_dir().ok_or_else(|| {
            ToolError::Unavailable(
                "The Documents folder, where artifacts are, is unavailable.".into(),
            )
        })?;
        let (data, id, path) = (self.data_dir.clone(), id.to_string(), path.to_string());
        let (subject, conversation) =
            blocking(move || creation_subject(&data, &documents, &id, &path)).await?;
        Ok((subject, Some(conversation)))
    }

    /// What the call names, and the conversation a fix would go to.
    async fn subject(
        &self,
        args: &Value,
        ctx: &ToolContext,
    ) -> Result<(Subject, Option<String>), ToolError> {
        let path = str_arg(args, "path").unwrap_or(".").to_string();
        let conversation = ctx.audit_scope().map(|s| s.conversation_id.clone());
        if let Some(id) = str_arg(args, "artifact_id") {
            return self.artifact(id, &path).await;
        }
        match &self.place {
            Place::Chat => Err(super::invalid(
                app_tools::DESIGN_CHECK,
                "this chat has no project folder: give artifact_id (from list_artifacts)",
            )),
            Place::Creator { creation_id } => {
                let id = creation_id.clone().ok_or_else(|| {
                    ToolError::Unavailable("This conversation has made no creation yet.".into())
                })?;
                self.artifact(&id, &path).await
            }
            Place::Code {
                workspaces,
                workspace_id,
            } => {
                let workspace = workspace_id
                    .clone()
                    .or_else(|| ctx.scope().workspace.clone())
                    .ok_or_else(|| {
                        ToolError::Unavailable(
                            "design_check works on a workspace's code folder, and this chat is in \
                             no workspace."
                                .into(),
                        )
                    })?;
                let folder = workspace_code_folder(workspaces, Some(&workspace))
                    .await
                    .map_err(ToolError::Unavailable)?;
                let root = folder.root().to_path_buf();
                let checked_root = root.clone();
                let path = blocking(move || checked_relative(&checked_root, &path)).await?;
                let label = if path == "." {
                    "the whole project".to_string()
                } else {
                    path.clone()
                };
                Ok((
                    Subject {
                        root,
                        scope: Scope::Path(path),
                        workspace_id: Some(workspace),
                        creation_id: None,
                        label,
                    },
                    conversation,
                ))
            }
        }
    }
}

/// The findings as the model reads them: each with its receipt (rule,
/// severity, file:line, what was found) and the fix, mechanical ones exact.
pub fn findings_text(check: &StoredDesignCheck) -> String {
    let report = &check.report;
    let mut text = format!(
        "The design check of {} in the code pane: {SHOWN_TO_USER} Shodh read {} UI file(s) with \
         {} deterministic rules (no model): {} finding(s), {} error(s), {} warning(s), {} \
         advisory.\n",
        check.target.label,
        report.files_checked,
        report.rules,
        report.total(),
        report.counts.error,
        report.counts.warning,
        report.counts.advisory,
    );
    if report.files_checked == 0 {
        text.push_str("There were no UI files (HTML, CSS, SCSS, JSX/TSX, Vue, Svelte) to check.\n");
    }
    for (i, f) in report.findings.iter().take(MAX_LISTED).enumerate() {
        text.push_str(&format!(
            "{}. [{}] {} ({}) at {}:{}: {}\n   Fix: {}\n",
            i + 1,
            f.severity.as_str(),
            f.title,
            f.rule,
            f.path,
            f.line,
            f.evidence,
            f.fix,
        ));
        if let Some(r) = &f.replace {
            text.push_str(&format!(
                "   Mechanical fix (a proposal; make it only with your edit tool): replace the \
                 line `{}` with `{}`\n",
                r.old.trim(),
                r.new.trim()
            ));
        }
    }
    let unlisted = report
        .total()
        .saturating_sub(report.findings.len().min(MAX_LISTED));
    if unlisted > 0 {
        text.push_str(&format!(
            "{unlisted} more finding(s) are in the code pane.\n"
        ));
    }
    for skipped in &report.skipped {
        text.push_str(&format!(
            "Not checked: {} ({}).\n",
            skipped.path, skipped.reason
        ));
    }
    if report.partial {
        text.push_str(
            "The check stopped at its file limit; check a smaller folder for the rest.\n",
        );
    }
    text
}

#[async_trait]
impl HostTool for DesignCheckTool {
    fn name(&self) -> &'static str {
        app_tools::DESIGN_CHECK
    }
    fn label(&self) -> &'static str {
        "Design check"
    }
    fn label_template(&self) -> &'static str {
        "Checking the design[ of {path}]"
    }
    fn description(&self) -> &'static str {
        "Run Shodh's design check on UI files and show the findings to the user in the code \
         pane: a file or folder of this project (path, relative to the code folder; \".\" for all \
         of it), or a creation (artifact_id from list_artifacts, optionally with a path inside \
         it; in Creator mode, this conversation's creation by default). It reads HTML, CSS, SCSS, \
         JSX/TSX, Vue and Svelte files and their Tailwind classes, with deterministic rules (no \
         model): missing alt text and labels, buttons with no name, clickable divs, removed \
         focus outlines, low contrast, text under 12px, skipped headings, placeholder images and \
         Lorem ipsum, emoji used as icons, purple-blue gradient defaults, gradient text, \
         side-tab borders, glows, inconsistent spacing, radius and type scales, and more. \
         Returns each finding with its rule, severity, file:line and a suggested fix (an exact \
         line replacement when the fix is mechanical). It changes nothing: make fixes with your \
         edit tool."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "minLength": 1, "maxLength": 1024},
                "artifact_id": {"type": "string", "minLength": 1, "maxLength": 64}
            },
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let (subject, conversation) = self.subject(&args, ctx).await?;
        let check = run_detached(subject, conversation, true)
            .await
            .map_err(ToolError::Failed)?;
        self.effects.design_check_done(&check);
        emit_shown(
            ctx,
            "ask",
            None,
            Some(NavigationTarget::DesignCheck {
                check_id: check.id.clone(),
            }),
            Some(&format!("the design check of {}", check.target.label)),
        );
        let report = &check.report;
        Ok(ToolOutput {
            text_for_model: findings_text(&check),
            summary_for_ui: format!(
                "Design check of {} · {} issue(s) in {} file(s)",
                check.target.label,
                report.total(),
                report.files_checked
            ),
            detail: Some(json!({
                "checkId": check.id,
                "files": report.files_checked,
                "findings": report.total(),
                "errors": report.counts.error,
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shodh_rag::workspaces::{NewSource, NewWorkspace, SourceKind, WorkspaceAuthor};

    #[tokio::test]
    async fn the_design_check_reads_the_project_shows_the_pane_and_returns_receipts() {
        let t = crate::agent_tools::testing::host().await;
        let project = t.dir.path().join("project");
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::write(
            project.join("src/Hero.tsx"),
            "export const Hero = () => (\n  <h1 className=\"text-[10px]\">Hi</h1>\n);\n",
        )
        .unwrap();
        let folder = project.display().to_string();
        let workspace = t
            .host
            .workspaces
            .run(move |s| {
                let ws = s.create(
                    &NewWorkspace {
                        name: "Project".into(),
                        ..NewWorkspace::default()
                    },
                    WorkspaceAuthor::User,
                )?;
                s.add_sources(
                    &ws.id,
                    &[NewSource {
                        kind: SourceKind::Folder,
                        reference: "folder-project".into(),
                        label: "project".into(),
                        path: Some(folder),
                    }],
                    WorkspaceAuthor::User,
                )?;
                Ok(ws)
            })
            .await
            .unwrap();
        let tool = DesignCheckTool {
            effects: t.effects.clone(),
            data_dir: t.dir.path().to_path_buf(),
            place: Place::Code {
                workspaces: t.host.workspaces.clone(),
                workspace_id: Some(workspace.id.clone()),
            },
        };
        assert_eq!(tool.tier(), RiskTier::Read);
        let (ctx, mut rx) = crate::agent_tools::testing::ctx();
        let out = tool.execute(json!({"path": "src"}), &ctx).await.unwrap();
        let shown = crate::agent_tools::testing::expect_navigation(&mut rx, "design_check");
        assert_eq!(shown.view, "ask");
        let Some(NavigationTarget::DesignCheck { check_id }) = shown.target else {
            panic!("the pane opens on the check");
        };
        assert!(check_id.starts_with("dc_"));
        assert_eq!(
            t.effects.design_checks.lock().unwrap().as_slice(),
            [check_id.clone()],
            "the check is kept for the pane"
        );
        // Receipts: rule, severity, file:line, and the exact mechanical fix.
        assert!(
            out.text_for_model.contains("(tiny-text) at src/Hero.tsx:2"),
            "{}",
            out.text_for_model
        );
        assert!(out.text_for_model.contains("[warning]"));
        assert!(out.text_for_model.contains("text-xs"));
        assert!(out
            .text_for_model
            .contains("make it only with your edit tool"));
        assert_eq!(out.detail.as_ref().unwrap()["checkId"], check_id);

        // Paths stay in the folder.
        assert!(tool.execute(json!({"path": "../x"}), &ctx).await.is_err());
        assert!(tool.execute(json!({"path": ".env"}), &ctx).await.is_err());
    }

    #[tokio::test]
    async fn a_research_chat_checks_artifacts_only() {
        let t = crate::agent_tools::testing::host().await;
        let tool = DesignCheckTool {
            effects: t.effects.clone(),
            data_dir: t.dir.path().to_path_buf(),
            place: Place::Chat,
        };
        let (ctx, _rx) = crate::agent_tools::testing::ctx();
        assert!(tool.execute(json!({"path": "."}), &ctx).await.is_err());
        let missing = tool
            .execute(
                json!({"artifact_id": "cr_00000000000000000000000000000000"}),
                &ctx,
            )
            .await;
        assert!(missing.is_err(), "no such creation");
        let schema = tool.schema();
        let mut properties: Vec<&String> =
            schema["properties"].as_object().unwrap().keys().collect();
        properties.sort();
        assert_eq!(properties, vec!["artifact_id", "path"]);
        assert_eq!(schema["additionalProperties"], false);
    }
}
