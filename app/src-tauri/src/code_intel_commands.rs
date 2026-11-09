//! Code intelligence for the UI: the tokens output compression saved (a
//! Code conversation's, shown in its status line, and the running total, in
//! Activity), the per-conversation compression switch, and Shodh's bundled
//! Code skills with their switches (Settings → Code).
//!
//! The switches are the user's only: the agent reads the same numbers and
//! skills through `code_tokens_saved` and `code_skills` and never changes
//! them (see `agent-coverage.json`).

use std::path::PathBuf;

use serde::Serialize;
use shodh_rag::harness::code_skills::{self, Source};
use shodh_rag::harness::compress::savings::{
    ConversationSavings, SavingsStore, Tally, TotalSavings, ESTIMATE_METHOD,
};
use shodh_rag::harness::skills::parse_skill_md;
use tauri::AppHandle;

use crate::mcp::skills as skill_settings;
use crate::profile::app_data_dir;

type CommandResult<T> = Result<T, String>;

/// Longest conversation id accepted.
const MAX_ID_CHARS: usize = 200;

fn data_dir(app: &AppHandle) -> CommandResult<PathBuf> {
    app_data_dir(app).map_err(|e| e.to_string())
}

fn conversation(id: &str) -> CommandResult<String> {
    let id = id.trim();
    if id.is_empty() || id.chars().count() > MAX_ID_CHARS {
        return Err("That is not a conversation id.".into());
    }
    Ok(id.to_string())
}

/// Token counts as the UI shows them (always estimates).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TallyView {
    pub saved_tokens: u64,
    pub original_tokens: u64,
    pub sent_tokens: u64,
    pub commands: u64,
    pub shortened_commands: u64,
    /// The numbers are estimates ([`ESTIMATE_METHOD`]); the UI writes "≈".
    pub estimated: bool,
    pub method: &'static str,
}

impl From<&Tally> for TallyView {
    fn from(tally: &Tally) -> Self {
        TallyView {
            saved_tokens: tally.saved_tokens(),
            original_tokens: tally.original_tokens,
            sent_tokens: tally.sent_tokens,
            commands: tally.commands,
            shortened_commands: tally.shortened,
            estimated: true,
            method: ESTIMATE_METHOD,
        }
    }
}

/// One Code conversation's savings and switch.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSavingsView {
    pub conversation_id: String,
    #[serde(flatten)]
    pub tally: TallyView,
    pub compression_on: bool,
}

impl ConversationSavingsView {
    fn of(conversation_id: String, record: &ConversationSavings, on: bool) -> Self {
        ConversationSavingsView {
            conversation_id,
            tally: TallyView::from(&record.tally),
            compression_on: on,
        }
    }
}

/// The running total.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TotalSavingsView {
    #[serde(flatten)]
    pub tally: TallyView,
    /// Conversations ever counted (deleted ones too, as in the tally).
    pub conversations: u64,
    pub since: Option<String>,
    /// Compression in Code conversations that did not choose for themselves.
    pub default_on: bool,
}

impl From<TotalSavings> for TotalSavingsView {
    fn from(total: TotalSavings) -> Self {
        TotalSavingsView {
            tally: TallyView::from(&total.tally),
            conversations: total.conversations,
            since: total.since,
            default_on: total.default_on,
        }
    }
}

/// A bundled Code skill as Settings → Code shows it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CodeSkillView {
    pub name: &'static str,
    pub title: &'static str,
    pub summary: &'static str,
    /// When the agent loads it (its `SKILL.md` description).
    pub when: String,
    pub sources: &'static [Source],
    pub enabled: bool,
}

/// Every bundled Code skill with its switch.
pub fn code_skill_views(data_dir: &std::path::Path) -> Vec<CodeSkillView> {
    let settings = skill_settings::load_settings(data_dir);
    code_skills::BUNDLED
        .iter()
        .map(|skill| CodeSkillView {
            name: skill.name,
            title: skill.title,
            summary: skill.summary,
            when: parse_skill_md(skill.skill_md)
                .map(|doc| doc.description)
                .unwrap_or_else(|_| skill.summary.to_string()),
            sources: skill.sources,
            enabled: skill_settings::code_skill_on(&settings, skill.name),
        })
        .collect()
}

