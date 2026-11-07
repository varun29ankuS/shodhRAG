//! The full output of a Code session's recent commands, for the model:
//! `read_command_output` reads lines of one, or searches it.
//!
//! What the model gets back from `run_command` may be shortened (output
//! compression, [`super::compress`]) or cut in the middle (the result's size
//! limit, [`super::compress::cut_keeping_failures`]); every such result names
//! its command id and this tool. The outputs are kept in memory only, per
//! conversation ([`remember`]): the last [`KEPT_PER_CONVERSATION`] commands,
//! and at most [`KEPT_BYTES`] over all conversations, oldest first out. A
//! session can read only its own conversation's outputs.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use serde_json::{json, Value};

use super::events::RiskTier;
use super::protocol::ToolLoadMode;
use super::tools::{req_str, HostTool, ToolContext, ToolError, ToolOutput};

pub const READ_COMMAND_OUTPUT: &str = "read_command_output";

/// Commands kept per conversation.
pub const KEPT_PER_CONVERSATION: usize = 20;
/// Bytes of output kept over every conversation.
pub const KEPT_BYTES: usize = 64 * 1024 * 1024;
/// Lines one call returns at most.
pub const MAX_LINES: usize = 1_000;
/// Characters one call returns at most.
pub const MAX_CHARS: usize = 40_000;
/// Search matches one call returns at most.
pub const MAX_MATCHES: usize = 300;

#[derive(Debug, Clone)]
struct Kept {
    conversation: String,
    command_id: String,
    command: String,
    output: Arc<str>,
}

#[derive(Default)]
struct Store {
    /// Oldest first.
    entries: VecDeque<Kept>,
    bytes: usize,
}

