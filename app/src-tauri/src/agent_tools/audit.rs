//! `audit_query` (read): filter the audit log and return one-line summaries.
//!
//! The log never holds secrets: its payload builders take no API keys and
//! store document text only as short snippets the user was shown
//! (`shodh_rag::audit::payload`). This tool returns summaries built from
//! those payloads, never whole payloads, so even long questions or tool
//! arguments reach the model truncated.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use serde_json::{json, Value};
use shodh_rag::audit::{AuditEventType, AuditLog, AuditQuery, AuditRow};
use shodh_rag::harness::profile::app_tools;
use shodh_rag::harness::tools::{
    HostTool, RegistryError, ToolContext, ToolError, ToolOutput, ToolRegistry,
};
use shodh_rag::harness::RiskTier;
use shodh_rag::statements::Scope;

use super::{invalid, limit_arg, str_arg, AgentHost};

const DEFAULT_ROWS: usize = 20;
const MAX_ROWS: usize = 100;
const MAX_SUMMARY_CHARS: usize = 200;

pub(super) fn register(
    registry: &mut ToolRegistry,
    host: &Arc<AgentHost>,
) -> Result<(), RegistryError> {
    registry.register(Arc::new(AuditQueryTool { host: host.clone() }))
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Parse a bound: RFC 3339, or a date (start of day for `from`, end of day
/// for `to`, in UTC).
fn bound(raw: &str, end_of_day: bool) -> Option<DateTime<Utc>> {
    if let Ok(ts) = DateTime::parse_from_rfc3339(raw) {
        return Some(ts.with_timezone(&Utc));
    }
    let date = NaiveDate::parse_from_str(raw, "%Y-%m-%d").ok()?;
    let time = if end_of_day {
        date.and_hms_opt(23, 59, 59)
    } else {
        date.and_hms_opt(0, 0, 0)
    }?;
    Utc.from_local_datetime(&time).single()
}

/// The memory tools, whose approval labels may name a memory.
const MEMORY_TOOLS: [&str; 3] = [
    app_tools::REMEMBER,
    app_tools::UPDATE_MEMORY,
    app_tools::FORGET,
];

/// Events about memories: shown only when the memory's scope is visible to the run.
const MEMORY_EVENTS: [AuditEventType; 3] = [
    AuditEventType::MemoryWrite,
    AuditEventType::MemoryForget,
    AuditEventType::MemoryUse,
];

/// Most memory events read (newest first) to filter by scope.
const MEMORY_SCAN: u32 = 1_000;

fn s<'a>(payload: &'a Value, key: &str) -> &'a str {
    payload.get(key).and_then(Value::as_str).unwrap_or("")
}

