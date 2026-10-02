# Agent Workbench (omp harness + Claude Code-style transcript + app control): Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans. Steps use `- [ ]` checkboxes.

**Goal.** Every question is answered by an omp agent session. The UI shows the run as a live, readable transcript in the style of omp and Claude Code:
- the step currently running, inline;
- a task list;
- a status line;
- inline approvals;
- interrupt and steer.

The agent can operate the app through risk-tiered tools. A global activity tray shows everything that is running.

**Architecture.**
- **Core harness** (`crates/shodh-rag/src/harness/`) drives a pinned `omp` sidecar over JSONL RPC. It registers Shodh host tools, validates and authorises every call in Rust, and normalises omp frames into one **`AgentEvent`** stream.
- **Tauri** exposes session commands and forwards `AgentEvent`s.
- **The React transcript** is a pure reducer over `AgentEvent`s.

**Tech.**
- Rust: tokio process + JSONL; `jsonschema` for validating tool arguments.
- React 19: reducer + components; tokens from `index.css`.

**Spec.** `docs/superpowers/specs/2026-10-02-folder-sources-grounded-answers-design.md`: §7 (Answering, Harness, Tools, Profiles), §10a (UX bar, Navigation and agent visibility, Conversation dock, Motion). Decision record: `docs/adr/0001-agent-harness-omp.md`.

**Re-sequencing.** Approved 2026-10-02: this plan runs **before** M1/M2.
- The legacy pipeline is preserved at tag `legacy-pipeline-2026-10-02` for later baselines.
- Retrieval evaluation is unaffected, because `search_documents` calls `RAGEngine::search`.

## Global Constraints

- **Code quality**
  - Production code only: no TODOs, stubs, mocks or placeholders in `src/`.
  - No bare `unwrap`/`expect` outside tests.
  - No `@ts-ignore`.
- **Builds (CLAUDE.md)**
  - The user runs builds. Agents may run `cargo check` and `npx tsc --noEmit`.
  - Long-path builds on OneDrive must use a short `CARGO_TARGET_DIR`, for example `C:/build/kalki-rag`.
- **omp binary**
  - omp v18.4.10 is pinned by sha256 `7232c209641f0cad7e20bdb3a074cdb2fb31ae2aa73d42c491c705d28e0d3895`. It is verified before **every** launch.
  - It is not committed to git. A `fetch-omp` script/command downloads and verifies it.
- **omp launch and isolation**
  - Flags: `--mode rpc --no-ui --no-tools --no-extensions --no-skills --no-rules --no-lsp --no-pty --no-session --thinking off`.
  - Isolated `HOME`/`USERPROFILE` and agent directory under the app data dir.
  - An overlay config disables every discovery provider, sets `providers.cacheWarming: off`, `retry.fallbackChains.judge: []` and `memory.backend: off`. Env: `PI_AUTO_QA=0`, `OTEL_SDK_DISABLED=true`.
  - `task` stays disabled. Sub-agents go through host-side `delegate` (ADR 0001).
- **Model**
  - Provider and model come from the existing LLM configuration (`LLMState` / `SHODH_LLM_*`), mapped to omp `--model provider/id`.
  - API keys are passed only through the child's environment and never logged.
- **Tools**
  - Every tool has a JSON schema and a risk tier: `read` (auto), `write` (approval unless the profile allows it), `destructive` (always needs approval).
  - Calls are validated, profile-checked and audited before execution.
  - Invalid arguments produce a structured error back to the model.
- **UI**
  - Plain-language step labels. Prefer omp's `intent` field; fall back to the tool's `label` template. Raw arguments go in an expandable detail.
  - Show only real events.
  - Motion and contrast follow §10a.
- **Release blocker, not a development blocker.** omp egress (`catalog.stencil.so`, local port probes) must be blocked by OS egress control or a patched build before any release.

## Shared event contract (the interface both halves build against)

### Rust: `crates/shodh-rag/src/harness/events.rs`

Serialized with serde `tag = "type"`, `rename_all = "snake_case"`, and camelCase fields.

```rust
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    RunStarted   { run_id: String, session_id: String, model: String, at_ms: u64 },
    TextDelta    { run_id: String, message_id: String, delta: String },
    Thinking     { run_id: String, message_id: String, delta: String },
    StepStarted  { run_id: String, step_id: String, parent_step_id: Option<String>, tool: String,
                   label: String, args: serde_json::Value, tier: RiskTier, at_ms: u64 },
    StepProgress { run_id: String, step_id: String, text: String },
    StepFinished { run_id: String, step_id: String, ok: bool, summary: String,
                   detail: Option<serde_json::Value>, duration_ms: u64 },
    ApprovalRequested { run_id: String, step_id: String, tool: String, label: String,
                        tier: RiskTier, preview: serde_json::Value },
    PlanUpdated  { run_id: String, items: Vec<PlanItem> },
    Navigated    { run_id: String, view: String, focus: Option<String> },
    Usage        { run_id: String, input_tokens: u64, output_tokens: u64, cache_read_tokens: u64,
                   cost_usd: f64 },
    RunFinished  { run_id: String, status: RunStatus, duration_ms: u64, error: Option<String> },
}

pub enum RiskTier  { Read, Write, Destructive }                       // snake_case
pub enum RunStatus { Completed, Aborted, Error }
pub struct PlanItem { pub id: String, pub text: String, pub status: PlanStatus }
pub enum PlanStatus { Pending, InProgress, Done }
```

