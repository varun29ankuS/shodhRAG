# Folder Sources → Grounded Answers — Design Spec

- **Date:** 2026-10-02
- **Status:** Draft for review
- **Sub-project:** 1 of 6 (see Roadmap)
- **Scope owner:** shodh core (`crates/shodh-rag`) + thin Tauri layer (`app/src-tauri`, `app/src`)

## 1. Intent

A user adds a folder. Shodh ingests everything in it, keeps the index in sync with the folder (including changes made while the app was closed), and answers any question about its contents with verifiable, page-level citations. Counts and totals are computed, not guessed.

The same core must later run headless behind a server with SSO and per-document permissions, and must be the base for multi-agent work, calendar, and conversation-driven control of the whole app. This sub-project ships the desktop experience. It builds the interfaces those later sub-projects need (`Principal`, `AgentProfile`, tool registry with risk tiers, audit) so they extend the design instead of rewriting it.

### What the user said vs. what is assumed

| Said by user | Assumed (correct in review if wrong) |
|---|---|
| Corporate-grade; desktop and server both | Desktop ships first; server mode reuses the same core |
| Desktop app must be impressive and fully controllable by conversation | Conversation control of *all* app actions ships in sub-project 2; this spec ships answering tools only, on the same registry |
| Watch folders, ingest everything, answer any question | "Everything" = common office formats, PDFs (incl. scanned), text, CSV, email files |
| Adopt widely used repos, don't rebuild; likes omp and hermes | omp is the agent harness; Hermes is deferred to the channels sub-project |
| Multiple agents, calendar, chat must be possible | Agent profiles ship here; delegation and calendar ship in sub-project 2; "chat" = persistent, searchable conversations |
| Test with public benchmarks and our own folders locally | Private folder evals never leave the machine and never run in CI |

## 2. Current state (verified 2026-10-02)

These findings motivate the design. All were confirmed in source.

- **Folder watching does not exist.**
  - `watch_folder` / `unwatch_folder` / `watch_global_folder` are `// TODO` stubs (`app/src-tauri/src/window_commands.rs:63-80`).
  - `file_watcher.rs` is never registered with Tauri, and its frontend hook is imported nowhere.
- **The folder list is not durable.** It lives only in WebView `localStorage['indexedSources']`. There is no file inventory, so changes made while the app is closed are never seen.
- **Re-indexing can lose a file.** It deletes a file's chunks *before* parsing (`crates/shodh-rag/src/rag_engine.rs:317-321`), so a failed re-parse removes the file from the index.
- **Parse errors are discarded** (`indexing.rs:370`, `Err(_e)`). Users cannot see why a file is missing.
- **Chat ignores the active space** (`chat/engine.rs:446,462` call `rag.search` with no filter).
- **Citations have no pages.** `Citation.page_numbers` is never populated anywhere.
- **Totals are guessed.** Aggregation questions rely on the LLM reading up to 40 chunks and doing arithmetic itself, with no date filter.
- **The agent harness:**
  - Per-agent tool lists are not enforced (`agent/executor.rs:634`).
  - Malformed tool arguments silently become `{}` (`agent/tool_loop.rs:318`).
  - Failed steps report `success: true`.
  - Cancel is not wired.
  - Token usage is never recorded.
  - Local providers ignore tools.
- **The knowledge graph is a dead stub.** The `graph/` module has no callers. `knowledge_graph_query` is advertised to the LLM with no graph behind it.
- **CI runs zero tests** (`cargo test --no-run` with `continue-on-error`). Clippy is also soft-failed.
- **There is no retrieval evaluation dataset.**

## 3. Roadmap context

| # | Sub-project | Depends on |
|---|---|---|
| 0 | Containment fixes, CI that executes tests, `ort` → 2.0.0-rc.13 alignment | — |
| **1** | **Folder sources → grounded answers (this spec)** | 0 |
| 2 | Multi-agent delegation, calendar, conversation control of all app actions | 1 |
| 3 | Knowledge graph (GLiNER2 extraction, graph tables in LanceDB) | 1 |
| 4 | Server mode, SSO, per-document ACL sync, connectors | 1 |
| 5 | Channels (WhatsApp/email/etc., Hermes gateway evaluated here) | 2, 4 |

