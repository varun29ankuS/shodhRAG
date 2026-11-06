//! The code review pane's commands: run the fresh-eyes reviewer, read a file
//! or the changes of a workspace's code folder, and get a review back.
//!
//! The reviewer is [`shodh_rag::code_review::run_review`]: one stateless call
//! to the configured model (with the app's fallback and Local-only mode, as
//! every direct model call), whose prompt holds only the [`ReviewRequest`]
//! and, for an Architecture review, the structural view Shodh builds of the
//! folder here. Nothing in this module reads a conversation, a memory or a
//! workspace's instructions; `fresh_eyes_tests` below keeps it that way.
//!
//! Files are read only inside the workspace's code folder (links that lead
//! out refused), at most [`MAX_FILE_BYTES`]; changes come from Code mode's
//! hardened git ([`shodh_rag::harness::code_mode::working_changes`]).

use std::collections::VecDeque;
use std::sync::Mutex;

use serde::Serialize;
use shodh_rag::code_review::diff::{capped, FileChange};
use shodh_rag::code_review::structure::language_of;
use shodh_rag::code_review::{
    run_review, LightStructure, ReviewCheck, ReviewReport, ReviewRequest, StructureSource,
};
use shodh_rag::harness::code_mode::{working_changes, CodeBranchStore};
use tauri::{AppHandle, Manager, State};

use crate::agent_session_commands::{inside_code_folder, workspace_code_folder};
use crate::llm_commands::LLMState;
use crate::model_fallback::Purpose;
use crate::workspace_commands::WorkspaceState;

/// Reviews kept for the pane (the newest); older ones are run again.
const MAX_KEPT_REVIEWS: usize = 32;
/// Bytes of a file the pane shows at most.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// Lines of changes the pane shows at most, over every file.
pub const MAX_DIFF_LINES: usize = 4_000;

/// A finished review, as the pane shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredReview {
    pub id: String,
    pub created_at: String,
    pub workspace_id: Option<String>,
    pub conversation_id: Option<String>,
    /// Asked for by the assistant (`review_code`), not by the user.
    pub by_agent: bool,
    /// Exactly what the reviewer was given.
    pub request: ReviewRequest,
    pub report: ReviewReport,
}

/// The newest reviews, for the pane and the assistant's `review_code`.
#[derive(Default)]
pub struct CodeReviewState {
    reviews: Mutex<VecDeque<StoredReview>>,
}

impl CodeReviewState {
    fn keep(&self, review: StoredReview) {
        let mut reviews = self.reviews.lock().unwrap_or_else(|e| e.into_inner());
        reviews.push_front(review);
        reviews.truncate(MAX_KEPT_REVIEWS);
    }

    fn get(&self, id: &str) -> Option<StoredReview> {
        self.reviews
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|r| r.id == id)
            .cloned()
    }
}

fn data_dir(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    crate::profile::app_data_dir(app).map_err(|e| format!("App data directory unavailable: {e}"))
}

/// The structural view of `folder` (relative to the workspace's code folder;
/// `.` for all of it).
async fn structure_of(
    workspaces: &WorkspaceState,
    workspace_id: Option<&str>,
    folder: &str,
) -> Result<shodh_rag::code_review::StructureView, String> {
    let code = workspace_code_folder(workspaces, workspace_id).await?;
    let root = code.root().to_path_buf();
    let folder = folder.trim();
    let target = if folder.is_empty() || folder == "." {
        root.clone()
    } else {
        inside_code_folder(&root, folder)
            .ok_or_else(|| format!("{folder} is not a folder of the code folder."))?
    };
    tokio::task::spawn_blocking(move || LightStructure.build(&root, &target))
        .await
        .map_err(|e| format!("Reading the folder did not finish: {e}"))?
}

/// Review `request` and keep the review. The request is passed on as it is:
/// the reviewer gets nothing else, apart from the structural view of an
/// Architecture review's folder. Commands are proposed only for a review
/// that a Code conversation can run them from (`conversation_id` and a code
/// folder).
pub(crate) async fn run_and_store(
    app: &AppHandle,
    mut request: ReviewRequest,
    workspace_id: Option<String>,
    conversation_id: Option<String>,
    by_agent: bool,
) -> Result<StoredReview, String> {
    let workspaces = app.state::<WorkspaceState>();
    let structure = match (&request.check, request.folder.as_deref()) {
        (ReviewCheck::Architecture, Some(folder)) => {
            Some(structure_of(&workspaces, workspace_id.as_deref(), folder).await?)
        }
        _ => None,
    };
    if request.can_verify {
        let has_code = workspace_code_folder(&workspaces, workspace_id.as_deref())
            .await
            .is_ok();
        request.can_verify = has_code && conversation_id.is_some();
    }
    let dir = data_dir(app)?;
    let llm = app.state::<LLMState>();
    let model = crate::memory_learn::answering_model(app, &dir, &llm, Purpose::CodeReview)?;
    let report = run_review(model.as_ref(), &request, structure.as_ref()).await?;
    let review = StoredReview {
        id: format!("rv_{}", uuid::Uuid::new_v4().simple()),
        created_at: chrono::Utc::now().to_rfc3339(),
        workspace_id,
        conversation_id,
        by_agent,
        request,
        report,
    };
    app.state::<CodeReviewState>().keep(review.clone());
    tracing::info!(
        target: "shodh::code_review",
        check = review.report.check.as_str(),
        findings = review.report.findings.len(),
        dropped = review.report.dropped,
        by_agent,
        "review finished"
    );
    Ok(review)
}