### TypeScript: `app/src/features/agent/events.ts`

The same union, discriminated on `type`, with camelCase fields. A contract test round-trips the JSON for each variant in Rust and asserts that the field names match the TS file's documented keys.

## Tools v1

All tools are host tools, implemented in Rust.

| Tool | Tier | Purpose |
|---|---|---|
| `search_documents(query, sources?, k?)` | read | Hybrid search; returns passages with file, page and score |
| `open_document(path, page?, range?)` | read | Text of a page or section; only for indexed files |
| `list_sources()` | read | Folders, file counts, indexing state |
| `update_plan(items)` | read (UI only) | Replaces the run's task list; emits `PlanUpdated` |
| `open_view(view, focus?)` | read | Switches the app tab (`ask`, `library`, `calendar`, `settings`); emits `Navigated` |
| `create_task(title, due?, source_ref?)` | write | Calendar task |
| `create_event(title, start, end?)` | write | Calendar event |
| `add_folder(path)` | write | Starts indexing a folder |
| `reindex_source(source_id)` | write | Re-indexes a folder |
| `remove_source(source_id)` | destructive | Removes a folder from the index |

## File structure

```
crates/shodh-rag/src/harness/
  mod.rs          # pub use; AgentHarness trait
  events.rs       # AgentEvent contract (+ serde tests)
  protocol.rs     # omp RPC frame types (subset used), serde
  sidecar.rs      # spawn, verify hash, JSONL read/write, request correlation, shutdown
  omp.rs          # OmpHarness: session lifecycle, frame→AgentEvent normalisation
  tools/mod.rs    # HostTool trait, registry, schema validation, tiers, approval gate
  tools/search.rs, tools/documents.rs, tools/sources.rs, tools/plan.rs,
  tools/navigate.rs, tools/calendar.rs
  fixtures/omp-spike-events.jsonl   # recorded frames (test-only fixture)
app/src-tauri/src/agent_session_commands.rs   # start/send/steer/abort/approve; emits "agent_event"
app/src/features/agent/
  events.ts       # contract types
  reducer.ts      # AgentEvent[] → TranscriptState (pure)
  Transcript.tsx  # turns, inline steps, text
  StepLine.tsx    # ● label   ⎿ summary   (expandable detail)
  PlanPanel.tsx   # task list (pending / in progress / done)
  StatusLine.tsx  # model · tokens · cost · elapsed · "Esc to interrupt"
  ApprovalPrompt.tsx
  AgentComposer.tsx  # send; while running: steer (Enter) / interrupt (Esc)
  useAgentSession.ts # invoke + listen("agent_event")
app/src/components/shell/ActivityTray.tsx  # global: indexing jobs + agent runs
```

## Tasks

### Task 1: Event contract + omp frame normalisation (Rust, pure)

- `events.rs` with serde tests that round-trip each variant and pin the JSON keys.
- `protocol.rs`: types for the frames we consume:
  - `ready`, `response`, `prompt_result`
  - `message_update` (`assistantMessageEvent`: `text_delta`, `thinking_delta`)
  - `tool_execution_start` / `update` / `end` (with `intent`)
  - `host_tool_call`, `host_tool_cancel`
  - `turn_end`, `agent_end`, `session_settled`
  - per-message `usage`
- `omp.rs::normalise(frame, &mut NormaliserState) -> Vec<AgentEvent>`: a pure function.
- **Test:** replay `fixtures/omp-spike-events.jsonl` through it and assert:
  - a `StepStarted` with label "Finding Acme MSA notice period" (from `intent`);
  - a `StepFinished` with ok=true;
  - `TextDelta`s concatenating to the final answer, which contains "60";
  - `Usage` totals;
  - `RunFinished`: Completed.

### Task 2: Sidecar + OmpHarness (Rust)

- **`sidecar.rs`**
  - Resolve the binary from `SHODH_OMP_PATH`, otherwise `<app_data>/bin/omp-<ver>.exe`, and verify its sha256 before launch.
  - Write the overlay config and isolated home on first run.
  - Spawn with the pinned flags and env. The provider key comes from the LLM config and goes only into the child's env.
  - JSONL writer with an `id` correlation map; reader task; `kill_on_drop`.
