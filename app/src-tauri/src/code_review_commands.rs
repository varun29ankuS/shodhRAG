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
use std::path::Path;
use std::sync::Mutex;

use serde::Serialize;
use shodh_rag::code_review::diff::FileChange;
use shodh_rag::code_review::secrets::secret_path;
use shodh_rag::code_review::structure::language_of;
use shodh_rag::code_review::{
    run_review, ReviewCheck, ReviewReport, ReviewRequest, StructureSource,
};
use shodh_rag::harness::code_mode::{git_ignores, working_changes, CodeBranchStore};
use tauri::{AppHandle, Manager, State};

use crate::agent_session_commands::{inside_code_folder, workspace_code_folder};
use crate::code_map_commands::CodeMapState;
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
/// `.` for all of it), from the project's code map, brought up to date first
/// (built on the spot when the project has none yet).
async fn structure_of(
    workspaces: &WorkspaceState,
    maps: &CodeMapState,
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
    let map = maps.map_for(&root).await?;
    tokio::task::spawn_blocking(move || map.build(&root, &target))
        .await
        .map_err(|e| format!("Reading the folder did not finish: {e}"))?
}

/// What a Code conversation's changes are compared with: the branch its
/// `shodh/…` branch started from, when it has one in `root`; `None` (the last
/// commit) otherwise.
pub(crate) fn conversation_base(
    data_dir: &Path,
    conversation_id: &str,
    root: &Path,
) -> Result<Option<String>, String> {
    Ok(CodeBranchStore::in_dir(data_dir)
        .get(conversation_id)?
        .filter(|record| Path::new(&record.folder) == root)
        .map(|record| record.base))
}

/// Review `request` with `model`: the request is passed on as it is, and the
/// reviewer gets nothing else, apart from the structural view of an
/// Architecture review's folder. Commands are proposed only for a review that
/// a Code conversation can run them from (`conversation_id` and a code folder).
/// `workspaces` reaches the workspace (its folder, and also its instructions,
/// which must never reach the reviewer): a test drives this with them in reach.
pub(crate) async fn review_with(
    model: &dyn shodh_rag::research::results::TextModel,
    workspaces: &WorkspaceState,
    maps: &CodeMapState,
    mut request: ReviewRequest,
    workspace_id: Option<String>,
    conversation_id: Option<String>,
    by_agent: bool,
) -> Result<StoredReview, String> {
    let structure = match (&request.check, request.folder.as_deref()) {
        (ReviewCheck::Architecture, Some(folder)) => {
            Some(structure_of(workspaces, maps, workspace_id.as_deref(), folder).await?)
        }
        _ => None,
    };
    if request.can_verify {
        let has_code = workspace_code_folder(workspaces, workspace_id.as_deref())
            .await
            .is_ok();
        request.can_verify = has_code && conversation_id.is_some();
    }
    let report = run_review(model, &request, structure.as_ref()).await?;
    Ok(StoredReview {
        id: format!("rv_{}", uuid::Uuid::new_v4().simple()),
        created_at: chrono::Utc::now().to_rfc3339(),
        workspace_id,
        conversation_id,
        by_agent,
        request,
        report,
    })
}

