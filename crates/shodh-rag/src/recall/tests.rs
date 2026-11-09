//! Recall end to end over versioned fixture transcripts, with planted secrets.
//!
//! The fixtures (`fixtures/<tool>/<format version>.jsonl`) are synthetic transcripts in
//! each format Recall reads. Secrets are planted into them at run time from
//! placeholders (`{{GHP}}`, `{{PEM_JSON}}`, ...), so no secret-shaped string is
//! committed; each is generated in its provider's shape. The tests check every exit a
//! transcript's text can take (the stored chunks, keyword search, the catalog, entities,
//! open, scope and search answers, error previews), not only the redactor.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::RwLock;

use super::catalog::{Catalog, FileState};
use super::discover::{project_chain, project_key, Roots};
use super::entities::{extract, EntityKind};
use super::index::{parse_source, source_id, RecallIndex, SyncControl, SyncProgress};
use super::query::{self, RecallQuery};
use super::redact::{Redactor, REDACTED};
use super::settings::RecallSettings;
use super::{read_transcript, Format, ParseBudget, Role, Tool};
use crate::embeddings::EmbeddingModel;
use crate::RAGEngine;

const CLAUDE_V21: &str = include_str!("fixtures/claude-code/v2.1.jsonl");
const CLAUDE_V10: &str = include_str!("fixtures/claude-code/v1.0-summary.jsonl");
const CODEX_V1: &str = include_str!("fixtures/codex/v1-flat.jsonl");
const CODEX_V2: &str = include_str!("fixtures/codex/v2-envelope.jsonl");

const LUMEN_SESSION: &str = "5b0c7a3e-1d2f-4a6b-9c8d-0e1f2a3b4c5d";
const TIDEPOOL_SESSION: &str = "9e8d7c6b-5a4f-4e3d-8c2b-1a0f9e8d7c6b";
const ORBIT_SESSION: &str = "0f1e2d3c-4b5a-4968-8776-655443322110";
const HARBOR_SESSION: &str = "7a6b5c4d-3e2f-4a1b-9c8d-7e6f5a4b3c2d";

/// The user's own pattern in the tests (between slashes: a pattern, not a word), and the
/// secret it covers.
const USER_PATTERN: &str = r"/ACME-STAGE-[0-9]{6}/";
const USER_SECRET: &str = "ACME-STAGE-482913";

/// A deterministic string of `n` characters drawn uniformly from `alphabet`: real keys
/// have long lower-case runs and missing digits as often as chance gives them.
fn random(seed: u64, n: usize, alphabet: &str) -> String {
    let chars: Vec<char> = alphabet.chars().collect();
    let mut state = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    let mut out = String::with_capacity(n);
    while out.chars().count() < n {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        out.push(chars[((state >> 33) as usize) % chars.len()]);
    }
    out
}

const BASE64: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

const ALNUM: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

/// The secrets planted into the fixtures, each in its provider's shape.
struct Secrets {
    pairs: Vec<(&'static str, String)>,
}

impl Secrets {
    fn new() -> Self {
        let pem_body: Vec<String> = (0..3)
            .map(|i| {
                random(
                    100 + i,
                    64,
                    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/",
                )
            })
            .collect();
        let pem = format!(
            "-----BEGIN RSA PRIVATE KEY-----\n{}\n-----END RSA PRIVATE KEY-----",
            pem_body.join("\n")
        );
        let pairs = vec![
            (
                "SK_ANT",
                format!(
                    "sk-ant-api03-{}-{}AA",
                    random(1, 40, ALNUM),
                    random(2, 10, ALNUM)
                ),
            ),
            ("OPENAI", format!("sk-proj-{}", random(3, 48, ALNUM))),
            ("GHP", format!("ghp_{}", random(4, 36, ALNUM))),
            (
                "SLACK",
                format!(
                    "xoxb-{}-{}-{}",
                    random(5, 12, "0123456789"),
                    random(6, 13, "0123456789"),
                    random(7, 24, ALNUM)
                ),
            ),
            (
                "AWS_ID",
                format!("AKIA{}", random(8, 16, "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567")),
            ),
            (
                "AWS_SECRET",
                random(
                    9,
                    40,
                    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/",
                ),
            ),
            (
                "JWT",
                format!(
                    "eyJ{}.eyJ{}.{}",
                    random(10, 20, ALNUM),
                    random(11, 30, ALNUM),
                    random(12, 30, ALNUM)
                ),
            ),
            ("GOOGLE", format!("AIza{}", random(13, 35, ALNUM))),
            ("PASSWORD", format!("Tr0ub4dor{}", random(14, 12, ALNUM))),
            (
                "SK_ANT_HEAD",
                format!("sk-ant-api03-{}", random(15, 30, ALNUM)),
            ),
            ("SK_ANT_TAIL", random(16, 40, ALNUM)),
            ("USER_SECRET", USER_SECRET.to_string()),
            ("PEM", pem.clone()),
            ("PEM_JSON", pem.replace('\n', "\\n")),
        ];
        Self { pairs }
    }

    fn plant(&self, fixture: &str) -> String {
        let mut out = fixture.to_string();
        for (name, value) in &self.pairs {
            out = out.replace(&format!("{{{{{name}}}}}"), value);
        }
        assert!(!out.contains("{{"), "a placeholder was left: {out}");
        out
    }

    /// Pieces of every secret that must never come out (the PEM body lines, not its
    /// header; every other secret whole).
    fn needles(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (name, value) in &self.pairs {
            match *name {
                "PEM" => out.extend(
                    value
                        .lines()
                        .filter(|l| !l.starts_with("-----"))
                        .map(str::to_string),
                ),
                "PEM_JSON" => {}
                _ => out.push(value.clone()),
            }
        }
        // A secret's distinctive middle too: a partial leak is a leak.
        let mut middles: Vec<String> = out
            .iter()
            .filter(|s| s.len() >= 24)
            .map(|s| s[8..20].to_string())
            .collect();
        out.append(&mut middles);
        out
    }

    fn get(&self, name: &str) -> &str {
        &self.pairs.iter().find(|(n, _)| *n == name).unwrap().1
    }
}

fn assert_clean(what: &str, text: &str, secrets: &Secrets) {
    for needle in secrets.needles() {
        assert!(
            !text.contains(&needle),
            "{what} holds a planted secret ({needle}):\n{text}"
        );
    }
}

// ── Test engine ─────────────────────────────────────────────────────

const DIM: usize = 256;

/// Words hashed into a fixed number of dimensions, L2-normalised: texts that share
/// words are near.
struct HashEmbedder;

impl HashEmbedder {
    fn embed(text: &str) -> Vec<f32> {
        let mut v = vec![0.0f32; DIM];
        for word in text
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() > 2)
        {
            let mut h: u64 = 0xcbf29ce484222325;
            for b in word.to_lowercase().bytes() {
                h ^= u64::from(b);
                h = h.wrapping_mul(0x100000001b3);
            }
            v[(h % DIM as u64) as usize] += 1.0;
        }
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm == 0.0 {
            v[0] = 1.0;
        } else {
            v.iter_mut().for_each(|x| *x /= norm);
        }
        v
    }
}

impl EmbeddingModel for HashEmbedder {
    fn embed_query(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        Ok(Self::embed(text))
    }
    fn embed_document(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        Ok(Self::embed(text))
    }
    fn dimension(&self) -> usize {
        DIM
    }
}

async fn test_index(dir: &Path) -> RecallIndex {
    let mut config = crate::config::RAGConfig::default();
    config.data_dir = dir.join("index");
    config.embedding.model_dir = dir.join("no-models");
    config.embedding.use_e5 = false;
    config.embedding.dimension = DIM;
    config.search.min_score_threshold = 0.0;
    let mut engine = RAGEngine::new(config).await.unwrap();
    engine
        .attach_search_models(crate::rag_engine::SearchModels::from_embedder(Arc::new(
            HashEmbedder,
        )))
        .unwrap();
    let catalog = Catalog::open(&dir.join("recall.db")).unwrap();
    RecallIndex::with_parts(dir, Arc::new(RwLock::new(engine)), Arc::new(catalog))
}

