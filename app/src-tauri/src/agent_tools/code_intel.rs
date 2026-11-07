//! Code intelligence, for the agent (read only): `code_tokens_saved` reports
//! the tokens Code mode's output compression saved, for a conversation and
//! in total; `code_skills` lists Shodh's bundled Code skills (which are on,
//! what each does, where it came from) or describes one.
//!
//! Neither changes anything: turning compression or a skill on or off is the
//! user's, in the Code session's status line and in Settings → Code (see
//! `agent-coverage.json`). In a Code session both tools are bound to that
//! session's conversation ([`code_intel_tools`]); the Research assistant's
//! copies take a conversation id or report the total.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use shodh_rag::harness::code_skills::{self, BundledSkill};
use shodh_rag::harness::compress::savings::{SavingsStore, Tally, ESTIMATE_METHOD};
use shodh_rag::harness::profile::app_tools;
use shodh_rag::harness::skills::parse_skill_md;
use shodh_rag::harness::tools::{
    HostTool, RegistryError, ToolContext, ToolError, ToolOutput, ToolRegistry,
};
use shodh_rag::harness::RiskTier;

use super::{invalid, str_arg, AgentHost};
use crate::mcp::skills as skill_settings;

/// A Code session's own copies of these tools, bound to its conversation.
pub const CODE_INTEL_TOOL_NAMES: [&str; 2] = [app_tools::CODE_TOKENS_SAVED, app_tools::CODE_SKILLS];

/// Longest conversation id accepted.
const MAX_ID_CHARS: usize = 200;

/// The Research assistant's copies: a conversation by id, or the total.
pub(super) fn register(
    registry: &mut ToolRegistry,
    host: &Arc<AgentHost>,
) -> Result<(), RegistryError> {
    registry.register(Arc::new(TokensSavedTool {
        data_dir: host.data_dir.clone(),
        conversation: None,
    }))?;
    registry.register(Arc::new(CodeSkillsTool {
        data_dir: host.data_dir.clone(),
    }))
}

/// A Code session's copies ([`CODE_INTEL_TOOL_NAMES`]): the savings are this
/// conversation's.
pub fn code_intel_tools(data_dir: PathBuf, conversation_id: &str) -> Vec<Arc<dyn HostTool>> {
    vec![
        Arc::new(TokensSavedTool {
            data_dir: data_dir.clone(),
            conversation: Some(conversation_id.to_string()),
        }),
        Arc::new(CodeSkillsTool { data_dir }),
    ]
}

/// `41200` → `41,200`.
pub(crate) fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn tally_json(tally: &Tally) -> Value {
    json!({
        "savedTokens": tally.saved_tokens(),
        "originalTokens": tally.original_tokens,
        "sentTokens": tally.sent_tokens,
        "commands": tally.commands,
        "shortenedCommands": tally.shortened,
    })
}

fn tally_words(tally: &Tally) -> String {
    format!(
        "≈{} tokens saved: ≈{} of command output went to the model instead of ≈{}, over {} \
         command{} ({} shortened)",
        grouped(tally.saved_tokens()),
        grouped(tally.sent_tokens),
        grouped(tally.original_tokens),
        tally.commands,
        if tally.commands == 1 { "" } else { "s" },
        tally.shortened
    )
}

/// `code_tokens_saved`.
struct TokensSavedTool {
    data_dir: PathBuf,
    /// The Code session's conversation; `None` for the Research assistant.
    conversation: Option<String>,
}

#[async_trait]
impl HostTool for TokensSavedTool {
    fn name(&self) -> &str {
        app_tools::CODE_TOKENS_SAVED
    }
    fn label(&self) -> &str {
        "Tokens saved"
    }
    fn label_template(&self) -> &str {
        "Checking the tokens saved"
    }
    fn description(&self) -> &str {
        match self.conversation {
            Some(_) => {
                "Report how many tokens Shodh's output compression saved in this Code session: \
                 command output reaches you with progress lines and passing tests removed \
                 (errors and failures are kept whole), and the user sees the full output. \
                 Also reports the running total over every Code conversation and whether \
                 compression is on here. Numbers are estimates (about four characters a \
                 token); say so with ≈. This changes nothing; only the user turns \
                 compression on or off."
            }
            None => {
                "Report how many tokens Code mode's output compression saved: for one Code \
                 conversation (conversation_id) or, without one, the running total over every \
                 Code conversation. Numbers are estimates (about four characters a token); \
                 say so with ≈. This changes nothing; only the user turns compression on or \
                 off."
            }
        }
    }
    fn schema(&self) -> Value {
        match self.conversation {
            Some(_) => json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            None => json!({
                "type": "object",
                "properties": {
                    "conversation_id": {"type": "string", "minLength": 1, "maxLength": MAX_ID_CHARS}
                },
                "additionalProperties": false
            }),
        }
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }
    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let tool = app_tools::CODE_TOKENS_SAVED;
        let conversation = match &self.conversation {
            Some(id) => Some(id.clone()),
            None => str_arg(&args, "conversation_id").map(str::to_string),
        };
        if conversation
            .as_deref()
            .is_some_and(|id| id.chars().count() > MAX_ID_CHARS)
        {
            return Err(invalid(tool, "conversation_id is too long"));
        }
        let store = SavingsStore::in_dir(&self.data_dir);
        let (total, record) = tokio::task::spawn_blocking({
            let store = store.clone();
            let conversation = conversation.clone();
            move || {
                let total = store.total()?;
                let record = conversation
                    .as_deref()
                    .map(|id| store.get(id))
                    .transpose()?;
                Ok::<_, String>((total, record))
            }
        })
        .await
        .map_err(|e| ToolError::Failed(e.to_string()))?
        .map_err(ToolError::Unavailable)?;