/// [`review_with`] the configured model (with the app's fallback and Local-only
/// mode), and keep the review for the pane.
pub(crate) async fn run_and_store(
    app: &AppHandle,
    request: ReviewRequest,
    workspace_id: Option<String>,
    conversation_id: Option<String>,
    by_agent: bool,
) -> Result<StoredReview, String> {
    let dir = data_dir(app)?;
    let llm = app.state::<LLMState>();
    let model = crate::memory_learn::answering_model(app, &dir, &llm, Purpose::CodeReview)?;
    let workspaces = app.state::<WorkspaceState>();
    let maps = app.state::<CodeMapState>();
    let review = review_with(
        model.as_ref(),
        &workspaces,
        &maps,
        request,
        workspace_id,
        conversation_id,
        by_agent,
    )
    .await?;
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

/// The model a review would be sent to now: the configured one (the fallback
/// may answer with another of the user's models). `None` when no model is set.
pub(crate) fn review_target(app: &AppHandle) -> Option<crate::agent_tools::ReviewTarget> {
    let llm = app.state::<LLMState>();
    let local_only = data_dir(app)
        .ok()
        .and_then(|dir| crate::app_settings::SettingsStore::in_dir(&dir).load().ok())
        .is_some_and(|s| s.policy.local_only);
    if let Some(model) = crate::model_picker_commands::configured_model(&llm) {
        return Some(crate::agent_tools::ReviewTarget {
            model: format!("{} · {}", model.provider.label(), model.model),
            cloud: !model.provider.is_local() && !local_only,
        });
    }
    let external = matches!(
        llm.config.lock().unwrap_or_else(|e| e.into_inner()).mode,
        shodh_rag::llm::LLMMode::External { .. }
    );
    (!external).then(|| crate::agent_tools::ReviewTarget {
        model: "the model on this computer".to_string(),
        cloud: false,
    })
}

/// Which model the pane's review goes to, said before it is sent.
#[tauri::command]
pub async fn code_review_target(
    app: AppHandle,
) -> Result<Option<crate::agent_tools::ReviewTarget>, String> {
    Ok(review_target(&app))
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
        "That review is no longer kept (reviews are kept until Shodh closes); run it again \
         from the code it was about."
            .to_string()
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

/// Why `shown` (a path inside `root`) is not read for the pane or a review: the
/// `.git` folder, a file that may hold secrets, a file git ignores. Blocking.
pub(crate) fn unreadable_reason(root: &Path, shown: &str) -> Option<String> {
    if let Some(why) = secret_path(shown) {
        return Some(format!("Shodh does not read {shown}: {why}."));
    }
    (shown != "." && git_ignores(root, shown)).then(|| {
        format!(
            "Shodh does not read {shown}: git ignores it, and ignored files often hold local \
             settings or secrets."
        )
    })
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
    let checked = shown.clone();
    let text = tokio::task::spawn_blocking(move || -> Result<String, String> {
        if let Some(why) = unreadable_reason(&root, &checked) {
            return Err(why);
        }
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
/// folder. Files that may hold secrets are listed by name only.
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
        Some(id) => conversation_base(&data_dir(&app)?, id, &root)?,
        None => None,
    };
    tokio::task::spawn_blocking(move || {
        working_changes(&root, base.as_deref(), path.as_deref(), MAX_DIFF_LINES)
    })
    .await
    .map_err(|e| format!("Reading the changes did not finish: {e}"))?
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

#[cfg(test)]
mod review_path_tests {
    //! The real call path of a review, with what it can reach planted with canaries: the
    //! workspace (its instructions, through the same state the review reads the folder
    //! from), files of the folder besides the selection, and function bodies. Only the
    //! request, and the structure Shodh built, reach the model.

    use std::sync::Mutex;

    use super::*;
    use shodh_rag::code_review::{LineKind, PieceLine, ReviewPiece};
    use shodh_rag::research::results::TextModel;
    use shodh_rag::workspaces::{NewSource, NewWorkspace, SourceKind, WorkspaceAuthor};

    struct Recording(Mutex<Vec<String>>);

    #[async_trait::async_trait]
    impl TextModel for Recording {
        fn model_id(&self) -> String {
            "recording".into()
        }
        async fn complete(&self, prompt: &str, _max_tokens: usize) -> Result<String, String> {
            self.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(prompt.to_string());
            Ok(r#"{"summary":"none","findings":[]}"#.to_string())
        }
    }

    const INSTRUCTIONS_CANARY: &str = "CANARY-WS-INSTRUCTIONS-41d2";
    const SIBLING_CANARY: &str = "CANARY-SIBLING-FILE-90ab";
    const BODY_CANARY: &str = "CANARY-FUNCTION-BODY-77ce";

    #[tokio::test]
    async fn only_the_request_reaches_the_reviewer_through_the_real_path() {
        let t = crate::agent_tools::testing::host().await;
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"demo\"\n").unwrap();
        std::fs::write(
            root.join("src/lib.rs"),
            format!("pub mod other;\npub fn api() {{ let _ = \"{BODY_CANARY}\"; }}\n"),
        )
        .unwrap();
        std::fs::write(
            root.join("src/other.rs"),
            format!("use crate::api;\npub fn helper() {{ let _ = \"{SIBLING_CANARY}\"; }}\n"),
        )
        .unwrap();
        let folder = root.display().to_string();
        let workspace = t
            .host
            .workspaces
            .run(move |s| {
                let created = s.create(
                    &NewWorkspace {
                        name: "Demo".into(),
                        instructions: Some(INSTRUCTIONS_CANARY.into()),
                        ..NewWorkspace::default()
                    },
                    WorkspaceAuthor::User,
                )?;
                s.add_sources(
                    &created.id,
                    &[NewSource {
                        kind: SourceKind::Folder,
                        reference: "src-demo".into(),
                        label: "Demo".into(),
                        path: Some(folder),
                    }],
                    WorkspaceAuthor::User,
                )?;
                Ok(created.id)
            })
            .await
            .unwrap();
        let model = Recording(Mutex::new(Vec::new()));

        // A snippet of src/lib.rs's first line.
        let snippet = ReviewRequest {
            check: ReviewCheck::Security,
            pieces: vec![ReviewPiece {
                path: "src/lib.rs".into(),
                language: "rust".into(),
                lines: vec![PieceLine {
                    number: 1,
                    kind: LineKind::Same,
                    text: "pub mod other;".into(),
                }],
            }],
            question: None,
            folder: None,
            flows: None,
            context: None,
            can_verify: true,
        };
        let maps = CodeMapState::new(t.host.data_dir.clone(), None);
        let review = review_with(
            &model,
            &t.host.workspaces,
            &maps,
            snippet,
            Some(workspace.clone()),
            Some("conv-1".into()),
            false,
        )
        .await
        .unwrap();
        assert!(
            review.request.can_verify,
            "a Code conversation's review may propose checks"
        );

        // An architecture review of the whole folder.
        let architecture = ReviewRequest {
            check: ReviewCheck::Architecture,
            pieces: Vec::new(),
            question: None,
            folder: Some(".".into()),
            flows: None,
            context: None,
            can_verify: false,
        };
        review_with(
            &model,
            &t.host.workspaces,
            &maps,
            architecture,
            Some(workspace),
            None,
            false,
        )
        .await
        .unwrap();

        let prompts = model.0.lock().unwrap().clone();
        assert_eq!(prompts.len(), 2);
        for prompt in &prompts {
            for canary in [INSTRUCTIONS_CANARY, SIBLING_CANARY, BODY_CANARY] {
                assert!(
                    !prompt.contains(canary),
                    "the reviewer saw {canary}:\n{prompt}"
                );
            }
        }
        assert!(prompts[0].contains("pub mod other;"));
        assert!(
            prompts[1].contains("src/other.rs"),
            "the structure names the files"
        );
    }
}
