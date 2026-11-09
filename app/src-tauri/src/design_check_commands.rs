//! The design check of UI files ([`shodh_rag::design_check`]): run on demand
//! (the Artifacts page, the code pane, a folder), automatically after Code
//! and Creator mode write files, and by the assistant's `design_check`; kept
//! for the code pane, which shows the findings with the review pane's cards.
//!
//! The check is deterministic and calls no model. A mechanical fix is only
//! proposed: the pane sends it to the conversation as a review action, so the
//! edit asks the user first in every mode, as every fix from the review pane
//! does. Nothing here writes a file.
//!
//! No idle cost: a check runs only when asked, or once after a Code or
//! Creator session's write steps go quiet ([`DEBOUNCE`]); there is no timer,
//! watcher or thread otherwise. Each check is bounded by the engine's limits
//! ([`shodh_rag::design_check::MAX_FILES`] and the byte caps).

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use shodh_rag::design_check::{changed_ui_files, check_files, ui_files, DesignReport, MAX_FILES};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::agent_session_commands::{inside_code_folder, workspace_code_folder};
use crate::code_review_commands::{conversation_base, unreadable_reason};
use crate::creations::{creations_root, valid_id, CreationStore};
use crate::workspace_commands::WorkspaceState;

/// Checks kept for the pane (the newest).
const MAX_KEPT: usize = 32;
/// How long a session's writes must be quiet before the automatic check runs.
pub const DEBOUNCE: Duration = Duration::from_millis(1_500);
/// The event the badges refresh on.
pub const DESIGN_CHECK_UPDATED: &str = "design-check-updated";

/// What was checked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckedTarget {
    /// `folder`, `file`, `changes` (a Code conversation's changed UI files) or
    /// `artifact` (a creation's folder).
    pub kind: &'static str,
    /// In words, for the pane ("src/components", "the changes", a creation's title).
    pub label: String,
    /// The folder or file checked, relative to the project or creation (`.` for all).
    pub path: String,
}

/// A finished check, as the pane and the badges show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredDesignCheck {
    pub id: String,
    pub created_at: String,
    pub target: CheckedTarget,
    pub workspace_id: Option<String>,
    pub creation_id: Option<String>,
    /// The conversation whose files these are (a fix goes to it).
    pub conversation_id: Option<String>,
    /// Asked for by the assistant (`design_check`).
    pub by_agent: bool,
    /// Run by Shodh after the agent wrote files.
    pub automatic: bool,
    pub report: DesignReport,
}

/// What a check reads: files under `root`.
#[derive(Debug, Clone)]
pub struct Subject {
    pub root: PathBuf,
    pub scope: Scope,
    pub workspace_id: Option<String>,
    pub creation_id: Option<String>,
    pub label: String,
}

#[derive(Debug, Clone)]
pub enum Scope {
    /// A file or folder of `root` (`.` for all of it).
    Path(String),
    /// The UI files changed against `base` (the last commit when `None`).
    Changes { base: Option<String> },
}

/// Run the check on `subject` (blocking). `None` when `Changes` was asked
/// outside a git repository, where what changed is unknown.
pub fn check_subject(subject: &Subject) -> Option<(CheckedTarget, DesignReport)> {
    let (kind, path, files) = match &subject.scope {
        Scope::Path(path) => {
            let path = if path.trim().is_empty() {
                ".".to_string()
            } else {
                path.trim().to_string()
            };
            let is_file =
                std::fs::symlink_metadata(subject.root.join(&path)).is_ok_and(|m| m.is_file());
            let files = ui_files(&subject.root, &path);
            (if is_file { "file" } else { "folder" }, path, files)
        }
        Scope::Changes { base } => (
            "changes",
            ".".to_string(),
            changed_ui_files(&subject.root, base.as_deref())?,
        ),
    };
    let kind = if subject.creation_id.is_some() && kind == "folder" {
        "artifact"
    } else {
        kind
    };
    let mut report = check_files(&subject.root, &files);
    report.partial |= files.len() > MAX_FILES;
    Some((
        CheckedTarget {
            kind,
            label: subject.label.clone(),
            path,
        },
        report,
    ))
}

/// A check as it is kept.
pub fn stored(
    subject: &Subject,
    target: CheckedTarget,
    report: DesignReport,
    conversation_id: Option<String>,
    by_agent: bool,
    automatic: bool,
) -> StoredDesignCheck {
    StoredDesignCheck {
        id: format!("dc_{}", uuid::Uuid::new_v4().simple()),
        created_at: chrono::Utc::now().to_rfc3339(),
        target,
        workspace_id: subject.workspace_id.clone(),
        creation_id: subject.creation_id.clone(),
        conversation_id,
        by_agent,
        automatic,
        report,
    }
}