/// A Code conversation's tokens saved and whether compression is on there.
#[tauri::command]
pub async fn code_savings_get(
    app: AppHandle,
    conversation_id: String,
) -> CommandResult<ConversationSavingsView> {
    let id = conversation(&conversation_id)?;
    let store = SavingsStore::in_dir(&data_dir(&app)?);
    blocking(move || {
        let (record, on) = store.get(&id)?;
        Ok(ConversationSavingsView::of(id, &record, on))
    })
    .await
}

/// The tokens saved over every Code conversation (Activity), and the default
/// switch (Settings → Code).
#[tauri::command]
pub async fn code_savings_total(app: AppHandle) -> CommandResult<TotalSavingsView> {
    let store = SavingsStore::in_dir(&data_dir(&app)?);
    blocking(move || Ok(store.total()?.into())).await
}

/// Turn output compression on or off for one Code conversation. It applies
/// from the conversation's next command.
#[tauri::command]
pub async fn code_savings_set_enabled(
    app: AppHandle,
    conversation_id: String,
    enabled: bool,
) -> CommandResult<ConversationSavingsView> {
    let id = conversation(&conversation_id)?;
    let store = SavingsStore::in_dir(&data_dir(&app)?);
    blocking(move || {
        let record = store.set_compression(&id, enabled)?;
        tracing::info!(target: "shodh::harness", conversation = %id, on = enabled, "output compression switched");
        Ok(ConversationSavingsView::of(id, &record, enabled))
    })
    .await
}

/// Turn output compression on or off for Code conversations that did not
/// choose for themselves, new ones included (Settings → Code).
#[tauri::command]
pub async fn code_savings_set_default(
    app: AppHandle,
    enabled: bool,
) -> CommandResult<TotalSavingsView> {
    let store = SavingsStore::in_dir(&data_dir(&app)?);
    blocking(move || {
        store.set_default(enabled)?;
        tracing::info!(target: "shodh::harness", on = enabled, "default output compression switched");
        Ok(store.total()?.into())
    })
    .await
}

/// Run file work off the async runtime.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> CommandResult<T> + Send + 'static,
) -> CommandResult<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| e.to_string())?
}

/// The bundled Code skills (Settings → Code).
#[tauri::command]
pub async fn code_skills_list(app: AppHandle) -> CommandResult<Vec<CodeSkillView>> {
    Ok(code_skill_views(&data_dir(&app)?))
}