- **`OmpHarness`**
  - `start_session(profile, tools)`: send `set_host_tools`, `set_event_filter{messageUpdates:"delta"}`, `set_subagent_subscription{level:"events"}`.
  - `prompt(text)`; `steer(text)` (`streamingBehavior:"steer"`); `abort()`.
  - Host tool dispatch: `host_tool_call` → registry → `host_tool_result`, with `host_tool_cancel` support.
- **`fetch_omp`** command/script: downloads the pinned release asset and verifies its sha256.
- **Tests:** pure parts only (argument building, hash check, overlay content). Process tests run in CI only once CI can link test binaries.

### Task 3: Tool registry + tools v1 (Rust)

- **`HostTool` trait:**
  - `name`
  - `label_template` (e.g. "Searching {query}")
  - `schema() -> Value`
  - `tier()`
  - `async execute(args, ctx) -> Result<ToolOutput, ToolError>`
  - `ToolOutput { text_for_model, summary_for_ui, detail }`
- **Registry flow:**
  1. Validate against the schema with `jsonschema`.
  2. Profile allowlist check.
  3. Tier gate: Write/Destructive emit `ApprovalRequested` and await `approve(step_id, decision)`, with a timeout that resolves as denied.
  4. Execute.
  5. Emit `StepFinished`.
- **Tools:** implement the table above. Access checks:
  - `open_document` only serves indexed files, matched against the store's `file_path`.
  - `remove_source` is always destructive.
- **Unit tests:**
  - schema rejection
  - allowlist denial
  - approval denial
  - `update_plan` emits `PlanUpdated`
  - `open_view` emits `Navigated`

### Task 4: Tauri session commands

`agent_session_commands.rs`:
- `agent_start(conversation_id, profile_id?) -> session_id`
- `agent_send(session_id, text, request_id)`, which **is the Ask path**: replaces `unified_chat` for the Ask view.
- `agent_steer(session_id, text)`
- `agent_abort(session_id)`
- `agent_approve(session_id, step_id, approved: bool)`
- Emits `"agent_event"` with `{ sessionId, event: AgentEvent }`.
- Sessions are kept per conversation and shut down on app exit.
- The search-first behaviour from §7 is achieved with the system prompt plus `search_documents` being `loadMode: essential`. Evaluate later whether a pre-injected first search is still worth it.

### Task 5: Transcript UI

Done after the citation-viewer PR merges, so the two don't collide in `AskView`.

- **`reducer.ts`** (pure)
  - Builds turns from text, steps (nested by `parentStepId` into lanes), plan, usage, approvals and status.
  - Unit-tested with the fixture by converting recorded frames through a TS port of the normaliser, test-only.
- **`StepLine`**
  - `● {label}` with a live spinner while running, then `⎿ {summary}` and duration.
  - Click to expand args/detail as monospace.
  - Errors in red with the reason.
- **`PlanPanel`**
  - Docked on the right of the transcript on wide screens, collapsible; shows "3 of 5 done".
  - Hidden when the run has no plan.
- **`StatusLine`**
  - Sticky bottom: model · tokens in/out · cost · elapsed · `Esc to interrupt` while running.
- **`AgentComposer`**
  - Idle: Enter sends.
  - Running: Enter steers. A placeholder says "Steer the agent…", and Esc interrupts.
- **`ApprovalPrompt`**
  - Inline in the transcript: tool label, preview (e.g. a task title and due date), Approve (Enter) / Deny (Esc).
- **`Navigated`** switches the tab and collapses the conversation into the dock (§10a Conversation dock).
- **Citations** in the answer text keep opening the document viewer.

### Task 6: Activity tray

- A sidebar footer button showing a running-count badge. It opens a popover listing:
  - active agent runs (conversation, current step label, elapsed, Stop);
  - indexing jobs (folder, progress, Pause).
- It subscribes to `agent_event` (RunStarted/RunFinished) and `indexing-progress`.

### Task 7: Remove the legacy Ask path

- When Ask no longer calls `unified_chat`, delete:
  - the LLM router;
  - keyword intent routing;
  - the legacy agent executor, crews and tool loop;
  - the AgentsPanel backend commands;
  - the fake tools.
- Verify there are no remaining callers. This executes spec §4 "Removed by this sub-project".

## Definition of done

- A question shows its first visible activity in under 150 ms, and answers stream through omp.
- Steps show plain-language labels with summaries, and expand to show details.
- The task list updates live when the agent plans.
- Interrupt (Esc) aborts the provider call (G3), and steering redirects a running answer.
- "Add a reminder for…" produces an approval prompt; approving creates the task, switches to Calendar and docks the conversation.
- The activity tray shows concurrent indexing and agent runs.
- `cargo check`, clippy, fmt and `tsc` are clean. CI is green.