Sub-project 0 is a hard prerequisite, for two reasons:
- **Dependency conflict.** docling.rs and the GLiNER2 Rust runtimes require `ort ^2.0.0-rc.13`, while shodh currently resolves rc.11. `ort-sys` declares `links = "onnxruntime"`, so only one version can be linked.
- **Unverifiable CI.** CI must execute tests before any change in this spec can be verified.

## 4. Architecture

```
            Tauri app (thin)                      Server mode (sub-project 4)
   commands + events, approval dialogs          HTTP API + SSO
                    │                                   │
                    └──────────────┬────────────────────┘
                                   ▼
crates/shodh-rag
 ├─ sources/        SourceRegistry, file inventory
 ├─ ingest/         Scanner · Watcher · Reconciler · JobQueue · Parser
 │                  Chunker · RecordExtractor · Indexer
 ├─ store/          LanceDB (chunks+vectors) · Tantivy (BM25) · SQLite (SQLCipher)
 ├─ answer/         AnswerService · CitationVerifier
 ├─ harness/        trait AgentHarness → OmpHarness, DirectHarness
 ├─ tools/          ToolRegistry: search · open_document · list_documents ·
 │                  query_records · get_neighbors
 ├─ agents/         AgentProfile, profile policy
 ├─ conversations/  persisted conversations, messages, tool steps
 ├─ authz/          Principal, Policy, query-level filters
 ├─ audit/          append-only hash-chained event log
 └─ eval/           shodh-eval CLI
```

### Principles

1. **Business logic lives in `crates/shodh-rag`.** Tauri commands are thin adapters, so the server front end is a new adapter rather than a rewrite.
2. **The inventory is the source of truth for sync.** Filesystem events are hints only.
3. **Every tool call is authorized, schema-validated, profile-checked and audited in Rust.** The LLM never decides what it is allowed to do.
4. **Numbers come from code; claims come with checked citations.**
5. **Every interface takes a `Principal`**, even though desktop uses a single `LocalOwner`.

### Storage choice

SQLite (rusqlite with bundled SQLCipher) holds the inventory, records, conversations and audit log. The audit export and `query_records` aggregation both need SQL with transactions.
- `shodh-redb` was considered and rejected for this role because it has no query language.
- LanceDB is not designed for transactional bookkeeping.

### Removed by this sub-project

- **Chat routing:** the keyword intent routing in `chat/engine.rs` and the LLM router path for retrieval decisions.
- **Dead sync code:** `app/src-tauri/src/file_watcher.rs`, the unused `useFileWatcher.tsx` hook, the TODO watch stubs in `window_commands.rs`, and the duplicate `link_folder` / `link_folder_enhanced` paths.
- **Fake and dead features:**
  - The fake tools `code_analysis` and `document_generation`.
  - The `graph/` stub, the dead `enable_knowledge_graph` flag, `generate_sample_graph_data`, and the advertised `knowledge_graph_query` tool. The graph returns in sub-project 3.
- **Old executor scaffolding:** the executor's placeholder "Reasoning" and "FinalSynthesis" steps and legacy synthesis fallback, replaced by the harness.

## 5. Sync engine

### Data model (SQLite)

- `sources(id, root_path, space_id, include_globs, exclude_globs, enabled, cloud_hydrate, date_locale, acl_tags, created_by, created_at)`
- `files(id, source_id, rel_path, size, mtime, blake3, parser_version, chunker_version, embedder_version, status, error_kind, error_message, attempts, current_generation, indexed_at)`
  - `status` is one of `pending | indexed | failed | skipped`.
  - `pending` rows are the work queue. There is no separate queue store, so restarts resume naturally.

### Change detection

1. **Reconciler (catch-up pass).**
   - **When it runs:** on startup, every 10 minutes (configurable), and on watcher overflow or error.
   - **How it walks the folder:** with the `ignore` crate, honoring default exclusions and a per-source `.shodhignore` (gitignore syntax).
   - **What it compares:**
     - Same `(size, mtime)` → skip.
     - Otherwise compute `blake3`. Same hash → update metadata only.
     - Different hash → mark the file `pending`.
     - A file missing from disk → delete it from the index.
   - **Renames and moves:** the same hash appearing at a new path while the old path disappears is a rename. Only the path is updated; nothing is re-embedded.
2. **Watcher.** `notify-debouncer-full` with trailing debounce and rename pairing. Events trigger targeted reconciles of the affected paths.
3. **Polling-only mode.** Used for network paths (UNC and mapped drives), where native watchers are unreliable.