        let mut text = String::new();
        let mut detail = json!({
            "estimated": true,
            "method": ESTIMATE_METHOD,
            "total": tally_json(&total.tally),
            "conversationsCounted": total.conversations,
            "since": total.since,
        });
        let summary = match (&conversation, &record) {
            (Some(id), Some(record)) => {
                let here = if self.conversation.is_some() {
                    "This Code session".to_string()
                } else {
                    format!("Conversation {id}")
                };
                text.push_str(&format!(
                    "{here}: {}. Output compression is {} here.\n",
                    tally_words(&record.tally),
                    if record.compression_off { "off" } else { "on" }
                ));
                detail["conversation"] = tally_json(&record.tally);
                detail["compressionOn"] = json!(!record.compression_off);
                format!("≈{} tokens saved", grouped(record.tally.saved_tokens()))
            }
            _ => format!(
                "≈{} tokens saved in total",
                grouped(total.tally.saved_tokens())
            ),
        };
        text.push_str(&format!(
            "All Code conversations{}: {}.\n{ESTIMATE_METHOD}",
            total
                .since
                .as_deref()
                .and_then(|s| s.get(..10))
                .map(|day| format!(" since {day}"))
                .unwrap_or_default(),
            tally_words(&total.tally)
        ));
        Ok(ToolOutput {
            text_for_model: text,
            summary_for_ui: summary,
            detail: Some(detail),
        })
    }
}

/// `code_skills`.
struct CodeSkillsTool {
    data_dir: PathBuf,
}

fn source_words(skill: &BundledSkill) -> String {
    skill
        .sources
        .iter()
        .map(|s| format!("{} ({}; {})", s.project, s.license, s.skills.join(", ")))
        .collect::<Vec<_>>()
        .join("; ")
}