/// The creation `id` as a subject: its folder (while it is still the folder
/// it was made in) and `path` inside it. Blocking. Returns the subject and
/// the creation's conversation.
pub fn creation_subject(
    data_dir: &Path,
    documents: &Path,
    id: &str,
    path: &str,
) -> Result<(Subject, String), String> {
    if !valid_id(id) {
        return Err(format!("{id} is not an artifact id."));
    }
    let creation = CreationStore::in_dir(data_dir)
        .get(id)?
        .ok_or_else(|| format!("No creation has the id {id}."))?;
    let folder = creation.checked_folder(&creations_root(documents)?)?;
    let path = checked_relative(&folder, path)?;
    Ok((
        Subject {
            root: folder,
            scope: Scope::Path(path),
            workspace_id: None,
            creation_id: Some(creation.id.clone()),
            label: creation.title.clone(),
        },
        creation.conversation_id,
    ))
}

/// `path` (relative, `/`-separated) when it is inside `root` and readable:
/// not the `.git` folder, not a file that may hold secrets, not one git
/// ignores. `.` for the whole folder. Blocking.
pub fn checked_relative(root: &Path, path: &str) -> Result<String, String> {
    let shown = path.trim().replace('\\', "/");
    let shown = shown.trim_start_matches("./").trim_end_matches('/');
    if shown.is_empty() || shown == "." {
        return Ok(".".to_string());
    }
    inside_code_folder(root, shown).ok_or_else(|| format!("{shown} is not in the folder."))?;
    if let Some(why) = unreadable_reason(root, shown) {
        return Err(why);
    }
    Ok(shown.to_string())
}

/// The newest checks, the latest one per conversation and per creation (for
/// the badges), and the automatic checks waiting for writes to go quiet.
#[derive(Default)]
pub struct DesignCheckState {
    checks: Mutex<VecDeque<StoredDesignCheck>>,
    /// Latest check id by `conversation:<id>` and `creation:<id>`.
    latest: Mutex<HashMap<String, String>>,
    /// The newest request of each conversation's automatic check.
    waiting: Mutex<HashMap<String, u64>>,
    generation: AtomicU64,
}

impl DesignCheckState {
    pub fn keep(&self, check: StoredDesignCheck) {
        {
            let mut latest = self.latest.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(conversation) = &check.conversation_id {
                latest.insert(format!("conversation:{conversation}"), check.id.clone());
            }
            if let Some(creation) = &check.creation_id {
                latest.insert(format!("creation:{creation}"), check.id.clone());
            }
        }
        let mut checks = self.checks.lock().unwrap_or_else(|e| e.into_inner());
        checks.push_front(check);
        checks.truncate(MAX_KEPT);
    }

    pub fn get(&self, id: &str) -> Option<StoredDesignCheck> {
        self.checks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|c| c.id == id)
            .cloned()
    }

    /// The latest check of a conversation or a creation.
    pub fn latest(
        &self,
        conversation_id: Option<&str>,
        creation_id: Option<&str>,
    ) -> Option<StoredDesignCheck> {
        let key = match (creation_id, conversation_id) {
            (Some(creation), _) => format!("creation:{creation}"),
            (None, Some(conversation)) => format!("conversation:{conversation}"),
            (None, None) => return None,
        };
        let id = self
            .latest
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
            .cloned()?;
        self.get(&id)
    }

    /// A new request for `key`'s automatic check; returns its number.
    fn request(&self, key: &str) -> u64 {
        let n = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        self.waiting
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.to_string(), n);
        n
    }

    /// Whether request `n` is still `key`'s newest.
    fn is_newest(&self, key: &str, n: u64) -> bool {
        self.waiting
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .is_some_and(|newest| *newest == n)
    }

    fn done(&self, key: &str, n: u64) {
        let mut waiting = self.waiting.lock().unwrap_or_else(|e| e.into_inner());
        if waiting.get(key).is_some_and(|newest| *newest == n) {
            waiting.remove(key);
        }
    }
}

fn data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    crate::profile::app_data_dir(app).map_err(|e| format!("App data directory unavailable: {e}"))
}

fn documents(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .document_dir()
        .map_err(|e| format!("The Documents folder is unavailable ({e})"))
}

