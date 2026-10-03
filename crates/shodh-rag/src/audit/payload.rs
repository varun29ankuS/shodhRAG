//! Payload builders. Each takes only what may be stored: none of them can
//! receive an API key, and document text is limited to short snippets the
//! user was already shown.

use serde_json::{json, Map, Value};

use super::MAX_SNIPPET_CHARS;
use crate::harness::events::RiskTier;
use crate::harness::tools::documents::OPEN_DOCUMENT;
use crate::harness::tools::search::SEARCH_DOCUMENTS;
use crate::harness::tools::web::{FETCH_URL, SEARCH_PAPERS, WEB_SEARCH};
use crate::harness::tools::ApprovalDecision;
use crate::llm::{ApiProvider, LLMMode};

/// Longest serialised tool-argument object stored verbatim; larger argument
/// objects are stored truncated as a string.
pub const MAX_ARGS_CHARS: usize = 2_000;

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Stable provider id (the ids the settings UI and key store use).
pub fn provider_id(provider: &ApiProvider) -> &'static str {
    match provider {
        ApiProvider::OpenAI => "openai",
        ApiProvider::Anthropic => "anthropic",
        ApiProvider::OpenRouter => "openrouter",
        ApiProvider::Together => "together",
        ApiProvider::Grok => "grok",
        ApiProvider::Perplexity => "perplexity",
        ApiProvider::Google => "google",
        ApiProvider::Replicate => "replicate",
        ApiProvider::Baseten => "baseten",
        ApiProvider::Ollama => "ollama",
        // The model id and endpoint can carry credentials in URLs; only the
        // kind is recorded.
        ApiProvider::HuggingFace { .. } => "huggingface",
        ApiProvider::Custom { .. } => "custom",
    }
}

/// Whether answers from `provider_or_model` leave the machine. Accepts a
/// provider id or an omp `provider/model` string.
pub fn is_cloud(provider_or_model: &str) -> bool {
    let provider = provider_or_model
        .split('/')
        .next()
        .unwrap_or(provider_or_model)
        .trim()
        .to_ascii_lowercase();
    !matches!(
        provider.as_str(),
        "" | "ollama" | "local" | "llamacpp" | "llama.cpp" | "lmstudio"
    )
}

/// `settings_change` for a model/provider switch. Reads provider and model
/// only; the mode's API key is never touched.
pub fn model_switch(mode: &LLMMode) -> Value {
    match mode {
        LLMMode::External {
            provider, model, ..
        } => {
            let id = provider_id(provider);
            json!({
                "action": "model_switch",
                "mode": "external",
                "provider": id,
                "model": model,
                "cloud": is_cloud(id),
            })
        }
        // The file name only: the full path can contain the user's name.
        LLMMode::Local { model_path } => json!({
            "action": "model_switch",
            "mode": "local",
            "provider": "local",
            "model": model_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned()),
            "cloud": false,
        }),
        LLMMode::Disabled => json!({
            "action": "model_switch",
            "mode": "disabled",
            "provider": null,
            "model": null,
            "cloud": false,
        }),
    }
}

/// What happened to a stored API key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    Set,
    Deleted,
}

/// `settings_change` for an API key. Takes the provider id only.
pub fn api_key_change(provider: &str, action: KeyAction) -> Value {
    json!({
        "action": match action {
            KeyAction::Set => "api_key_set",
            KeyAction::Deleted => "api_key_deleted",
        },
        "provider": provider,
    })
}

fn tier_str(tier: Option<RiskTier>) -> Value {
    match tier {
        Some(RiskTier::Read) => json!("read"),
        Some(RiskTier::Write) => json!("write"),
        Some(RiskTier::Destructive) => json!("destructive"),
        None => Value::Null,
    }
}

/// Tool arguments as stored: the object itself when small, otherwise its
/// truncated JSON text.
fn stored_args(args: &Value) -> Value {
    let text = args.to_string();
    if text.chars().count() <= MAX_ARGS_CHARS {
        args.clone()
    } else {
        Value::String(truncate(&text, MAX_ARGS_CHARS))
    }
}

/// `tool_call` payload.
pub fn tool_call(
    tool: &str,
    tier: Option<RiskTier>,
    args: &Value,
    ok: bool,
    summary: &str,
    duration_ms: u64,
) -> Value {
    json!({
        "tool": tool,
        "tier": tier_str(tier),
        "args": stored_args(args),
        "ok": ok,
        "summary": summary,
        "duration_ms": duration_ms,
    })
}

