//! Recall's scorecard: a synthetic set of coding sessions in the formats the tools write
//! (Claude Code JSONL, Codex v1 and v2 rollouts), and a frozen question set over them
//! (`data/recall/questions.json`), measuring what Recall promises: the right session in
//! the top 5, and how long a search takes.
//!
//! The sessions are generated, never taken from anyone's transcripts: 28 tasks across six
//! made-up projects, each told the way a coding session goes (a prompt, the files read,
//! commands with long outputs, a diagnosis, a fix, a PR), with the noise real transcripts
//! carry (request records, attachments, thinking, UI events, whole-suite test listings
//! that name other sessions' tests). Questions are about half paraphrases of the problem
//! and half the ids and error messages a user remembers.
//!
//! Target, as Parcha's recall: at least 90% of questions find their session in the top
//! 5, and the 95th percentile of search time is under 2 seconds.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use shodh_rag::recall::discover::{project_key, Roots};
use shodh_rag::recall::index::{RecallIndex, SyncControl, SyncProgress};
use shodh_rag::recall::query::{self, RecallQuery};
use shodh_rag::recall::settings::RecallSettings;
use shodh_rag::recall::Tool;

/// The target share of questions whose session is in the top 5.
pub const TARGET_HIT_AT_5: f64 = 0.90;
/// The target 95th percentile of search time.
pub const TARGET_P95_MS: f64 = 2000.0;

/// The committed questions.
pub const QUESTIONS: &str = include_str!("../data/recall/questions.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Writer {
    Claude,
    CodexFlat,
    CodexEnvelope,
}

#[derive(Debug, Clone, Copy)]
struct Project {
    name: &'static str,
    language: Language,
    /// Identifiers the project's code uses (for the files read and the test listings).
    vocabulary: &'static [&'static str],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Language {
    Rust,
    TypeScript,
    Go,
    Python,
}

const LUMEN: Project = Project {
    name: "lumen-cache",
    language: Language::Rust,
    vocabulary: &[
        "insert",
        "get",
        "evict_if_full",
        "touch",
        "shard_for",
        "snapshot",
        "expires_at",
        "capacity",
        "LruOrder",
        "Entry",
    ],
};
const ORBIT: Project = Project {
    name: "orbit-web",
    language: Language::TypeScript,
    vocabulary: &[
        "login",
        "useSession",
        "Dashboard",
        "ThemeProvider",
        "deviceFlow",
        "checkout",
        "formatDate",
        "apiClient",
        "routes",
        "PaymentForm",
    ],
};
const TIDEPOOL: Project = Project {
    name: "tidepool",
    language: Language::Go,
    vocabulary: &[
        "SendWebhook",
        "VerifySignature",
        "Deliver",
        "Worker",
        "DeadLetter",
        "Replay",
        "Pool",
        "History",
        "Endpoint",
        "Backoff",
    ],
};
const HARBOR: Project = Project {
    name: "harbor-cli",
    language: Language::Rust,
    vocabulary: &[
        "push",
        "pull",
        "login",
        "Layer",
        "Manifest",
        "BlobStore",
        "Credentials",
        "BuildContext",
        "Registry",
        "Digest",
    ],
};
const QUILL: Project = Project {
    name: "quill-notes",
    language: Language::Python,
    vocabulary: &[
        "create_note",
        "list_notes",
        "search_notes",
        "Tag",
        "Reminder",
        "RateLimiter",
        "get_db",
        "NoteOut",
        "schedule",
        "User",
    ],
};
const LEDGER: Project = Project {
    name: "ledger-sync",
    language: Language::Python,
    vocabulary: &[
        "import_statement",
        "BankMapping",
        "Transaction",
        "fingerprint",
        "load_transactions",
        "fx_rate",
        "Account",
        "backfill",
        "dedupe",
        "Ledger",
    ],
};

/// One task, told as a session.
#[derive(Debug, Clone, Copy)]
struct Story {
    key: &'static str,
    project: Project,
    writer: Writer,
    /// `YYYY-MM-DD`.
    day: &'static str,
    branch: &'static str,
    title: &'static str,
    prompt: &'static str,
    file: &'static str,
    command: &'static str,
    /// The line of the command's output that shows the problem.
    symptom: &'static str,
    diagnosis: &'static str,
    fix: &'static str,
    pr: u32,
}