/// The memory an event is about: its `id`, else its `ids` joined.
fn memory_ids(payload: &Value) -> String {
    match payload.get("id").and_then(Value::as_str) {
        Some(id) => id.to_string(),
        None => payload
            .get("ids")
            .and_then(Value::as_array)
            .map(|ids| {
                ids.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default(),
    }
}

/// ` (<class>)` when an event names a class, nothing otherwise.
fn of_class(payload: &Value) -> String {
    match payload.get("class").and_then(Value::as_str) {
        Some(class) if !class.is_empty() => format!(" ({class})"),
        _ => String::new(),
    }
}

/// One line describing an audit event.
pub(crate) fn summarize(row: &AuditRow) -> String {
    let p = &row.payload;
    let text = match row.event_type.as_str() {
        "question" => format!("Asked: {}", s(p, "text")),
        "tool_call" => format!(
            "{} {}: {}",
            s(p, "tool"),
            if p.get("ok").and_then(Value::as_bool).unwrap_or(false) {
                "ran"
            } else {
                "failed"
            },
            s(p, "summary")
        ),
        // Labels of the memory tools' approvals named the memory before they stopped doing
        // so: those are described by their tool only.
        "approval" if MEMORY_TOOLS.contains(&s(p, "tool")) => format!(
            "Approval for {} — {} ({})",
            s(p, "tool"),
            s(p, "decision"),
            s(p, "who")
        ),
        "approval" => format!("{} — {} ({})", s(p, "label"), s(p, "decision"), s(p, "who")),
        "retrieval" => {
            let passages = p
                .get("passages")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or(0);
            match p.get("query").and_then(Value::as_str) {
                Some(q) => format!("{} retrieved {passages} passages for \"{q}\"", s(p, "tool")),
                None => format!("{} read {}", s(p, "tool"), s(p, "path")),
            }
        }
        "answer" => {
            let chars = p.get("answer_chars").and_then(Value::as_u64).unwrap_or(0);
            let steps = p.get("tool_steps").and_then(Value::as_u64).unwrap_or(0);
            format!(
                "Answer {} with {} ({chars} characters, {steps} tool steps)",
                s(p, "status"),
                s(p, "model")
            )
        }
        "source_change" => format!("{} {} via {}", s(p, "action"), s(p, "path"), s(p, "via")),
        "settings_change" => {
            let action = s(p, "action");
            match p.get("key").and_then(Value::as_str) {
                Some(key) => format!("{action} {key}"),
                None => action.to_string(),
            }
        }
        "runtime_install" => format!("Agent runtime {} installed", s(p, "version")),
        "retention_checkpoint" => "Older events removed by retention".to_string(),
        // Memory events are described by action, class and ids, never by their words:
        // events written before memory events stopped carrying the text still have it,
        // and a forgotten memory's words must not reach the model from here.
        "memory_write" => format!(
            "Memory {} {} ({}) via {}{}",
            s(p, "action"),
            memory_ids(p),
            p.get("outcome")
                .and_then(Value::as_str)
                .unwrap_or("changed"),
            s(p, "via"),
            of_class(p)
        ),
        "memory_forget" => format!(
            "Memory {} forgotten via {} ({} versions){}",
            s(p, "id"),
            s(p, "via"),
            p.get("ids").and_then(Value::as_array).map_or(1, Vec::len),
            of_class(p)
        ),
        "memory_use" => {
            let count = p
                .get("ids")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or(0);
            format!("{count} memories recalled for \"{}\"", s(p, "query"))
        }
        "model_change" => format!(
            "Model changed from {} to {} ({})",
            s(p, "from"),
            s(p, "to"),
            s(p, "reason")
        ),
        "code_change" => format!(
            "Code mode {} branch {} (from {}) in {}",
            s(p, "action"),
            s(p, "branch"),
            s(p, "base"),
            s(p, "folder")
        ),
        "citation_corrected" => {
            let numbers = |key: &str| -> String {
                p.get(key)
                    .and_then(Value::as_array)
                    .map(|ns| {
                        ns.iter()
                            .filter_map(Value::as_u64)
                            .map(|n| format!("[{n}]"))
                            .collect::<String>()
                    })
                    .unwrap_or_default()
            };
            format!(
                "The user changed a citation from {} to [{}]",
                numbers("from"),
                p.get("to").and_then(Value::as_u64).unwrap_or(0)
            )
        }
        other => other.to_string(),
    };
    truncate(text.trim(), MAX_SUMMARY_CHARS)
}

fn type_names() -> Vec<&'static str> {
    AuditEventType::ALL.iter().map(|t| t.as_str()).collect()
}

pub struct AuditQueryTool {
    host: Arc<AgentHost>,
}

impl AuditQueryTool {
    fn log(&self) -> Result<Arc<AuditLog>, ToolError> {
        self.host
            .audit
            .clone()
            .ok_or_else(|| ToolError::Unavailable("The audit log is unavailable.".to_string()))
    }
}

#[async_trait]
impl HostTool for AuditQueryTool {
    fn name(&self) -> &'static str {
        app_tools::AUDIT_QUERY
    }
    fn label(&self) -> &'static str {
        "Query audit log"
    }
    fn label_template(&self) -> &'static str {
        "Checking the audit log[ for {tool}][ from {from!}][ to {to!}]"
    }
    fn description(&self) -> &'static str {
        "Read the tamper-evident audit log: what was asked, which tools ran (and whether they \
         succeeded), approvals, retrievals, source and settings changes. Filter by time range \
         (RFC 3339 or YYYY-MM-DD), event types, tool name and text. Returns one-line summaries, \
         newest first, plus counts per event type for the same filters."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "from": {"type": "string", "minLength": 10, "maxLength": 40},
                "to": {"type": "string", "minLength": 10, "maxLength": 40},
                "types": {
                    "type": "array",
                    "items": {"type": "string", "enum": type_names()},
                    "maxItems": AuditEventType::ALL.len(),
                    "uniqueItems": true
                },
                "tool": {"type": "string", "minLength": 1, "maxLength": 100},
                "text": {"type": "string", "minLength": 1, "maxLength": 200},
                "limit": {"type": "integer", "minimum": 1, "maximum": MAX_ROWS}
            },
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let tool = app_tools::AUDIT_QUERY;
        let from = match str_arg(&args, "from") {
            Some(raw) => Some(bound(raw, false).ok_or_else(|| {
                invalid(tool, format!("`from` {raw:?}: use RFC 3339 or YYYY-MM-DD"))
            })?),
            None => None,
        };
        let to = match str_arg(&args, "to") {
            Some(raw) => Some(bound(raw, true).ok_or_else(|| {
                invalid(tool, format!("`to` {raw:?}: use RFC 3339 or YYYY-MM-DD"))
            })?),
            None => None,
        };
        let types: Vec<AuditEventType> = args
            .get("types")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .filter_map(|t| t.parse().ok())
                    .collect()
            })
            .unwrap_or_default();
        let limit = limit_arg(&args, DEFAULT_ROWS, MAX_ROWS);
        let query = AuditQuery {
            types: types.clone(),
            from,
            to,
            conversation_id: None,
            tool: str_arg(&args, "tool").map(str::to_string),
            text: str_arg(&args, "text").map(str::to_string),
            limit: u32::try_from(limit).ok(),
            offset: 0,
        };
        let log = self.log()?;
        // Memories the run cannot see (another workspace's) must not show through their
        // events: not their class, ids, or that they exist.
        let visible: Vec<String> = Scope::for_workspace(ctx.scope().workspace.as_deref())
            .visible()
            .iter()
            .map(Scope::as_key)
            .collect();
        let (rows, total, by_type) = tokio::task::spawn_blocking(move || {
            let kinds: Vec<AuditEventType> = if query.types.is_empty() {
                AuditEventType::ALL.to_vec()
            } else {
                query.types.clone()
            };
            let (memory, other): (Vec<AuditEventType>, Vec<AuditEventType>) =
                kinds.into_iter().partition(|k| MEMORY_EVENTS.contains(k));
            let mut rows: Vec<AuditRow> = Vec::new();
            let mut total = 0u64;
            let mut by_type = serde_json::Map::new();
            if !other.is_empty() {
                let others = AuditQuery {
                    types: other.clone(),
                    ..query.clone()
                };
                rows = log.query(&others)?;
                total += log.count(&others)?;
                for kind in other {
                    let n = log.count(&AuditQuery {
                        types: vec![kind],
                        ..query.clone()
                    })?;
                    if n > 0 {
                        by_type.insert(kind.as_str().to_string(), json!(n));
                    }
                }
            }
            if !memory.is_empty() {
                // Read and filtered by scope here (an event without one is not shown);
                // counted from what was read, at most MEMORY_SCAN.
                let seen: Vec<AuditRow> = log
                    .query(&AuditQuery {
                        types: memory,
                        limit: Some(MEMORY_SCAN),
                        offset: 0,
                        ..query.clone()
                    })?
                    .into_iter()
                    .filter(|row| {
                        row.payload
                            .get("scope")
                            .and_then(Value::as_str)
                            .is_some_and(|scope| visible.iter().any(|v| v == scope))
                    })
                    .collect();
                total += u64::try_from(seen.len()).unwrap_or(u64::MAX);
                let mut counts: std::collections::BTreeMap<String, u64> = Default::default();
                for row in &seen {
                    *counts.entry(row.event_type.clone()).or_default() += 1;
                }
                for (kind, n) in counts {
                    by_type.insert(kind, json!(n));
                }
                rows.extend(seen);
                rows.sort_by(|a, b| b.id.cmp(&a.id));
                rows.truncate(limit);
            }
            Ok::<_, shodh_rag::audit::AuditError>((rows, total, by_type))
        })
        .await
        .map_err(|e| ToolError::Failed(format!("Audit query failed: {e}")))?
        .map_err(|e| ToolError::Failed(format!("Audit query failed: {e}")))?;

        let events: Vec<Value> = rows
            .iter()
            .map(|r| {
                json!({
                    "id": r.id,
                    "ts": r.ts,
                    "type": r.event_type,
                    "conversationId": r.conversation_id,
                    "summary": summarize(r),
                })
            })
            .collect();
        let body = serde_json::to_string(&events)
            .map_err(|e| ToolError::Failed(format!("Could not encode events: {e}")))?;
        let noun = if total == 1 { "event" } else { "events" };
        let shown = if (total as usize) > events.len() {
            format!(" (newest {} shown)", events.len())
        } else {
            String::new()
        };
        Ok(ToolOutput {
            text_for_model: format!(
                "{total} matching audit {noun}{shown}. Counts by type: {}.\n{body}",
                Value::Object(by_type.clone())
            ),
            summary_for_ui: format!("{total} audit {noun}"),
            detail: Some(json!({ "total": total, "byType": by_type, "events": events })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing;
    use super::*;
    use shodh_rag::audit::AuditRecord;

    #[tokio::test]
    async fn memory_events_of_memories_out_of_scope_and_memory_approval_labels_stay_out() {
        let t = testing::host().await;
        let log = t.host.audit.clone().unwrap();
        for (id, scope, class) in [
            ("mem-alpha", "workspace:alpha", "Person"),
            ("mem-beta", "workspace:beta", "Project"),
            ("mem-global", "global", "Note"),
        ] {
            log.append(AuditRecord::new(
                AuditEventType::MemoryWrite,
                json!({"action": "remember", "id": id, "outcome": "added", "class": class,
                       "scope": scope, "via": "ui"}),
            ))
            .unwrap();
        }
        // Written before memory approval labels stopped naming the memory.
        log.append(AuditRecord::new(
            AuditEventType::Approval,
            json!({"tool": "remember", "label": "Remember “my locker code is 4411”",
                   "decision": "approved", "who": "local-owner"}),
        ))
        .unwrap();
        let tool = AuditQueryTool {
            host: t.host.clone(),
        };
        let (ctx, _rx) = testing::ctx();
        let ctx = ctx.with_scope(Arc::new(shodh_rag::harness::tools::RunScope {
            workspace: Some("beta".into()),
            ..Default::default()
        }));
        let out = tool.execute(json!({}), &ctx).await.unwrap();
        let text = out.text_for_model;
        assert!(
            !text.contains("mem-alpha") && !text.contains("Person"),
            "{text}"
        );
        assert!(
            text.contains("mem-beta") && text.contains("mem-global"),
            "{text}"
        );
        assert!(!text.contains("locker"), "{text}");
        assert!(text.contains("Approval for remember — approved"), "{text}");
        let detail = out.detail.unwrap();
        assert_eq!(detail["byType"]["memory_write"], 2, "{detail}");
        assert_eq!(detail["total"], 3, "{detail}");
        // A run outside any workspace sees global memories only.
        let (global, _rx) = testing::ctx();
        let only = tool
            .execute(json!({"types": ["memory_write"]}), &global)
            .await
            .unwrap();
        assert_eq!(only.detail.unwrap()["total"], 1);
    }

    #[tokio::test]
    async fn filters_by_tool_type_and_time_and_summarizes() {
        let t = testing::host().await;
        let log = t.host.audit.clone().unwrap();
        log.append(AuditRecord::new(
            AuditEventType::Question,
            json!({"text": "what is due this week?"}),
        ))
        .unwrap();
        log.append(AuditRecord::new(
            AuditEventType::ToolCall,
            json!({"tool": "list_tasks", "ok": true, "summary": "3 tasks", "args": {}}),
        ))
        .unwrap();
        log.append(AuditRecord::new(
            AuditEventType::ToolCall,
            json!({"tool": "web_search", "ok": false, "summary": "Web access is off", "args": {"query": "x"}}),
        ))
        .unwrap();
        let (ctx, _rx) = testing::ctx();
        let tool = AuditQueryTool {
            host: t.host.clone(),
        };
        let out = tool
            .execute(json!({"tool": "list_tasks"}), &ctx)
            .await
            .unwrap();
        let detail = out.detail.unwrap();
        assert_eq!(detail["total"], 1);
        assert_eq!(detail["events"][0]["summary"], "list_tasks ran: 3 tasks");

        let all = tool
            .execute(json!({"from": "2000-01-01", "to": "2999-12-31"}), &ctx)
            .await
            .unwrap();
        let detail = all.detail.unwrap();
        assert_eq!(detail["total"], 3);
        assert_eq!(detail["byType"]["tool_call"], 2);
        assert_eq!(detail["byType"]["question"], 1);

        let future = tool
            .execute(json!({"from": "2999-01-01"}), &ctx)
            .await
            .unwrap();
        assert_eq!(future.detail.unwrap()["total"], 0);
        assert!(matches!(
            tool.execute(json!({"from": "yesterday"}), &ctx).await,
            Err(ToolError::InvalidArguments { .. })
        ));
    }

    #[test]
    fn summaries_are_capped() {
        let row = AuditRow {
            id: 1,
            ts: "2026-10-01T00:00:00Z".into(),
            principal: "local-owner".into(),
            conversation_id: None,
            profile_id: None,
            run_id: None,
            event_type: "question".into(),
            payload: json!({"text": "x".repeat(5000)}),
            prev_hash: String::new(),
            hash: String::new(),
        };
        assert_eq!(summarize(&row).chars().count(), MAX_SUMMARY_CHARS);
        // Memory events written before they stopped carrying words: the words stay out.
        let forget = AuditRow {
            event_type: "memory_forget".into(),
            payload: json!({"id": "mem-1", "ids": ["mem-0", "mem-1"], "class": "Note",
                            "text": "My locker code is 4411", "via": "ui"}),
            ..row.clone()
        };
        assert_eq!(
            summarize(&forget),
            "Memory mem-1 forgotten via ui (2 versions) (Note)"
        );
        let write = AuditRow {
            event_type: "memory_write".into(),
            payload: json!({"action": "remember", "id": "mem-1", "outcome": "added",
                            "class": "Note", "text": "My locker code is 4411", "via": "ui"}),
            ..row.clone()
        };
        assert!(
            !summarize(&write).contains("locker"),
            "{}",
            summarize(&write)
        );
    }
}