/// A home folder with the four fixtures where the tools keep them.
struct Home {
    roots: Roots,
    lumen: PathBuf,
    tidepool: PathBuf,
    orbit: PathBuf,
    harbor: PathBuf,
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn set_mtime(path: &Path, unix_secs: i64) {
    let at = std::time::UNIX_EPOCH + std::time::Duration::from_secs(unix_secs as u64);
    std::fs::File::options()
        .append(true)
        .open(path)
        .unwrap()
        .set_modified(at)
        .unwrap();
}

fn home(dir: &Path, secrets: &Secrets) -> Home {
    let claude = dir.join("home").join(".claude").join("projects");
    let codex = dir.join("home").join(".codex").join("sessions");
    let lumen = claude
        .join("-home-dev-lumen-cache")
        .join(format!("{LUMEN_SESSION}.jsonl"));
    let tidepool = claude
        .join("-home-dev-tidepool")
        .join(format!("{TIDEPOOL_SESSION}.jsonl"));
    let orbit = codex
        .join("2026")
        .join("08")
        .join("20")
        .join(format!("rollout-2026-08-20T08-15-00-{ORBIT_SESSION}.jsonl"));
    let harbor = codex.join("2026").join("10").join("28").join(format!(
        "rollout-2026-10-28T16-40-00-{HARBOR_SESSION}.jsonl"
    ));
    // Each written when its session last changed, as the tools leave them.
    for (path, fixture, last_write) in [
        (&lumen, CLAUDE_V21, "2026-10-12T09:06:20Z"),
        (&tidepool, CLAUDE_V10, "2026-09-03T14:00:21Z"),
        (&orbit, CODEX_V1, "2026-08-20T09:00:00Z"),
        (&harbor, CODEX_V2, "2026-10-28T16:42:00Z"),
    ] {
        write(path, &secrets.plant(fixture));
        set_mtime(path, super::parse_time(last_write).unwrap().timestamp());
    }
    Home {
        roots: Roots {
            claude_code: Some(claude),
            codex: Some(codex),
        },
        lumen,
        tidepool,
        orbit,
        harbor,
    }
}

fn all_projects() -> Vec<&'static str> {
    vec![
        "/home/dev/lumen-cache",
        "/home/dev/tidepool",
        "/home/dev/orbit-web",
        "/home/dev/harbor-cli",
    ]
}

fn settings(projects: &[&str], patterns: &[&str]) -> RecallSettings {
    let mut s = RecallSettings {
        enabled: true,
        ..RecallSettings::default()
    };
    s.tools.insert(Tool::ClaudeCode);
    s.tools.insert(Tool::Codex);
    for p in projects {
        s.projects.insert(project_key(p), true);
    }
    s.patterns = patterns.iter().map(|p| p.to_string()).collect();
    s
}

fn no_stop() -> bool {
    false
}

fn ignore_progress(_: &SyncProgress) {}

const CONTROL: SyncControl<'static> = SyncControl {
    cancelled: &no_stop,
    paused: &no_stop,
    progress: &ignore_progress,
};

/// Well after every fixture was written: all are idle.
fn later() -> i64 {
    super::index::now_secs() + 86_400
}

fn planted(dir: &Path, secrets: &Secrets, name: &str, fixture: &str) -> PathBuf {
    let path = dir.join(name);
    write(&path, &secrets.plant(fixture));
    path
}

// ── Parsers ─────────────────────────────────────────────────────────

#[test]
fn claude_code_v2_1_turns_titles_and_skipped_lines() {
    let dir = tempfile::tempdir().unwrap();
    let secrets = Secrets::new();
    let path = planted(dir.path(), &secrets, "s.jsonl", CLAUDE_V21);
    let t = read_transcript(
        Tool::ClaudeCode,
        &path,
        &Redactor::new(&[USER_PATTERN.to_string()]).unwrap(),
        ParseBudget::default(),
    )
    .unwrap();
    assert_eq!(t.format, Format::ClaudeCodeJsonl);
    assert_eq!(t.info.session_id.as_deref(), Some(LUMEN_SESSION));
    assert_eq!(t.info.cwd.as_deref(), Some("/home/dev/lumen-cache"));
    assert_eq!(t.info.branch.as_deref(), Some("fix/eviction-off-by-one"));
    assert_eq!(t.info.model.as_deref(), Some("claude-opus-4-1"));
    assert_eq!(t.info.version.as_deref(), Some("2.1.312"));
    // The user's own title wins over the generated one.
    assert_eq!(t.info.title.as_deref(), Some("Lumen cache eviction fix"));
    assert!(t.info.pull_requests.contains(&"#214".to_string()));
    let roles: Vec<Role> = t.turns.iter().map(|x| x.role).collect();
    assert_eq!(roles[0], Role::User, "the caveat (isMeta) is no turn");
    assert!(t.turns[0].text.contains("evicts one entry too early"));
    // Thinking and the repeated request are never read.
    assert!(!t
        .turns
        .iter()
        .any(|x| x.text.contains("thinking is never indexed")));
    assert!(!t
        .turns
        .iter()
        .any(|x| x.text.contains("must never be read")));
    // Tool calls carry their name; outputs are linked to their call.
    let bash = t.turns.iter().find(|x| x.role == Role::ToolCall).unwrap();
    assert_eq!(bash.tool_name.as_deref(), Some("Bash"));
    assert!(bash.text.contains("cargo test -p lumen-cache eviction"));
    let output = t.turns.iter().find(|x| x.role == Role::ToolOutput).unwrap();
    assert_eq!(output.tool_name.as_deref(), Some("Bash"));
    assert!(output.text.contains("error[E0502]"));
    // Reading .env: the output is kept back, not indexed.
    let env = t
        .turns
        .iter()
        .find(|x| x.role == Role::ToolOutput && x.tool_name.as_deref() == Some("Read"))
        .unwrap();
    assert!(env.kept_back.is_some());
    assert!(env.text.is_empty());
    // An error output says so.
    assert!(t
        .turns
        .iter()
        .any(|x| x.text.starts_with("Error: curl: (6)")));
    // The half-written line is kept as an error with a redacted preview.
    assert_eq!(t.errors.len(), 1);
    assert_eq!(t.errors[0].line, 19);
    assert!(t.errors[0].preview.contains("half written line"));
    assert!(t.errors[0].preview.contains(REDACTED));
    assert!(
        t.skipped >= 6,
        "permission, attachment, requests, system: {}",
        t.skipped
    );
    assert!(t.turns.iter().all(|x| x.at.is_some()));
    assert_eq!(
        t.info.started.unwrap().to_rfc3339(),
        "2026-10-12T09:00:00+00:00"
    );
}

#[test]
fn claude_code_v1_0_summary_title_and_mixed_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let path = planted(dir.path(), &Secrets::new(), "s.jsonl", CLAUDE_V10);
    let t = read_transcript(
        Tool::ClaudeCode,
        &path,
        &Redactor::default(),
        ParseBudget::default(),
    )
    .unwrap();
    assert_eq!(t.info.title.as_deref(), Some("Tidepool webhook retries"));
    assert_eq!(t.info.model.as_deref(), Some("claude-sonnet-4-20250514"));
    let roles: Vec<Role> = t.turns.iter().map(|x| x.role).collect();
    assert_eq!(
        roles,
        vec![
            Role::User,
            Role::Assistant,
            Role::ToolCall,
            Role::ToolOutput
        ]
    );
    assert_eq!(t.turns[2].tool_name.as_deref(), Some("Edit"));
    assert!(t.turns[2].text.contains("internal/notify/sender.go"));
    assert_eq!(t.turns[3].tool_name.as_deref(), Some("Edit"));
}

#[test]
fn a_subagent_thread_belongs_to_its_session() {
    let path = Path::new("/p/-home-dev-app/1111-2222/subagents/agent-a8f3.jsonl");
    assert_eq!(
        super::claude::session_from_path(path).as_deref(),
        Some("1111-2222")
    );
    assert_eq!(
        super::claude::thread_from_path(path).as_deref(),
        Some("a8f3")
    );
    let main = Path::new("/p/-home-dev-app/1111-2222.jsonl");
    assert_eq!(
        super::claude::session_from_path(main).as_deref(),
        Some("1111-2222")
    );
    assert_eq!(super::claude::thread_from_path(main), None);
}

#[test]
fn codex_v1_flat_items_outputs_and_context() {
    let dir = tempfile::tempdir().unwrap();
    let secrets = Secrets::new();
    let path = planted(dir.path(), &secrets, "rollout-x.jsonl", CODEX_V1);
    let t = read_transcript(
        Tool::Codex,
        &path,
        &Redactor::default(),
        ParseBudget::default(),
    )
    .unwrap();
    assert_eq!(t.format, Format::CodexFlat);
    assert_eq!(t.info.session_id.as_deref(), Some(ORBIT_SESSION));
    assert_eq!(
        t.info.cwd.as_deref(),
        Some("/home/dev/orbit-web"),
        "read from the environment context"
    );
    assert_eq!(t.info.branch.as_deref(), Some("feat/orbit-oauth"));
    // The environment context is no turn; reasoning is never read.
    assert!(t.turns[0].text.starts_with("Add the OAuth device flow"));
    assert!(!t
        .turns
        .iter()
        .any(|x| x.text.contains("reasoning is never indexed")));
    // `arguments` is decoded twice; `{output, metadata}` is unwrapped.
    let call = t.turns.iter().find(|x| x.role == Role::ToolCall).unwrap();
    assert_eq!(call.tool_name.as_deref(), Some("shell"));
    assert!(
        call.text.contains("rg -n \"login\" src/auth"),
        "{}",
        call.text
    );
    let out = t.turns.iter().find(|x| x.role == Role::ToolOutput).unwrap();
    assert!(out.text.starts_with("src/auth/login.ts:12"), "{}", out.text);
    // `cat .env.local` names a secrets file: kept back.
    assert_eq!(
        t.turns.iter().filter(|x| x.kept_back.is_some()).count(),
        2,
        "the call and its output"
    );
    assert!(t.turns.iter().any(|x| x
        .text
        .starts_with("Exit code 1\nFAIL src/auth/deviceFlow.test.ts")));
    // No times of their own: the session's start.
    assert!(t.turns.iter().all(|x| x.at == t.info.started));
    assert_eq!(t.errors.len(), 1);
    assert_clean("an error preview", &t.errors[0].preview, &secrets);
}