const STORIES: [Story; 28] = [
    Story { key: "L1", project: LUMEN, writer: Writer::Claude, day: "2026-09-02", branch: "feat/ttl-expiry", title: "Add per-entry TTL expiry",
        prompt: "Entries in lumen-cache should expire after a per-entry time to live. Add a ttl argument to insert and make get skip expired entries.",
        file: "src/entry.rs", command: "cargo test ttl",
        symptom: "test ttl::expired_entry_is_not_returned ... FAILED\nassertion `left == right` failed\n  left: Some(7)\n right: None",
        diagnosis: "get compared Instant::now() against the insertion time without adding the ttl, so nothing ever expired.",
        fix: "store expires_at = inserted + ttl in Entry and check it in get; expired entries are removed lazily on access", pr: 188 },
    Story { key: "L2", project: LUMEN, writer: Writer::Claude, day: "2026-09-15", branch: "perf/shard-locks", title: "Shard the cache lock",
        prompt: "Benchmarks show lumen-cache throughput collapses with 16 threads. Can we reduce lock contention?",
        file: "src/shard.rs", command: "cargo bench --bench concurrent",
        symptom: "concurrent/16_threads   time:   [41.2 ms 41.9 ms 42.7 ms]\n                        thrpt:  [2.1 Melem/s]",
        diagnosis: "every get takes the single global Mutex, even plain reads, so with 16 threads they queue on it.",
        fix: "split the map into 32 shards keyed by the hash, each behind its own RwLock; throughput went from 2.1M to 9.8M ops/s", pr: 195 },
    Story { key: "L3", project: LUMEN, writer: Writer::Claude, day: "2026-10-12", branch: "fix/eviction-off-by-one", title: "Fix LRU eviction off-by-one",
        prompt: "lumen-cache evicts an entry while it still has one free slot. Find out why.",
        file: "src/lru.rs", command: "cargo test -p lumen-cache lru",
        symptom: "test lru::evicts_at_capacity ... FAILED\nthread 'lru::evicts_at_capacity' panicked at src/lru.rs:141:9:\nexpected 4 entries, found 3",
        diagnosis: "evict_if_full compares len() >= capacity before inserting, so a full cache drops an entry it still has room for.",
        fix: "compare with > and only after checking whether the key is already present", pr: 214 },
    Story { key: "L4", project: LUMEN, writer: Writer::CodexEnvelope, day: "2026-10-20", branch: "chore/serde-feature", title: "Make serde optional",
        prompt: "Put the serde derives in lumen-cache behind an optional feature so embedded users don't pull serde.",
        file: "Cargo.toml", command: "cargo build --no-default-features",
        symptom: "error[E0433]: failed to resolve: use of undeclared crate or module `serde`\n --> src/snapshot.rs:3:5",
        diagnosis: "src/snapshot.rs imported serde unconditionally, so a build without the default features could not resolve it.",
        fix: "gate snapshot.rs with #[cfg(feature = \"serde\")] and declare serde as an optional dependency behind the serde feature", pr: 221 },
    Story { key: "L5", project: LUMEN, writer: Writer::Claude, day: "2026-11-03", branch: "docs/readme-examples", title: "Fix README examples",
        prompt: "The README examples for lumen-cache don't compile anymore. Fix them and make sure they keep compiling.",
        file: "README.md", command: "cargo test --doc",
        symptom: "error[E0061]: this method takes 3 arguments but 2 arguments were supplied\n  --> README.md:14:7",
        diagnosis: "insert gained the ttl argument but the README still called insert(key, value).",
        fix: "updated the examples to pass Duration::from_secs(60) and included README.md as crate docs so the examples run as doctests", pr: 230 },
    Story { key: "O1", project: ORBIT, writer: Writer::CodexFlat, day: "2026-08-20", branch: "feat/orbit-oauth", title: "OAuth device flow login",
        prompt: "Add the OAuth device flow login to orbit-web, for people signing in on a TV.",
        file: "src/auth/deviceFlow.ts", command: "npm test -- deviceFlow",
        symptom: "FAIL src/auth/deviceFlow.test.ts\n  TypeError: pollInterval is not a function",
        diagnosis: "the device flow passed the interval number where a callback was expected.",
        fix: "poll /oauth/device/token every interval seconds and back off on slow_down responses", pr: 77 },
    Story { key: "O2", project: ORBIT, writer: Writer::Claude, day: "2026-09-08", branch: "fix/hydration-mismatch", title: "Dashboard hydration mismatch",
        prompt: "orbit-web shows a hydration warning on the dashboard right after login and the last-login date flickers.",
        file: "src/pages/Dashboard.tsx", command: "npm run build && npm run start",
        symptom: "Warning: Text content did not match. Server: \"9/8/2026\" Client: \"08/09/2026\"",
        diagnosis: "the last-login date was formatted with toLocaleDateString on the server and in the browser, which use different locales.",
        fix: "format dates in UTC with one fixed en-GB formatter and render the relative time only on the client in useEffect", pr: 91 },
    Story { key: "O3", project: ORBIT, writer: Writer::Claude, day: "2026-09-25", branch: "feat/dark-mode", title: "Dark mode toggle",
        prompt: "Add a dark mode toggle to orbit-web that follows the system setting until the user picks one.",
        file: "src/theme/ThemeProvider.tsx", command: "npm test -- ThemeProvider",
        symptom: "expect(received).toBe(expected)\n    Expected: \"dark\"\n    Received: \"light\"",
        diagnosis: "the matchMedia listener was registered after the first render, so the initial system preference was missed.",
        fix: "read prefers-color-scheme synchronously for the initial state and store the user's choice in localStorage under orbit.theme", pr: 104 },
    Story { key: "O4", project: ORBIT, writer: Writer::CodexEnvelope, day: "2026-10-07", branch: "fix/bundle-size", title: "Bundle size regression",
        prompt: "orbit-web's main bundle grew from 310 kB to 1.2 MB in the last release. Find what pulled it in.",
        file: "src/components/DatePicker.tsx", command: "npx vite-bundle-visualizer --open false",
        symptom: "moment-timezone        812.4 kB  (data/packed/latest.json: every zone)",
        diagnosis: "the date picker imported moment-timezone, which brings the data for every time zone.",
        fix: "replaced it with date-fns-tz and lazy-loaded the picker; the main bundle is back to 298 kB", pr: 118 },
    Story { key: "O5", project: ORBIT, writer: Writer::Claude, day: "2026-10-30", branch: "test/flaky-e2e", title: "Flaky checkout e2e test",
        prompt: "The Playwright checkout test in orbit-web fails about one run in five on CI.",
        file: "e2e/checkout.spec.ts", command: "npx playwright test checkout --repeat-each 20",
        symptom: "TimeoutError: locator.click: Timeout 5000ms exceeded.\n  waiting for getByRole('button', { name: 'Pay now' })",
        diagnosis: "the Pay now button stays disabled until the Stripe iframe reports ready, and the test clicked before that event.",
        fix: "wait for the payment-ready test id instead of a fixed timeout; 20 of 20 runs pass", pr: 131 },
    Story { key: "T1", project: TIDEPOOL, writer: Writer::Claude, day: "2026-09-03", branch: "feat/webhook-backoff", title: "Webhook retries with backoff",
        prompt: "Add exponential backoff with jitter to the webhook sender in tidepool.",
        file: "internal/notify/sender.go", command: "go test ./internal/notify/...",
        symptom: "--- FAIL: TestSendRetries (0.20s)\n    sender_test.go:41: got 1 attempts, want 4",
        diagnosis: "SendWebhook returned on the first 503 without retrying.",
        fix: "a retry loop with base 200ms, factor 2, full jitter and a 30s cap around SendWebhook", pr: 52 },
    Story { key: "T2", project: TIDEPOOL, writer: Writer::CodexFlat, day: "2026-09-19", branch: "fix/signature-verify", title: "Shopify HMAC signatures rejected",
        prompt: "Customers say tidepool rejects valid webhook signatures from Shopify.",
        file: "internal/verify/hmac.go", command: "go test ./internal/verify/...",
        symptom: "--- FAIL: TestVerifyShopify (0.00s)\n    hmac_test.go:22: signature mismatch",
        diagnosis: "we hashed the parsed and re-encoded JSON body instead of the raw request bytes, so key order and spacing changed.",
        fix: "read the raw body once with io.ReadAll and verify the HMAC-SHA256 over it before decoding", pr: 61 },
    Story { key: "T3", project: TIDEPOOL, writer: Writer::Claude, day: "2026-10-02", branch: "feat/dead-letter", title: "Dead letter queue for failed webhooks",
        prompt: "When a webhook keeps failing after all retries, tidepool just drops it. Keep them somewhere so we can replay them.",
        file: "internal/queue/deadletter.go", command: "go test -race ./internal/queue/...",
        symptom: "panic: send on closed channel\n\ngoroutine 37 [running]:\ntidepool/internal/queue.(*Worker).finish",
        diagnosis: "the worker closed the results channel while retries were still in flight.",
        fix: "a dead_letters Postgres table with a POST /admin/dead-letters/{id}/replay endpoint, and the channel closes only after the WaitGroup is done", pr: 70 },
    Story { key: "T4", project: TIDEPOOL, writer: Writer::Claude, day: "2026-10-16", branch: "perf/pgx-pool", title: "Connection pool exhaustion",
        prompt: "tidepool logs 'failed to acquire connection: context deadline exceeded' under load.",
        file: "internal/store/history.go", command: "go tool pprof -top http://localhost:6060/debug/pprof/goroutine",
        symptom: "1203 goroutines blocked in github.com/jackc/pgx/v5/pgxpool.(*Pool).Acquire",
        diagnosis: "rows from the delivery history query were never closed, so every request leaked a pooled connection.",
        fix: "defer rows.Close() in History, MaxConns 40 and a health check period of 30s", pr: 79 },
    Story { key: "T5", project: TIDEPOOL, writer: Writer::CodexEnvelope, day: "2026-11-04", branch: "chore/go-1-23", title: "Upgrade to Go 1.23",
        prompt: "Upgrade tidepool to Go 1.23 and fix whatever breaks.",
        file: "go.mod", command: "go vet ./...",
        symptom: "internal/notify/sender.go:88:2: loop variable d captured by func literal",
        diagnosis: "with the new per-iteration loop variables the old copies were redundant, and vet flagged one capture left in a goroutine.",
        fix: "removed the d := d copies, bumped go.mod and the CI image to golang:1.23", pr: 85 },
    Story { key: "H1", project: HARBOR, writer: Writer::CodexEnvelope, day: "2026-10-28", branch: "fix/harbor-retry-panic", title: "push panics on a 429",
        prompt: "harbor push panics with index out of bounds when the registry returns 429. Make it retry.",
        file: "src/push.rs", command: "cargo run -- push ghcr.io/acme/app:1.2",
        symptom: "thread 'main' panicked at src/push.rs:141:27:\nindex out of bounds: the len is 0 but the index is 0",
        diagnosis: "after a 429 the layer list came back empty and push indexed it anyway.",
        fix: "push retries 429 responses after the Retry-After header and returns PushError::Empty instead of panicking", pr: 41 },
    Story { key: "H2", project: HARBOR, writer: Writer::Claude, day: "2026-09-11", branch: "feat/layer-dedupe", title: "Skip layers the registry has",
        prompt: "harbor push uploads every layer even when the registry already has it. Check before uploading.",
        file: "src/blob.rs", command: "cargo run -- push ghcr.io/acme/app:1.1 -v",
        symptom: "PUT /v2/acme/app/blobs/uploads/ 202 (layer 3 of 7, 48.2 MB) - uploaded again",
        diagnosis: "we never sent the HEAD blob request because the digest was computed after compression.",
        fix: "compute the digest of the compressed layer first, HEAD /v2/<name>/blobs/<digest>, and mount from another repository when the registry supports cross-repo mounts", pr: 33 },
    Story { key: "H3", project: HARBOR, writer: Writer::Claude, day: "2026-10-05", branch: "feat/keychain-login", title: "Credentials in the OS keychain",
        prompt: "harbor login writes the registry password to its config file in plain text. Use the OS keychain instead.",
        file: "src/auth/store.rs", command: "cargo test auth",
        symptom: "Error: PlatformFailure(\"no secret service available\")",
        diagnosis: "the Linux CI runner has no Secret Service, so the keyring crate failed there.",
        fix: "use the keyring crate on macOS and Windows and fall back to an encrypted file only when no keychain exists, with a warning", pr: 37 },
    Story { key: "H4", project: HARBOR, writer: Writer::Claude, day: "2026-10-22", branch: "fix/windows-paths", title: "Windows paths in the build context",
        prompt: "harbor build fails on Windows when the build context has subfolders.",
        file: "src/build/context.rs", command: "cargo run -- build . -t app:dev",
        symptom: "Error: invalid tar header: path contains backslash: \"assets\\\\img\\\\logo.png\"",
        diagnosis: "the tar entries used Path::display(), which writes backslashes on Windows.",
        fix: "convert path components to forward slashes when writing tar headers, with a test of nested folders", pr: 39 },
    Story { key: "H5", project: HARBOR, writer: Writer::CodexFlat, day: "2026-11-06", branch: "docs/man-pages", title: "Generate man pages",
        prompt: "Generate man pages for harbor from the clap definitions and ship them in the release.",
        file: "xtask/src/main.rs", command: "cargo xtask gen-man",
        symptom: "wrote target/man/harbor.1\nwrote target/man/harbor-push.1",
        diagnosis: "clap_mangen renders one page per subcommand from the existing Command definitions.",
        fix: "an xtask gen-man command, and the release workflow packages harbor.1 and harbor-push.1 under share/man/man1", pr: 44 },
    Story { key: "Q1", project: QUILL, writer: Writer::Claude, day: "2026-08-28", branch: "feat/fulltext-search", title: "Full-text search for notes",
        prompt: "Add full-text search to quill-notes. Notes are stored in Postgres.",
        file: "app/search.py", command: "pytest tests/test_search.py",
        symptom: "psycopg.errors.UndefinedFunction: function to_tsvector(unknown, json) does not exist",
        diagnosis: "the body column is JSON, so to_tsvector needed the text extracted first.",
        fix: "a generated tsvector column over body->>'text' with a GIN index, queried with websearch_to_tsquery", pr: 12 },
    Story { key: "Q2", project: QUILL, writer: Writer::Claude, day: "2026-09-22", branch: "fix/n-plus-one", title: "Slow notes list endpoint",
        prompt: "GET /notes takes 4 seconds in quill-notes for users with many tags.",
        file: "app/routes/notes.py", command: "pytest -k list_notes --durations=5",
        symptom: "SQL queries: 1201 for 600 notes\n4.12s call     tests/test_notes.py::test_list_notes_many_tags",
        diagnosis: "an N+1 query: each note loaded its tags lazily.",
        fix: "selectinload(Note.tags) and a composite index on note_tags(note_id, tag_id); 1201 queries down to 3 and 4.1 s down to 120 ms", pr: 19 },
    Story { key: "Q3", project: QUILL, writer: Writer::CodexEnvelope, day: "2026-10-09", branch: "feat/rate-limit", title: "Per-user API rate limit",
        prompt: "quill-notes needs per-user rate limiting on the API, 100 requests a minute.",
        file: "app/middleware/ratelimit.py", command: "pytest tests/test_ratelimit.py",
        symptom: "AssertionError: assert 200 == 429",
        diagnosis: "the sliding window was keyed on the client IP, and behind the proxy every user shared the load balancer's address.",
        fix: "key the Redis sliding window on the authenticated user id and trust X-Forwarded-For only from the proxy", pr: 26 },
    Story { key: "Q4", project: QUILL, writer: Writer::Claude, day: "2026-10-25", branch: "fix/timezone-reminders", title: "Reminders at the wrong hour",
        prompt: "Users in India get their quill-notes reminders five and a half hours late.",
        file: "app/reminders.py", command: "pytest tests/test_reminders.py",
        symptom: "AssertionError: assert reminder.fire_at.hour == 9\n  where 14 = datetime(2026, 10, 25, 14, 30).hour",
        diagnosis: "fire_at was stored as a naive datetime and compared with UTC now.",
        fix: "store timezone-aware datetimes with zoneinfo in the user's time zone, and migrate existing rows assuming UTC", pr: 33 },
    Story { key: "G1", project: LEDGER, writer: Writer::Claude, day: "2026-09-05", branch: "feat/bank-import", title: "Bank statement CSV import",
        prompt: "Import bank statement CSVs into ledger-sync. Each bank uses different columns.",
        file: "ledger_sync/importers/csv_mapping.py", command: "pytest tests/test_import.py",
        symptom: "ValueError: could not convert string to float: '1.234,56'",
        diagnosis: "the German bank writes comma decimals and dot thousands.",
        fix: "per-bank column mappings in banks.yaml with a decimal separator setting, parsed with Decimal", pr: 8 },
    Story { key: "G2", project: LEDGER, writer: Writer::CodexFlat, day: "2026-09-29", branch: "fix/duplicate-transactions", title: "Duplicates after re-import",
        prompt: "Re-importing the same statement in ledger-sync creates duplicate transactions.",
        file: "ledger_sync/dedupe.py", command: "pytest tests/test_dedupe.py",
        symptom: "AssertionError: 24 != 12",
        diagnosis: "there was no natural key, and amounts and dates repeat for recurring payments.",
        fix: "a fingerprint of account, booking date, amount, counterparty and the bank's reference, with a unique index and ON CONFLICT DO NOTHING", pr: 15 },
    Story { key: "G3", project: LEDGER, writer: Writer::Claude, day: "2026-10-14", branch: "perf/airflow-backfill", title: "Backfill takes all night",
        prompt: "The ledger-sync Airflow backfill for 2024 takes nine hours.",
        file: "dags/ledger_daily.py", command: "airflow tasks test ledger_daily load_transactions 2024-03-01",
        symptom: "INSERT 0 1\nINSERT 0 1\nINSERT 0 1\n... 48000 single-row inserts for one day",
        diagnosis: "load_transactions inserted rows one at a time in a loop.",
        fix: "COPY from an in-memory CSV buffer in batches of 10 000 and run days in parallel with max_active_runs 8; nine hours down to 35 minutes", pr: 22 },
    Story { key: "G4", project: LEDGER, writer: Writer::Claude, day: "2026-11-01", branch: "feat/fx-rates", title: "Currency conversion to EUR",
        prompt: "ledger-sync should report everything in EUR. Add currency conversion with daily ECB rates.",
        file: "ledger_sync/fx.py", command: "pytest tests/test_fx.py",
        symptom: "KeyError: datetime.date(2026, 10, 31)",
        diagnosis: "the ECB publishes no rates on weekends and holidays.",
        fix: "use the last rate published on or before the booking date, cached in an fx_rates table", pr: 29 },
];