/// Keep `check` and tell the badges.
pub fn keep_and_announce(app: &AppHandle, check: &StoredDesignCheck) {
    app.state::<DesignCheckState>().keep(check.clone());
    tracing::info!(
        target: "shodh::design_check",
        kind = check.target.kind,
        files = check.report.files_checked,
        findings = check.report.total(),
        automatic = check.automatic,
        by_agent = check.by_agent,
        "design check finished"
    );
    if let Err(e) = app.emit(
        DESIGN_CHECK_UPDATED,
        serde_json::json!({
            "id": check.id,
            "conversationId": check.conversation_id,
            "creationId": check.creation_id,
            "workspaceId": check.workspace_id,
            "total": check.report.total(),
        }),
    ) {
        tracing::warn!(target: "shodh::design_check", error = %e, "the design-check-updated event could not be sent");
    }
}

/// The subject a request from the UI names: a creation (its id, and a path
/// inside it), or a workspace's code folder (a path inside it, `.` for all).
async fn subject_of(
    app: &AppHandle,
    workspaces: &WorkspaceState,
    workspace_id: Option<String>,
    creation_id: Option<String>,
    path: Option<String>,
) -> Result<(Subject, Option<String>), String> {
    let path = path.unwrap_or_else(|| ".".to_string());
    if let Some(id) = creation_id.map(|id| id.trim().to_string()) {
        let (data, docs) = (data_dir(app)?, documents(app)?);
        let (subject, conversation) =
            tokio::task::spawn_blocking(move || creation_subject(&data, &docs, &id, &path))
                .await
                .map_err(|e| format!("Reading the creation did not finish: {e}"))??;
        return Ok((subject, Some(conversation)));
    }
    let workspace = workspace_id
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| "A design check needs a project folder or an artifact.".to_string())?;
    let folder = workspace_code_folder(workspaces, Some(&workspace)).await?;
    let root = folder.root().to_path_buf();
    let checked_root = root.clone();
    let path = tokio::task::spawn_blocking(move || checked_relative(&checked_root, &path))
        .await
        .map_err(|e| format!("Checking the path did not finish: {e}"))??;
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
        None,
    ))
}

/// Run a design check from the UI: on a creation (`creation_id`), or on a
/// file or folder of a workspace's code folder (`path`, `.` for all of it),
/// or on a Code conversation's changed UI files (`changes`).
#[tauri::command]
pub async fn design_check_run(
    app: AppHandle,
    workspace_id: Option<String>,
    conversation_id: Option<String>,
    creation_id: Option<String>,
    path: Option<String>,
    changes: Option<bool>,
    workspaces: State<'_, WorkspaceState>,
) -> Result<StoredDesignCheck, String> {
    let (mut subject, creation_conversation) =
        subject_of(&app, &workspaces, workspace_id, creation_id, path).await?;
    if changes == Some(true) && subject.creation_id.is_none() {
        let base = match conversation_id.as_deref() {
            Some(id) => conversation_base(&data_dir(&app)?, id, &subject.root)?,
            None => None,
        };
        subject.scope = Scope::Changes { base };
        subject.label = "the changes".to_string();
    }
    let conversation = creation_conversation.or(conversation_id);
    let check = run_detached(subject, conversation, false).await?;
    keep_and_announce(&app, &check);
    Ok(check)
}

/// A kept check (the assistant's `design_check` opens one by id).
#[tauri::command]
pub async fn design_check_get(
    id: String,
    state: State<'_, DesignCheckState>,
) -> Result<StoredDesignCheck, String> {
    state.get(id.trim()).ok_or_else(|| {
        "That design check is no longer kept (checks are kept until Shodh closes); run it again."
            .to_string()
    })
}

/// The latest check of a creation or a conversation, for its badge.
#[tauri::command]
pub async fn design_check_latest(
    conversation_id: Option<String>,
    creation_id: Option<String>,
    state: State<'_, DesignCheckState>,
) -> Result<Option<StoredDesignCheck>, String> {
    Ok(state.latest(conversation_id.as_deref(), creation_id.as_deref()))
}

/// A Code or Creator session whose writes are followed by a design check.
#[derive(Debug, Clone)]
pub struct AutoTarget {
    pub conversation_id: String,
    /// The session's folder: the workspace's code folder or the creation folder.
    pub root: PathBuf,
    pub workspace_id: Option<String>,
    /// A Creator session: its whole creation folder is checked.
    pub creates: bool,
}