#[test]
fn codex_v2_envelope_items_and_events() {
    let dir = tempfile::tempdir().unwrap();
    let path = planted(dir.path(), &Secrets::new(), "rollout-y.jsonl", CODEX_V2);
    let t = read_transcript(
        Tool::Codex,
        &path,
        &Redactor::default(),
        ParseBudget::default(),
    )
    .unwrap();
    assert_eq!(t.format, Format::CodexEnvelope);
    assert_eq!(t.info.session_id.as_deref(), Some(HARBOR_SESSION));
    assert_eq!(t.info.cwd.as_deref(), Some("/home/dev/harbor-cli"));
    assert_eq!(t.info.model.as_deref(), Some("gpt-5-codex"));
    assert_eq!(t.info.branch.as_deref(), Some("fix/harbor-retry-panic"));
    assert_eq!(t.info.version.as_deref(), Some("0.48.0"));
    let roles: Vec<Role> = t.turns.iter().map(|x| x.role).collect();
    assert_eq!(
        roles,
        vec![
            Role::User,
            Role::ToolCall,
            Role::ToolOutput,
            Role::ToolCall,
            Role::ToolOutput,
            Role::Assistant
        ]
    );
    // Events repeat the items and are not read.
    assert!(!t.turns.iter().any(|x| x.text.contains("not read")));
    assert_eq!(t.turns[3].tool_name.as_deref(), Some("apply_patch"));
    assert!(t.turns[3].text.contains("*** Update File: src/push.rs"));
    assert!(t.turns.iter().all(|x| x.at.is_some()));
    assert!(t.errors.is_empty());
}

#[test]
fn an_overlong_line_is_an_error_not_a_crash() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.jsonl");
    let big = format!(
        "{{\"type\":\"user\",\"message\":{{\"content\":\"{}\"}},\"timestamp\":\"2026-10-12T09:00:00Z\"}}\n{{\"type\":\"user\",\"message\":{{\"content\":\"after\"}}}}\n",
        "x".repeat(4096)
    );
    std::fs::write(&path, big).unwrap();
    let budget = ParseBudget {
        max_line_bytes: 1024,
        ..ParseBudget::default()
    };
    let t = read_transcript(Tool::ClaudeCode, &path, &Redactor::default(), budget).unwrap();
    assert_eq!(t.errors.len(), 1);
    assert_eq!(t.turns.len(), 1);
    assert_eq!(t.turns[0].text, "after");
    assert_eq!(t.turns[0].line, 2);
}

// ── Redaction ───────────────────────────────────────────────────────

#[test]
fn every_planted_secret_shape_is_redacted() {
    let s = Secrets::new();
    let r = Redactor::default();
    let cases: Vec<(String, String)> = vec![
        (
            format!("key {}", s.get("SK_ANT")),
            s.get("SK_ANT").to_string(),
        ),
        (
            format!("export OPENAI_API_KEY={}", s.get("OPENAI")),
            s.get("OPENAI").to_string(),
        ),
        (
            format!("\"apiKey\": \"{}\"", s.get("GOOGLE")),
            s.get("GOOGLE").to_string(),
        ),
        (format!("token={}", s.get("GHP")), s.get("GHP").to_string()),
        (
            format!(
                "git clone https://x-access-token:{}@github.com/a/b",
                s.get("GHP")
            ),
            s.get("GHP").to_string(),
        ),
        (
            format!("SLACK_BOT_TOKEN = '{}'", s.get("SLACK")),
            s.get("SLACK").to_string(),
        ),
        (
            format!(
                "[default]\naws_access_key_id = {}\naws_secret_access_key = {}",
                s.get("AWS_ID"),
                s.get("AWS_SECRET")
            ),
            s.get("AWS_SECRET").to_string(),
        ),
        (
            format!("id {}", s.get("AWS_ID")),
            s.get("AWS_ID").to_string(),
        ),
        (
            format!("curl -H 'Authorization: Bearer {}' x", s.get("JWT")),
            s.get("JWT").to_string(),
        ),
        (
            format!("Authorization: token {}", random(20, 40, ALNUM)),
            random(20, 40, ALNUM),
        ),
        (
            format!("x-api-key: {}", random(21, 32, ALNUM)),
            random(21, 32, ALNUM),
        ),
        (
            format!("postgres://app:{}@db:5432/x", s.get("PASSWORD")),
            s.get("PASSWORD").to_string(),
        ),
        (
            format!(
                "https://api.example.com/v1?api_key={}&page=2",
                s.get("GOOGLE")
            ),
            s.get("GOOGLE").to_string(),
        ),
        (
            format!("mysql --password {} -h db", s.get("PASSWORD")),
            s.get("PASSWORD").to_string(),
        ),
        (
            format!("Server=db;User Id=sa;Password={};", s.get("PASSWORD")),
            s.get("PASSWORD").to_string(),
        ),
        (format!("DB_PASSWORD=hunter{}", 2), "hunter2".to_string()),
        ("password: changeme".to_string(), "changeme".to_string()),
        (
            s.get("PEM").to_string(),
            s.get("PEM").lines().nth(1).unwrap().to_string(),
        ),
        // A PEM block JSON-escaped inside a tool's output.
        (
            format!("{{\"content\":\"{}\"}}", s.get("PEM_JSON")),
            s.get("PEM").lines().nth(2).unwrap().to_string(),
        ),
        // A key block cut off by an output's head.
        (
            s.get("PEM").lines().take(3).collect::<Vec<_>>().join("\n"),
            s.get("PEM").lines().nth(2).unwrap().to_string(),
        ),
        // Wrapped by a terminal, raw and as a JSON `\n`.
        (
            format!("{}\n{}\nnext", s.get("SK_ANT_HEAD"), s.get("SK_ANT_TAIL")),
            s.get("SK_ANT_TAIL").to_string(),
        ),
        (
            format!("{}\\n{}\\nnext", s.get("SK_ANT_HEAD"), s.get("SK_ANT_TAIL")),
            s.get("SK_ANT_TAIL").to_string(),
        ),
        (
            format!(
                "https://hooks.slack.com/services/T0{}/B0{}/{}",
                random(22, 8, "ABCDEFGHIJ0123456789"),
                random(23, 8, "ABCDEFGHIJ0123456789"),
                random(24, 24, ALNUM)
            ),
            random(24, 24, ALNUM),
        ),
        (
            format!(
                "bot {}:AA{}",
                random(25, 10, "0123456789"),
                random(26, 33, ALNUM)
            ),
            random(26, 33, ALNUM),
        ),
        (
            format!("stripe sk_live_{}", random(27, 24, ALNUM)),
            random(27, 24, ALNUM),
        ),
        (
            format!("hf_{} in HF_TOKEN", random(28, 34, ALNUM)),
            random(28, 34, ALNUM),
        ),
        (
            format!("glpat-{}", random(29, 20, ALNUM)),
            random(29, 20, ALNUM),
        ),
        (
            format!("github_pat_{}", random(30, 60, ALNUM)),
            random(30, 60, ALNUM),
        ),
        // No known prefix: long and random.
        (
            format!("the key is {} ok", random(31, 40, ALNUM)),
            random(31, 40, ALNUM),
        ),
        (
            format!("Cookie: session={}; theme=dark", random(32, 30, ALNUM)),
            random(32, 30, ALNUM),
        ),
    ];
    for (text, secret) in cases {
        let out = r.redact(&text);
        assert!(!out.contains(&secret), "not redacted: {text}\n-> {out}");
        assert!(out.contains(REDACTED), "{text} -> {out}");
    }
}

#[test]
fn redaction_keeps_what_is_not_secret() {
    let r = Redactor::default();
    for text in [
        "commit 3f9a2c1e8b7d6a5f4e3d2c1b0a9f8e7d6c5b4a39 fixed it",
        "session 5b0c7a3e-1d2f-4a6b-9c8d-0e1f2a3b4c5d",
        "see crates/shodh-rag/src/recall/index.rs and app/src/features/library/LibraryView.tsx",
        "\"max_tokens\": 4096",
        "let token = tokens.next();",
        "the bearer of bad news",
        "TOKEN_LIMIT is a constant",
        "password = form.password",
        "OPENAI_API_KEY=${OPENAI_API_KEY}",
        "api_key: <your key here>",
        "fn validateAuthorizationHeader(request: &Request)",
        "error[E0502]: cannot borrow `self.order` as mutable",
        "https://github.com/acme/lumen-cache/pull/214",
        "useNavigationTarget('library', target => {",
    ] {
        assert_eq!(r.redact(text), text, "changed: {text}");
    }
}