/// Turn a bundled Code skill on or off. Sessions started from now on get the
/// change (a running session keeps the skills it started with until it
/// restarts, as with installed skills).
#[tauri::command]
pub async fn code_skills_set_enabled(
    app: AppHandle,
    name: String,
    enabled: bool,
) -> CommandResult<Vec<CodeSkillView>> {
    let dir = data_dir(&app)?;
    skill_settings::set_code_skill(&dir, &name, enabled)?;
    Ok(code_skill_views(&dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversation_ids_are_checked() {
        assert_eq!(conversation(" c1 ").unwrap(), "c1");
        assert!(conversation("").is_err());
        assert!(conversation(&"x".repeat(MAX_ID_CHARS + 1)).is_err());
    }

    #[test]
    fn views_are_estimates_in_camel_case() {
        let dir = tempfile::tempdir().unwrap();
        let store = SavingsStore::in_dir(dir.path());
        let record = store.record("c1", 1_000, 250).unwrap();
        let view = ConversationSavingsView::of("c1".into(), &record, true);
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["savedTokens"], 750);
        assert_eq!(json["shortenedCommands"], 1);
        assert_eq!(json["compressionOn"], true);
        assert_eq!(json["estimated"], true);
        assert_eq!(json["conversationId"], "c1");
        let total = TotalSavingsView::from(store.total().unwrap());
        let json = serde_json::to_value(&total).unwrap();
        assert_eq!(json["savedTokens"], 750);
        assert_eq!(json["conversations"], 1);
        assert_eq!(json["defaultOn"], true);
    }

    #[test]
    fn skill_views_follow_the_switches() {
        let dir = tempfile::tempdir().unwrap();
        let views = code_skill_views(dir.path());
        assert_eq!(views.len(), code_skills::BUNDLED.len());
        assert!(views
            .iter()
            .all(|v| v.enabled && v.when.starts_with("Use ")));
        skill_settings::set_code_skill(dir.path(), "fresh-eyes-review", false).unwrap();
        let views = code_skill_views(dir.path());
        let off: Vec<&str> = views
            .iter()
            .filter(|v| !v.enabled)
            .map(|v| v.name)
            .collect();
        assert_eq!(off, ["fresh-eyes-review"]);
        let json = serde_json::to_value(&views[0]).unwrap();
        assert_eq!(json["sources"][0]["project"], "obra/superpowers");
        assert!(json["sources"][0].get("licenseText").is_none());
    }

    /// The repository's NOTICE credits every project the bundled skills and
    /// output compression come from, with the pinned commit or tag, and says
    /// rtk's BUSL engine is not used. (Read here, not in shodh-rag, whose
    /// files the retrieval evals must not depend on.)
    #[test]
    fn the_notice_credits_every_source() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let notice = std::fs::read_to_string(root.join("NOTICE")).unwrap();
        for source in code_skills::BUNDLED.iter().flat_map(|s| s.sources) {
            assert!(notice.contains(source.url), "NOTICE lacks {}", source.url);
            assert!(
                notice.contains(source.commit),
                "NOTICE lacks {}",
                source.commit
            );
        }
        for skill in &code_skills::BUNDLED {
            let path = format!("skills/{}/SKILL.md", skill.name);
            assert!(notice.contains(&path), "NOTICE lacks {path}");
            assert!(
                root.join("crates/shodh-rag").join(&path).is_file(),
                "{path} is not where NOTICE says"
            );
        }
        for rtk in [
            "https://github.com/rtk-ai/rtk, tag v1.1.0",
            "crates/shodh-rag/src/harness/compress/mod.rs",
            "Business Source License 1.1",
        ] {
            assert!(notice.contains(rtk), "NOTICE lacks {rtk:?}");
        }
        // The code map's parser and grammars, at the versions Cargo.lock builds.
        let lock = std::fs::read_to_string(root.join("Cargo.lock")).unwrap();
        for name in [
            "tree-sitter",
            "tree-sitter-language",
            "tree-sitter-rust",
            "tree-sitter-typescript",
            "tree-sitter-javascript",
            "tree-sitter-python",
            "tree-sitter-go",
            "tree-sitter-html",
            "tree-sitter-css",
            "streaming-iterator",
            "ignore",
            "globset",
            "bstr",
        ] {
            let entry = format!("name = \"{name}\"\nversion = \"");
            let at = lock
                .find(&entry)
                .unwrap_or_else(|| panic!("Cargo.lock has no {name}"));
            let version = lock[at + entry.len()..].split('"').next().unwrap();
            let credited = format!("{name} {version}");
            assert!(notice.contains(&credited), "NOTICE lacks {credited}");
        }
        assert!(notice.contains("Permission is hereby granted, free of charge"));
        // tree-sitter's runtime carries ICU code, whose notice goes with it.
        assert!(notice.contains("COPYRIGHT AND PERMISSION NOTICE (ICU 58 and later)"));
        assert!(notice.contains("Copyright © 1991-2019 Unicode, Inc."));
        for license in [
            "superpowers-MIT.txt",
            "ECC-MIT.txt",
            "elephant-goldfish-model-Apache-2.0.txt",
            "impeccable-Apache-2.0.txt",
        ] {
            assert!(
                root.join("crates/shodh-rag/skills/licenses")
                    .join(license)
                    .is_file(),
                "{license} is missing"
            );
        }
    }
}