/// `approval` payload. `who` is the principal for a user decision and
/// `system` for a timeout or cancellation.
pub fn approval(
    tool: &str,
    label: &str,
    tier: RiskTier,
    decision: ApprovalDecision,
    who: &str,
) -> Value {
    json!({
        "tool": tool,
        "label": label,
        "tier": tier_str(Some(tier)),
        "decision": match decision {
            ApprovalDecision::Approved => "approved",
            ApprovalDecision::Denied => "denied",
            ApprovalDecision::TimedOut => "timed_out",
            ApprovalDecision::Cancelled => "cancelled",
        },
        "who": who,
    })
}

/// `retrieval` payload for a successful document tool call: the passages a
/// search returned (path, page, score, short snippet) or the target of an
/// `open_document`. `None` for other tools.
pub fn retrieval(tool: &str, args: &Value, detail: Option<&Value>) -> Option<Value> {
    let detail = detail?;
    match tool {
        SEARCH_DOCUMENTS => {
            let passages: Vec<Value> = detail
                .get("passages")
                .and_then(Value::as_array)
                .map(|items| items.iter().map(passage_entry).collect())
                .unwrap_or_default();
            Some(json!({
                "tool": tool,
                "query": args.get("query").cloned().unwrap_or(Value::Null),
                "sources": args.get("sources").cloned().unwrap_or(Value::Null),
                "passages": passages,
            }))
        }
        OPEN_DOCUMENT => Some(json!({
            "tool": tool,
            "path": detail.get("path").cloned().unwrap_or(Value::Null),
            "location": detail.get("location").cloned().unwrap_or(Value::Null),
        })),
        WEB_SEARCH | FETCH_URL | SEARCH_PAPERS => {
            let sources: Vec<Value> = detail
                .get("webSources")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .map(|s| {
                            json!({
                                "n": s.get("n").cloned().unwrap_or(Value::Null),
                                "title": s.get("title").and_then(Value::as_str).map(|t| truncate(t, 200)),
                                "url": s.get("url").cloned().unwrap_or(Value::Null),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(json!({
                "tool": tool,
                "query": args.get("query").cloned().unwrap_or(Value::Null),
                "url": args.get("url").cloned().unwrap_or(Value::Null),
                "final_url": detail.get("finalUrl").cloned().unwrap_or(Value::Null),
                "provider": detail.get("provider").cloned().unwrap_or(Value::Null),
                "sources": sources,
            }))
        }
        _ => None,
    }
}

fn passage_entry(passage: &Value) -> Value {
    let mut entry = Map::new();
    for key in ["n", "file", "path", "page", "score"] {
        if let Some(value) = passage.get(key) {
            entry.insert(key.to_string(), value.clone());
        }
    }
    if let Some(text) = passage.get("text").and_then(Value::as_str) {
        entry.insert(
            "snippet".to_string(),
            Value::String(truncate(text, MAX_SNIPPET_CHARS)),
        );
    }
    Value::Object(entry)
}

/// Who started a source change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeOrigin {
    /// The user, through the app's UI.
    Ui,
    /// The agent, after the user's approval where the tool needs one.
    Agent,
}

impl ChangeOrigin {
    fn as_str(self) -> &'static str {
        match self {
            ChangeOrigin::Ui => "ui",
            ChangeOrigin::Agent => "agent",
        }
    }
}

/// `source_change` payload. `action` is e.g. `index_folder`, `add_file`,
/// `reindex`, `remove`, `clear_all`.
pub fn source_change(
    action: &str,
    origin: ChangeOrigin,
    source_id: Option<&str>,
    path: Option<&str>,
    details: Value,
) -> Value {
    let mut payload = json!({
        "action": action,
        "via": origin.as_str(),
        "source_id": source_id,
        "path": path,
    });
    if let (Value::Object(map), Value::Object(extra)) = (&mut payload, details) {
        map.extend(extra);
    }
    payload
}

/// Outcome fields of an indexing job for a `source_change` payload.
pub fn indexing_outcome<E: std::fmt::Display>(
    result: &Result<crate::indexing::IndexingResult, E>,
) -> Value {
    match result {
        Ok(r) => json!({
            "ok": true,
            "files": r.files_processed,
            "chunks": r.total_chunks,
            "failed_files": r.failed_files.len(),
            "duration_ms": r.duration,
        }),
        Err(e) => json!({"ok": false, "error": truncate(&e.to_string(), 500)}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_switch_never_includes_the_key() {
        let sentinel = "sk-SENTINEL-do-not-log";
        let payload = model_switch(&LLMMode::External {
            provider: ApiProvider::OpenRouter,
            api_key: sentinel.to_string(),
            model: "deepseek/deepseek-chat".to_string(),
        });
        let text = payload.to_string();
        assert!(!text.contains(sentinel));
        assert_eq!(payload["provider"], "openrouter");
        assert_eq!(payload["cloud"], true);

        let custom = model_switch(&LLMMode::External {
            provider: ApiProvider::Custom {
                endpoint: format!("https://user:{sentinel}@example.com"),
            },
            api_key: sentinel.to_string(),
            model: "m".to_string(),
        });
        assert!(!custom.to_string().contains(sentinel));

        let local = model_switch(&LLMMode::Local {
            model_path: std::path::PathBuf::from("C:/Users/someone/models/qwen3-4b-q4_k_m.gguf"),
        });
        assert_eq!(local["cloud"], false);
        assert_eq!(local["model"], "qwen3-4b-q4_k_m.gguf");
        assert!(!local.to_string().contains("someone"));
    }

    #[test]
    fn cloud_detection() {
        assert!(is_cloud("anthropic/claude-x"));
        assert!(is_cloud("openai"));
        assert!(!is_cloud("ollama/qwen3:4b"));
        assert!(!is_cloud(""));
    }

    #[test]
    fn retrieval_keeps_locations_and_caps_snippets() {
        let long = "x".repeat(1_500);
        let detail = json!({"passages": [
            {"n": 1, "file": "a.pdf", "path": "c:/d/a.pdf", "page": "4", "score": 0.5, "text": long, "heading": "H"}
        ]});
        let payload =
            retrieval(SEARCH_DOCUMENTS, &json!({"query": "notice"}), Some(&detail)).unwrap();
        let passage = &payload["passages"][0];
        assert_eq!(passage["path"], "c:/d/a.pdf");
        assert_eq!(passage["page"], "4");
        assert_eq!(
            passage["snippet"].as_str().unwrap().chars().count(),
            MAX_SNIPPET_CHARS
        );
        assert!(passage.get("text").is_none());
        assert_eq!(payload["query"], "notice");

        let open = retrieval(
            OPEN_DOCUMENT,
            &json!({"path": "c:/d/a.pdf"}),
            Some(&json!({"path": "c:/d/a.pdf", "location": "page 4 of 9"})),
        )
        .unwrap();
        assert_eq!(open["location"], "page 4 of 9");
        assert!(retrieval("list_sources", &json!({}), Some(&json!({}))).is_none());

        let web = retrieval(
            WEB_SEARCH,
            &json!({"query": "rust 2024"}),
            Some(&json!({"provider": "SearXNG", "webSources": [
                {"n": 3, "title": "Rust", "url": "https://example.org/", "snippet": "long text"}
            ]})),
        )
        .unwrap();
        assert_eq!(web["query"], "rust 2024");
        assert_eq!(web["sources"][0]["url"], "https://example.org/");
        assert!(web["sources"][0].get("snippet").is_none());
        let page = retrieval(
            FETCH_URL,
            &json!({"url": "http://example.org/a"}),
            Some(&json!({"finalUrl": "https://example.org/a", "webSources": []})),
        )
        .unwrap();
        assert_eq!(page["final_url"], "https://example.org/a");
        assert!(retrieval(SEARCH_DOCUMENTS, &json!({}), None).is_none());
    }

    #[test]
    fn source_changes_merge_details() {
        let p = source_change(
            "remove",
            ChangeOrigin::Agent,
            Some("s1"),
            Some("c:/docs"),
            json!({"ok": true, "chunks": 12}),
        );
        assert_eq!(p["action"], "remove");
        assert_eq!(p["via"], "agent");
        assert_eq!(p["chunks"], 12);
        let failed: Result<crate::indexing::IndexingResult, String> = Err("disk full".into());
        assert_eq!(indexing_outcome(&failed)["ok"], false);
    }

    #[test]
    fn large_tool_arguments_are_truncated() {
        let args = json!({"text": "y".repeat(5_000)});
        let payload = tool_call("create_task", Some(RiskTier::Write), &args, true, "ok", 3);
        assert!(payload["args"].is_string());
        assert_eq!(
            payload["args"].as_str().unwrap().chars().count(),
            MAX_ARGS_CHARS
        );
        let small = tool_call("x", None, &json!({"a": 1}), false, "", 0);
        assert_eq!(small["args"]["a"], 1);
        assert!(small["tier"].is_null());
    }
}