#[test]
fn user_patterns_are_redacted_and_bad_ones_refused() {
    let plain = Redactor::default().redact(&format!("passphrase is {USER_SECRET}"));
    assert!(
        plain.contains(USER_SECRET),
        "the built-in rules do not know it"
    );
    let r = Redactor::new(&[USER_PATTERN.to_string()]).unwrap();
    assert!(!r
        .redact(&format!("passphrase is {USER_SECRET}"))
        .contains(USER_SECRET));
    assert_ne!(r.fingerprint(), Redactor::default().fingerprint());
    assert!(Redactor::new(&["/(unclosed/".to_string()]).is_err());
    assert!(
        Redactor::new(&["/a*/".to_string()]).is_err(),
        "matches empty text"
    );
    assert!(Redactor::new(&[String::new()]).is_err());
}

#[test]
fn raw_lines_are_unescaped_before_redaction() {
    let s = Secrets::new();
    let r = Redactor::default();
    // A key with an escaped dash and a PEM with escaped line breaks.
    let escaped = s.get("OPENAI").replacen('-', "\\u002d", 1);
    let raw = format!("{{\"text\":\"key {escaped} and {}", s.get("PEM_JSON"));
    let preview = r.redact_raw_line(raw.as_bytes(), 10_000);
    assert_clean("a raw line preview", &preview, &s);
}

// ── Entities ────────────────────────────────────────────────────────

#[test]
fn entities_are_found_by_pattern() {
    let text = "Fixed in src/lru.rs (commit 3f9a2c1), see PR #214 and issues/120; branch fix/eviction-off-by-one; \
                error[E0502] and TS2345 and ENOENT, TypeError too; model claude-opus-4-1; https://github.com/acme/x/pull/9. \
                id 5b0c7a3e-1d2f-4a6b-9c8d-0e1f2a3b4c5d";
    let found = extract(text);
    let has =
        |kind: EntityKind, value: &str| found.iter().any(|e| e.kind == kind && e.value == value);
    assert!(has(EntityKind::File, "src/lru.rs"), "{found:?}");
    assert!(has(EntityKind::Commit, "3f9a2c1"));
    assert!(has(EntityKind::Issue, "#214"));
    assert!(has(EntityKind::Issue, "#120"));
    assert!(has(EntityKind::Issue, "#9"));
    assert!(has(EntityKind::Branch, "fix/eviction-off-by-one"));
    assert!(has(EntityKind::Error, "E0502"));
    assert!(has(EntityKind::Error, "TS2345"));
    assert!(has(EntityKind::Error, "ENOENT"));
    assert!(has(EntityKind::Error, "TypeError"));
    assert!(has(EntityKind::Model, "claude-opus-4-1"));
    assert!(has(EntityKind::Url, "https://github.com/acme/x/pull/9"));
    assert!(
        !found.iter().any(|e| e.kind == EntityKind::Commit
            && "5b0c7a3e-1d2f-4a6b-9c8d-0e1f2a3b4c5d".contains(&e.value)),
        "no commit from a UUID"
    );
    assert!(
        !found
            .iter()
            .any(|e| e.kind == EntityKind::File && e.value.contains("github.com")),
        "a URL's path is no file"
    );
}

// ── The index end to end ────────────────────────────────────────────

/// Everything a stored chunk holds, for every part of every indexed file.
async fn stored_text(index: &RecallIndex) -> String {
    let engine = index.engine().read().await;
    let mut out = String::new();
    for file in index.catalog().files().unwrap() {
        for part in 0..file.parts {
            for hit in engine
                .chunks_of_source(&source_id(file.tool, file.id, part))
                .await
                .unwrap()
            {
                for text in [
                    &hit.text,
                    &hit.title,
                    &hit.heading,
                    &hit.metadata_json,
                    &hit.citation_json,
                    &hit.source,
                ] {
                    out.push_str(text);
                    out.push('\n');
                }
            }
        }
    }
    out
}

async fn synced(dir: &Path, secrets: &Secrets, s: &RecallSettings) -> (RecallIndex, Home) {
    let home = home(dir, secrets);
    let index = test_index(&dir.join("recall")).await;
    let report = index.sync(s, &home.roots, later(), &CONTROL).await.unwrap();
    assert_eq!(report.failed, 0, "{report:?}");
    (index, home)
}

#[tokio::test]
async fn no_planted_secret_leaves_the_transcripts() {
    let dir = tempfile::tempdir().unwrap();
    let secrets = Secrets::new();
    let s = settings(&all_projects(), &[USER_PATTERN]);
    let (index, _home) = synced(dir.path(), &secrets, &s).await;
    let files = index.catalog().files().unwrap();
    assert_eq!(
        files
            .iter()
            .filter(|f| f.state == FileState::Indexed)
            .count(),
        4
    );

    let stored = stored_text(&index).await;
    assert!(
        stored.contains("evicts one entry too early"),
        "the session was indexed"
    );
    assert_clean("the stored chunks", &stored, &secrets);

    // Keyword and vector search for each secret: nothing comes back that holds it.
    for needle in secrets.needles() {
        let answer = query::search(
            &index,
            &s,
            &RecallQuery {
                text: needle.clone(),
                limit: 20,
                ..RecallQuery::default()
            },
        )
        .await
        .unwrap();
        let json = serde_json::to_string(&answer).unwrap();
        assert_clean("a search answer", &json, &secrets);
        let raw = index
            .engine()
            .read()
            .await
            .search(&needle, 20)
            .await
            .unwrap();
        for hit in raw {
            assert_clean("a raw search hit", &hit.text, &secrets);
        }
    }

    // The catalog: titles, entities, error previews.
    for file in &files {
        assert_clean("a catalog record", &format!("{file:?}"), &secrets);
        assert_clean(
            "the entities",
            &format!(
                "{:?}",
                index.catalog().file_entities(file.id, &[], 10_000).unwrap()
            ),
            &secrets,
        );
        assert_clean(
            "the error records",
            &format!("{:?}", index.catalog().errors(file.id).unwrap()),
            &secrets,
        );
        // Every turn, opened.
        for turn in 0..file.turns {
            let opened = query::open(&index, &s, &query::receipt_id(file.id, turn), 0).unwrap();
            assert_clean(
                "an opened turn",
                &serde_json::to_string(&opened).unwrap(),
                &secrets,
            );
        }
    }
    let scope = query::scope(&index, &s, None, None, &[], &[]).unwrap();
    assert_eq!(scope.sessions.len(), 4);
    assert_clean(
        "the scope",
        &serde_json::to_string(&scope).unwrap(),
        &secrets,
    );
}