### Default exclusions

- Hidden files and folders, `~$*`, `*.tmp`, `.git`, `node_modules`, `Thumbs.db`, `desktop.ini`.
- Files larger than 200 MB (configurable per source).
- **OneDrive / cloud-only placeholders.** Detected via file attributes (`FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS`, `FILE_ATTRIBUTE_RECALL_ON_OPEN`, `FILE_ATTRIBUTE_OFFLINE`). They are marked `skipped/cloud_only` without triggering a download, unless the source opts in with `cloud_hydrate`.

### Processing pipeline

- **Parser pool:** `min(cores − 1, 4)` workers on blocking threads.
- **Embedding:** batched.
- **Index writer:** a single task owns all LanceDB and Tantivy writes. It commits in batches of 50 documents or every 2 seconds, whichever comes first.
- **No global lock during indexing.** Search reads committed state and is never blocked by ingest.

### Atomic replace (generations)

1. Parse, chunk, embed and extract records in memory.
2. Write chunks and records under generation `g+1`.
3. In one SQLite transaction, set `files.current_generation = g+1` and `status = indexed`.
4. Delete generation `g` from LanceDB, Tantivy and the records table.

On startup, garbage collection removes any chunk or record whose generation differs from its file's `current_generation`. This repairs crashes at any step and any LanceDB/Tantivy divergence. A parse failure leaves generation `g` intact and searchable.

### Failure handling

- **Error kinds:** `unsupported`, `encrypted`, `corrupt`, `too_large`, `cloud_only`, `ocr_unavailable`, `io_transient`, `internal`.
- **Retries:** `io_transient` retries up to 3 times with exponential backoff. Every other kind is recorded with its message.
- **UI:** a "Couldn't read" panel per source lists the file, the reason, and **Retry** / **Ignore** actions.
- **Panics:** a panic in a parser worker is caught, recorded as `internal` for that file, and never stops the sync.

### Versioning and upgrades

- **Parser, chunker or embedder version change:** affected files are marked stale and re-indexed in the background, at low priority.
- **Embedding dimension change:**
  1. Create a new LanceDB table.
  2. Build it in the background.
  3. Switch reads atomically when complete.
  4. Drop the old table.
- **Schema migrations:** a `schema_version` table and ordered, idempotent migrations for SQLite. The LanceDB table name carries its schema version.

### Progress

Per source the app emits:
- counts: indexed, pending, failed, skipped
- throughput in files per second
- ETA

Pause and resume work per source.

## 6. Parsing, chunking, citations, records

### Parsing

`trait DocumentParser { fn parse(&self, path: &Path, ctx: &ParseContext) -> Result<ParsedDocument, ParseError> }`

- **`ParsedDocument`:**
  - metadata: title, author, created, modified
  - `elements: Vec<Element>`, where each element has:
    - `kind`: `heading | paragraph | table | list | form_field | ocr_text | caption`
    - `text`, `page`, `bbox`, `reading_order`, `section_path`
- **Primary backend:** docling.rs, covering PDF (including scanned, via ONNX OCR), DOCX, PPTX, XLSX and HTML.
  - It replaces the Windows-only OCR path, which is required for Linux and macOS server builds.
- **Backend selection gate:** before committing, docling.rs and xberg are both run on the public eval corpus and on the team's local folders. The winner on extraction accuracy and throughput becomes primary. The choice is recorded in an ADR in `docs/`, and the parser trait keeps switching to a one-module change.
- **Secondary parsers:**
  - Plain text, CSV and Markdown use encoding detection (`chardetng` + `encoding_rs`).
  - `.eml` uses `mail-parser`.
- **Encrypted or password-protected files** are reported as `encrypted`, never silently dropped.

### Chunking

- **Size is measured in embedder tokens:** target 350, hard maximum 480. E5's limit is 512, so nothing is silently truncated at embed time.
- **All slicing is on `char` boundaries.**
- **Structure-aware splitting:**
  - Tables are never split mid-row. Large tables are split by rows, with the header repeated in each part.
  - Split points prefer heading and paragraph boundaries, then sentences.
  - Overlap is at sentence boundaries.
- **Each chunk stores:** `doc_id`, `generation`, `page_start`, `page_end`, `char_start`, `char_end` (into the normalized document text), `bboxes`, `section_path`, `element_kinds`, `acl_tags`.
- **Embedding and BM25 text** is prefixed with the title and section path. Stored text stays clean.