/// A frozen question.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Question {
    pub id: String,
    pub question: String,
    /// The story (session) that answers it.
    pub story: String,
}

/// The frozen questions.
pub fn questions() -> Result<Vec<Question>> {
    serde_json::from_str(QUESTIONS).context("data/recall/questions.json")
}

/// A story's session id: a UUID from its key.
fn session_id(key: &str) -> String {
    let d = Sha256::digest(format!("shodh-recall-eval/{key}").as_bytes());
    let h: String = d.iter().take(16).map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-4{}-8{}-{}",
        &h[0..8],
        &h[8..12],
        &h[13..16],
        &h[17..20],
        &h[20..32]
    )
}

fn cwd(project: &Project) -> String {
    format!("/home/dev/{}", project.name)
}

/// A small deterministic number from a text.
fn seed(text: &str) -> u64 {
    let d = Sha256::digest(text.as_bytes());
    u64::from_le_bytes([d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]])
}

/// What a file the session reads looks like: code in the project's language, with its
/// vocabulary, long enough to be cut to head and tail.
fn code_file(story: &Story) -> String {
    let p = &story.project;
    let mut s = seed(story.key);
    let mut lines = Vec::new();
    for i in 0..60 {
        s = s
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let a = p.vocabulary[(s >> 33) as usize % p.vocabulary.len()];
        let b = p.vocabulary[(s >> 41) as usize % p.vocabulary.len()];
        let line = match p.language {
            Language::Rust => format!("    pub fn {}_{i}(&mut self, key: &K) -> Option<{b}> {{ self.{a}(key).map(|v| v.clone()) }}", a.to_lowercase()),
            Language::TypeScript => format!("export function {a}{i}(input: {b}Props): Promise<void> {{ return {b}.call(input); }}"),
            Language::Go => format!("func (w *{b}) {a}{i}(ctx context.Context) error {{ return w.{a}(ctx) }}"),
            Language::Python => format!("def {}_{i}(db: Session, item: {b}) -> {b}:\n    return {a}(db, item)", a.to_lowercase()),
        };
        lines.push(format!("{:>4}\t{line}", i + 1));
    }
    lines.join("\n")
}

