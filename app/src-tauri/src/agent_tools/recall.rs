//! Recall's tools, for the assistant (read only):
//! - `recall_search`: the user's own AI coding sessions (Claude Code, Codex) that match,
//!   as pointers with receipts;
//! - `recall_scope`: every session in a period, complete and unranked;
//! - `recall_open`: the exact turn a receipt points at, with the turns around it, read
//!   again from the transcript; shown to the user in the Library's session viewer.
//!
//! Everything they return was redacted when it was read (keys, tokens, passwords,
//! private keys and the user's own patterns), and the user's home folder is written as
//! `~`. Recall is the user's to turn on, and to choose the tools and projects it reads
//! (`agent-coverage.json`): these tools only read what the user chose.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use shodh_rag::harness::profile::app_tools;
use shodh_rag::harness::tools::{
    HostTool, RegistryError, ToolContext, ToolError, ToolOutput, ToolRegistry,
};
use shodh_rag::harness::RiskTier;
use shodh_rag::recall::query::{Opened, ScopeAnswer, SearchAnswer, MAX_AROUND, MAX_SESSIONS};
use shodh_rag::recall::{hide_home, home_folder, Role};

use super::{invalid, show_in_library, str_arg, AgentHost};
use crate::recall_commands::{self, RecallState, ScopeRequest, SearchRequest, OFF};

/// Longest query accepted.
const MAX_QUERY_CHARS: usize = 500;
/// Sessions a scope answer lists to the model at most (the rest are counted).
const MAX_SCOPE_LINES: usize = 150;

pub(super) fn register(
    registry: &mut ToolRegistry,
    host: &Arc<AgentHost>,
) -> Result<(), RegistryError> {
    registry.register(Arc::new(SearchTool {
        recall: host.recall.clone(),
    }))?;
    registry.register(Arc::new(ScopeTool {
        recall: host.recall.clone(),
    }))?;
    registry.register(Arc::new(OpenTool {
        recall: host.recall.clone(),
    }))
}

fn private(text: &str) -> String {
    match home_folder() {
        Some(home) => hide_home(text, &home),
        None => text.to_string(),
    }
}