### Citations

`Citation { doc_id, display_path (original case), pages, char_span, quote }`

- A separate normalized path key is used for matching. Displayed paths are never lowercased.
- Clicking a citation opens a preview at that page with the span highlighted: pdf.js for PDFs, a text view with highlight for other formats.

### Records

`trait RecordExtractor { fn extract(&self, doc: &ParsedDocument) -> Vec<Record> }`

- `Record { record_type, fields: typed map, doc_id, generation, source_span, acl_tags }`
- Amounts are stored as `(decimal, currency)`. Dates are stored in ISO 8601, parsed with the source's `date_locale` (default `en-IN`, i.e. DD/MM).
- **v1 extractors:**
  - Spreadsheet and CSV rows become records with the column header as the schema.
  - Typed rule extractors for invoices: invoice number, invoice date, total amount, GSTIN, PAN, party names.
  - These replace the current regex fields, which are unsearchable, keep amounts as strings, and keep only the first invoice number per chunk.
- **v2 (sub-project 3)** adds GLiNER2 schema extraction behind the same trait.
- **`query_records` never runs LLM-authored SQL.** The model emits a structured spec:
  `{ record_type, filters: [{field, op, value}], group_by?, aggregate: sum|count|min|max|avg, field? }`.
  It is compiled to parameterized SQL with the principal's ACL filter injected.
- **Coverage reporting is mandatory.** Every aggregate result states how many records matched and how many documents in scope lacked the requested field. For example: "14 invoices dated Jul–Sep; 3 documents in that period had no recognizable total."

## 7. Answering

### Flow

1. **`AnswerRequest`:** `{ principal, conversation_id, profile_id, question, scope }`.
2. **Retrieval-first:**
   - Hybrid search over `scope`, with ACL and scope filters applied inside both LanceDB (`where`) and Tantivy (term filter). Fixes included in this step:
     - Tantivy analyzer with stemming and stop words.
     - Query parsing that escapes punctuation instead of falling back to an exact-phrase query.
     - Removal of the post-normalization `min_score_threshold`.
   - Fusion uses RRF. The cross-encoder reranker is a required, bundled model.
   - The top 8 passages are injected into the first harness turn as evidence `[E1]…[E8]`, with a note that more results are available via `search(page=2)`.
3. **Harness turn:** the model answers directly (one LLM call) or calls tools.
4. **Citation verification** (section 7.4).
5. **Delivery:** tokens and tool steps stream to the UI, and the answer is persisted to conversations and audit.

### 7.1 Harness

```
trait AgentHarness {
    async fn start_session(&self, profile: &AgentProfile, tools: ToolSet, principal: &Principal) -> Result<SessionId>;
    async fn send(&self, session: SessionId, turn: Turn) -> Result<()>;
    fn events(&self, session: SessionId) -> EventStream; // tokens, tool steps, usage, done, error
    async fn cancel(&self, session: SessionId) -> Result<()>;
}
```

**`OmpHarness`**
- **Binary:** a pinned `omp` release binary, verified by SHA-256 against a hash committed in the repo before each launch.
- **Isolation:** one empty working directory per session.
- **Launch flags:** `--mode rpc --no-ui --no-extensions --no-skills --no-rules --no-lsp`.
  - All omp built-in tools are disabled.
  - Only the profile's tools are registered via `set_host_tools`.
  - `set_event_filter` pins the event types shodh handles.
- **Tool-call format:** `tools.format: auto`, so models without native tool calling use in-band dialects (hermes, qwen3, gemma, deepseek, …).
- **LLM access:**
  - omp performs the LLM calls. Provider, endpoint and credentials come from shodh policy (keys read from the OS keychain).
  - Token usage is taken from omp usage events and recorded in audit.
- **Ambient discovery must be off.** omp must not load configuration, MCP servers, rules or skills from `.claude`, `.cursor`, `.vscode` or similar directories. If no flag disables a discovery path, the pinned build is patched or the session runs with an isolated `HOME`/config directory. This is verified by a test.
- **Egress requirement (release blocker):**
  - The omp process may contact only the configured LLM endpoint. A CI test runs it behind a recording proxy and fails on any other destination, including install-ID reporting.
  - If omp cannot meet this, shodh ships a patched pinned build or switches the harness implementation to upstream pi.