/// Every test of the project, as a whole-suite run lists them: other sessions' tests
/// appear in every session's output, as in real transcripts.
fn suite_listing(project: &Project) -> String {
    let mut lines = Vec::new();
    for story in STORIES.iter().filter(|s| s.project.name == project.name) {
        let name = story
            .branch
            .rsplit('/')
            .next()
            .unwrap_or(story.branch)
            .replace('-', "_");
        lines.push(match project.language {
            Language::Rust => format!("test {name}::works ... ok"),
            Language::TypeScript => format!("  ✓ {name} renders (12 ms)"),
            Language::Go => format!("=== RUN   Test_{name}\n--- PASS: Test_{name} (0.01s)"),
            Language::Python => format!("tests/test_{name}.py::test_{name} PASSED"),
        });
    }
    lines.join("\n")
}

/// The noise a build prints before anything that matters.
fn build_noise(project: &Project) -> String {
    let deps = [
        "serde",
        "tokio",
        "regex",
        "clap",
        "tracing",
        "anyhow",
        "thiserror",
        "hashbrown",
        "parking_lot",
        "bytes",
    ];
    match project.language {
        Language::Rust => deps
            .iter()
            .enumerate()
            .map(|(i, d)| format!("   Compiling {d} v1.{i}.{}", i * 3))
            .chain(std::iter::once(format!("   Compiling {} v0.4.2 ({})", project.name, cwd(project))))
            .collect::<Vec<_>>()
            .join("\n"),
        Language::TypeScript => "> orbit-web@2.3.0 test\n> vitest run\n\n RUN  v2.1.4 /home/dev/orbit-web\n".to_string(),
        Language::Go => "go: downloading github.com/jackc/pgx/v5 v5.7.1\ngo: downloading golang.org/x/sync v0.8.0\n".to_string(),
        Language::Python => "============================= test session starts ==============================\nplatform linux -- Python 3.12.6, pytest-8.3.3\nrootdir: /home/dev\ncollected 48 items\n".to_string(),
    }
}