#[async_trait]
impl HostTool for CodeSkillsTool {
    fn name(&self) -> &str {
        app_tools::CODE_SKILLS
    }
    fn label(&self) -> &str {
        "Code skills"
    }
    fn label_template(&self) -> &str {
        "Listing the Code skills"
    }
    fn description(&self) -> &str {
        "List Shodh's bundled Code mode skills (plan before building, test first, debug \
         systematically, verify before done, fresh-eyes review): whether each is on, what it \
         does and the projects it was written from. With name, describe one skill and return \
         its full instructions. This changes nothing; only the user turns skills on or off, \
         in Settings → Code."
    }
    fn schema(&self) -> Value {
        let names: Vec<&str> = code_skills::BUNDLED.iter().map(|s| s.name).collect();
        json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "enum": names}
            },
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }
    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let tool = app_tools::CODE_SKILLS;
        let data_dir = self.data_dir.clone();
        let settings =
            tokio::task::spawn_blocking(move || skill_settings::load_settings(&data_dir))
                .await
                .map_err(|e| ToolError::Failed(e.to_string()))?;
        if let Some(name) = str_arg(&args, "name") {
            let skill = code_skills::bundled(name)
                .ok_or_else(|| invalid(tool, format!("{name} is not a Code skill")))?;
            let on = settings.enabled(skill.name, None);
            let doc = parse_skill_md(skill.skill_md).map_err(ToolError::Failed)?;
            let text = format!(
                "Code skill \"{}\" ({}), {} in Settings → Code.\nWhat it does: {}\nWritten from: \
                 {}.\n\nIts instructions:\n\n{}",
                skill.name,
                skill.title,
                if on { "on" } else { "off" },
                skill.summary,
                source_words(skill),
                doc.body
            );
            return Ok(ToolOutput {
                text_for_model: text,
                summary_for_ui: format!("{} ({})", skill.title, if on { "on" } else { "off" }),
                detail: Some(json!({ "skill": skill.name, "enabled": on })),
            });
        }
        let mut text = String::from(
            "Shodh's Code skills, in the order they are used (the user turns each on or off in \
             Settings → Code):\n",
        );
        let mut on_count = 0;
        let mut rows = Vec::new();
        for skill in &code_skills::BUNDLED {
            let on = settings.enabled(skill.name, None);
            on_count += usize::from(on);
            text.push_str(&format!(
                "- {} ({}): {} {} Written from {}.\n",
                skill.name,
                if on { "on" } else { "off" },
                skill.title,
                skill.summary,
                source_words(skill)
            ));
            rows.push(json!({ "name": skill.name, "enabled": on }));
        }
        Ok(ToolOutput {
            text_for_model: text,
            summary_for_ui: format!(
                "{on_count} of {} Code skills on",
                code_skills::BUNDLED.len()
            ),
            detail: Some(json!({ "skills": rows })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_tools::testing;

    fn ctx() -> ToolContext {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        ToolContext::new("r", "s", tx)
    }

    #[test]
    fn numbers_are_grouped_by_thousands() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(41_200), "41,200");
        assert_eq!(grouped(1_234_567), "1,234,567");
    }

    #[tokio::test]
    async fn a_code_session_reports_its_own_savings_and_the_total() {
        let dir = tempfile::tempdir().unwrap();
        let store = SavingsStore::in_dir(dir.path());
        store.record("conv-1", 50_000, 8_800).unwrap();
        store.record("conv-2", 1_000, 1_000).unwrap();
        let tools = code_intel_tools(dir.path().to_path_buf(), "conv-1");
        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert_eq!(names, CODE_INTEL_TOOL_NAMES);
        for tool in &tools {
            assert_eq!(tool.tier(), RiskTier::Read);
        }
        let out = tools[0].execute(json!({}), &ctx()).await.unwrap();
        assert_eq!(out.summary_for_ui, "≈41,200 tokens saved");
        assert!(
            out.text_for_model
                .starts_with("This Code session: ≈41,200 tokens saved"),
            "{}",
            out.text_for_model
        );
        assert!(out
            .text_for_model
            .contains("Output compression is on here."));
        assert!(out.text_for_model.contains("over 2 commands"));
        assert!(out.text_for_model.contains(ESTIMATE_METHOD));
        assert_eq!(
            out.detail.as_ref().unwrap()["conversation"]["savedTokens"],
            41_200
        );
        assert_eq!(out.detail.as_ref().unwrap()["estimated"], true);
        // The session's schema takes no conversation: it reports its own.
        assert_eq!(tools[0].schema()["properties"], json!({}));
        store.set_compression("conv-1", false).unwrap();
        let out = tools[0].execute(json!({}), &ctx()).await.unwrap();
        assert!(out
            .text_for_model
            .contains("Output compression is off here."));
    }

    #[tokio::test]
    async fn the_assistant_reports_a_conversation_or_the_total() {
        let t = testing::host().await;
        let store = SavingsStore::in_dir(&t.host.data_dir);
        store.record("conv-9", 4_000, 1_000).unwrap();
        let registry = crate::agent_tools::build_registry(t.host.clone()).unwrap();
        let tool = registry.tool(app_tools::CODE_TOKENS_SAVED).unwrap();
        let total = tool.execute(json!({}), &ctx()).await.unwrap();
        assert_eq!(total.summary_for_ui, "≈3,000 tokens saved in total");
        let one = tool
            .execute(json!({"conversation_id": "conv-9"}), &ctx())
            .await
            .unwrap();
        assert!(one
            .text_for_model
            .starts_with("Conversation conv-9: ≈3,000 tokens saved"));
    }

    #[tokio::test]
    async fn skills_are_listed_and_described_with_their_switches() {
        let dir = tempfile::tempdir().unwrap();
        skill_settings::set_code_skill(dir.path(), "test-first", false).unwrap();
        let tools = code_intel_tools(dir.path().to_path_buf(), "conv-1");
        let list = tools[1].execute(json!({}), &ctx()).await.unwrap();
        assert_eq!(list.summary_for_ui, "4 of 5 Code skills on");
        assert!(list
            .text_for_model
            .contains("- test-first (off): Test first"));
        assert!(list.text_for_model.contains("- fresh-eyes-review (on)"));
        assert!(list
            .text_for_model
            .contains("google/elephant-goldfish-model (Apache-2.0"));
        let one = tools[1]
            .execute(json!({"name": "verify-before-done"}), &ctx())
            .await
            .unwrap();
        assert!(one.text_for_model.contains("on in Settings → Code"));
        assert!(one.text_for_model.contains("# Verify before done"));
        assert!(tools[1]
            .execute(json!({"name": "nope"}), &ctx())
            .await
            .is_err());
    }
}