**Acceptance gate: omp spike (milestone M5, before any harness integration code)**

The capabilities above come from omp's documentation and have not yet been exercised. A scripted spike drives the pinned `omp --mode rpc --no-ui` with one host tool against the local model endpoint. omp is adopted only if all of the following pass:

1. **No built-in tools.** A documented flag or config combination disables every omp built-in tool. The tool list reported by the session contains only host tools.
2. **Host tool round-trip.** `set_host_tools` registers a tool. A host tool round-trip completes with a local model through `tools.format: auto`, and with one cloud model using native tool calling.
3. **Cancellation.** `abort` stops generation, and `host_tool_cancel` is delivered for an in-flight host tool.
4. **Usage.** Usage events, with input and output tokens, arrive in `--no-ui` mode.
5. **Isolation.** The discovery-isolation test passes, and the egress test passes.

If omp fails, the fallback is evaluated in this order:
1. Upstream pi in RPC mode, with shodh tools served to it as an MCP server over stdio.
2. An in-process Rust tool loop behind the same `AgentHarness` trait.

The spike result and the decision are recorded in an ADR.

**Local model endpoint**

omp makes LLM calls over HTTP. shodh's local model runs in-process (`llama-cpp-2`), so shodh exposes it as an OpenAI-compatible chat endpoint for harness sessions:

- **Network exposure**
  - Bound to `127.0.0.1` on an ephemeral port.
  - No CORS headers are set. Any request carrying an `Origin` header is rejected.
- **Authentication:** requests must carry a random per-session bearer token, which is passed only to the omp child process and rotated each session.
- **Concurrency:** requests are serialized onto the single loaded model, with a bounded queue.
- **Allowed endpoints:** in Local-only mode, this endpoint and any admin-configured local OpenAI-compatible servers (Ollama, vLLM, LM Studio on loopback or LAN) are the only permitted LLM endpoints. The egress test treats exactly these as allowed destinations.
- **Truncation bug fix (in scope):** the existing llama.cpp provider truncates over-length prompts from the front (`llm/llamacpp_provider.rs:209-216`), which can drop the system prompt and tool definitions. The fix:
  - Over-length prompts trim the oldest conversation turns and then the lowest-ranked evidence.
  - They never trim the system prompt or tool schemas.
  - When the prompt still does not fit, the request fails with a typed `ContextOverflow` error.

  `DirectHarness` and the loopback endpoint share this provider.

**`DirectHarness`**
- Single-shot answer from the retrieved evidence, using the existing in-process LLM providers, with no tools.
- **Used when:**
  - the omp binary is missing or fails hash verification
  - omp crashes twice in a session
  - policy forbids sidecars
- The UI shows a visible "tools unavailable" banner.

**Budgets**
- Per answer: max tool calls (default 8), max wall time (default 90 s) and a max token budget. All are configurable per profile.
- **Cancel:** the UI stop sends omp `abort` and cancels in-flight host tool calls via `host_tool_cancel`.

### 7.2 Tools (v1, all `read` tier)

| Tool | Purpose |
|---|---|
| `search(query, filters?, page?)` | Hybrid search within scope |
| `open_document(doc_id, pages? \| char_range?)` | Read a section of a document |
| `list_documents(filter)` | Filter by source, type, modified range, name |
| `query_records(spec)` | Filter and aggregate typed records (section 6) |
| `get_neighbors(chunk_id, before, after)` | Adjacent chunks |

**Every call passes these checks in Rust, in order:**
1. The tool is in the profile's `allowed_tools`.
2. Arguments validate against the JSON schema. On failure, a structured error is returned to the model and the tool does not execute.
3. Authz filters are applied in the store query.
4. Results are size-capped, with a "truncated, N more" notice.
5. Results are wrapped as untrusted content with evidence ids.
6. An audit event is written.

**Risk tiers.** Every tool declares one: `read`, `write` (auto, undoable), `send/external` (always confirmed), or `destructive/admin` (confirmed and role-gated). Only `read` tools ship here. Sub-project 2 adds the rest to the same registry, with:
- **Taint rule:** once untrusted connector content is in a session, `send/external` actions require confirmation even under permissive policy.

### 7.3 Agent profiles

`AgentProfile { id, name, instructions, model_policy, allowed_tools, source_scope, risk_ceiling, budgets }`