/// One step of a session.
enum Step {
    User(String),
    Assistant(String),
    /// A tool call: its name and input.
    Call(&'static str, Value),
    Output(String),
}

fn steps(story: &Story, tool: Writer) -> Vec<Step> {
    let codex = tool != Writer::Claude;
    let read = |path: &str| -> (&'static str, Value) {
        if codex {
            (
                "shell",
                json!({ "command": ["bash", "-lc", format!("sed -n '1,200p' {path}")], "workdir": cwd(&story.project) }),
            )
        } else {
            (
                "Read",
                json!({ "file_path": format!("{}/{path}", cwd(&story.project)) }),
            )
        }
    };
    let run = |command: &str| -> (&'static str, Value) {
        if codex {
            (
                "shell",
                json!({ "command": ["bash", "-lc", command], "workdir": cwd(&story.project), "timeout_ms": 120000 }),
            )
        } else {
            (
                "Bash",
                json!({ "command": command, "description": "Run it" }),
            )
        }
    };
    let edit = |path: &str| -> (&'static str, Value) {
        if codex {
            ("apply_patch", Value::String(format!("*** Begin Patch\n*** Update File: {path}\n@@\n-    // before\n+    // after: {}\n*** End Patch", story.fix)))
        } else {
            (
                "Edit",
                json!({ "file_path": format!("{}/{path}", cwd(&story.project)), "old_string": "// before", "new_string": format!("// after: {}", story.fix) }),
            )
        }
    };
    let (read_name, read_input) = read(story.file);
    let (run_name, run_input) = run(story.command);
    let (edit_name, edit_input) = edit(story.file);
    let failing = format!(
        "{}\n{}\n{}\n\nfailures:\n{}\n",
        build_noise(&story.project),
        suite_listing(&story.project),
        "running the affected tests",
        story.symptom
    );
    let passing = format!(
        "{}\n{}\nall checks passed",
        build_noise(&story.project),
        suite_listing(&story.project)
    );
    vec![
        Step::User(story.prompt.to_string()),
        Step::Assistant(format!(
            "I'll read {} first and then run `{}` to see the problem.",
            story.file, story.command
        )),
        Step::Call(read_name, read_input),
        Step::Output(code_file(story)),
        Step::Call(run_name, run_input.clone()),
        Step::Output(failing),
        Step::Assistant(format!("Found it: {}", story.diagnosis)),
        Step::Call(edit_name, edit_input),
        Step::Output(format!("The file {} has been updated.", story.file)),
        Step::Call(run_name, run_input),
        Step::Output(passing),
        Step::Assistant(format!(
            "Done: {}. Pushed {} and opened PR #{}.",
            story.fix, story.branch, story.pr
        )),
        Step::User("Thanks. Merge it once CI is green.".to_string()),
        Step::Assistant(format!("CI is green; merged PR #{} into main.", story.pr)),
    ]
}

/// The session's start, and each step 40 seconds after the one before.
fn time_of(story: &Story, step: usize) -> String {
    let hour = 9 + (seed(story.key) % 8);
    let secs = step * 40;
    format!(
        "{}T{:02}:{:02}:{:02}.000Z",
        story.day,
        hour,
        secs / 60,
        secs % 60
    )
}

fn claude_lines(story: &Story) -> Vec<String> {
    let id = session_id(story.key);
    let dir = cwd(&story.project);
    let mut lines = vec![
        json!({ "type": "permission-mode", "permissionMode": "default", "sessionId": id })
            .to_string(),
    ];
    let base = |kind: &str, at: String| {
        json!({ "parentUuid": null, "isSidechain": false, "type": kind, "uuid": format!("{}-{at}", story.key), "timestamp": at,
            "userType": "external", "entrypoint": "cli", "cwd": dir, "sessionId": id, "version": "2.1.312", "gitBranch": story.branch })
    };
    let mut call = 0;
    for (n, step) in steps(story, Writer::Claude).into_iter().enumerate() {
        let at = time_of(story, n);
        let mut line = base(
            match step {
                Step::User(_) | Step::Output(_) => "user",
                _ => "assistant",
            },
            at.clone(),
        );
        match step {
            Step::User(text) => {
                line["message"] = json!({ "role": "user", "content": text });
                lines.push(line.to_string());
                let mut attachment = base("attachment", at.clone());
                attachment["attachment"] = json!({ "type": "todo", "content": [] });
                lines.push(attachment.to_string());
                continue;
            }
            Step::Assistant(text) => {
                // A thinking block first, never indexed.
                let mut thinking = base("assistant", at.clone());
                thinking["message"] = json!({ "id": format!("msg_{n}"), "role": "assistant", "model": "claude-opus-4-1",
                    "content": [{ "type": "thinking", "thinking": format!("Considering {}", story.title), "signature": "s" }] });
                lines.push(thinking.to_string());
                line["message"] = json!({ "id": format!("msg_{n}"), "role": "assistant", "model": "claude-opus-4-1", "content": [{ "type": "text", "text": text }] });
            }
            Step::Call(name, input) => {
                call += 1;
                line["message"] = json!({ "id": format!("msg_{n}"), "role": "assistant", "model": "claude-opus-4-1",
                    "content": [{ "type": "tool_use", "id": format!("toolu_{call}"), "name": name, "input": input }] });
            }
            Step::Output(text) => {
                line["message"] = json!({ "role": "user", "content": [{ "tool_use_id": format!("toolu_{call}"), "type": "tool_result", "content": text }] });
                line["toolUseResult"] = json!({ "stdout": "(repeated)", "stderr": "" });
            }
        }
        lines.push(line.to_string());
        lines.push(json!({ "type": "api-request", "sessionId": id, "id": format!("r{n}"), "timestamp": at, "params": { "model": "claude-opus-4-1" } }).to_string());
    }
    lines.push(json!({ "type": "ai-title", "aiTitle": story.title, "sessionId": id }).to_string());
    lines.push(json!({ "type": "pr-link", "sessionId": id, "prNumber": story.pr, "prUrl": format!("https://github.com/acme/{}/pull/{}", story.project.name, story.pr), "prRepository": format!("acme/{}", story.project.name) }).to_string());
    lines
}

fn codex_input_value(input: &Value) -> Value {
    match input {
        Value::String(_) => input.clone(),
        other => Value::String(other.to_string()),
    }
}

fn codex_envelope_lines(story: &Story) -> Vec<String> {
    let id = session_id(story.key);
    let dir = cwd(&story.project);
    let mut lines = vec![
        json!({ "timestamp": time_of(story, 0), "type": "session_meta", "payload": { "id": id, "timestamp": time_of(story, 0), "cwd": dir,
            "originator": "codex_cli_rs", "cli_version": "0.48.0", "instructions": null, "source": "cli", "model_provider": "openai",
            "git": { "commit_hash": format!("{:040x}", seed(story.key) as u128 * 7919), "branch": story.branch, "repository_url": format!("https://github.com/acme/{}.git", story.project.name) } } }).to_string(),
        json!({ "timestamp": time_of(story, 0), "type": "response_item", "payload": { "type": "message", "role": "user",
            "content": [{ "type": "input_text", "text": format!("<environment_context>\n  <cwd>{dir}</cwd>\n</environment_context>") }] } }).to_string(),
        json!({ "timestamp": time_of(story, 0), "type": "turn_context", "payload": { "cwd": dir, "approval_policy": "on-request", "model": "gpt-5-codex", "summary": "auto" } }).to_string(),
    ];
    let mut call = 0;
    for (n, step) in steps(story, Writer::CodexEnvelope).into_iter().enumerate() {
        let at = time_of(story, n);
        let item = match step {
            Step::User(text) => {
                lines.push(json!({ "timestamp": at, "type": "event_msg", "payload": { "type": "user_message", "message": text } }).to_string());
                json!({ "type": "message", "role": "user", "content": [{ "type": "input_text", "text": text }] })
            }
            Step::Assistant(text) => {
                lines.push(json!({ "timestamp": at, "type": "response_item", "payload": { "type": "reasoning", "summary": [], "content": null, "encrypted_content": "gAAAA" } }).to_string());
                json!({ "type": "message", "role": "assistant", "content": [{ "type": "output_text", "text": text }] })
            }
            Step::Call(name, input) => {
                call += 1;
                if name == "apply_patch" {
                    json!({ "type": "custom_tool_call", "status": "completed", "call_id": format!("call_{call}"), "name": name, "input": input })
                } else {
                    json!({ "type": "function_call", "name": name, "arguments": codex_input_value(&input), "call_id": format!("call_{call}") })
                }
            }
            Step::Output(text) => {
                json!({ "type": "function_call_output", "call_id": format!("call_{call}"), "output": text })
            }
        };
        lines
            .push(json!({ "timestamp": at, "type": "response_item", "payload": item }).to_string());
    }
    lines.push(json!({ "timestamp": time_of(story, 20), "type": "event_msg", "payload": { "type": "token_count", "info": null } }).to_string());
    lines
}

fn codex_flat_lines(story: &Story) -> Vec<String> {
    let id = session_id(story.key);
    let dir = cwd(&story.project);
    let mut lines = vec![
        json!({ "id": id, "timestamp": time_of(story, 0), "instructions": null,
            "git": { "commit_hash": format!("{:040x}", seed(story.key) as u128 * 104729), "branch": story.branch, "repository_url": format!("https://github.com/acme/{}.git", story.project.name) } }).to_string(),
        json!({ "record_type": "state" }).to_string(),
        json!({ "type": "message", "id": null, "role": "user", "content": [{ "type": "input_text", "text": format!("<environment_context>\n  <cwd>{dir}</cwd>\n</environment_context>") }] }).to_string(),
    ];
    let mut call = 0;
    for step in steps(story, Writer::CodexFlat) {
        let item = match step {
            Step::User(text) => {
                json!({ "type": "message", "id": null, "role": "user", "content": [{ "type": "input_text", "text": text }] })
            }
            Step::Assistant(text) => {
                lines.push(json!({ "type": "reasoning", "id": "rs", "summary": [{ "type": "summary_text", "text": "Thinking it through" }], "encrypted_content": "gAAAA" }).to_string());
                json!({ "type": "message", "id": "msg", "role": "assistant", "content": [{ "type": "output_text", "text": text }] })
            }
            Step::Call(name, input) => {
                call += 1;
                json!({ "type": "function_call", "id": format!("fc_{call}"), "name": if name == "apply_patch" { "apply_patch" } else { name },
                    "arguments": codex_input_value(&input), "call_id": format!("call_{call}") })
            }
            Step::Output(text) => {
                let exit = if text.contains("failures:") { 1 } else { 0 };
                json!({ "type": "function_call_output", "call_id": format!("call_{call}"),
                    "output": json!({ "output": text, "metadata": { "exit_code": exit, "duration_seconds": 1.2 } }).to_string() })
            }
        };
        lines.push(item.to_string());
        lines.push(json!({ "record_type": "state" }).to_string());
    }
    lines
}

/// The last time a story's session was written (Unix seconds).
fn last_write(story: &Story) -> i64 {
    let at = time_of(story, 16);
    chrono::DateTime::parse_from_rfc3339(&at)
        .map(|t| t.timestamp())
        .unwrap_or(0)
}

/// A generated transcript file.
pub struct CorpusFile {
    pub path: PathBuf,
    pub text: String,
    pub modified: i64,
    pub story: &'static str,
    pub session_id: String,
}

/// The corpus as files under `home` (in `.claude/projects` and `.codex/sessions`), with
/// one subagent thread for the lock-sharding session.
pub fn corpus(home: &Path) -> Vec<CorpusFile> {
    let mut out = Vec::new();
    for story in &STORIES {
        let id = session_id(story.key);
        let (path, lines) = match story.writer {
            Writer::Claude => (
                home.join(".claude")
                    .join("projects")
                    .join(format!("-home-dev-{}", story.project.name))
                    .join(format!("{id}.jsonl")),
                claude_lines(story),
            ),
            Writer::CodexEnvelope | Writer::CodexFlat => {
                let (y, rest) = story.day.split_at(4);
                let m = &rest[1..3];
                let d = &rest[4..6];
                let file = format!("rollout-{}T09-00-00-{id}.jsonl", story.day);
                let lines = if story.writer == Writer::CodexEnvelope {
                    codex_envelope_lines(story)
                } else {
                    codex_flat_lines(story)
                };
                (
                    home.join(".codex")
                        .join("sessions")
                        .join(y)
                        .join(m)
                        .join(d)
                        .join(file),
                    lines,
                )
            }
        };
        out.push(CorpusFile {
            path,
            text: lines.join("\n") + "\n",
            modified: last_write(story),
            story: story.key,
            session_id: id.clone(),
        });
        if story.key == "L2" {
            let thread = home
                .join(".claude")
                .join("projects")
                .join(format!("-home-dev-{}", story.project.name))
                .join(&id)
                .join("subagents")
                .join("agent-a1b2c3.jsonl");
            let lines: Vec<String> = [
                ("user", json!({ "role": "user", "content": "Find every place lumen-cache takes the global lock." })),
                ("assistant", json!({ "role": "assistant", "model": "claude-haiku-4-5", "content": [{ "type": "text", "text": "The global Mutex is taken in get, insert and evict_if_full in src/cache.rs; touch takes it a second time while reordering." }] })),
            ]
            .iter()
            .enumerate()
            .map(|(n, (kind, message))| {
                json!({ "type": kind, "isSidechain": true, "agentId": "a1b2c3", "message": message, "timestamp": time_of(story, n + 3),
                    "cwd": cwd(&story.project), "sessionId": id, "version": "2.1.312", "gitBranch": story.branch }).to_string()
            })
            .collect();
            out.push(CorpusFile {
                path: thread,
                text: lines.join("\n") + "\n",
                modified: last_write(story),
                story: story.key,
                session_id: id.clone(),
            });
        }
    }
    out
}

/// Writes the corpus under `home`, each file dated when its session last changed.
pub fn write_corpus(home: &Path) -> Result<Vec<CorpusFile>> {
    let files = corpus(home);
    for file in &files {
        if let Some(parent) = file.path.parent() {
            std::fs::create_dir_all(parent).with_context(|| parent.display().to_string())?;
        }
        std::fs::write(&file.path, &file.text).with_context(|| file.path.display().to_string())?;
        let at = std::time::UNIX_EPOCH + Duration::from_secs(file.modified.max(0) as u64);
        std::fs::File::options()
            .append(true)
            .open(&file.path)?
            .set_modified(at)?;
    }
    Ok(files)
}

/// One question's result.
#[derive(Debug, Clone, Serialize)]
pub struct QuestionResult {
    pub id: String,
    pub question: String,
    pub story: String,
    /// 1-based rank of the right session, when it came back.
    pub rank: Option<usize>,
    pub ms: f64,
    /// What came back, by story key.
    pub returned: Vec<String>,
}

/// The scorecard.
#[derive(Debug, Clone, Serialize)]
pub struct Scorecard {
    pub sessions: usize,
    pub files: usize,
    pub questions: usize,
    pub hit_at_1: f64,
    pub hit_at_5: f64,
    pub mrr: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub max_ms: f64,
    pub index_seconds: f64,
    pub target_hit_at_5: f64,
    pub target_p95_ms: f64,
    pub passed: bool,
    pub results: Vec<QuestionResult>,
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = ((p * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
    sorted[rank - 1]
}

fn no() -> bool {
    false
}

fn quiet(_: &SyncProgress) {}

/// Builds the corpus in `work`, indexes it with the search models in `models`, and asks
/// every frozen question.
pub async fn run(models: &Path, work: &Path) -> Result<Scorecard> {
    let home = work.join("home");
    let files = write_corpus(&home)?;
    let index = RecallIndex::open(
        &work.join("recall"),
        models,
        shodh_rag::compute::provider::ComputePreference::Cpu,
    )
    .await?;
    if !index.engine().read().await.has_search_models() {
        bail!(
            "the search models are not installed under {}",
            models.display()
        );
    }
    let mut settings = RecallSettings {
        enabled: true,
        ..RecallSettings::default()
    };
    settings.tools.extend(Tool::ALL);
    for story in &STORIES {
        settings
            .projects
            .insert(project_key(&cwd(&story.project)), true);
    }
    let roots = Roots {
        claude_code: Some(home.join(".claude").join("projects")),
        codex: Some(home.join(".codex").join("sessions")),
    };
    let started = Instant::now();
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
        .await?;
    let index_seconds = started.elapsed().as_secs_f64();
    if report.failed > 0 || report.indexed != files.len() {
        bail!("indexing the corpus: {report:?} for {} files", files.len());
    }
    let by_session: BTreeMap<String, &'static str> = files
        .iter()
        .map(|f| (f.session_id.clone(), f.story))
        .collect();

    // The first search loads the models; it is not timed.
    query::search(
        &index,
        &settings,
        &RecallQuery {
            text: "warm up".into(),
            ..RecallQuery::default()
        },
    )
    .await?;

    let questions = questions()?;
    let mut results = Vec::new();
    for q in &questions {
        let started = Instant::now();
        let answer = query::search(
            &index,
            &settings,
            &RecallQuery {
                text: q.question.clone(),
                limit: 5,
                ..RecallQuery::default()
            },
        )
        .await?;
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        let returned: Vec<String> = answer
            .sessions
            .iter()
            .map(|s| {
                by_session
                    .get(&s.session_id)
                    .map_or_else(|| s.session_id.clone(), |k| k.to_string())
            })
            .collect();
        let rank = returned.iter().position(|k| *k == q.story).map(|i| i + 1);
        results.push(QuestionResult {
            id: q.id.clone(),
            question: q.question.clone(),
            story: q.story.clone(),
            rank,
            ms,
            returned,
        });
    }
    let n = results.len().max(1) as f64;
    let hit = |k: usize| {
        results
            .iter()
            .filter(|r| r.rank.is_some_and(|x| x <= k))
            .count() as f64
            / n
    };
    let mrr = results
        .iter()
        .map(|r| r.rank.map_or(0.0, |x| 1.0 / x as f64))
        .sum::<f64>()
        / n;
    let mut times: Vec<f64> = results.iter().map(|r| r.ms).collect();
    times.sort_by(f64::total_cmp);
    let hit_at_5 = hit(5);
    let p95 = percentile(&times, 0.95);
    Ok(Scorecard {
        sessions: STORIES.len(),
        files: files.len(),
        questions: results.len(),
        hit_at_1: hit(1),
        hit_at_5,
        mrr,
        p50_ms: percentile(&times, 0.50),
        p95_ms: p95,
        max_ms: times.last().copied().unwrap_or(0.0),
        index_seconds,
        target_hit_at_5: TARGET_HIT_AT_5,
        target_p95_ms: TARGET_P95_MS,
        passed: hit_at_5 >= TARGET_HIT_AT_5 && p95 < TARGET_P95_MS,
        results,
    })
}

impl Scorecard {
    /// The scorecard as Markdown, with every miss and the slowest searches.
    pub fn markdown(&self) -> String {
        let mut out = String::new();
        out.push_str("## Recall scorecard (synthetic sessions)\n\n");
        out.push_str(&format!(
            "{} sessions in {} transcript files, {} frozen questions, indexed in {:.1} s.\n\n",
            self.sessions, self.files, self.questions, self.index_seconds
        ));
        out.push_str("| Measure | Result | Target |\n|---|---|---|\n");
        out.push_str(&format!(
            "| Right session in the top 5 | {:.1}% | at least {:.0}% |\n",
            self.hit_at_5 * 100.0,
            self.target_hit_at_5 * 100.0
        ));
        out.push_str(&format!(
            "| Right session first | {:.1}% | |\n",
            self.hit_at_1 * 100.0
        ));
        out.push_str(&format!("| MRR | {:.3} | |\n", self.mrr));
        out.push_str(&format!(
            "| Search time, median | {:.0} ms | |\n",
            self.p50_ms
        ));
        out.push_str(&format!(
            "| Search time, 95th percentile | {:.0} ms | under {:.0} ms |\n",
            self.p95_ms, self.target_p95_ms
        ));
        out.push_str(&format!(
            "| Search time, slowest | {:.0} ms | |\n\n",
            self.max_ms
        ));
        out.push_str(if self.passed {
            "**Passed.**\n\n"
        } else {
            "**Below target.**\n\n"
        });
        let misses: Vec<&QuestionResult> = self
            .results
            .iter()
            .filter(|r| r.rank.is_none_or(|x| x > 5))
            .collect();
        if misses.is_empty() {
            out.push_str("Every question found its session in the top 5.\n");
        } else {
            out.push_str("### Questions whose session was not in the top 5\n\n");
            for r in misses {
                out.push_str(&format!(
                    "- {} \"{}\": wanted {}, got {}\n",
                    r.id,
                    r.question,
                    r.story,
                    r.returned.join(", ")
                ));
            }
        }
        out
    }
}

/// Checks the frozen questions against the stories: each names a story, every story is
/// asked about, ids are unique.
pub fn check_questions(questions: &[Question]) -> Result<()> {
    let keys: BTreeSet<&str> = STORIES.iter().map(|s| s.key).collect();
    let mut ids = BTreeSet::new();
    let mut asked = BTreeSet::new();
    for q in questions {
        if !keys.contains(q.story.as_str()) {
            bail!("question {} names no story: {}", q.id, q.story);
        }
        if !ids.insert(q.id.as_str()) {
            bail!("question id {} is used twice", q.id);
        }
        if q.question.trim().is_empty() {
            bail!("question {} is empty", q.id);
        }
        asked.insert(q.story.as_str());
    }
    let unasked: Vec<&&str> = keys.difference(&asked).collect();
    if !unasked.is_empty() {
        bail!("stories no question asks about: {unasked:?}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use shodh_rag::recall::redact::Redactor;
    use shodh_rag::recall::{read_transcript, ParseBudget, Role};

    #[test]
    fn the_questions_cover_every_story() {
        let questions = questions().unwrap();
        check_questions(&questions).unwrap();
        assert!(questions.len() >= 2 * STORIES.len());
    }

    #[test]
    fn the_corpus_is_the_same_every_time_and_every_file_reads() {
        let dir = tempfile::tempdir().unwrap();
        let first = write_corpus(dir.path()).unwrap();
        let again = corpus(dir.path());
        assert_eq!(first.len(), STORIES.len() + 1);
        for (a, b) in first.iter().zip(&again) {
            assert_eq!(a.text, b.text, "{}", a.story);
        }
        for file in &first {
            let tool = if file.path.components().any(|c| c.as_os_str() == ".codex") {
                Tool::Codex
            } else {
                Tool::ClaudeCode
            };
            let t = read_transcript(
                tool,
                &file.path,
                &Redactor::default(),
                ParseBudget::default(),
            )
            .unwrap();
            assert!(t.errors.is_empty(), "{}: {:?}", file.story, t.errors);
            assert_eq!(
                t.info.session_id.as_deref(),
                Some(file.session_id.as_str()),
                "{}",
                file.story
            );
            assert!(
                t.turns.iter().any(|x| x.role == Role::User),
                "{}",
                file.story
            );
            assert!(t.info.cwd.is_some(), "{}", file.story);
        }
        let ids: BTreeSet<&str> = first.iter().map(|f| f.session_id.as_str()).collect();
        assert_eq!(ids.len(), STORIES.len());
    }

    #[test]
    fn percentiles_take_the_rank_above() {
        let v: Vec<f64> = (1..=20).map(f64::from).collect();
        assert_eq!(percentile(&v, 0.95), 19.0);
        assert_eq!(percentile(&v, 0.50), 10.0);
        assert_eq!(percentile(&[], 0.95), 0.0);
    }
}