fn store() -> std::sync::MutexGuard<'static, Store> {
    static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
    STORE
        .get_or_init(|| Mutex::new(Store::default()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Keep the full `output` of `command` (`command_id`) for `conversation`.
pub fn remember(conversation: &str, command_id: &str, command: &str, output: &str) {
    let mut store = store();
    let size = output.len();
    store.entries.push_back(Kept {
        conversation: conversation.to_string(),
        command_id: command_id.to_string(),
        command: command.to_string(),
        output: Arc::from(output),
    });
    store.bytes += size;
    // This conversation's oldest beyond its share.
    let mine = store
        .entries
        .iter()
        .filter(|e| e.conversation == conversation)
        .count();
    if mine > KEPT_PER_CONVERSATION {
        if let Some(at) = store
            .entries
            .iter()
            .position(|e| e.conversation == conversation)
        {
            if let Some(gone) = store.entries.remove(at) {
                store.bytes -= gone.output.len();
            }
        }
    }
    while store.bytes > KEPT_BYTES && store.entries.len() > 1 {
        if let Some(gone) = store.entries.pop_front() {
            store.bytes -= gone.output.len();
        }
    }
}

/// Forget a conversation's outputs (it was deleted).
pub fn forget(conversation: &str) {
    let mut store = store();
    store.entries.retain(|e| e.conversation != conversation);
    store.bytes = store.entries.iter().map(|e| e.output.len()).sum();
}

/// The command and full output of `command_id` in `conversation`.
fn find(conversation: &str, command_id: &str) -> Option<(String, Arc<str>)> {
    store()
        .entries
        .iter()
        .rev()
        .find(|e| e.conversation == conversation && e.command_id == command_id)
        .map(|e| (e.command.clone(), e.output.clone()))
}

/// The ids of `conversation`'s kept commands, newest first.
fn kept_ids(conversation: &str) -> Vec<(String, String)> {
    store()
        .entries
        .iter()
        .rev()
        .filter(|e| e.conversation == conversation)
        .map(|e| (e.command_id.clone(), e.command.clone()))
        .collect()
}

/// `read_command_output`, bound to one conversation.
pub struct ReadCommandOutputTool {
    conversation: String,
}

impl ReadCommandOutputTool {
    pub fn new(conversation: impl Into<String>) -> Self {
        Self {
            conversation: conversation.into(),
        }
    }
}

fn whole_number(args: &Value, key: &str) -> Option<usize> {
    args.get(key)
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
}

/// Lines `from..from+count` (1-based) of `output`, numbered, within
/// [`MAX_CHARS`]. Returns the text and the last line shown.
fn numbered(lines: &[&str], from: usize, count: usize) -> (String, usize) {
    let mut text = String::new();
    let mut last = from.saturating_sub(1);
    for (i, line) in lines
        .iter()
        .enumerate()
        .skip(from.saturating_sub(1))
        .take(count)
    {
        let row = format!("{:>6}| {line}\n", i + 1);
        if text.len() + row.len() > MAX_CHARS && !text.is_empty() {
            break;
        }
        text.push_str(&row);
        last = i + 1;
    }
    (text, last)
}

#[async_trait]
impl HostTool for ReadCommandOutputTool {
    fn name(&self) -> &str {
        READ_COMMAND_OUTPUT
    }
    fn label(&self) -> &str {
        "Read command output"
    }
    fn label_template(&self) -> &str {
        "Reading the output of command {command_id}"
    }
    fn description(&self) -> &str {
        "Read the full, unshortened output of a command this session ran with run_command, by \
         the command id its result names (a shortened or cut result says so and gives the \
         id). With search, return every line containing that text (any case), numbered, \
         instead; otherwise return the lines from from_line (1 first, default 1), at most \
         line_count (default and most 1000). Without command_id, list the commands whose \
         output is kept. Outputs are kept in memory for this conversation's last 20 commands \
         and are lost when Shodh restarts. This only reads."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command_id": {"type": "string", "minLength": 1, "maxLength": 64},
                "search": {"type": "string", "minLength": 1, "maxLength": 200},
                "from_line": {"type": "integer", "minimum": 1},
                "line_count": {"type": "integer", "minimum": 1, "maximum": MAX_LINES}
            },
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }
    fn load_mode(&self) -> ToolLoadMode {
        ToolLoadMode::Essential
    }
    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        if args.get("command_id").and_then(Value::as_str).is_none() {
            let ids = kept_ids(&self.conversation);
            let mut text = format!(
                "{} command output{} kept for this conversation (newest first):\n",
                ids.len(),
                if ids.len() == 1 { " is" } else { "s are" }
            );
            for (id, command) in &ids {
                text.push_str(&format!(
                    "- {id}: {}\n",
                    super::truncate_chars(command, 120)
                ));
            }
            return Ok(ToolOutput {
                summary_for_ui: format!("{} kept outputs", ids.len()),
                text_for_model: text,
                detail: None,
            });
        }
        let id = req_str(&args, "command_id", READ_COMMAND_OUTPUT)?
            .trim()
            .to_string();
        let Some((command, output)) = find(&self.conversation, &id) else {
            return Err(ToolError::NotFound(format!(
                "No output of command {id} is kept for this conversation (only the last \
                 {KEPT_PER_CONVERSATION} commands are, until Shodh restarts). Call \
                 {READ_COMMAND_OUTPUT} without command_id to list them, or run the command again."
            )));
        };
        let lines: Vec<&str> = output.lines().collect();
        let total = lines.len();
        if let Some(search) = args.get("search").and_then(Value::as_str) {
            let needle = search.to_lowercase();
            let mut text = String::new();
            let mut matches = 0;
            let mut shown = 0;
            for (i, line) in lines.iter().enumerate() {
                if !line.to_lowercase().contains(&needle) {
                    continue;
                }
                matches += 1;
                let row = format!("{:>6}| {line}\n", i + 1);
                if shown < MAX_MATCHES && text.len() + row.len() <= MAX_CHARS {
                    text.push_str(&row);
                    shown += 1;
                }
            }
            let mut head = format!(
                "Output of `{}` (command {id}, {total} lines): {matches} {} \"{search}\"",
                super::truncate_chars(&command, 120),
                if matches == 1 {
                    "line contains"
                } else {
                    "lines contain"
                }
            );
            if shown < matches {
                head.push_str(&format!(
                    "; the first {shown} are shown (read around one with from_line)"
                ));
            }
            head.push_str(".\n");
            return Ok(ToolOutput {
                summary_for_ui: format!("{matches} matching lines"),
                text_for_model: head + &text,
                detail: Some(json!({ "commandId": id, "matches": matches })),
            });
        }
        let from = whole_number(&args, "from_line").unwrap_or(1).max(1);
        let count = whole_number(&args, "line_count")
            .unwrap_or(MAX_LINES)
            .clamp(1, MAX_LINES);
        if total == 0 {
            return Ok(ToolOutput {
                summary_for_ui: "No output".into(),
                text_for_model: format!("Command {id} printed nothing."),
                detail: None,
            });
        }
        if from > total {
            return Err(ToolError::InvalidArguments {
                tool: READ_COMMAND_OUTPUT.to_string(),
                reasons: format!("from_line {from} is past the output's last line, {total}"),
            });
        }
        let (body, last) = numbered(&lines, from, count);
        let mut text = format!(
            "Output of `{}` (command {id}): lines {from}-{last} of {total}.\n",
            super::truncate_chars(&command, 120)
        );
        text.push_str(&body);
        if last < total {
            text.push_str(&format!(
                "[More: call again with from_line {}.]\n",
                last + 1
            ));
        }
        Ok(ToolOutput {
            summary_for_ui: format!("Lines {from}-{last} of {total}"),
            text_for_model: text,
            detail: Some(json!({ "commandId": id, "from": from, "to": last, "total": total })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> ToolContext {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        ToolContext::new("r", "s", tx)
    }

    #[tokio::test]
    async fn a_session_reads_and_searches_only_its_own_outputs() {
        let mut output = String::new();
        for i in 1..=50 {
            output.push_str(&format!("test case_{i} ... ok\n"));
        }
        output.push_str("test case_51 ... FAILED\n");
        remember("cov-a", "c1", "cargo test", &output);
        remember("cov-b", "c2", "ls", "secret.txt\n");
        let tool = ReadCommandOutputTool::new("cov-a");
        assert_eq!(tool.tier(), RiskTier::Read);

        let found = tool
            .execute(json!({"command_id": "c1", "search": "failed"}), &ctx())
            .await
            .unwrap();
        assert!(
            found.text_for_model.contains("1 line contains \"failed\""),
            "{}",
            found.text_for_model
        );
        assert!(found
            .text_for_model
            .contains("    51| test case_51 ... FAILED"));

        let page = tool
            .execute(
                json!({"command_id": "c1", "from_line": 49, "line_count": 2}),
                &ctx(),
            )
            .await
            .unwrap();
        assert!(page.text_for_model.contains("lines 49-50 of 51"));
        assert!(page
            .text_for_model
            .contains("[More: call again with from_line 51.]"));

        // Another conversation's output is not reachable.
        assert!(matches!(
            tool.execute(json!({"command_id": "c2"}), &ctx()).await,
            Err(ToolError::NotFound(_))
        ));
        let list = tool.execute(json!({}), &ctx()).await.unwrap();
        assert!(list.text_for_model.contains("- c1: cargo test"));
        assert!(!list.text_for_model.contains("c2"));
        forget("cov-a");
        assert!(tool
            .execute(json!({"command_id": "c1"}), &ctx())
            .await
            .is_err());
    }

    #[test]
    fn a_conversation_keeps_its_last_commands() {
        for i in 0..(KEPT_PER_CONVERSATION + 5) {
            remember("cov-keep", &format!("k{i}"), "echo", "x\n");
        }
        let ids = kept_ids("cov-keep");
        assert_eq!(ids.len(), KEPT_PER_CONVERSATION);
        assert_eq!(ids[0].0, format!("k{}", KEPT_PER_CONVERSATION + 4));
        assert!(find("cov-keep", "k0").is_none());
        forget("cov-keep");
    }
}