- A default "Assistant" profile ships. Users can create, edit and delete profiles in the UI.
- Profile limits are enforced at every tool call, not only at session start.
- **Profile creation through conversation:** an LLM may *propose* a profile. Persisting it is a `write` action requiring user confirmation.
  - Profile ids are validated slugs.
  - Profiles are stored in SQLite, never as LLM-named files. This fixes the current path-traversal bug.
- Delegation between profiles (`delegate(agent, task)`, child permissions = intersection with the parent, depth ≤ 2) is designed for here but ships in sub-project 2.

### 7.4 Citation verification (local, no LLM judge)

- Every `[En]` must reference evidence delivered in this session: search results, opened ranges, or record query results.
- Each cited sentence is scored against its evidence with the cross-encoder. Sentences below a calibrated threshold are marked "unverified" in the UI. The threshold is calibrated on the eval set, not hand-picked.
- Every number in the answer must appear in its cited text or in a `query_records` result. Otherwise it is flagged.
- Answers are flagged, never silently blocked.
- When evidence is insufficient, the answer says so explicitly and shows what was searched.

### 7.5 UX

- Token streaming.
- Visible tool steps, for example: "Searched 'Q3 invoices' → 14 records · opened Acme_MSA.pdf p.4".
- Click-through citations.
- A per-answer data-path label, for example: "local · Qwen3" or "sent to Anthropic".

## 8. Authorization, audit, conversations, data protection

### Authorization

- `Principal { id, kind: LocalOwner | User | Service, groups }`
- **v1 policy is source-level ACL.** Each source carries `acl_tags`, and chunks and records copy them at index time.
- Sub-project 4 replaces source-level tags with per-document tags from connector permission sync, with no schema change.
- **Filters are applied inside every store query, never post-hoc.**

### Audit

`audit_events(id, ts, principal, conversation_id, profile_id, event_type, payload_json, prev_hash, hash)`

- **Event types:** `question`, `tool_call`, `retrieval`, `answer`, `source_change`, `settings_change`, `profile_change`.
- **Payload:** arguments, evidence and doc ids, model, provider, token usage, latency.
- **Tamper evidence:**
  - `hash = SHA-256(prev_hash || canonical(row))`.
  - `shodh audit verify` validates the chain.
  - Retention trims are recorded as signed checkpoint events, so the chain stays verifiable.
- Export to JSONL or CSV. Retention defaults to 365 days and is configurable.

### Conversations

- **Tables:**
  - `conversations(id, principal, profile_id, title, created_at, updated_at)`
  - `messages(id, conversation_id, role, content, citations_json, status, created_at)`
  - `tool_steps(id, message_id, tool, args_json, result_summary, evidence_ids, duration_ms)`
- All writes are transactional. This replaces the non-atomic `chat_history.json`.
- FTS5 index over messages. Conversations become a searchable source in sub-project 2.

### Data protection

- **Encrypted SQLite.** The SQLite database is encrypted with SQLCipher. The key is generated on first run and stored in the OS keychain (`keyring`: Windows Credential Manager, macOS Keychain, Secret Service on Linux).
- **Secrets in the keychain.** API keys and other secrets move from WebView `localStorage` to the keychain.
- **Known limitation, stated explicitly.**
  - LanceDB and Tantivy files are not encrypted by shodh. Shodh requires OS full-disk encryption (BitLocker or FileVault).
  - The health page checks for it and warns when it is absent.
  - Product copy must not claim full encryption at rest.
- **Egress policy.**
  - Admin and profile-level allowlists of LLM providers.
  - A **Local-only** mode disables all cloud providers.

### Error handling

- **Typed errors:** `thiserror` throughout new modules.
- **Lints:** `clippy::unwrap_used` and `clippy::expect_used` are denied in new modules.
- **User-visible errors:** every error the user sees states what failed and what they can do.
- **Harness crash recovery:** an omp crash triggers one restart, then `DirectHarness` with a visible notice.

### Health page

- **Index state:** inventory counts versus LanceDB and Tantivy counts, with a consistency check result. Last reconcile time per source.
- **Components:** model status (embedder, reranker, LLM provider) and harness status, including the omp version and hash.
- **Security:** the disk-encryption check.

## 9. Testing and evaluation

### Policy on test doubles

Production code contains no mocks or stubs.