fn strings(args: &Value, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn unavailable(e: String) -> ToolError {
    if e == OFF {
        ToolError::Unavailable(e)
    } else {
        ToolError::Failed(e)
    }
}

fn day(at: Option<&str>) -> String {
    at.and_then(|t| t.get(..10))
        .unwrap_or("an unknown day")
        .to_string()
}

fn window_schema() -> serde_json::Map<String, Value> {
    let mut properties = serde_json::Map::new();
    properties.insert(
        "period".into(),
        json!({"type": "string", "maxLength": 40, "description": "today, yesterday, this week, last week, this month, last month, last N days, or a date YYYY-MM-DD"}),
    );
    properties.insert(
        "since".into(),
        json!({"type": "string", "maxLength": 40, "description": "Start: a date YYYY-MM-DD or an RFC 3339 time"}),
    );
    properties.insert(
        "until".into(),
        json!({"type": "string", "maxLength": 40, "description": "End: a date YYYY-MM-DD (the whole day) or an RFC 3339 time"}),
    );
    properties.insert(
        "tools".into(),
        json!({"type": "array", "items": {"type": "string", "enum": ["claude_code", "codex"]}, "maxItems": 2}),
    );
    properties.insert(
        "projects".into(),
        json!({"type": "array", "items": {"type": "string", "minLength": 1, "maxLength": 120}, "maxItems": 10, "description": "Project names (a folder name, or part of it)"}),
    );
    properties
}

/// `recall_search`.
struct SearchTool {
    recall: RecallState,
}

/// The answer as the model reads it: sessions, each with its receipts.
pub(crate) fn search_text(answer: &SearchAnswer) -> String {
    if answer.sessions.is_empty() {
        return format!(
            "No coding session matched (searched {}). Try other words, a wider period, or recall_scope for a period.",
            answer.searched
        );
    }
    let mut out = format!(
        "{} coding session{} (of {} searched). Open a receipt with recall_open before saying what a session did.\n",
        answer.sessions.len(),
        if answer.sessions.len() == 1 { "" } else { "s" },
        answer.searched
    );
    for (n, s) in answer.sessions.iter().enumerate() {
        out.push_str(&format!(
            "{}. {} ({}, project {}, {}{}{})\n",
            n + 1,
            s.title,
            s.tool.label(),
            s.project,
            day(s.started.as_deref()),
            s.branch
                .as_deref()
                .map(|b| format!(", branch {b}"))
                .unwrap_or_default(),
            if s.exact.is_empty() {
                String::new()
            } else {
                format!(", holds {}", s.exact.join(", "))
            }
        ));
        for r in &s.receipts {
            out.push_str(&format!("   receipt {}: {}", r.id, r.heading));
            if !r.snippet.is_empty() {
                out.push_str(&format!(": {}", r.snippet.replace('\n', " ")));
            }
            out.push('\n');
        }
    }
    private(&out)
}

#[async_trait]
impl HostTool for SearchTool {
    fn name(&self) -> &str {
        app_tools::RECALL_SEARCH
    }
    fn label(&self) -> &str {
        "Search coding sessions"
    }
    fn label_template(&self) -> &str {
        "Searching your coding sessions"
    }
    fn description(&self) -> &str {
        "Search the user's own AI coding sessions (their Claude Code and Codex transcripts on \
         this computer, the tools and projects they chose in the Library) for what they did: a \
         bug, a fix, a decision, a PR, a file, an error. Returns sessions as pointers, each with \
         receipts (a receipt opens the exact turn); open one with recall_open before saying what \
         a session did. Filters: tools, projects, a period (\"last week\", \"2026-10-12\") or \
         since and until, and entity, an exact id the session must hold (\"#214\", a commit SHA, \
         a file path or its end, an error code). Secrets were redacted when the sessions were \
         read. For everything in a period, use recall_scope."
    }
    fn schema(&self) -> Value {
        let mut properties = window_schema();
        properties.insert(
            "query".into(),
            json!({"type": "string", "minLength": 1, "maxLength": MAX_QUERY_CHARS}),
        );
        properties.insert(
            "entity".into(),
            json!({"type": "string", "minLength": 1, "maxLength": 300}),
        );
        properties.insert(
            "limit".into(),
            json!({"type": "integer", "minimum": 1, "maximum": MAX_SESSIONS}),
        );
        json!({
            "type": "object",
            "properties": properties,
            "required": ["query"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }
    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let tool = app_tools::RECALL_SEARCH;
        let query = str_arg(&args, "query").ok_or_else(|| invalid(tool, "query is required"))?;
        if query.chars().count() > MAX_QUERY_CHARS {
            return Err(invalid(tool, "query is too long"));
        }
        let request = SearchRequest {
            query: query.to_string(),
            tools: strings(&args, "tools"),
            projects: strings(&args, "projects"),
            period: str_arg(&args, "period").map(str::to_string),
            since: str_arg(&args, "since").map(str::to_string),
            until: str_arg(&args, "until").map(str::to_string),
            entity: str_arg(&args, "entity").map(str::to_string),
            limit: args
                .get("limit")
                .and_then(Value::as_u64)
                .map(|n| (n as usize).clamp(1, MAX_SESSIONS)),
        };
        let answer = recall_commands::search(&self.recall, &request)
            .await
            .map_err(unavailable)?;
        ctx.mark_private();
        let summary = match answer.sessions.len() {
            0 => "No session matched".to_string(),
            1 => "1 session".to_string(),
            n => format!("{n} sessions"),
        };
        Ok(ToolOutput {
            text_for_model: search_text(&answer),
            summary_for_ui: summary,
            detail: serde_json::to_value(&answer).ok(),
        })
    }
}

/// `recall_scope`.
struct ScopeTool {
    recall: RecallState,
}

pub(crate) fn scope_text(answer: &ScopeAnswer) -> String {
    if answer.sessions.is_empty() {
        return "No coding session in that period.".to_string();
    }
    let total = answer.sessions.len();
    let mut out = format!(
        "{total} coding session{} in that period{}, oldest first (every one, not ranked):\n",
        if total == 1 { "" } else { "s" },
        if answer.more {
            " (more than Recall lists)"
        } else {
            ""
        }
    );
    for s in answer.sessions.iter().take(MAX_SCOPE_LINES) {
        out.push_str(&format!(
            "- {} to {}: {} ({}, project {}{}, {} turns{}{})",
            s.started.as_deref().unwrap_or("?"),
            s.ended.as_deref().unwrap_or("?"),
            s.title,
            s.tool.label(),
            s.project,
            s.branch
                .as_deref()
                .map(|b| format!(", branch {b}"))
                .unwrap_or_default(),
            s.turns,
            if s.threads > 0 {
                format!(
                    ", {} subagent thread{}",
                    s.threads,
                    if s.threads == 1 { "" } else { "s" }
                )
            } else {
                String::new()
            },
            if s.entities.is_empty() {
                String::new()
            } else {
                format!(", ids {}", s.entities.join(", "))
            }
        ));
        if let Some(first) = &s.first {
            out.push_str(&format!(", receipt {first}"));
        }
        out.push('\n');
    }
    if total > MAX_SCOPE_LINES {
        out.push_str(&format!(
            "… and {} more; narrow the period or the projects to list them.\n",
            total - MAX_SCOPE_LINES
        ));
    }
    private(&out)
}

#[async_trait]
impl HostTool for ScopeTool {
    fn name(&self) -> &str {
        app_tools::RECALL_SCOPE
    }
    fn label(&self) -> &str {
        "List coding sessions"
    }
    fn label_template(&self) -> &str {
        "Listing your coding sessions"
    }
    fn description(&self) -> &str {
        "List every one of the user's AI coding sessions in a period, complete and unranked, \
         oldest first: for \"what did I do last week\" and summaries of a period. Each comes \
         with its project, tool, times, branch, a few ids it holds (PRs, branches, files) and a \
         receipt of its first turn in the period for recall_open. Give period, or since and \
         until; tools and projects narrow it."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": window_schema(),
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }
    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let tool = app_tools::RECALL_SCOPE;
        let request = ScopeRequest {
            period: str_arg(&args, "period").map(str::to_string),
            since: str_arg(&args, "since").map(str::to_string),
            until: str_arg(&args, "until").map(str::to_string),
            tools: strings(&args, "tools"),
            projects: strings(&args, "projects"),
        };
        if request.period.is_none() && request.since.is_none() && request.until.is_none() {
            return Err(invalid(tool, "give period, or since and until"));
        }
        let answer = recall_commands::scope(&self.recall, &request)
            .await
            .map_err(|e| {
                if e.contains("is not a period") || e.contains("must be a date") {
                    invalid(tool, e)
                } else {
                    unavailable(e)
                }
            })?;
        ctx.mark_private();
        Ok(ToolOutput {
            text_for_model: scope_text(&answer),
            summary_for_ui: match answer.sessions.len() {
                1 => "1 session".to_string(),
                n => format!("{n} sessions"),
            },
            detail: serde_json::to_value(&answer).ok(),
        })
    }
}

/// `recall_open`.
struct OpenTool {
    recall: RecallState,
}

pub(crate) fn open_text(opened: &Opened) -> String {
    let mut out = format!(
        "{} ({}, project {}{}), turn {} of {}. Receipt {}:{}.\n",
        opened.title,
        opened.tool.label(),
        opened.project,
        opened
            .branch
            .as_deref()
            .map(|b| format!(", branch {b}"))
            .unwrap_or_default(),
        opened.turn + 1,
        opened.turns_total,
        opened.file_id,
        opened.turn
    );
    for t in &opened.turns {
        let marker = if t.target {
            " (the receipt's turn)"
        } else {
            ""
        };
        out.push_str(&format!(
            "\n[{}{}{}]\n",
            t.heading,
            t.at.as_deref()
                .map(|a| format!(", {a}"))
                .unwrap_or_default(),
            marker
        ));
        match &t.kept_back {
            Some(reason) => out.push_str(reason),
            None => out.push_str(&t.text),
        }
        if t.left_out > 0 && t.role == Role::ToolOutput {
            out.push_str(&format!(
                "\n({} characters of the output are not shown)",
                t.left_out
            ));
        }
        out.push('\n');
    }
    if opened.unreadable_lines > 0 {
        out.push_str(&format!(
            "\n{} line{} of this transcript could not be read.\n",
            opened.unreadable_lines,
            if opened.unreadable_lines == 1 {
                ""
            } else {
                "s"
            }
        ));
    }
    private(&out)
}

#[async_trait]
impl HostTool for OpenTool {
    fn name(&self) -> &str {
        app_tools::RECALL_OPEN
    }
    fn label(&self) -> &str {
        "Open a coding session"
    }
    fn label_template(&self) -> &str {
        "Opening a coding session"
    }
    fn description(&self) -> &str {
        "Open a receipt from recall_search or recall_scope (like \"12:40\"): the exact turn of \
         the coding session and the turns around it (around, default 2), read again from the \
         transcript with secrets redacted. The user sees the session open at that turn in the \
         Library. Say what the session did from what this returns."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "receipt": {"type": "string", "pattern": "^[0-9]{1,12}:[0-9]{1,7}$"},
                "around": {"type": "integer", "minimum": 0, "maximum": MAX_AROUND}
            },
            "required": ["receipt"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }
    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let tool = app_tools::RECALL_OPEN;
        let receipt =
            str_arg(&args, "receipt").ok_or_else(|| invalid(tool, "receipt is required"))?;
        if shodh_rag::recall::query::parse_receipt(receipt).is_none() {
            return Err(invalid(tool, "a receipt looks like 12:40"));
        }
        let around = args
            .get("around")
            .and_then(Value::as_u64)
            .map_or(shodh_rag::recall::query::DEFAULT_AROUND, |n| {
                (n as u32).min(MAX_AROUND)
            });
        let opened = recall_commands::open(&self.recall, receipt, around)
            .await
            .map_err(unavailable)?;
        ctx.mark_private();
        show_in_library(ctx, "recall", Some(receipt.to_string()), &opened.title);
        Ok(ToolOutput {
            text_for_model: open_text(&opened),
            summary_for_ui: format!("Turn {} of {}", opened.turn + 1, opened.title),
            detail: serde_json::to_value(&opened).ok(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing;
    use super::*;
    use shodh_rag::harness::events::NavigationTarget;
    use shodh_rag::recall::catalog::Catalog;
    use shodh_rag::recall::discover::{project_key, Roots};
    use shodh_rag::recall::index::{RecallIndex, SyncControl, SyncProgress};
    use shodh_rag::recall::settings::RecallSettings;
    use shodh_rag::recall::Tool;

    const SESSION: &str = "3c2b1a09-8f7e-4d6c-9b5a-4e3d2c1b0a99";

    fn line(kind: &str, content: Value, at: &str) -> String {
        json!({"type": kind, "message": {"role": kind, "content": content}, "timestamp": at,
            "cwd": "/home/dev/demo", "sessionId": SESSION, "gitBranch": "fix/demo-crash"})
        .to_string()
    }

    fn no() -> bool {
        false
    }

    fn quiet(_: &SyncProgress) {}

    /// Recall turned on over one indexed session.
    async fn recall_with_a_session(t: &testing::TestHost) -> RecallState {
        let root = t.dir.path().join("claude");
        let file = root.join("-home-dev-demo").join(format!("{SESSION}.jsonl"));
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let lines = [
            line(
                "user",
                json!("The demo app crashes when the config file is empty."),
                "2026-10-01T10:00:00Z",
            ),
            line(
                "assistant",
                json!([{"type": "text", "text": "The parser unwraps an empty document; it now returns the default config."}]),
                "2026-10-01T10:01:00Z",
            ),
        ];
        std::fs::write(&file, lines.join("\n") + "\n").unwrap();
        let engine = testing::engine_with_search_models(&t.dir.path().join("recall-engine")).await;
        let catalog = Catalog::open(&t.dir.path().join("recall").join("recall.db")).unwrap();
        let index = Arc::new(RecallIndex::with_parts(
            &t.dir.path().join("recall"),
            Arc::new(tokio::sync::RwLock::new(engine)),
            Arc::new(catalog),
        ));
        let mut settings = RecallSettings {
            enabled: true,
            ..RecallSettings::default()
        };
        settings.tools.insert(Tool::ClaudeCode);
        settings
            .projects
            .insert(project_key("/home/dev/demo"), true);
        let roots = Roots {
            claude_code: Some(root),
            codex: None,
        };
        let control = SyncControl {
            cancelled: &no,
            paused: &no,
            progress: &quiet,
        };
        let report = index
            .sync(
                &settings,
                &roots,
                shodh_rag::recall::index::now_secs() + 86_400,
                &control,
            )
            .await
            .unwrap();
        assert_eq!(report.indexed, 1, "{report:?}");
        RecallState::with_index(t.dir.path().join("recall"), index, settings, roots)
    }

    #[tokio::test]
    async fn the_tools_find_list_and_open_a_session_and_show_it() {
        let t = testing::host().await;
        let recall = recall_with_a_session(&t).await;
        let (ctx, mut rx) = testing::ctx();

        let found = SearchTool {
            recall: recall.clone(),
        }
        .execute(json!({"query": "config file empty crash"}), &ctx)
        .await
        .unwrap();
        assert!(
            found.text_for_model.contains("project demo"),
            "{}",
            found.text_for_model
        );
        assert!(
            found.text_for_model.contains("receipt "),
            "{}",
            found.text_for_model
        );
        assert!(ctx.private_content(), "session text arms the web gate");
        assert!(testing::no_navigation(&mut rx), "a search shows nothing");

        let listed = ScopeTool {
            recall: recall.clone(),
        }
        .execute(json!({"since": "2026-10-01", "until": "2026-10-01"}), &ctx)
        .await
        .unwrap();
        assert!(
            listed.text_for_model.starts_with("1 coding session"),
            "{}",
            listed.text_for_model
        );
        assert!(listed.text_for_model.contains("branch fix/demo-crash"));
        let receipt = listed
            .text_for_model
            .split("receipt ")
            .nth(1)
            .map(|r| r.trim().to_string())
            .unwrap();

        let opened = OpenTool {
            recall: recall.clone(),
        }
        .execute(json!({"receipt": receipt, "around": 1}), &ctx)
        .await
        .unwrap();
        assert!(
            opened.text_for_model.contains("The demo app crashes"),
            "{}",
            opened.text_for_model
        );
        assert!(opened.text_for_model.contains("(the receipt's turn)"));
        let shown = testing::expect_navigation(&mut rx, "recall_open");
        assert_eq!(shown.view, "library");
        assert_eq!(
            shown.target,
            Some(NavigationTarget::Library {
                section: "recall".into(),
                item: Some(receipt.clone()),
            })
        );

        let bad = OpenTool { recall }
            .execute(json!({"receipt": "nonsense"}), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(bad, ToolError::InvalidArguments { .. }));
    }

    #[tokio::test]
    async fn while_recall_is_off_the_tools_say_how_to_turn_it_on() {
        let t = testing::host().await;
        let (ctx, mut rx) = testing::ctx();
        let err = SearchTool {
            recall: t.host.recall.clone(),
        }
        .execute(json!({"query": "anything"}), &ctx)
        .await
        .unwrap_err();
        assert!(
            matches!(err, ToolError::Unavailable(ref m) if m == OFF),
            "{err:?}"
        );
        let err = ScopeTool {
            recall: t.host.recall.clone(),
        }
        .execute(json!({"period": "last week"}), &ctx)
        .await
        .unwrap_err();
        assert!(matches!(err, ToolError::Unavailable(_)), "{err:?}");
        assert!(!ctx.private_content());
        assert!(testing::no_navigation(&mut rx));
    }

    #[test]
    fn the_home_folder_never_reaches_the_model() {
        let Some(home) = home_folder() else { return };
        let text = private(&format!("read {home}/code/app/src/a.rs"));
        assert!(!text.contains(&home), "{text}");
        assert!(text.starts_with("read ~"), "{text}");
    }
}