/// The session's agent finished a step that may have written files: check
/// its UI files once its writes have been quiet for [`DEBOUNCE`]. A newer
/// write replaces a waiting check; a check whose files changed again while
/// it ran is not kept (the newer one is).
pub fn touched(app: &AppHandle, target: &AutoTarget) {
    let key = target.conversation_id.clone();
    let n = app.state::<DesignCheckState>().request(&key);
    let app = app.clone();
    let target = target.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(DEBOUNCE).await;
        let state = app.state::<DesignCheckState>();
        if !state.is_newest(&key, n) {
            return;
        }
        let data = match data_dir(&app) {
            Ok(dir) => dir,
            Err(e) => {
                tracing::debug!(target: "shodh::design_check", error = %e, "no data folder");
                return;
            }
        };
        let had_check = state.latest(Some(key.as_str()), None).is_some();
        let run = target.clone();
        let outcome =
            tokio::task::spawn_blocking(move || automatic_check(&data, &run, had_check)).await;
        let state = app.state::<DesignCheckState>();
        let newest = state.is_newest(&key, n);
        state.done(&key, n);
        match outcome {
            Ok(Some(check)) if newest => keep_and_announce(&app, &check),
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(target: "shodh::design_check", error = %e, "the automatic design check did not finish")
            }
        }
    });
}

/// The automatic check of `target` (blocking): a Creator session's whole
/// creation folder; a Code session's changed UI files. `None` when there is
/// nothing to show (no UI file touched, and no earlier check to update).
fn automatic_check(
    data_dir: &Path,
    target: &AutoTarget,
    had_check: bool,
) -> Option<StoredDesignCheck> {
    let subject = if target.creates {
        let creation = CreationStore::in_dir(data_dir)
            .for_conversation(&target.conversation_id)
            .ok()
            .flatten()?;
        Subject {
            root: target.root.clone(),
            scope: Scope::Path(".".to_string()),
            workspace_id: None,
            creation_id: Some(creation.id),
            label: creation.title,
        }
    } else {
        let base = conversation_base(data_dir, &target.conversation_id, &target.root)
            .ok()
            .flatten();
        Subject {
            root: target.root.clone(),
            scope: Scope::Changes { base },
            workspace_id: target.workspace_id.clone(),
            creation_id: None,
            label: "the changes".to_string(),
        }
    };
    let (checked, report) = check_subject(&subject)?;
    if report.files_checked == 0 && !had_check {
        return None;
    }
    Some(stored(
        &subject,
        checked,
        report,
        Some(target.conversation_id.clone()),
        false,
        true,
    ))
}