- **Where the scripted server is allowed.** Deterministic edge-case tests of the tool loop need a scripted LLM that sends malformed arguments, loops, or requests forbidden tools. These use a test-only, OpenAI-compatible scripted server under `tests/fixtures/`, never compiled into the app.
- **Where real models are required.** Every end-to-end and eval claim runs against a real model.

### Tests

- **Chunker property tests (`proptest`):**
  - Always `char`-boundary safe.
  - Never more than 480 tokens.
  - `doc_text[char_start..char_end] == chunk.text` for every chunk.
- **Sync integration tests.** These use real temp directories, LanceDB, Tantivy and SQLite. Scenarios:
  - add, modify and delete
  - rename and move without re-embedding
  - offline changes detected on start
  - kill at each fault-injection point mid-index, then restart: no lost or duplicated chunks
  - a re-parse failure keeps the previous generation searchable
  - ignore rules
  - cloud-only placeholder handling (Windows runner)
- **Authz leak tests.** Every tool, with an unauthorized principal, returns nothing, including when called with guessed ids.
- **Audit tests.** The chain verifies, and tampering with, deleting or reordering a row is detected.
- **Harness tests:**
  - Contract tests for every omp RPC frame used, pinned to the omp version and re-run on each upgrade.
  - Tool-loop budget, cancel, schema-rejection and profile-denial tests.
  - The egress test (section 7.1).
- **Migration tests.** Legacy `localStorage` sources, `chat_history.json` and the legacy LanceDB table migrate correctly.

### Evaluation (`shodh-eval`)

**Datasets**
- **CUAD** (commercial contracts with expert clause annotations, CC BY 4.0): a fixed subset for PRs and the full set nightly.
- **Synthetic invoice corpus.** Generated deterministically from a fixed seed and rendered to PDFs with varied layouts, so ground-truth totals are exact and licensing is clean.
- **Private folders:**
  - Run with `shodh-eval run --questions my_questions.yaml --source <folder>`.
  - Each entry has: `question`, `expected_answer`, `expected_docs`, `expected_pages`, `type` (`lookup | list | aggregate | compare | none`).
  - Local only. Results never leave the machine.

**Metrics**

| Area | Metrics |
|---|---|
| Retrieval | recall@8, MRR |
| Answers | correctness: F1 for extractive answers, exact match for numeric |
| Citations | precision, unsupported-sentence rate |
| Aggregation | aggregation exactness, coverage accuracy |
| Performance | latency p50/p95, ingest throughput |

**CI policy**
- **Every PR:** retrieval metrics and deterministic `query_records` aggregation, with no LLM involved.
- **Nightly:** full answer metrics with a pinned local model.
- **Regression gates:**
  1. The baseline is measured first, including run-to-run noise over repeated runs.
  2. A regression beyond measured noise fails the build.
  3. Gates are never relaxed to pass.

## 10. Migration

On first launch of the new version:

1. Import `localStorage['indexedSources']` into `sources` exactly once, then remove the key.
2. Import `chat_history.json` into `conversations` and `messages`.
3. Move API keys from `localStorage` to the keychain.
4. Build the new LanceDB table by re-indexing all registered sources in the background. Search continues on the legacy table until the new one is complete, then switches atomically and the legacy table is dropped.

Each step is idempotent and recorded in `schema_version` and audit.

## 10a. UX quality bar

"Smooth" is defined by measurable budgets. Playwright tests against the built Tauri app measure each budget.
- The baseline is recorded in the UI foundation milestone (M1.5).
- After that, a regression beyond measured noise fails CI, under the same rule as the eval gates.

| Moment | Budget |
|---|---|
| Cold start to interactive | < 1.5 s |
| Input (keypress/click) to visible response | < 100 ms (p95) |
| Question to first visible activity (tool step or token) | < 150 ms |
| Question to first answer token, cloud model | < 1 s (p50) |
| Scrolling a long conversation or a 10,000-file source list | 60 fps (virtualized lists) |
| During indexing | UI and chat never block; progress visible from every screen |
| Errors | Always state what happened and the next action; no blank screens, no spinners without a timeout |

**Rules:**
- Every milestone ships the UI for its feature, with Playwright coverage. For example, M2 ships the sources panel, the "Couldn't read" panel and live progress. M3 ships citation click-through to the highlighted page.
- `@sentry/react` crash reporting is opt-in, off by default, and disabled in Local-only mode.

## 11. Milestones

Each milestone is one or more PRs. Each ships independently with CI green, executing its tests. Order matters: the baseline has to be measured before anything is replaced.