#[tokio::test]
async fn search_finds_the_session_and_opens_the_exact_turn() {
    let dir = tempfile::tempdir().unwrap();
    let secrets = Secrets::new();
    let s = settings(&all_projects(), &[]);
    let (index, _home) = synced(dir.path(), &secrets, &s).await;

    let answer = query::search(
        &index,
        &s,
        &RecallQuery {
            text: "LRU cache evicts an entry too early off-by-one".into(),
            ..RecallQuery::default()
        },
    )
    .await
    .unwrap();
    let top = &answer.sessions[0];
    assert_eq!(top.session_id, LUMEN_SESSION);
    assert_eq!(top.title, "Lumen cache eviction fix");
    assert_eq!(top.project, "lumen-cache");
    assert!(!top.receipts.is_empty());

    let receipt = &top.receipts[0];
    let opened = query::open(&index, &s, &receipt.id, 1).unwrap();
    assert_eq!(opened.turn, receipt.turn);
    let target = opened.turns.iter().find(|t| t.target).unwrap();
    assert_eq!(target.turn, receipt.turn);
    assert!(opened.turns.len() >= 2, "with the turns around it");

    // An exact id comes first: the PR, a file path's end, a SHA.
    for named in [
        "what happened in #214",
        "the change to lru.rs",
        "commit 3f9a2c1",
    ] {
        let answer = query::search(
            &index,
            &s,
            &RecallQuery {
                text: named.into(),
                ..RecallQuery::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(answer.sessions[0].session_id, LUMEN_SESSION, "{named}");
        assert!(!answer.sessions[0].exact.is_empty(), "{named}");
    }
    // Filters: tool, project, window, entity.
    let codex_only = query::search(
        &index,
        &s,
        &RecallQuery {
            text: "panic retry registry".into(),
            tools: vec![Tool::Codex],
            ..RecallQuery::default()
        },
    )
    .await
    .unwrap();
    assert!(codex_only.sessions.iter().all(|x| x.tool == Tool::Codex));
    assert_eq!(codex_only.sessions[0].session_id, HARBOR_SESSION);
    let project = query::search(
        &index,
        &s,
        &RecallQuery {
            text: "retry".into(),
            projects: vec!["tidepool".into()],
            ..RecallQuery::default()
        },
    )
    .await
    .unwrap();
    assert!(project
        .sessions
        .iter()
        .all(|x| x.session_id == TIDEPOOL_SESSION));
    let (since, until) = query::period("2026-10-28", chrono::Utc::now()).unwrap();
    let window = query::search(
        &index,
        &s,
        &RecallQuery {
            text: "panic".into(),
            since: Some(since),
            until: Some(until),
            ..RecallQuery::default()
        },
    )
    .await
    .unwrap();
    assert!(
        window
            .sessions
            .iter()
            .all(|x| x.session_id == HARBOR_SESSION),
        "{window:?}"
    );
    let entity = query::search(
        &index,
        &s,
        &RecallQuery {
            text: "fix".into(),
            entity: Some("E0502".into()),
            ..RecallQuery::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(entity.sessions.len(), 1);
    assert_eq!(entity.sessions[0].session_id, LUMEN_SESSION);

    // Scope: every session overlapping the window, unranked.
    let (since, until) = query::period("2026-10-12", chrono::Utc::now()).unwrap();
    let day = query::scope(&index, &s, Some(since), Some(until), &[], &[]).unwrap();
    assert_eq!(day.sessions.len(), 1);
    assert_eq!(day.sessions[0].session_id, LUMEN_SESSION);
    assert!(
        day.sessions[0].entities.iter().any(|e| e == "#214"),
        "{:?}",
        day.sessions[0].entities
    );
    let all = query::scope(&index, &s, None, None, &[], &[]).unwrap();
    let order: Vec<&str> = all.sessions.iter().map(|x| x.session_id.as_str()).collect();
    assert_eq!(
        order,
        vec![
            ORBIT_SESSION,
            TIDEPOOL_SESSION,
            LUMEN_SESSION,
            HARBOR_SESSION
        ],
        "oldest first"
    );

    // A kept-back output opens with its reason, never its text.
    let lumen = index
        .catalog()
        .files()
        .unwrap()
        .into_iter()
        .find(|f| f.session_id == LUMEN_SESSION)
        .unwrap();
    let opened = query::open(&index, &s, &query::receipt_id(lumen.id, 0), 10).unwrap();
    assert!(opened
        .turns
        .iter()
        .any(|t| t.kept_back.is_some() && t.text.is_empty()));
    assert!(query::open(&index, &s, "99999:0", 1).is_err());
    assert!(query::open(&index, &s, "nonsense", 1).is_err());
}

#[tokio::test]
async fn only_chosen_projects_and_tools_are_read() {
    let dir = tempfile::tempdir().unwrap();
    let secrets = Secrets::new();
    let home = home(dir.path(), &secrets);
    let index = test_index(&dir.path().join("recall")).await;

    // Off: nothing is read.
    let off = RecallSettings::default();
    let report = index
        .sync(&off, &home.roots, later(), &CONTROL)
        .await
        .unwrap();
    assert_eq!(report.indexed, 0);
    assert!(
        index.catalog().files().unwrap().is_empty(),
        "nothing is even listed"
    );

    // One project of one tool.
    let mut one = settings(&["/home/dev/lumen-cache"], &[]);
    one.tools.remove(&Tool::Codex);
    let report = index
        .sync(&one, &home.roots, later(), &CONTROL)
        .await
        .unwrap();
    assert_eq!(report.indexed, 1, "{report:?}");
    assert_eq!(report.excluded, 1, "tidepool is not chosen");
    let indexed = |index: &RecallIndex| -> Vec<String> {
        index
            .catalog()
            .files()
            .unwrap()
            .into_iter()
            .filter(|f| f.state == FileState::Indexed)
            .map(|f| f.session_id)
            .collect()
    };
    assert_eq!(indexed(&index), vec![LUMEN_SESSION.to_string()]);

    // Excluding the project removes it from the index.
    let mut none = one.clone();
    none.projects
        .insert(project_key("/home/dev/lumen-cache"), false);
    index
        .sync(&none, &home.roots, later(), &CONTROL)
        .await
        .unwrap();
    assert!(indexed(&index).is_empty());
    assert!(stored_text(&index).await.is_empty());
    let raw = index
        .engine()
        .read()
        .await
        .search("evicts one entry too early", 10)
        .await
        .unwrap();
    assert!(raw.is_empty(), "excluded chunks are deleted");

    // Turning Recall off deletes the index.
    let all = settings(&all_projects(), &[]);
    index
        .sync(&all, &home.roots, later(), &CONTROL)
        .await
        .unwrap();
    assert_eq!(indexed(&index).len(), 4);
    index
        .sync(&RecallSettings::default(), &home.roots, later(), &CONTROL)
        .await
        .unwrap();
    assert!(indexed(&index).is_empty());
    assert!(index
        .engine()
        .read()
        .await
        .search("panic retry", 10)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn removed_sessions_and_deleted_transcripts_leave_the_index() {
    let dir = tempfile::tempdir().unwrap();
    let secrets = Secrets::new();
    let s = settings(&all_projects(), &[]);
    let (index, home) = synced(dir.path(), &secrets, &s).await;

    index
        .remove_session(Tool::ClaudeCode, LUMEN_SESSION)
        .await
        .unwrap();
    let answer = query::search(
        &index,
        &s,
        &RecallQuery {
            text: "LRU eviction".into(),
            ..RecallQuery::default()
        },
    )
    .await
    .unwrap();
    assert!(answer
        .sessions
        .iter()
        .all(|x| x.session_id != LUMEN_SESSION));
    // Never indexed again.
    let report = index
        .sync(&s, &home.roots, later() + 10, &CONTROL)
        .await
        .unwrap();
    assert_eq!(report.indexed, 0, "{report:?}");
    // The other sessions' chunks still come back (the test embedder ranks everything);
    // none of the removed session's does.
    let hits = index
        .engine()
        .read()
        .await
        .search("evicts one entry too early", 50)
        .await
        .unwrap();
    assert!(!hits.is_empty(), "the other sessions are still indexed");
    assert!(hits
        .iter()
        .all(|h| !h.text.contains("evicts one entry too early")));

    // A transcript deleted from disk leaves the index and the catalog.
    std::fs::remove_file(&home.harbor).unwrap();
    let report = index
        .sync(&s, &home.roots, later() + 20, &CONTROL)
        .await
        .unwrap();
    assert_eq!(report.removed, 1);
    assert!(index
        .catalog()
        .files()
        .unwrap()
        .iter()
        .all(|f| f.session_id != HARBOR_SESSION));
    assert!(index
        .engine()
        .read()
        .await
        .search("index out of bounds layer", 10)
        .await
        .unwrap()
        .iter()
        .all(|h| !h.text.contains("out of bounds")));
}

#[tokio::test]
async fn a_live_session_waits_and_a_grown_one_re_embeds_only_what_changed() {
    let dir = tempfile::tempdir().unwrap();
    let secrets = Secrets::new();
    let s = settings(&all_projects(), &[]);
    let home = home(dir.path(), &secrets);
    let index = test_index(&dir.path().join("recall")).await;

    // Just written: not idle yet.
    let now = super::index::now_secs();
    for path in [&home.lumen, &home.tidepool, &home.orbit, &home.harbor] {
        set_mtime(path, now);
    }
    let report = index.sync(&s, &home.roots, now, &CONTROL).await.unwrap();
    assert_eq!(report.indexed, 0);
    assert_eq!(report.waiting, 4);

    let report = index
        .sync(&s, &home.roots, later(), &CONTROL)
        .await
        .unwrap();
    assert_eq!(report.indexed, 4);
    let again = index
        .sync(&s, &home.roots, later(), &CONTROL)
        .await
        .unwrap();
    assert_eq!(again.indexed, 0);
    assert_eq!(again.unchanged, 4);

    // A long session grows by one turn: only its last part is embedded again.
    let long = dir
        .path()
        .join("home/.claude/projects/-home-dev-long/1234abcd-0000-4000-8000-000000000001.jsonl");
    let line = |i: usize| {
        format!(
            "{{\"type\":\"user\",\"message\":{{\"role\":\"user\",\"content\":\"step {i} of the migration plan for the ledger tables\"}},\"timestamp\":\"2026-10-01T10:{:02}:{:02}Z\",\"cwd\":\"/home/dev/long\",\"sessionId\":\"1234abcd-0000-4000-8000-000000000001\"}}\n",
            i / 60,
            i % 60
        )
    };
    let mut text: String = (0..(super::chunks::PART_TURNS * 2 + 5)).map(line).collect();
    write(&long, &text);
    let mut s = s;
    s.projects.insert(project_key("/home/dev/long"), true);
    let first = index
        .sync(&s, &home.roots, later(), &CONTROL)
        .await
        .unwrap();
    assert_eq!(first.indexed, 1);
    assert_eq!(first.parts_embedded, 3);
    text.push_str(&line(super::chunks::PART_TURNS * 2 + 5));
    write(&long, &text);
    // A later write time.
    set_mtime(&long, now + 5);
    let grown = index
        .sync(&s, &home.roots, later(), &CONTROL)
        .await
        .unwrap();
    assert_eq!(grown.indexed, 1, "{grown:?}");
    assert_eq!(grown.parts_embedded, 1, "only the last part changed");

    // New user patterns index every session again.
    s.patterns = vec![USER_PATTERN.to_string()];
    let redo = index
        .sync(&s, &home.roots, later(), &CONTROL)
        .await
        .unwrap();
    assert_eq!(redo.indexed, 5);
}

#[tokio::test]
async fn a_sync_stops_when_cancelled_or_paused() {
    let dir = tempfile::tempdir().unwrap();
    let secrets = Secrets::new();
    let s = settings(&all_projects(), &[]);
    let home = home(dir.path(), &secrets);
    let index = test_index(&dir.path().join("recall")).await;
    fn yes() -> bool {
        true
    }
    let cancelled = SyncControl {
        cancelled: &yes,
        paused: &no_stop,
        progress: &ignore_progress,
    };
    let report = index
        .sync(&s, &home.roots, later(), &cancelled)
        .await
        .unwrap();
    assert_eq!(report.stopped, Some(super::index::Stopped::Cancelled));
    assert_eq!(report.indexed, 0);
    let paused = SyncControl {
        cancelled: &no_stop,
        paused: &yes,
        progress: &ignore_progress,
    };
    let report = index.sync(&s, &home.roots, later(), &paused).await.unwrap();
    assert_eq!(report.stopped, Some(super::index::Stopped::Paused));
    // Resumed: everything is indexed.
    let report = index
        .sync(&s, &home.roots, later(), &CONTROL)
        .await
        .unwrap();
    assert_eq!(report.indexed, 4);
}

#[test]
fn sources_and_receipts_round_trip() {
    assert_eq!(parse_source(&source_id(Tool::Codex, 42, 3)), Some((42, 3)));
    assert_eq!(parse_source("recall://codex/42"), None);
    assert_eq!(parse_source("note://x"), None);
    assert_eq!(
        query::parse_receipt(&query::receipt_id(7, 12)),
        Some((7, 12))
    );
    assert_eq!(query::parse_receipt("7-12"), None);
}

#[test]
fn periods_are_windows() {
    use chrono::TimeZone;
    let now = chrono::Utc.with_ymd_and_hms(2026, 11, 5, 15, 0, 0).unwrap(); // a Thursday
    let day = |s: i64| {
        chrono::Utc
            .timestamp_opt(s, 0)
            .unwrap()
            .format("%Y-%m-%d %H:%M")
            .to_string()
    };
    let (a, b) = query::period("last week", now).unwrap();
    assert_eq!(
        (day(a), day(b)),
        (
            "2026-10-26 00:00".to_string(),
            "2026-11-01 23:59".to_string()
        )
    );
    let (a, _) = query::period("this week", now).unwrap();
    assert_eq!(day(a), "2026-11-02 00:00");
    let (a, b) = query::period("yesterday", now).unwrap();
    assert_eq!(
        (day(a), day(b)),
        (
            "2026-11-04 00:00".to_string(),
            "2026-11-04 23:59".to_string()
        )
    );
    let (a, b) = query::period("last month", now).unwrap();
    assert_eq!(
        (day(a), day(b)),
        (
            "2026-10-01 00:00".to_string(),
            "2026-10-31 23:59".to_string()
        )
    );
    let (a, _) = query::period("last 3 days", now).unwrap();
    assert_eq!(day(a), "2026-11-02 15:00");
    assert!(query::period("whenever", now).is_none());
}

#[test]
fn settings_are_off_by_default_and_checked() {
    let dir = tempfile::tempdir().unwrap();
    let loaded = RecallSettings::load(dir.path()).unwrap();
    assert!(!loaded.enabled);
    assert!(!loaded.wants(Tool::ClaudeCode, &[project_chain("/x")]));
    let mut s = settings(&["/x"], &["/(bad/"]);
    assert!(s.save(dir.path()).is_err());
    s.patterns = vec![USER_PATTERN.to_string()];
    s.idle_minutes = 0;
    s.save(dir.path()).unwrap();
    let back = RecallSettings::load(dir.path()).unwrap();
    assert_eq!(back.idle_minutes, super::settings::MIN_IDLE_MINUTES);
    assert!(back.wants(Tool::Codex, &[project_chain("/x")]));
    assert!(
        !back.wants(Tool::Codex, &[project_chain("/y")]),
        "a project not chosen is not read"
    );
}

#[test]
fn the_home_folder_is_written_as_a_tilde() {
    assert_eq!(
        super::hide_home(
            r"read C:\Users\Ada\code\app\src\a.rs and c:/users/ada/notes",
            r"C:\Users\Ada"
        ),
        r"read ~\code\app\src\a.rs and ~/notes"
    );
    assert_eq!(super::hide_home("/home/ada/x", "/home/ada/"), "~/x");
    assert_eq!(super::hide_home("nothing here", "/"), "nothing here");
}

// ── Review round: every shape the security review found getting through ─────────────

/// Each case: the text, and what of it must not survive.
fn assert_redacted(cases: &[(String, String)]) {
    let r = Redactor::default();
    for (text, secret) in cases {
        let out = r.redact(text);
        assert!(
            !out.contains(secret.as_str()),
            "not redacted: {text}\n-> {out}"
        );
    }
}

#[test]
fn credentials_on_command_lines_and_in_urls_are_redacted() {
    let mailgun = format!(
        "key-{}",
        random(40, 32, "abcdefghijklmnopqrstuvwxyz0123456789")
    );
    let aws = "wJalrXUtnFEMIK7MDENGbPxRfiCYEXAMPLEKEY".to_string();
    assert_redacted(&[
        (
            "curl -u admin:S3cr3tPass https://es.internal:9200/_cat".into(),
            "S3cr3tPass".into(),
        ),
        (
            format!("curl --user api:{mailgun} https://api.mailgun.net/v3"),
            mailgun.clone(),
        ),
        (
            "CELERY_BROKER_URL = 'redis://:hunter2@redis:6379/0'".into(),
            "hunter2".into(),
        ),
        (
            "docker login -u me -p hunter2pass registry.acme.dev".into(),
            "hunter2pass".into(),
        ),
        (
            "sshpass -p 'hunter2pass' ssh deploy@host".into(),
            "hunter2pass".into(),
        ),
        ("mysql -uroot -phunter2pass app".into(), "hunter2pass".into()),
        (
            "gh secret set DB_PASSWORD --body \"hunter2pass\"".into(),
            "hunter2pass".into(),
        ),
        (
            format!("aws configure set aws_secret_access_key {aws}"),
            aws.clone(),
        ),
        (
            "npm config set //registry.npmjs.org/:_authToken abc123def456".into(),
            "abc123def456".into(),
        ),
        (
            "aws_secret_access_key=wJalrXUtnFEMI%2FK7MDENG%2BbPxRfiCYEXAMPLEKEY".into(),
            "K7MDENG".into(),
        ),
        (
            "https://s3.amazonaws.com/b/k?X-Amz-Security-Token=IQoJb3JpZ2luX2VjEJr%2Fabcdef&X-Amz-Signature=deadbeef1234".into(),
            "IQoJb3JpZ2luX2VjEJr".into(),
        ),
    ]);
}

#[test]
fn every_secret_name_hides_its_value_whatever_it_looks_like() {
    let hex64 = random(41, 64, "0123456789abcdef");
    let hex32 = random(42, 32, "0123456789abcdef");
    let uuid = "3fa85f64-5717-4562-b3fc-2c963f66afa6".to_string();
    assert_redacted(&[
        (
            "SMTP_PASS=abcdefghijklmnop".into(),
            "abcdefghijklmnop".into(),
        ),
        ("DB_PASS=hunter2".into(), "hunter2".into()),
        ("ADMIN_PW=correcthorse".into(), "correcthorse".into()),
        ("PASSWD=letmein".into(), "letmein".into()),
        (format!("ENCRYPTION_KEY={hex64}"), hex64.clone()),
        (format!("HEROKU_KEY={uuid}"), uuid.clone()),
        (format!("OPENWEATHER_KEY={hex32}"), hex32.clone()),
        (
            "MY_APP_TOKEN=abcdefghijklmnopqrstuvwx".into(),
            "abcdefghijklmnopqrstuvwx".into(),
        ),
        (
            "WEBHOOK_SECRET=plainwordsecret".into(),
            "plainwordsecret".into(),
        ),
        (
            "docker run -e SMTP_PASS=abcdefghijklmnop app".into(),
            "abcdefghijklmnop".into(),
        ),
        (
            "encryption_key = \"abcdefabcdefabcdef\"".into(),
            "abcdefabcdefabcdef".into(),
        ),
        ("\"password\": \"admin:admin\"".into(), "admin:admin".into()),
        (
            "\"password\": \"correct.horse.battery.staple\"".into(),
            "correct.horse.battery.staple".into(),
        ),
        ("password: \"{noop}hunter2\"".into(), "{noop}hunter2".into()),
        ("+DB_PASSWORD=fromadiffline".into(), "fromadiffline".into()),
    ]);
    // Names that hold no secret keep their values, and so do references to secrets.
    let r = Redactor::default();
    for text in [
        "KEYBOARD_LAYOUT=us",
        "max_tokens = 4096",
        "token: string;",
        "api_key = ${OPENAI_API_KEY}",
        "api_key: <your key here>",
        "password = os.environ[\"DB_PASSWORD\"]",
    ] {
        assert_eq!(r.redact(text), text, "changed: {text}");
    }
}

#[test]
fn provider_tokens_are_caught_in_every_written_form() {
    let telegram = format!(
        "{}:{}",
        random(43, 10, "0123456789"),
        random(
            44,
            35,
            "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_-"
        )
    );
    let discord = format!(
        "M{}.{}.{}",
        random(45, 23, ALNUM),
        random(46, 6, ALNUM),
        random(47, 27, ALNUM)
    );
    let xoxe = format!("xoxe-1-{}", random(48, 40, ALNUM));
    let akia = format!("AKIA{}", random(49, 16, "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567"));
    let ghp = format!("ghp_{}", random(50, 36, ALNUM));
    let head = format!("sk-ant-api03-{}", random(51, 2, ALNUM));
    let tail = random(52, 80, ALNUM);
    assert_redacted(&[
        (
            format!("curl https://api.telegram.org/bot{telegram}/getMe"),
            telegram.clone(),
        ),
        (format!("DISCORD_TOKEN: {discord}"), discord.clone()),
        (format!("refresh {xoxe}"), xoxe.clone()),
        // After an escaped line break, as JSON text holds it.
        (
            format!("{{\"content\":\"access key:\\n{akia}\\nregion us-east-1\"}}"),
            akia.clone(),
        ),
        (format!("{{\"content\":\"x\\n{ghp}\"}}"), ghp.clone()),
        // Wrapped right after its prefix.
        (format!("echo {head}\n{tail}\ndone"), tail.clone()),
    ]);
}

#[test]
fn long_random_secrets_are_caught_however_long_and_whatever_runs_they_hold() {
    let r = Redactor::default();
    for (alphabet, name) in [(BASE64, "base64"), (ALNUM, "base62")] {
        for length in [40usize, 64, 100, 300, 1700] {
            let trials = 300u64;
            let mut missed = 0;
            for seed in 0..trials {
                let secret = random(10_000 + seed * 7 + length as u64, length, alphabet);
                let out = r.redact(&format!("value {secret} end"));
                if out.contains(&secret[length / 2 - 6..length / 2 + 6]) {
                    missed += 1;
                }
            }
            assert!(
                missed * 200 <= trials,
                "{name} of {length}: {missed} of {trials} missed"
            );
        }
    }
    // A private key's base64 body at its real length, and base64-encoded PEM as
    // Kubernetes writes it.
    let body: Vec<String> = (0..26).map(|i| random(900 + i, 64, BASE64)).collect();
    let pem = format!(
        "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----",
        body.join("\n")
    );
    let out = r.redact(&pem);
    for line in &body {
        assert!(!out.contains(line.as_str()), "{line}");
    }
    let encoded = format!(
        "  ca: LS0tLS1CRUdJTiBQUklWQVRFIEtFWS0tLS0t{}",
        random(901, 1600, BASE64)
    );
    assert!(!r.redact(&encoded).contains(&encoded[60..90]));
    let kubeconfig = format!("    client-key-data: {}", random(902, 1600, BASE64));
    assert!(!r.redact(&kubeconfig).contains(&kubeconfig[40..70]));
}

#[test]
fn emails_are_hidden_unless_the_user_keeps_them() {
    let text = "Signed-off-by: Ada Lovelace <ada.lovelace@example.org>";
    let hidden = Redactor::default().redact(text);
    assert!(!hidden.contains("ada.lovelace@example.org"), "{hidden}");
    assert!(hidden.contains(super::redact::EMAIL));
    let kept = Redactor::with_options(&[], super::redact::RedactOptions { keep_emails: true })
        .unwrap()
        .redact(text);
    assert_eq!(kept, text);
}

#[test]
fn hidden_words_are_literal_and_any_case_and_never_echoed() {
    let r = Redactor::new(&["Globex Corp".to_string(), "price$.list".to_string()]).unwrap();
    let out = r.redact("Mail globex corp about the PRICE$.LIST today");
    assert!(!out.to_lowercase().contains("globex"), "{out}");
    assert!(!out.to_lowercase().contains("price$.list"), "{out}");
    let error = Redactor::new(&["fine".to_string(), "/(sekrit/".to_string()]).unwrap_err();
    assert_eq!(error.line, 2);
    assert!(!error.to_string().contains("sekrit"), "{error}");
}

#[test]
fn the_home_folder_is_hidden_in_every_spelling() {
    let home = r"C:\Users\Ada";
    for text in [
        r#"{"path":"C:\\Users\\Ada\\code\\app"}"#,
        "cd /c/Users/Ada/code/app",
        "project C--Users-Ada-code-app",
        "read C:/Users/Ada/notes",
    ] {
        let out = super::hide_home(text, home);
        assert!(!out.contains("Ada"), "{text} -> {out}");
    }
}

#[test]
fn writes_to_secret_files_and_commands_that_print_secrets_are_kept_back() {
    use serde_json::json;
    for (name, input) in [
        (
            "Write",
            json!({"file_path": "/home/u/app/.env", "content": "TOKEN=abc"}),
        ),
        (
            "Edit",
            json!({"file_path": "/home/u/app/certs/server.key", "old_string": "a", "new_string": "b"}),
        ),
        (
            "MultiEdit",
            json!({"file_path": "/home/u/.pgpass", "edits": [{"old_string": "a", "new_string": "b"}]}),
        ),
        (
            "apply_patch",
            json!("*** Begin Patch\n*** Update File: config/.env.production\n@@\n+KEY=x\n*** End Patch"),
        ),
        ("Bash", json!({"command": "printenv"})),
        ("Bash", json!({"command": "printenv AWS_SECRET_ACCESS_KEY"})),
        ("Bash", json!({"command": "env | grep -i token"})),
        ("Bash", json!({"command": "kubectl config view --raw"})),
        ("Bash", json!({"command": "gh auth token"})),
        (
            "Bash",
            json!({"command": "aws configure get aws_secret_access_key"}),
        ),
        ("Bash", json!({"command": "cat ~/.codex/auth.json"})),
        (
            "shell",
            json!({"command": ["bash", "-lc", "sudo printenv AWS_SECRET_ACCESS_KEY"]}),
        ),
    ] {
        assert!(super::claude::keeps_back(name, &input), "{name} {input}");
    }
    for (name, input) in [
        (
            "Write",
            json!({"file_path": "/home/u/app/README.md", "content": "hi"}),
        ),
        ("Bash", json!({"command": "env FOO=1 cargo test"})),
        ("Bash", json!({"command": "kubectl config view"})),
    ] {
        assert!(!super::claude::keeps_back(name, &input), "{name} {input}");
    }
}

#[test]
fn tool_inputs_are_summarized_without_json_escapes() {
    use serde_json::json;
    let akia = format!("AKIA{}", random(60, 16, "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567"));
    let input = json!({
        "file_path": "/home/u/app/notes/aws.txt",
        "content": format!("access key:\n{akia}\nregion us-east-1"),
        "edits": [{"new_string": "APP_ENV=prod\nMY_APP_TOKEN=abcdefghijklmnopqrstuvwx"}],
    });
    let summary = super::claude::summarize_input(&input);
    assert!(!summary.contains("\\n"), "{summary}");
    let out = Redactor::default().redact(&summary);
    assert!(!out.contains(&akia), "{out}");
    assert!(!out.contains("abcdefghijklmnopqrstuvwx"), "{out}");
}

const TWO_FOLDER_SESSION: &str = "aaaa1111-0000-4000-8000-000000000002";

/// A session that started in one folder and worked in another.
fn two_folder_session(dir: &Path) {
    let file = dir
        .join("home/.claude/projects/-home-dev-mono")
        .join(format!("{TWO_FOLDER_SESSION}.jsonl"));
    let line = |cwd: &str, text: &str, at: &str| {
        serde_json::json!({"type": "user", "message": {"role": "user", "content": text},
            "timestamp": at, "cwd": cwd, "sessionId": TWO_FOLDER_SESSION})
        .to_string()
    };
    let text = [
        line(
            "/home/dev/mono",
            "Start in the monorepo.",
            "2026-10-01T10:00:00Z",
        ),
        line(
            "/home/dev/clients/globex/app",
            "Now fix the invoice export of the client app.",
            "2026-10-01T10:01:00Z",
        ),
    ]
    .join("\n")
        + "\n";
    write(&file, &text);
}

#[tokio::test]
async fn a_session_that_touched_an_excluded_folder_is_not_indexed() {
    let dir = tempfile::tempdir().unwrap();
    two_folder_session(dir.path());
    let roots = Roots {
        claude_code: Some(dir.path().join("home/.claude/projects")),
        codex: None,
    };
    let index = test_index(&dir.path().join("recall")).await;
    let mut s = settings(&["/home/dev/mono"], &[]);
    // Excluding the clients folder excludes everything under it.
    s.projects.insert(project_key("/home/dev/clients"), false);
    let report = index.sync(&s, &roots, later(), &CONTROL).await.unwrap();
    assert_eq!(report.indexed, 0, "{report:?}");
    assert_eq!(report.excluded, 1, "{report:?}");
    let query = RecallQuery {
        text: "invoice export".into(),
        ..RecallQuery::default()
    };
    assert!(query::search(&index, &s, &query)
        .await
        .unwrap()
        .sessions
        .is_empty());
    // Allowed again: indexed.
    s.projects.insert(project_key("/home/dev/clients"), true);
    let report = index.sync(&s, &roots, later(), &CONTROL).await.unwrap();
    assert_eq!(report.indexed, 1, "{report:?}");
    // A worktree elsewhere is a project of its own; a subfolder follows its parent.
    assert_ne!(
        project_key("/home/wt/mono-hotfix"),
        project_key("/home/dev/mono")
    );
    assert!(
        project_chain("/home/dev/clients/globex/app").contains(&project_key("/home/dev/clients"))
    );
    // Keys are of the folder as written, so a hidden word never makes two folders one;
    // one folder spelled two ways is one project.
    assert_ne!(
        project_key("C:/clients/AcmeCorp/app"),
        project_key("C:/clients/Globex/app")
    );
    assert_eq!(
        project_key("/c/Users/Ada/app"),
        project_key("C:/Users/Ada/app")
    );
}

#[tokio::test]
async fn after_new_hidden_words_nothing_indexed_before_is_served() {
    let dir = tempfile::tempdir().unwrap();
    let secrets = Secrets::new();
    let s = settings(&all_projects(), &[]);
    let (index, home) = synced(dir.path(), &secrets, &s).await;
    let query = RecallQuery {
        text: "staging passphrase".into(),
        limit: 20,
        ..RecallQuery::default()
    };
    let answer = query::search(&index, &s, &query).await.unwrap();
    let lumen = answer
        .sessions
        .iter()
        .find(|x| x.session_id == LUMEN_SESSION)
        .expect("found before the change");
    let receipt = lumen.receipts[0].id.clone();
    let indexed = index
        .catalog()
        .files()
        .unwrap()
        .iter()
        .filter(|f| f.state == FileState::Indexed)
        .count();
    // The user hides the passphrase; no sync has run since.
    let hidden = settings(&all_projects(), &[USER_PATTERN]);
    let answer = query::search(&index, &hidden, &query).await.unwrap();
    assert!(
        answer.sessions.is_empty(),
        "stale sessions are served: {answer:?}"
    );
    assert!(query::scope(&index, &hidden, None, None, &[], &[])
        .unwrap()
        .sessions
        .is_empty());
    assert!(query::open(&index, &hidden, &receipt, 2).is_err());
    assert_eq!(query::stale_files(&index, &hidden).unwrap(), indexed);
    // Indexed again: served, without the hidden words.
    index
        .sync(&hidden, &home.roots, later(), &CONTROL)
        .await
        .unwrap();
    let answer = query::search(&index, &hidden, &query).await.unwrap();
    assert!(!answer.sessions.is_empty());
    assert!(!serde_json::to_string(&answer)
        .unwrap()
        .contains(USER_SECRET));
    assert_eq!(query::stale_files(&index, &hidden).unwrap(), 0);
}

#[tokio::test]
async fn open_reads_only_around_its_turn_and_keeps_kept_back_turns_back() {
    let dir = tempfile::tempdir().unwrap();
    let secrets = Secrets::new();
    let s = settings(&all_projects(), &[]);
    let (index, _home) = synced(dir.path(), &secrets, &s).await;
    let lumen = index
        .catalog()
        .files()
        .unwrap()
        .into_iter()
        .find(|f| f.session_id == LUMEN_SESSION)
        .unwrap();
    // Every turn opens on its own, read from its line, as reading the whole file does.
    let whole = read_transcript(
        Tool::ClaudeCode,
        Path::new(&lumen.path),
        &s.redactor().unwrap(),
        ParseBudget::default(),
    )
    .unwrap();
    assert_eq!(whole.turns.len() as u32, lumen.turns);
    let mut kept = 0;
    for turn in 0..lumen.turns {
        let one = query::open(&index, &s, &query::receipt_id(lumen.id, turn), 0).unwrap();
        assert_eq!(one.turns.len(), 1);
        let t = &one.turns[0];
        let same = &whole.turns[turn as usize];
        assert_eq!((t.turn, t.role, &t.text), (turn, same.role, &same.text));
        if t.kept_back.is_some() {
            assert!(t.text.is_empty());
            kept += 1;
        }
    }
    assert!(kept >= 2, "the .env read and its output are kept back");
    assert!(whole
        .turns
        .iter()
        .any(|t| t.text.contains("opened PR #214")));
}

#[test]
fn very_long_turns_keep_their_head_and_tail_only() {
    let text: String = "x".repeat(super::KEEP_TURN_CHARS * 3);
    let (kept, left_out) = super::keep_text(text);
    assert!(kept.chars().count() < super::KEEP_TURN_CHARS + 100);
    assert_eq!(left_out, super::KEEP_TURN_CHARS * 2);
    let short = "a short turn".to_string();
    assert_eq!(super::keep_text(short.clone()), (short, 0));
}

#[tokio::test]
async fn hard_secrets_planted_in_a_session_never_leave_it() {
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    let akia = format!("AKIA{}", random(70, 16, "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567"));
    let telegram = format!("{}:{}", random(71, 10, "0123456789"), random(72, 35, ALNUM));
    let long = random(73, 120, BASE64);
    let planted = [
        "S3cr3tPassX1".to_string(),
        "hunter2redis".to_string(),
        "abcdefghijklmnop".to_string(),
        akia.clone(),
        telegram.clone(),
        long.clone(),
        "ada.lovelace@example.org".to_string(),
    ];
    let session = "bbbb2222-0000-4000-8000-000000000003";
    let file = dir
        .path()
        .join("home/.claude/projects/-home-dev-vault")
        .join(format!("{session}.jsonl"));
    let line = |kind: &str, message: serde_json::Value| {
        json!({"type": kind, "message": message, "timestamp": "2026-10-03T10:00:00Z",
            "cwd": "/home/dev/vault", "sessionId": session})
        .to_string()
    };
    let lines = [
        line(
            "user",
            json!({"role": "user", "content": "Set up the vault deploy. I am ada.lovelace@example.org."}),
        ),
        line(
            "assistant",
            json!({"role": "assistant", "content": [{"type": "tool_use", "id": "t1", "name": "Bash",
                "input": {"command": "curl -u admin:S3cr3tPassX1 https://es.internal:9200/_cat"}}]}),
        ),
        line(
            "user",
            json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t1",
                "content": "BROKER=redis://:hunter2redis@redis:6379/0\nSMTP_PASS=abcdefghijklmnop"}]}),
        ),
        line(
            "assistant",
            json!({"role": "assistant", "content": [{"type": "tool_use", "id": "t2", "name": "Write",
                "input": {"file_path": "/home/dev/vault/notes/aws.txt",
                    "content": format!("key:\n{akia}\nbot https://api.telegram.org/bot{telegram}/getMe")}}]}),
        ),
        line(
            "user",
            json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t2",
                "content": [{"type": "json", "data": {"token": long}}]}]}),
        ),
    ];
    write(&file, &(lines.join("\n") + "\n"));
    let roots = Roots {
        claude_code: Some(dir.path().join("home/.claude/projects")),
        codex: None,
    };
    let index = test_index(&dir.path().join("recall")).await;
    let s = settings(&["/home/dev/vault"], &[]);
    let report = index.sync(&s, &roots, later(), &CONTROL).await.unwrap();
    assert_eq!(report.indexed, 1, "{report:?}");
    let stored = stored_text(&index).await;
    assert!(stored.contains("vault deploy"), "{stored}");
    let record = index.catalog().files().unwrap().remove(0);
    let mut exits = vec![stored];
    for turn in 0..record.turns {
        let opened = query::open(&index, &s, &query::receipt_id(record.id, turn), 0).unwrap();
        exits.push(serde_json::to_string(&opened).unwrap());
    }
    exits.push(
        serde_json::to_string(&query::scope(&index, &s, None, None, &[], &[]).unwrap()).unwrap(),
    );
    exits.push(format!(
        "{:?}",
        index.catalog().file_entities(record.id, &[], 1000).unwrap()
    ));
    exits.push(format!("{:?}", index.catalog().files().unwrap()));
    for secret in &planted {
        let query = RecallQuery {
            text: secret.clone(),
            limit: 20,
            ..RecallQuery::default()
        };
        let answer = query::search(&index, &s, &query).await.unwrap();
        exits.push(serde_json::to_string(&answer).unwrap());
    }
    for text in &exits {
        for secret in &planted {
            assert!(
                !text.contains(secret.as_str()),
                "{secret} left the session:\n{text}"
            );
        }
    }
}