/// Run `subject`'s check off the async threads, without keeping it (the
/// assistant's `design_check` keeps it through its host).
pub async fn run_detached(
    subject: Subject,
    conversation_id: Option<String>,
    by_agent: bool,
) -> Result<StoredDesignCheck, String> {
    let run = subject.clone();
    let (target, report) = tokio::task::spawn_blocking(move || check_subject(&run))
        .await
        .map_err(|e| format!("The design check did not finish: {e}"))?
        .ok_or_else(|| {
            "The folder is not a git repository, so Shodh cannot tell which files changed. \
             Check the whole folder instead."
                .to_string()
        })?;
    Ok(stored(
        &subject,
        target,
        report,
        conversation_id,
        by_agent,
        false,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("src/Card.tsx"),
            "export const Card = () => (\n  <div className=\"text-[10px]\"><img src=\"/a.png\" /></div>\n);\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("src/ok.css"), ".a { font-size: 14px; }\n").unwrap();
        dir
    }

    #[test]
    fn a_folder_check_counts_what_the_badge_shows() {
        let dir = folder();
        let subject = Subject {
            root: dir.path().to_path_buf(),
            scope: Scope::Path("src".into()),
            workspace_id: Some("ws".into()),
            creation_id: None,
            label: "src".into(),
        };
        let (target, report) = check_subject(&subject).unwrap();
        assert_eq!(target.kind, "folder");
        assert_eq!(report.files_checked, 2);
        let rules: Vec<&str> = report.findings.iter().map(|f| f.rule).collect();
        assert!(rules.contains(&"img-missing-alt"), "{rules:?}");
        assert!(rules.contains(&"tiny-text"), "{rules:?}");
        assert_eq!(report.total(), report.findings.len());
        let check = stored(&subject, target, report, Some("conv".into()), true, false);
        assert!(check.id.starts_with("dc_"));
        let state = DesignCheckState::default();
        state.keep(check.clone());
        assert_eq!(state.latest(Some("conv"), None), Some(check.clone()));
        assert_eq!(state.get(&check.id), Some(check));
        assert_eq!(state.latest(Some("other"), None), None);
    }

    /// The ported rules and the design-polish skill credit impeccable in the
    /// repository's NOTICE (pinned commit, copyright, what changed) and keep
    /// its Apache-2.0 licence beside the skill; the engine's files say so too.
    #[test]
    fn impeccable_is_credited_with_its_licence() {
        use shodh_rag::design_check::rules::{IMPECCABLE_COMMIT, IMPECCABLE_URL};
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let notice = std::fs::read_to_string(root.join("NOTICE")).unwrap();
        let credit = format!("{IMPECCABLE_URL}, commit {IMPECCABLE_COMMIT}");
        for needed in [
            credit.as_str(),
            "Copyright 2025 Paul Bakaus",
            "crates/shodh-rag/src/design_check/",
            "skills/design-polish/SKILL.md",
            "impeccable-Apache-2.0.txt",
            "Changed:",
        ] {
            assert!(notice.contains(needed), "NOTICE lacks {needed:?}");
        }
        let licence = std::fs::read_to_string(
            root.join("crates/shodh-rag/skills/licenses/impeccable-Apache-2.0.txt"),
        )
        .unwrap();
        assert!(
            licence.contains("Apache License") && licence.contains("Copyright 2025 Paul Bakaus")
        );
        for file in ["mod.rs", "rules.rs", "scan.rs"] {
            let text =
                std::fs::read_to_string(root.join("crates/shodh-rag/src/design_check").join(file))
                    .unwrap();
            assert!(
                text.contains("impeccable"),
                "{file} does not credit impeccable"
            );
        }
        let skill = shodh_rag::harness::code_skills::bundled("design-polish").expect("bundled");
        assert_eq!(skill.sources[0].commit, IMPECCABLE_COMMIT);
        assert_eq!(skill.sources[0].license, "Apache-2.0");
    }

    #[test]
    fn a_file_check_reads_only_that_file() {
        let dir = folder();
        let subject = Subject {
            root: dir.path().to_path_buf(),
            scope: Scope::Path("src/ok.css".into()),
            workspace_id: None,
            creation_id: None,
            label: "src/ok.css".into(),
        };
        let (target, report) = check_subject(&subject).unwrap();
        assert_eq!(
            (target.kind, report.files_checked, report.total()),
            ("file", 1, 0)
        );
    }

    #[test]
    fn paths_stay_inside_the_folder_and_out_of_secrets() {
        let dir = folder();
        assert_eq!(checked_relative(dir.path(), "./src/").unwrap(), "src");
        assert_eq!(checked_relative(dir.path(), "").unwrap(), ".");
        for refused in ["../x", ".env", ".git/config"] {
            assert!(checked_relative(dir.path(), refused).is_err(), "{refused}");
        }
    }

    #[test]
    fn only_the_newest_automatic_request_runs() {
        let state = DesignCheckState::default();
        let first = state.request("conv");
        let second = state.request("conv");
        assert!(!state.is_newest("conv", first));
        assert!(state.is_newest("conv", second));
        state.done("conv", first);
        assert!(
            state.is_newest("conv", second),
            "an older run never clears a newer request"
        );
        state.done("conv", second);
        assert!(!state.is_newest("conv", second));
    }

    #[test]
    fn an_automatic_check_of_a_code_session_reads_its_changed_ui_files() {
        let dir = folder();
        let root = dir.path();
        let git = |args: &[&str]| {
            assert!(std::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .status()
                .unwrap()
                .success());
        };
        git(&["init", "--quiet"]);
        git(&[
            "-c",
            "user.email=t@example.invalid",
            "-c",
            "user.name=t",
            "add",
            ".",
        ]);
        git(&[
            "-c",
            "user.email=t@example.invalid",
            "-c",
            "user.name=t",
            "commit",
            "--quiet",
            "-m",
            "start",
        ]);
        let data = tempfile::tempdir().unwrap();
        let target = AutoTarget {
            conversation_id: "conv".into(),
            root: root.to_path_buf(),
            workspace_id: Some("ws".into()),
            creates: false,
        };
        // Nothing changed and nothing shown yet: no check to show.
        assert_eq!(automatic_check(data.path(), &target, false), None);
        std::fs::write(
            root.join("src/New.tsx"),
            "<p className=\"text-[9px]\">x</p>\n",
        )
        .unwrap();
        std::fs::write(root.join("notes.txt"), "not a UI file\n").unwrap();
        let check = automatic_check(data.path(), &target, false).expect("a check");
        assert!(check.automatic && !check.by_agent);
        assert_eq!(check.target.kind, "changes");
        assert_eq!(check.report.files_checked, 1, "only the changed UI file");
        assert!(check
            .report
            .findings
            .iter()
            .all(|f| f.path == "src/New.tsx"));
        assert_eq!(check.conversation_id.as_deref(), Some("conv"));
    }
}