| # | Milestone | Ships |
|---|---|---|
| M1 | Eval harness and current-pipeline baseline | `shodh-eval`, CUAD subset, synthetic invoice corpus, private-folder YAML runner. Baseline metrics and idle search latency recorded for **today's** pipeline, with run-to-run noise measured |
| M1.5 | UI foundation | Visual direction, approved via mockups. Steps: split `App-SplitView.tsx` (4,315 lines) into screens with a state store; design tokens and component library; lazy-load heavy libraries (Monaco, Mermaid, three.js, Recharts); Playwright harness with UX-budget baseline; frontend typecheck and lint in CI; Sentry made opt-in |
| M2 | Store, inventory and sync | SQLite (SQLCipher plus keychain key), `sources`/`files`, reconciler, watcher, job queue, generations, failure panel, progress. Uses the current parser and chunker |
| M3 | Parser bake-off, chunker, citations | docling.rs vs xberg ADR, `DocumentParser`, token-based structure-aware chunker, page and span citations, citation preview UI |
| M4 | Records | `RecordExtractor` v1, records table, `query_records` with coverage reporting |
| M5 | Harness gate | omp spike against the acceptance gate (section 7.1). ADR with the decision. Loopback local model endpoint. llama.cpp truncation fix |
| M6 | Harness integration and profiles | `AgentHarness`, `OmpHarness` (or the gated fallback), `DirectHarness`, tool registry with schema validation and risk tiers, agent profiles, budgets, cancel, streaming tool steps, citation verifier |
| M7 | Authz, audit, conversations | `Principal`, source-level ACL filters in all stores, hash-chained audit with verify and export, conversations in SQLite, health page |
| M8 | Migration and deletion | First-launch migrations; removal of all code listed in section 4; final eval run compared against the M1 baseline |

## 12. Definition of done

1. **Sync at scale.** A folder of about 10,000 mixed documents:
   - indexes completely
   - survives restart mid-index without loss or duplication
   - detects changes made while closed on next start
   - handles renames without re-embedding
2. **Search during ingest.** Search p95 latency during active ingest stays within a margin of the idle baseline. The margin is set after both are measured.
3. **Citations and totals.** Every citation opens at its page with the span highlighted. Every aggregate states its coverage.
4. **Eval baselines.** Recorded for the current pipeline and for the new one, with the deltas reported.
5. **Required CI tests pass:** authz leak, audit chain, egress, crash recovery, migration.
6. **Cleanup.** The code listed in section 4 "Removed by this sub-project" is deleted. New modules contain no TODOs, `unwrap` or `expect`.
7. **Real CI.** CI executes all of the above. No `continue-on-error` on test or lint jobs.

## 13. Risks

| Risk | Mitigation |
|---|---|
| omp is effectively single-maintainer with very frequent breaking releases | Depend only on the RPC protocol; pin the binary by hash; contract-test every frame; `AgentHarness` allows switching to pi or an in-process harness |
| omp phones home or loads ambient config | Egress test and discovery test are release blockers; patched pinned build if needed |
| docling.rs is new (created June 2026) | Bake-off against xberg before commitment; parser trait isolates the choice |
| Rule-based invoice extraction is brittle across layouts | Mandatory coverage reporting; GLiNER2 extractor in sub-project 3 |
| Credentials and document content pass into a third-party sidecar | Hash-pinned binary, egress test, isolated working directory; `DirectHarness` or an in-process harness for customers whose policy forbids sidecars |
| LanceDB/Tantivy unencrypted at rest | Require and check OS full-disk encryption; no false product claims |
| Sidecar-per-conversation limits server-mode scale | Acceptable for department scale; harness interface allows replacement before sub-project 4 if needed |

## 14. Open items for sub-project 0 (prerequisites, not this spec)

- **Containment:**
  - WhatsApp bot auto-authorizes every sender (`whatsapp_commands.rs:239-248`).
  - Bot HTTP servers allow any origin with no auth.
  - `csp: null` and `assetProtocol.scope: ["**"]` in `tauri.conf.json`.
  - API keys in `localStorage`.
- **CI:** fix the CRT linker mismatch (ort-sys vs esaxx-rs) so tests execute, and make clippy blocking.
- **Dependencies:** align `ort` to 2.0.0-rc.13 across the workspace, including the E5 embedder.