/// Run the fresh-eyes reviewer (Review this, Architecture).
#[tauri::command]
pub async fn code_review_run(
    app: AppHandle,
    request: ReviewRequest,
    workspace_id: Option<String>,
    conversation_id: Option<String>,
) -> Result<StoredReview, String> {
    run_and_store(&app, request, workspace_id, conversation_id, false).await
}

/// A review kept for the pane (the assistant's `review_code` opens one by id).
#[tauri::command]
pub async fn code_review_get(
    id: String,
    state: State<'_, CodeReviewState>,
) -> Result<StoredReview, String> {
    state.get(id.trim()).ok_or_else(|| {
        "That review is no longer kept; run it again from the code it was about.".to_string()
    })
}

/// A file of the code folder, as the pane shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeFile {
    pub path: String,
    pub language: String,
    pub text: String,
}

/// Read `path` of the workspace's code folder for the pane.
#[tauri::command]
pub async fn code_file_read(
    workspace_id: String,
    path: String,
    workspaces: State<'_, WorkspaceState>,
) -> Result<CodeFile, String> {
    let code = workspace_code_folder(&workspaces, Some(&workspace_id)).await?;
    let root = code.root().to_path_buf();
    let shown = path.trim().replace('\\', "/");
    let full = inside_code_folder(&root, &shown)
        .ok_or_else(|| format!("{shown} is not a file of the code folder."))?;
    let text = tokio::task::spawn_blocking(move || -> Result<String, String> {
        let meta = std::fs::symlink_metadata(&full).map_err(|e| e.to_string())?;
        if !meta.is_file() {
            return Err("That is not a file.".to_string());
        }
        if meta.len() > MAX_FILE_BYTES {
            return Err("The file is too large to show here (over 1 MB).".to_string());
        }
        let bytes = std::fs::read(&full).map_err(|e| e.to_string())?;
        String::from_utf8(bytes).map_err(|_| "The file is not text.".to_string())
    })
    .await
    .map_err(|e| format!("Reading the file did not finish: {e}"))??;
    Ok(CodeFile {
        language: language_of(&shown).unwrap_or("text").to_string(),
        path: shown,
        text,
    })
}

/// The code changes of the workspace's code folder: against the branch a
/// Code conversation started from (its `shodh/…` branch's base) when it
/// has one, else against the last commit. `path` limits it to one file or
/// folder.
#[tauri::command]
pub async fn code_diff(
    app: AppHandle,
    workspace_id: String,
    conversation_id: Option<String>,
    path: Option<String>,
    workspaces: State<'_, WorkspaceState>,
) -> Result<Vec<FileChange>, String> {
    let code = workspace_code_folder(&workspaces, Some(&workspace_id)).await?;
    let root = code.root().to_path_buf();
    let path = match path.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => {
            let shown = p.replace('\\', "/");
            inside_code_folder(&root, &shown)
                .ok_or_else(|| format!("{shown} is not in the code folder."))?;
            Some(shown)
        }
        None => None,
    };
    let base = match conversation_id.as_deref() {
        Some(id) => CodeBranchStore::in_dir(&data_dir(&app)?)
            .get(id)?
            .filter(|record| std::path::Path::new(&record.folder) == root.as_path())
            .map(|record| record.base),
        None => None,
    };
    let changes = tokio::task::spawn_blocking(move || {
        working_changes(&root, base.as_deref(), path.as_deref())
    })
    .await
    .map_err(|e| format!("Reading the changes did not finish: {e}"))??;
    Ok(capped(changes, MAX_DIFF_LINES))
}

#[cfg(test)]
mod fresh_eyes_tests {
    //! The reviewer gets only its request. These read this module and the
    //! assistant's review tool as text: neither may reach for anything a
    //! conversation holds.

    const SOURCES: [(&str, &str); 2] = [
        (
            "code_review_commands.rs",
            include_str!("code_review_commands.rs"),
        ),
        (
            "agent_tools/code_review.rs",
            include_str!("agent_tools/code_review.rs"),
        ),
    ];

    /// What would bring conversation content to the reviewer.
    const FORBIDDEN: [&str; 12] = [
        "MemoryState",
        "recall_for_run",
        "compose_message",
        "HistoryTurn",
        "agent_send",
        "agent_start",
        "conversation_commands",
        "load_conversation",
        "instructions_block",
        "carried_passages",
        "LearnState",
        "AgentSessions",
    ];

    #[test]
    fn the_reviewer_code_never_reaches_for_conversation_content() {
        for (name, source) in SOURCES {
            // This test's own list is not a use.
            let code = source
                .split("mod fresh_eyes_tests")
                .next()
                .unwrap_or(source);
            for word in FORBIDDEN {
                assert!(
                    !code.contains(word),
                    "{name} mentions {word}: the reviewer must get only its request"
                );
            }
        }
    }
}
