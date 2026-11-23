# Proactive assistant, PR 3 (What's new) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A **What's new** section in the Library has two parts:
- the files that arrived in the user's watched folders since Shodh last looked;
- if the user opts in, new papers on a topic, found once a week on arXiv, OpenAlex and Semantic Scholar.

Each run posts one Info item. Local-only mode sends nothing, and no request carries the user's identity.

**Architecture:**
- **New files.**
  - `folder_sync` reports the files a sync indexed for the first time (`SyncReport.added`). A folder's first sync is a baseline and reports none.
  - The watcher writes them as `feed_item` rows the moment the report arrives, then sends a `folder_watch` engine job an event.
  - The job's handler is state-based, like `task_repeat`: it reads the rows not yet posted and posts one Info item. Coalescing, crash safety and "one item per run" all follow from that.
- **Paper alerts.**
  - An alert is a `paper_alert` engine job with a weekly schedule. Its `input_json` holds the words, the consent and each API's `checkedThrough` date.
  - The work is all in `shodh-rag`, so the shared test fakes (`harness::web::client::testing`) reach it: date-bounded requests (one per API), parsing, merging, de-dup against the library and earlier items, and local ranking. No model calls.
  - The app handler is a thin adapter. It checks the policy, then calls the core, posts the item and handles retries.
- **One engine, one loop.** The app's engine stops being private to repeating tasks. One `Engine` and one loop tick every handler: `task_repeat`, `folder_watch` and `paper_alert`.

**Tech stack:**
- Rust: rusqlite, chrono, and the existing `SafeClient`, `roxmltree` and cross-encoder ranker. No new crate.
- React and TypeScript.
- Tests:
  - shodh-rag unit tests, run by CI "Tests";
  - app tests, run by CI "App tests and doc tests";
  - Node strip-types tests, run by CI "Frontend Typecheck + Tests".

**Spec:** `docs/superpowers/specs/2026-11-22-proactive-assistant-design.md`. The parts that apply:
- §7 (What's new);
- §5.3 (event triggers, retries, the "2 minutes after a wake" rule);
- §2's anti-goals (untrusted text, a second runner);
- §12 item 4 (a new Library section);
- the §13 row "3. What's new";
- §15 decision 6 (all three APIs, named in the consent line).

The UI rules come from `docs/superpowers/specs/2026-11-23-ui-structure-audit.md`:
- rule 1: the primary-action slot stays "Add folder";
- rule 7: buttons never wrap;
- the memory rule "UI: sturdy, unambiguous".

The branch is `feat/proactive-whats-new`, from `origin/master` `a1f6f145` (PR 1 #199 and PR 2 #202 merged). The next free migration is **20**. Open PRs #186 and #198 touch neither `audit/store.rs`, `folder_sync.rs`, `file_watcher.rs`, `engine_commands.rs`, `LibraryView.tsx` nor `agent_tools/mod.rs` (checked 2026-11-23).

## Rulings and amendments (2026-11-23)

These settle the open questions at the end of this plan. Where they differ from the plan, they override it.

- **Q1:** the consent line is `CONSENT_TEMPLATE` in Global Constraints. It is one constant on each side, `whats_new::alerts::CONSENT_TEMPLATE` and `features/whatsNew/consent.ts` `CONSENT_TEMPLATE`. Tests on both sides pin the same literal for "linear attention KV cache".
- **Q2, Q6, Q7, Q8, Q9:** as recommended:
  - all three APIs, with no per-API switches;
  - at most 10 alerts, with the per-API spacing and per-run limits as planned;
  - the web-access switch also gates alerts;
  - a rename is matched by size and modification time;
  - the "N indexed" item is dropped when What's new covers the same files.
- **Q3: topics are suggested from the library, as chips the user ticks.** This is the new Task 6b.
- **Q4/Q5:** the first check runs immediately when the alert is turned on, over the last 14 days; after that, Mondays at 09:00. An Info item is posted only when something new was found. There are no desktop notifications.
- **Q10:** Add to library stays in PR 3: open-access PDFs only, into a watched folder the user picks, refused in Local-only mode.
- **D1–D16** are accepted.

### Task 6b (new): Topic suggestions from the library

**Files:**
- Create: `crates/shodh-rag/src/whats_new/topics.rs`.
- Modify:
  - `crates/shodh-rag/src/research/citations/graph.rs`: `pub fn method_use_counts(&self) -> Vec<(&ConceptNode, usize)>`, the number of library papers that use each method;
  - `app/src-tauri/src/whats_new.rs`: the command `paper_alert_suggestions`;
  - `agent-coverage.json`: it maps to `whats_new`, whose report gains the suggestions;
  - `api.ts`, and the alert dialog (Task 10b).
- Test: the `topics.rs` tests over a fixture library (`whats_new/fixtures/topic-library.json`: titles and method counts).

**What the library already has, and what is used.** Nothing here makes a model call or a network request, and the result is deterministic:
- titles of library papers: `citation_scans` identities, else the file names of indexed PDFs with the extension and any arXiv id dropped;
  - **ruled (coordinator):** a file name that is an id or junk gives no phrase. That covers arXiv ids (`2301.12345v2`), DOIs, `paper_final(1)`, `download`, hashes and pure numbers. A chip must read like a topic a person would type, and the fixture library holds such names as negatives;
- method concepts of the paper graph, with how many library papers use each (only once the graph is built and results are accepted).

**Interfaces:**
```rust
pub struct TopicInputs { pub titles: Vec<String>, pub methods: Vec<(String, usize)> }
pub struct TopicSuggestion { pub words: String, pub from: TopicSource /* Titles | Method */, pub papers: usize }
pub const MAX_SUGGESTIONS: usize = 8;
pub fn suggest_topics(inputs: &TopicInputs, existing_alerts: &[String]) -> Vec<TopicSuggestion>;
```

**Ranking:**
- **Phrases from titles.** A phrase is 2–3 consecutive words of one title, lower-cased by `relevance::words`.
  - No stopword at either end, and no word shorter than 2 characters.
  - A phrase's score is the number of distinct titles that contain it, and a phrase must reach at least 2.
  - A phrase contained in a longer phrase with the same score is dropped.
- **Method labels.** Each label scores `2 × papers` and is kept whole.
- **Order:** by score (descending), then the number of words (descending), then alphabetically.
- **Filtering:** phrases equal to an existing alert's words are dropped, ignoring case and spacing. At most `MAX_SUGGESTIONS` are returned.
- **The empty library:** no titles and no methods gives `[]`.

**UX (in the alert dialog):**
- Under "Papers about", a row of chips, "Suggested from your library", shows each suggestion's exact words.
- Ticking chips fills the words box with them, joined with " ", the exact text that will be sent. The box stays editable, and the user can also type a topic of their own.
- The consent line always shows the box's text, and nothing is sent before **Turn on**.
- With no suggestions, the chip row is absent and the box reads "Type a topic, e.g. linear attention".
- Chip labels never wrap; past one row, chips scroll horizontally.

**Tests:**
- `suggestions_from_a_fixture_library_rank_by_shared_titles`: exact expected list.
- `a_phrase_inside_a_longer_one_with_the_same_count_is_dropped`.
- `methods_rank_above_single_title_phrases`.
- `existing_alerts_are_not_suggested`.
- `an_empty_library_suggests_nothing`.
- `suggestions_are_deterministic`: shuffled inputs give an identical output.
- `the_suggestions_command_sends_nothing` (app; the recording transport stays empty).
- Frontend `ticking chips fills the words and the consent line shows them`.

### The PR split

The PR splits into two, opened one after the other. 3b is cut from 3a.

- **PR 3a, folder What's new:**
  - Tasks 1, 2, 4 and 5;
  - Task 8 without Add to library:
    - list, mark seen, dismiss, folder watch;
    - the `whats_new` tool, `set_folder_watch` and navigation;
    - `WhatsNewView` has `items` and `folders` only;
  - Task 9 without the alert lines;
  - Task 10 without the dialog or the alert rows ("+ Paper alert" is absent in 3a).
- **PR 3b, paper alerts:**
  - Tasks 3, 6, 6b and 7;
  - Task 8's Add to library;
  - Task 9's alert lines;
  - Task 10's dialog (with the chips), alert rows, Local-only and failure states, and Resume.
  - In 3b, `WhatsNewView` gains `alerts` and `blockedReason`.

## Global Constraints

- **Builds and checks:**
  - No local builds. cargo, `npm install`, `npm test` and running the app happen in CI only. Each "Run" step names the CI job that shows the result.
  - Rust only for any tooling.
- **Code quality:**
  - Production code only: no TODO, stub, mock or placeholder.
  - Mutex locks use `.unwrap_or_else(|e| e.into_inner())`.
  - Never weaken a test. Existing whole-struct `assert_eq!`s on `SyncReport` (e.g. `folder_sync.rs:2370`) gain the new fields at their correct values. No existing expected value changes.
- **Commits:** plain prose subjects, with no footers (no Co-Authored-By, no "Generated with").
- **Network and identity (load-bearing):**
  - Every alert request goes through `SafeClient`, with `client::USER_AGENT` (no email in it).
  - No `mailto` parameter, no `From` header, no email, no user name, no key.
  - Tests use fixture replies only. They reuse `harness/fixtures/papers/linear-attention/{arxiv.xml,openalex.json,semantic-scholar.json}` as bodies; there is no live network in any test.
  - **Local-only mode sends nothing.** The policy (`app_settings::Policy::web_block_reason`) is checked at create and update, before **every** alert run, and before "Add to library". A blocked run sends no request at all.
- **Untrusted text.** Paper titles, abstracts and watched-folder file names are data:
  - they render as plain text (never Markdown or HTML);
  - they never choose an action;
  - the `whats_new` tool returns them under `UNTRUSTED_NOTICE`.
- **Same-task registration.** Each new command lands in one task with:
  - its `lib.rs` registration;
  - its `permissions/main-window.json` entry;
  - its `agent-coverage.json` entry;
  - a literal-string frontend use (`app/src/features/whatsNew/api.ts`).
- **No foreign keys.** `shodh.db` never enables `PRAGMA foreign_keys`. Deleting a job deletes its feed rows in the same transaction.
- **Background runs never move the screen and never call `notify`.** What's new is in-app only (Q4).
- **Copy.** The exact strings below are pinned by tests:
  - `CONSENT_TEMPLATE` (ruled Q1): `Shodh sends these words: {words} to arXiv, OpenAlex and Semantic Scholar to look for new papers. Your files, your library and who you are stay on this computer.` `{words}` is the exact query sent, trimmed, with no quotes added.
  - `EMPTY_COPY`: `Nothing new yet. New files in your Library folders show up here. You can also turn on alerts for new papers on a topic.`
  - `LOCAL_ONLY_COPY`: `Paper alerts are paused: Local-only mode is on, so nothing leaves this computer.`

## Review Focus

1. **An interrupted first sync must stay a baseline.** The manifest is saved after every file, so a first sync stopped half-way leaves a manifest. Without care, the next sync reports the rest of the folder as new.
   - Test `an_interrupted_first_sync_stays_a_baseline` (Task 1).
2. **Tidying a folder must not flood What's new.** A rename or move is a removal plus a new file. A file re-read by the scan reader, or a chat upload handed over to a folder (`forget_file`), is "absent from the manifest" too.
   - Tests `a_renamed_file_is_not_new`, `a_forgotten_file_read_again_is_not_new` and `a_file_that_failed_before_is_not_new` (Task 1).
3. **Local-only switched on after an alert exists.** The weekly run must send zero requests, end `skipped` (never `failed`, so it never leads to a hold), and the section must say why.
   - Test `a_blocked_run_sends_nothing_and_is_skipped` (Task 7), which asserts `FakeTransport.sent` is empty.
4. **One API down must not hide the others, double-list a paper, or hold the alert.**
   - The papers that came back are posted at once. Only the failed API is retried.
   - A paper the retry returns again is not added twice.
   - A run fails only when no API answered.
   - Tests `one_api_failing_posts_the_rest_and_retries_only_it` and `a_retry_that_returns_the_same_paper_adds_nothing` (Tasks 6–7).
5. **A paper seen first by title, then with a DOI, is one paper.** The same holds for a paper the user added with "Add to library", which must not come back as a "new file" too.
   - Tests `a_paper_known_by_title_is_not_new_when_it_gains_a_doi` (Task 6) and `a_paper_added_to_the_library_is_not_also_a_new_file` (Task 5).

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/shodh-rag/src/folder_sync.rs` (modify, Task 1) | `SyncReport.added`/`added_more`; the manifest's `added_baseline` and `reread`; rename matching. |
| `crates/shodh-rag/src/audit/store.rs` (modify, Task 2) | Migration 20: `feed_item`. |
| `crates/shodh-rag/src/whats_new/mod.rs`, `store.rs` (create, Task 2) | `FeedStore`: insert-if-absent, unposted rows, mark posted, seen, dismissed, prune, delete by job. |
| `crates/shodh-rag/src/inbox.rs`, `app/src/features/inbox/model.ts` (modify, Task 2) | `InboxKind::WhatsNew` (`whats_new`). |
| `crates/shodh-rag/src/engine/runner.rs` (modify, Task 3) | `Handled::RetryAt`. |
| `app/src-tauri/src/engine_commands.rs` (modify, Task 4) | One `Engine`, a handler list, one loop. |
| `app/src-tauri/src/whats_new.rs` (create, Tasks 5, 7, 8) | `WhatsNew` service: the `folder_watch` and `paper_alert` handlers, digests, holds, and the commands. |
| `app/src-tauri/src/file_watcher.rs` (modify, Task 5) | Hand `added` to What's new after a sync; delete a removed source's job and rows; skip the `indexing:` item covered by What's new (D6). |
| `crates/shodh-rag/src/harness/web/papers.rs` (modify, Task 6) | `alert_requests`; `pub fn same_paper`; `PaperApi`. |
| `crates/shodh-rag/src/whats_new/alerts.rs`, `pace.rs`, `library.rs` (create, Task 6) | The alert check, the per-API pacer, and the library's paper identities. |
| `app/src-tauri/src/agent_tools/research.rs` (modify, Task 8) | `download_into` extracted from `download_file`'s `execute` (behaviour kept). |
| `app/src-tauri/src/agent_tools/whats_new.rs` (create, Tasks 7–8), `agent_tools/mod.rs` (modify) | Tools; `LIBRARY_SECTIONS` gains `whats_new`; `HostEffects::whats_new`. |
| `app/src-tauri/src/lib.rs`, `permissions/main-window.json`, `agent-coverage.json` (modify, Tasks 7–8) | Registration, ACL, coverage, `navigation.librarySections.whats_new`. |
| `app/src/features/whatsNew/types.ts`, `api.ts`, `consent.ts` (create, Task 7), `model.ts` (create, Task 9) | Wire types; command wrappers with literal names; the consent line; the pure view model. |
| `app/src/features/whatsNew/WhatsNewSection.tsx`, `PaperAlertDialog.tsx` (create, Task 10) | The Library section and the consent dialog. |
| `app/src/features/library/LibraryView.tsx`, `app/src/features/agent/navigation.ts` (modify, Tasks 8, 10) | Mount the section; the `whats_new` section target. |
| `app/tests/whatsNew.test.ts` (create), `app/tests/inbox.test.ts` (modify) | Frontend tests. |

---

### Task 1: `SyncReport.added`: what a sync indexed for the first time

**Files:**
- Modify: `crates/shodh-rag/src/folder_sync.rs`. The changes:
  - `SyncReport` at line 85;
  - `Manifest`/`ManifestRef` at line 476;
  - `forget_file` at line 578;
  - `sync_folder_scanned` at lines 1640–1865.
- Test: the `folder_sync.rs` tests module.

**Interfaces (produces):**
```rust
pub const MAX_ADDED: usize = 200;
pub struct SyncReport {
    /* existing fields unchanged */
    /// Files indexed for the first time in this sync, as spelled on disk, at most MAX_ADDED.
    pub added: Vec<String>,
    #[serde(rename = "addedMore")]
    pub added_more: usize,
}
// Manifest gains (serde default):
//   added_baseline: bool  -- default true for manifests saved before this field existed
//   reread: BTreeSet<String>
```

**Rules:**
- **A sync reports `added` only when its manifest existed at the start and has `added_baseline == true`.**
  - A sync that starts without a manifest (a new folder, or the legacy `known_from_index` path) saves `added_baseline: false`. It sets `added_baseline: true` only when it ends **without stopping**.
  - A legacy manifest without the field reads as `true` (`#[serde(default = "yes")]`). Files that arrived while an older build was closed are then genuinely new.
- **A key is added when all of these hold:**
  - it indexed successfully (`Synced::Indexed`) in this sync;
  - it was absent from the start-of-sync `known` and from `failures`;
  - it is not in `reread`;
  - it is not matched as a rename.
- **Rename matching:** take the multiset of `known[k]` stamps over `plan.remove`. An added key whose on-disk `FileStamp` equals one of them consumes that stamp and is not added.
- `forget_file` inserts the key into `reread`. The sync removes a key from `reread` once that file is processed.
- **Every manifest write states both fields:**
  - the per-file `save` closure writes the sync's current values;
  - `forget_file` and the adoption save (`folder_sync.rs:993`, through `Manifest::borrowed()`) keep the loaded manifest's values;
  - a manifest an adoption creates for a source that had none gets `added_baseline: false`. Only a completed sync sets it true.
- `added` is capped at `MAX_ADDED`, in plan order. `added_more` = the rest.

- [ ] **Step 1: Write the failing tests:**
  - `a_first_sync_is_a_baseline`:
    - a folder with 3 files and no manifest: the first sync has `added == []`, `indexed == 3`;
    - add `d.md`: the second sync has `added == [<path of d.md>]`.
  - `an_interrupted_first_sync_stays_a_baseline`:
    - the first sync's `stop` returns true after 1 file;
    - the next sync, which indexes the other 2, still has `added == []`;
    - a third sync after adding `e.md` reports only `e.md`.
  - `a_changed_file_is_not_new`: touch and rewrite `a.md`; `indexed == 1`, `added == []`.
  - `a_renamed_file_is_not_new`: `std::fs::rename(a.md, a2.md)` keeps the stamp; `removed == 1`, `indexed == 1`, `added == []`.
  - `a_forgotten_file_read_again_is_not_new`: `store.forget_file("s1", key_of(a.md))`, then a sync gives `indexed == 1`, `added == []`, and `reread` is empty afterwards.
  - `a_file_that_failed_before_is_not_new`: a file that failed in sync 2, fixed and indexed in sync 3, gives `added == []`.
  - `added_is_capped_with_a_count_of_the_rest`: 205 new files give `added.len() == 200`, `added_more == 5`.
  - `an_adopted_manifest_keeps_its_baseline_flag`: after an adoption save, a source that had finished its baseline still reports added files.
  - `a_legacy_manifest_reports_files_added_since`: a manifest JSON without `added_baseline` or `reread`, plus one new file on disk, gives `added == [that file]`.
  - `sync_report_serialises_added_more_in_camel_case`: `serde_json::to_value(report)["addedMore"]` exists, and `"new_failures"` keeps its current spelling.
  - Existing whole-struct expectations (`first` at line 2370 and the "Added, changed, deleted and renamed" sync after it) gain `added`/`added_more` at their real values. In that test, `rules.txt` is added and the renamed file is not.
- [ ] **Step 2: Run** CI "Tests". Expected: the new tests fail.
- [ ] **Step 3: Implement** the rules above in `sync_folder_scanned`, `Manifest` and `forget_file`.
- [ ] **Step 4: Run** CI "Tests". Expected: every `folder_sync` test passes.
- [ ] **Step 5: Commit** "Folder sync reports the files it indexed for the first time; a folder's first sync is a baseline".

### Task 2: Migration 20 (`feed_item`), `FeedStore`, and the `whats_new` Inbox kind

**Files:**
- Modify:
  - `crates/shodh-rag/src/audit/store.rs`: append migration 20.
  - `crates/shodh-rag/src/lib.rs`: `pub mod whats_new;`.
  - `crates/shodh-rag/src/inbox.rs`: `WhatsNew => "whats_new"`. Migration 19 already allows it in the CHECK.
  - `app/src/features/inbox/model.ts`: `'whats_new'` in `InboxKind` and `KINDS`, and `FAILED_STOPS.whats_new = () => false`.
- Create: `crates/shodh-rag/src/whats_new/mod.rs`, `store.rs`.
- Test: `audit/store.rs`, `whats_new/store.rs`, `inbox.rs` and `app/tests/inbox.test.ts`.

**Schema (migration 20):**
```
feed_item(id TEXT PK, kind TEXT NOT NULL CHECK (kind IN ('file','paper')),
  source_ref TEXT NOT NULL, job_id TEXT NOT NULL, title TEXT NOT NULL, detail TEXT NULL,
  found_at TEXT NOT NULL, posted_run_id TEXT NULL, seen_at TEXT NULL, dismissed_at TEXT NULL,
  link_json TEXT NOT NULL DEFAULT '{}', data_json TEXT NOT NULL DEFAULT '{}')
UNIQUE INDEX feed_item_ref ON feed_item(kind, source_ref)
INDEX feed_item_job ON feed_item(job_id, posted_run_id)
INDEX feed_item_found ON feed_item(found_at)
```
- `source_ref` is the normalized index key for a file. For a paper it is the strongest identifier: `doi:<doi>`, else `arxiv:<id>`, else `title:<title_key>`.
- `data_json` for a paper holds `{doi, arxivId, titleKey, year, authors, venue, abstract, pdfUrl, landingUrl, sources, relevance, addedPath?}`.

**Interfaces (produces), in `whats_new::store`:**
```rust
pub const FEED_RETENTION_DAYS: i64 = 90;
pub enum FeedKind { File, Paper }
pub struct NewFeedItem { pub kind: FeedKind, pub source_ref: String, pub job_id: String,
    pub title: String, pub detail: Option<String>, pub link: Value, pub data: Value }
pub struct FeedItem { /* every column, camelCase on the wire */ }
impl FeedStore {
    pub fn open(path: &Path, key: Option<&AuditKey>) -> FeedResult<Self>;
    pub fn insert_if_absent(&self, items: &[NewFeedItem], now: DateTime<Utc>) -> FeedResult<Vec<FeedItem>>; // the inserted ones
    pub fn unposted(&self, job_id: &str) -> FeedResult<Vec<FeedItem>>;
    pub fn mark_posted(&self, ids: &[String], run_id: &str) -> FeedResult<()>;
    pub fn of_job_since(&self, job_id: &str, since: DateTime<Utc>) -> FeedResult<Vec<FeedItem>>; // not dismissed
    pub fn papers(&self) -> FeedResult<Vec<FeedItem>>;          // dismissed included: de-dup tombstones
    pub fn list(&self, limit: usize) -> FeedResult<Vec<FeedItem>>; // not dismissed, newest first
    pub fn mark_seen(&self, ids: &[String], now: DateTime<Utc>) -> FeedResult<usize>;
    pub fn set_dismissed(&self, id: &str, dismissed: bool, now: DateTime<Utc>) -> FeedResult<bool>;
    pub fn set_data(&self, id: &str, data: &Value) -> FeedResult<bool>;
    pub fn delete_job(&self, job_id: &str) -> FeedResult<usize>;
    pub fn prune(&self, now: DateTime<Utc>) -> FeedResult<usize>; // found_at older than 90 days, dismissed or not
}
```

- [ ] **Step 1: Write the failing tests:**
  - `migration_20_adds_the_feed_and_keeps_every_earlier_table`: a v19 database with one Inbox row and one engine job migrates, and both rows are kept.
  - `a_second_item_with_the_same_ref_is_ignored`: `insert_if_absent` twice returns 1 item, then 0.
  - `unposted_rows_are_posted_once`.
  - `dismissed_papers_stay_as_tombstones_until_pruned`: `list` hides the dismissed paper; `papers()` still returns it; after `prune(now + 91 days)` it is gone.
  - `deleting_a_job_deletes_its_rows`.
  - `a_whats_new_inbox_item_is_stored`.
  - In `inbox.test.ts`, `a whats_new item parses and lands in info`: `{kind:'whats_new', status:'done'}` and `{status:'failed'}` both give `'info'`. Extend the 44-row table from PR 2 by the 4 `whats_new` rows: every one is Info except `needs_you`.
- [ ] **Step 2: Run** CI "Tests" and "Frontend Typecheck + Tests". Expected: the new tests fail.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the same CI jobs. Expected: they pass.
- [ ] **Step 5: Commit** "What's new: a feed table in shodh.db and a whats_new Inbox kind".

### Task 3: `Handled::RetryAt`: a run waits until a time it names

**Files:**
- Modify: `crates/shodh-rag/src/engine/runner.rs` (`Handled`, `apply`).
- Test: the `runner.rs` tests module.

**Interfaces (produces):**
```rust
Handled::RetryAt { at: DateTime<Utc>, reason: WaitReason /* Backoff | Offline */, code: String, detail: String }
```
- It waits as `reason` until `at`, with `attempt + 1`, and counts against `max_attempts` exactly as `Transient` does.
- Past `max_attempts` the run fails through `fail()` (streak, hold). The handler sees `run.attempt` and can end a run as `Succeeded` or `Skipped` before that point (Task 7 does).

- [ ] **Step 1: Write the failing tests:**
  - `retry_at_waits_until_the_time_given`: the run is resumed by `tick` at `at`, not one minute earlier.
  - `retry_at_counts_attempts_and_then_fails`.
  - `an_offline_wait_reads_as_offline`: `wait_reason == Offline`.
- [ ] **Step 2: Run** CI "Tests". Expected: the new tests fail.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** CI "Tests". Expected: every engine test passes, the existing ones unchanged.
- [ ] **Step 5: Commit** "Engine: a run can wait until a time it names".

### Task 4: One engine and one loop for every job kind in the app

**Files:**
- Modify:
  - `app/src-tauri/src/engine_commands.rs`;
  - `app/src-tauri/src/task_repeat.rs`: `Repeats::new` takes the shared `Arc<Engine>` (it already does), and `Repeats::handler()` exposes its `Arc<dyn JobHandler>`.
- Test: the `engine_commands.rs` tests module (new).

**Interfaces (produces):**
- `EngineState`:
  - `engine: OnceCell<Arc<Engine>>`, opened once on the audit database;
  - `repeats: OnceCell<Arc<Repeats>>`;
  - `whats_new: OnceCell<Arc<WhatsNew>>` (Task 5).
- `pub async fn engine(app) -> Result<Arc<Engine>, String>`.
- `pub async fn whats_new(app) -> Option<Arc<WhatsNew>>` and `whats_new_for_command(app) -> Result<Arc<WhatsNew>, String>`, mirroring `repeats`/`repeats_for_command`.
- `fn handlers(&self) -> Vec<Arc<dyn JobHandler>>`.
- `pub async fn tick_all(engine: &Engine, handlers: &[Arc<dyn JobHandler>]) -> Vec<(JobKind, EngineError)>`: one kind's error never stops the others.
- `pub fn next_wake_all(engine: &Engine, kinds: &[JobKind]) -> Option<DateTime<Utc>>`: the minimum over the kinds.

**Rules:**
- The loop in `spawn` calls `repeats.launch()` (the `LaunchReport` drop guard stays exactly as it is), then `WhatsNew::launch()` (Task 5), then loops on `tick_all` and `next_wake_all`.
- `wake(app)` is unchanged.

- [ ] **Step 1: Write the failing tests** (temp `shodh.db`, two recording handlers of different kinds):
  - `one_tick_serves_every_kind`;
  - `next_wake_is_the_earliest_over_every_kind`;
  - `a_failing_kind_does_not_stop_the_others`.
  - `a_failing_engine_still_lets_the_missed_reminder_summary_through` (PR 1) must pass unchanged.
- [ ] **Step 2: Run** CI "App tests and doc tests". Expected: the new tests fail.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** CI "App tests and doc tests". Expected: they pass, and every `task_repeat` test still passes.
- [ ] **Step 5: Commit** "Engine: one engine and one loop for every kind of job".

### Task 5: Folder watch: new files become feed rows and one Info item per run

**Files:**
- Create: `app/src-tauri/src/whats_new.rs` (the `WhatsNew` service, `FolderWatch` handler).
- Modify:
  - `app/src-tauri/src/file_watcher.rs`:
    - `sync_once` at about line 702: after the result, `whats_new::files_added(app, &source, &report)`;
    - `inbox_item`: D6;
    - `remove_source` at line 262: `whats_new::source_removed`.
  - `app/src-tauri/src/engine_commands.rs` (register the handler).
- Test: the `whats_new.rs` tests module (temp `shodh.db`, a recording `WhatsNewSink { post, resolve }`, a `ManualClock`).

**Interfaces (produces):**
```rust
pub const FOLDER_DIGEST_PREFIX: &str = "whats_new:files:"; // + <source_id>:<yyyymmdd local>
pub trait WhatsNewSink: Send + Sync { async fn post(&self, item: NewInboxItem); async fn resolve(&self, id: &str); fn changed(&self); }
pub const WHATS_NEW_CHANGED_EVENT: &str = "whats-new-changed";
impl WhatsNew {
    pub async fn files_added(&self, source_id: &str, folder: &str, added: &[String], added_more: usize) -> Result<usize, String>;
    pub async fn source_removed(&self, source_id: &str);
    pub fn folder_watch_on(&self, source_id: &str) -> Result<bool, String>;   // no job yet = on
    pub async fn set_folder_watch(&self, source_id: &str, folder: &str, on: bool) -> Result<(), String>;
    pub async fn launch(&self);   // prune the feed; post what a crash left unposted
}
```

**Rules:**
- **The job.** `folder_watch` has `subject = source_id`, `schedule = None` and `trigger_json = {"files_added": {"sourceId": ...}}`.
  - It is made on the first non-empty `files_added` call for a folder that has no job. A job that exists and is disabled means the user turned the folder's watch off: `files_added` then writes nothing.
  - Further settings: `overlap = queue_one`, `catch_up = skip`, `max_attempts = 3`, `created_by = system`.
- **`files_added`.**
  1. Insert one `file` row per path: `source_ref = normalize_source_path`, `title = file name`, `detail = folder name`, `link = {view:'library', target:{kind:'library', section:'whats_new', item:<row id>}}`.
  2. Skip a path that a `paper` row's `data.addedPath` names (Review Focus 5).
  3. If any row was inserted, call `engine.event(folder_watch, job_id)`.
  - `added_more > 0` is stored on the newest row's `data.more`, so the digest counts it.
- **The handler (`run`)** is state-based:
  - no unposted rows gives `Handled::Nothing`;
  - otherwise it posts (upserts) the day's digest, **then** `mark_posted`, and returns `Succeeded { summary: "3 new files" }`.
  - The order makes a crash between the two re-post the same content (an upsert), never lose it.
- **The digest:**
  - id `whats_new:files:<source>:<local yyyymmdd>`, kind `whats_new`, status `done`;
  - title `N new files in <Folder>` (`1 new file in <Folder>`), where N counts the day's rows that are not dismissed, plus `more`;
  - detail: the first 3 file names joined by ", ", then ` and N more`;
  - link: library `{section:'whats_new'}`;
  - `data = {jobId, sourceId, day, count}`, with `announce: false`.
- **D6.** `inbox_item` returns `None` for a watcher sync when all of these hold:
  - What's new is on for the folder;
  - the report's only change is `added` (`indexed == added.len() + added_more`, with `removed == 0`, `new_failures == 0`, no failures and nothing unreadable).
  Any other sync keeps today's item.
  - When D6 suppresses the item, it also resolves any existing `indexing:<source>` item. A failed sync (`failure:'sync'`, Attention) is therefore cleared by the next sync that recovers, even one that only added files.
- **Removal.** `source_removed` deletes the job, its rows (`FeedStore::delete_job`) and every `whats_new:files:<source>:` Inbox item (`resolve_prefixed`).

- [ ] **Step 1: Write the failing tests:**
  - `added_files_become_rows_and_one_digest`: 3 paths give 3 rows and exactly one posted item, titled "3 new files in Research Papers".
  - `two_syncs_on_one_day_update_one_digest`: 2 paths, then 1 path, give one item id titled "3 new files…" and 2 succeeded runs.
  - `a_second_report_of_the_same_file_adds_nothing`: no new row, no run row (`Nothing`).
  - `a_crash_between_post_and_mark_reposts_the_same_digest`.
  - `turning_a_folder_off_records_nothing`: no rows, no event.
  - `a_paper_added_to_the_library_is_not_also_a_new_file`.
  - `removing_a_source_deletes_its_job_rows_and_digests`.
  - `a_sync_that_only_added_files_posts_no_indexing_item_when_watched`, plus its pair `…_still_posts_it_when_not_watched` (`file_watcher::inbox_item` unit tests).
  - `a_recovered_sync_that_only_added_files_clears_the_failed_item`: a failed `indexing:<source>` item, then a sync whose only change is added files, leaves no `indexing:<source>` item.
- [ ] **Step 2: Run** CI "App tests and doc tests". Expected: the new tests fail.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** CI "App tests and doc tests". Expected: they pass.
- [ ] **Step 5: Commit** "What's new: files new in a watched folder, one Inbox item per folder and day".

### Task 6: The paper alert core in shodh-rag

**Files:**
- Modify: `crates/shodh-rag/src/harness/web/papers.rs`:
  - add `PaperApi` and `alert_requests`;
  - `same_paper` becomes `pub fn same_paper`;
  - `pub fn title_key(title) -> String`, the normalization `same_paper`'s title test uses.
- Create: `crates/shodh-rag/src/whats_new/alerts.rs`, `pace.rs`, `library.rs`.
- Test: the tests modules of those files, with `harness::web::client::testing::{client, FakeResolver, FakeTransport, Canned}` and the `linear-attention` fixtures.

**Interfaces (produces):**
```rust
// papers.rs
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)] #[serde(rename_all = "snake_case")]
pub enum PaperApi { Arxiv, OpenAlex, SemanticScholar }   // name(): "arXiv" | "OpenAlex" | "Semantic Scholar"
pub const ALERT_RESULTS_PER_API: usize = 25;
pub fn alert_requests(query: &str, since: &BTreeMap<PaperApi, NaiveDate>, now: DateTime<Utc>)
    -> Result<BTreeMap<PaperApi, Url>, WebError>;   // one URL per API, no OpenAlex title request

// whats_new/library.rs
pub struct LibraryPapers { dois: HashSet<String>, arxiv_ids: HashSet<String>, title_keys: HashSet<String> }
impl LibraryPapers {
    pub fn from_research(db: &ResearchDb, indexed_files: &[String]) -> ResearchResult<Self>;
    pub fn contains(&self, p: &Paper) -> bool;
}

// whats_new/pace.rs
pub struct ApiPacer;  // process-wide; pub async fn wait_turn(&self, api: PaperApi)
pub const MIN_SPACING: [(PaperApi, Duration); 3] = [(Arxiv, 3 s), (OpenAlex, 1 s), (SemanticScholar, 1 s)];

// whats_new/alerts.rs
pub const NEW_PAPERS_PER_RUN: usize = 10;
pub struct AlertInput { pub query: String, pub consent: Consent, pub checked_through: BTreeMap<PaperApi, NaiveDate>,
    pub run_checked: Option<(String /* run id */, BTreeSet<PaperApi>)> }
pub enum ApiOutcome { Answered(usize), Failed { kind: FailKind /* Offline | RateLimited { retry_after } | Status | Invalid */, detail: String } }
pub struct AlertCheck { pub new: Vec<Paper>, pub outcomes: BTreeMap<PaperApi, ApiOutcome> }
pub async fn check_alert(client: &SafeClient, pacer: &ApiPacer, input: &AlertInput, apis: &BTreeSet<PaperApi>,
    now: DateTime<Utc>, library: &LibraryPapers, earlier: &[FeedItem], scorer: Option<SharedScorer>) -> AlertCheck;
pub fn consent_line(words: &str) -> String;  // CONSENT_TEMPLATE with {words}
```

**Request shapes.** These were checked against each API's own documentation on 2026-11-23; tests pin them as exact URLs.
- **arXiv:**
  - `search_query = (<arxiv_search_query(query)>) AND submittedDate:[<since>0000 TO <now yyyymmddHHMM>]`;
  - `sortBy=submittedDate`, `sortOrder=descending`, `start=0`, `max_results=25`.
  - The manual asks for a 3-second delay between calls.
- **OpenAlex:**
  - `search=<query>`, `filter=from_publication_date:<YYYY-MM-DD>`, `per-page=25`, `select=OPENALEX_SELECT`.
  - No `sort` and no `from_created_date`: the latter needs a paid Member+ plan.
  - Keyless use is 10 requests a second per IP, within a small daily budget.
- **Semantic Scholar:** `/paper/search` with `query=<words>`, `publicationDateOrYear=<YYYY-MM-DD>:`, `limit=25`, `fields=SEMANTIC_SCHOLAR_FIELDS`. That endpoint has no sort.
- **`since` per API** is `checked_through − overlap`: 2 days for arXiv, 7 for OpenAlex and Semantic Scholar, whose indexing lags. With no `checked_through` (the first run), `since = now − 14 days` (Q5).

**Rules:**
- A paper is new when `!library.contains(p)` and no earlier `paper` row matches it, dismissed rows included.
  - A match is any shared DOI, arXiv id or `title_key`: earlier rows are turned back into `Paper`s from `data_json`, then compared with `same_paper`.
- The new papers are ranked with `rank_papers(query, merge(lists), scorer, NEW_PAPERS_PER_RUN)`. Ranking drops weak matches.
- APIs in `run_checked` for the current run are not asked again.
- `LibraryPapers::from_research` reads:
  - every `citation_scans.scan_json` identity (DOI, arXiv id, title);
  - `filename_arxiv_id` of every indexed file name;
  - each paper row's `data.addedPath` paper.

  Without a graph build, only the last two apply (D10).
- **Failure kinds:**
  - `WebError::{Unresolvable, Timeout}` and connection errors are `Offline`;
  - status 429 is `RateLimited`, with `retry_after` taken from the `Retry-After` header when present;
  - other statuses are `Status`;
  - a parse error is `Invalid`.

- [ ] **Step 1: Write the failing tests:**
  - `alert_requests_are_one_per_api_bounded_by_date`: an exact URL per API for "linear attention KV cache" and a fixed `since`/`now`; no `openalex_title`.
  - `alert_requests_carry_no_identity`: run `check_alert` on fakes. Then:
    - every request in `FakeTransport.sent` has an empty `headers` list (the alert code adds none: no `user-agent`, no `from`);
    - no URL contains `mailto` or `@`;
    - `!client::USER_AGENT.contains('@')`. The user agent is set only by `ReqwestTransport`'s builder.
  - `new_papers_skip_the_library_and_earlier_items`: from the `linear-attention` fixtures:
    - a `LibraryPapers` holding one fixture paper's arXiv id;
    - an earlier row holding another's DOI;
    - neither is in `new`.
  - `the_same_paper_from_three_apis_is_one`: with the three fixtures, no two `new` papers satisfy `same_paper`.
  - `a_paper_known_by_title_is_not_new_when_it_gains_a_doi`: an earlier row with only a `titleKey`, and an API record with the same title plus a DOI, give no new paper.
  - `a_dismissed_paper_does_not_come_back`.
  - `apis_already_checked_this_run_are_not_asked_again`: `run_checked = {OpenAlex}` gives exactly 2 sent requests.
  - `one_api_failing_reports_which`: Semantic Scholar answers 503, so `outcomes[SemanticScholar]` is `Failed{Status}` and the others are `Answered`.
  - `a_rate_limit_reads_retry_after`.
  - `the_pacer_spaces_arxiv_by_three_seconds`: under `#[tokio::test(start_paused = true)]`, two `wait_turn(Arxiv)` calls are ≥ 3 s apart, and `OpenAlex` ≥ 1 s.
  - `library_papers_without_a_graph_use_file_names`.
  - `the_consent_line_names_all_three_apis`: `consent_line("linear attention KV cache")` equals the literal, and contains "arXiv", "OpenAlex" and "Semantic Scholar".
- [ ] **Step 2: Run** CI "Tests". Expected: the new tests fail.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** CI "Tests". Expected: they pass. The `search_papers` and `papers_calibration` tests are unchanged and pass.
- [ ] **Step 5: Commit** "Paper alerts: date-bounded checks of arXiv, OpenAlex and Semantic Scholar, de-duplicated against the library".

### Task 7: The paper alert job, its commands, its tools and the consent

The coverage gate (`agent_coverage.rs:237`) refuses a command that maps to an agent tool not yet registered. Each tool therefore lands in the same task as its command.

**Files:**
- Modify:
  - `app/src-tauri/src/whats_new.rs`: the `PaperAlerts` handler and the commands.
  - `app/src-tauri/src/agent_tools/mod.rs`: `HostEffects::whats_new(&self) -> Option<Arc<crate::whats_new::WhatsNew>>`, default `None` (as `task_repeats`). `TauriEffects` and the test `Recorder` implement it. Register the new module.
  - `app/src-tauri/src/lib.rs`, `permissions/main-window.json`, `agent-coverage.json`.
- Create:
  - `app/src-tauri/src/agent_tools/whats_new.rs`: the three alert tools.
  - `app/src/features/whatsNew/types.ts`: the wire types `FeedItem`, `AlertView`, `AlertState` and `WhatsNewView`, used by `api.ts`, `model.ts` and the components.
  - `app/src/features/whatsNew/api.ts`: wrappers for this task's commands, with literal names.
  - `app/src/features/whatsNew/consent.ts`: `consentLine`, pure.
- Test: the `whats_new.rs` tests module, using a `SafeClient` over an app-side recording `HttpTransport` and `Resolver` (both traits are public); `agent_tools/whats_new.rs` tests; `app/tests/whatsNew.test.ts`.

**Commands:**
- `paper_alert_create(words: String) -> AlertView`:
  - refused while `web_block_reason` is `Some`, with that reason;
  - refused for empty words, or words over 200 characters;
  - refused past `MAX_ALERTS = 10`, with the message "You have 10 paper alerts. Remove one to add another.";
  - stores `consent = {text: consent_line(words), at: now}`;
  - the job has `schedule = Weekly { days: [Mon], at: 09:00 }` (Q4), `catch_up = once`, `overlap = skip`, `created_by = user`;
  - the job has `max_attempts = 5`. That number is only the engine's safety stop: the handler's own counter, below, ends a run before it is reached.
  - It starts the first check at once with `Engine::event(handler, job_id)`, a `Trigger::Event` run (Q5). The handler is state-based, so the event needs no payload.
- `paper_alert_update(id, words: Option<String>, enabled: Option<bool>) -> AlertView`:
  - new words are refused under Local-only, store a new consent and keep `checked_through`;
  - `enabled: true` clears a hold and the streak, and resolves `paper-alert-hold:<id>` (D11).
- `paper_alert_delete(id) -> bool`: deletes the job and its rows, and resolves its Inbox items.
- `AlertView`:
  - `{id, words, consentText, state, lastCheckedAt, lastSummary, nextCheckAt, failedApis, blockedReason}`;
  - `state` is one of `checking | waiting_retry | offline | active | off | held | blocked`.
  - `blocked` is computed from the policy at read time.
  - A run waiting with code `after_wake` reads `checking`, never `offline`.

**The handler (`run`).** `AlertInput.run_checked` is `{run_id, apis: BTreeSet<PaperApi>, retries: u32}`. It is reset when `run.id` differs from the stored run id. In order:
1. **The policy is blocked:** return `Skipped { summary: "Not checked: Local-only mode is on." }` (or the web-access reason) and send nothing.
2. **A `catch_up` run seen for the first time** (no `run_checked` for it): store `run_checked`, then return `RetryAt { at: now + 2 min, reason: Backoff, code: "after_wake" }` (§5.3). This wait does not count toward `retries`.
3. **Otherwise** `check_alert` over the APIs not yet in `run_checked.apis`. Then:
   - insert the new papers as rows;
   - post (upsert) `whats_new:papers:<job>:<run>` when this run has found ≥ 1 paper so far:
     - title `4 new papers for “linear attention”`;
     - detail `From arXiv and OpenAlex. Semantic Scholar didn't answer; Shodh tries again at 11:40.`, in plain words;
   - `mark_posted`;
   - write `checked_through[api] = today` for each answered API, and add it to `run_checked.apis`;
   - prune the feed.
4. **What it returns.** `MAX_API_RETRIES = 3` is counted in `run_checked.retries`:
   - every API answered: `Succeeded { summary: "Found 4 new papers" | "Nothing new" }`;
   - some answered and some failed, with `retries < 3`: `retries += 1`, then `RetryAt { at: now + max(1 h, retry_after), reason: Backoff }`. The items already posted are kept;
   - some answered, retries used up: `Succeeded { summary: "Found 4 new papers; Semantic Scholar didn't answer" }`;
   - none answered and every failure is `Offline`: `RetryAt { now + 15 min, Offline }` while `retries < 3`, then `Skipped { "Not checked: no internet connection" }`. Never `Failed`, so being offline never holds an alert;
   - none answered, otherwise: `RetryAt` 1 h while `retries < 3`, then `Permanent` (counts toward the hold).
- **`held`:** posts `paper-alert-hold:<job>`, kind `automation`, `needs_you`:
  - title `The alert for “linear attention” stopped: arXiv, OpenAlex and Semantic Scholar failed 3 weeks in a row.`;
  - `data = {jobId, fix:'resume', resume:'paper_alert'}`.

**Tools:**
- `create_paper_alert {words}` and `update_paper_alert {id, words?, enabled?}`:
  - Write tier, with `must_confirm` true;
  - the preview is the consent line, plus "The first check runs now." on create;
  - refused with the policy's reason under Local-only.
- `delete_paper_alert {id}`: Write tier, local edit. Preview: "Stop the alert for “x”. The papers it found stay in What's new."
- After saving, each tool highlights the alert row it changed (§12 item 5) through `show_in_library` once Task 8 adds the section. In this task it emits no navigation.

**Coverage:**

| Command | Maps to |
|---|---|
| `paper_alert_create` | `create_paper_alert` |
| `paper_alert_update` | `update_paper_alert` |
| `paper_alert_delete` | `delete_paper_alert` |

- [ ] **Step 1: Write the failing tests:**
  - `a_blocked_run_sends_nothing_and_is_skipped`: Local-only is on, so the recording transport's `sent` is empty, the run is `skipped`, and `failure_streak` is 0.
  - `local_only_refuses_creating_and_rewording_an_alert`. Also asserted: web access off refuses with its own reason (Q7).
  - `creating_an_alert_stores_the_consent_and_checks_at_once`: `consentText` equals `consent_line(words)`, and one `event` run is started.
  - `one_api_failing_posts_the_rest_and_retries_only_it`:
    - first pass: the posted item names arXiv and OpenAlex, and the run waits `backoff` until +1 h;
    - second pass: only Semantic Scholar is asked, and the same item id is updated.
  - `three_retries_then_the_run_succeeds_with_what_it_found`. Also asserted: after a catch-up wait there are still 3 retries.
  - `a_retry_that_returns_the_same_paper_adds_nothing`.
  - `offline_three_weeks_running_never_holds`.
  - `three_failed_weeks_hold_with_one_attention_item_and_resume_clears_it`.
  - `nothing_new_posts_no_inbox_item` (Q4).
  - `a_catch_up_run_waits_two_minutes_after_a_wake_and_reads_checking`.
  - `ten_alerts_is_the_most`.
  - Tool tests:
    - `create_paper_alert_asks_with_the_consent_line`;
    - `update_with_new_words_asks_again`;
    - `paper_alert_tools_are_refused_in_local_only`, with the reason passed through.
  - In `whatsNew.test.ts`, `the consent line names all three APIs`: `consentLine('linear attention KV cache')` equals the same literal as the Rust test.
- [ ] **Step 2: Run** CI "App tests and doc tests" and "Frontend Typecheck + Tests". Expected: the new tests fail.
- [ ] **Step 3: Implement.** Register the three commands in all four places, and the three tools.
- [ ] **Step 4: Run** the same CI jobs. Expected: they pass, including the ACL and coverage gates.
- [ ] **Step 5: Commit** "Paper alerts: weekly checks the user consents to, refused in Local-only mode".

### Task 8: The What's new commands, folder watch, Add to library, the `whats_new` tool and navigation

`whats_new_list`, the `whats_new` tool, `LIBRARY_SECTIONS` on both sides, `navigation.librarySections.whats_new` and the `LibraryView` arm all land here. The coverage gate checks them together.

**Files:**
- Modify:
  - `app/src-tauri/src/whats_new.rs`.
  - `app/src-tauri/src/agent_tools/research.rs`: extract `pub(crate) async fn download_into(client: &SafeClient, url: &Url, plan: &FolderPlan, file_name: Option<&str>) -> Result<Downloaded, ToolError>` from `DownloadFileTool::execute` (line 642 to the end of the write). `execute` calls it, so its behaviour and tests are unchanged.
  - `app/src-tauri/src/agent_tools/whats_new.rs`: the `whats_new` and `set_folder_watch` tools.
  - `app/src-tauri/src/agent_tools/mod.rs`: `LIBRARY_SECTIONS: [&str; 5]`, ending in `"whats_new"`.
  - `app/src/features/agent/navigation.ts`: `LIBRARY_SECTIONS` adds `'whats_new'`.
  - `app/src/features/library/LibraryView.tsx`: the `target.section === 'whats_new'` arm. It reveals `agentItemKey('library-section','whats_new')` or `agentItemKey('feed-item', item)`, with a `missed` message until Task 10 renders the section.
  - `lib.rs`, `permissions/main-window.json`, `app/src/features/whatsNew/api.ts`.
  - `agent-coverage.json`, with these entries:
    - the commands below;
    - `navigation.librarySections.whats_new = ["whats_new"]`.
- Test: `whats_new.rs`; `agent_tools/whats_new.rs` (the `expect_navigation` helper); the `research.rs` tests, unchanged and passing; the coverage gate.

**Commands:**
- `whats_new_list() -> WhatsNewView`: maps to `whats_new`. It returns:
  - `items: FeedItem[]`: at most 200, newest first, with file rows whose file no longer exists dropped (D14);
  - `alerts: AlertView[]`;
  - `folders: {sourceId, folder, on}[]`;
  - `blockedReason`.
- `whats_new_mark_seen(ids)`: **excluded** ("What the user has seen is theirs, as with `inbox_dismiss`.").
- `whats_new_dismiss(id, restore: bool)`: **excluded**, with the same reason. `restore` backs Undo.
- `folder_watch_set(source_id, on)`: maps to `set_folder_watch`.
- `whats_new_add_to_library(item_id, source_id)`: maps to `download_file`.
  - It is refused under Local-only, for a non-paper row, for a row with no `pdfUrl` ("This paper has no open-access PDF."), and for a source that is not a watched folder.
  - It downloads only the API-reported `data.pdfUrl` into the folder's root, named `<title_key truncated to 80>.pdf`.
  - It then sets `data.addedPath` and returns the path. The watcher indexes the file; Task 5's rule keeps it from showing again as a new file.

**Tools:**
- `whats_new`:
  - Read tier;
  - returns the items (title, folder or sources, found, seen) and the alerts with their states, under `UNTRUSTED_NOTICE`;
  - emits `show_in_library(ctx, "whats_new", item, label)`.
- `set_folder_watch {source, on}`: Write tier, local edit. Preview: "Show new files in *Research Papers* in What's new: on". After saving, it highlights the folder row.
- The Task 7 alert tools now also highlight their row through `show_in_library(ctx, "whats_new", Some(alert_id), …)`.

- [ ] **Step 1: Write the failing tests:**
  - `the_list_hides_dismissed_rows_and_missing_files`;
  - `dismiss_and_restore`;
  - `mark_seen_sets_seen_at_once`;
  - `folder_watch_off_then_on`;
  - `add_to_library_refuses_without_an_open_access_pdf_and_in_local_only`;
  - `add_to_library_downloads_only_the_reported_pdf_url` (recording transport: one request, to `pdfUrl`);
  - `whats_new_opens_the_section_and_marks_text_untrusted`;
  - `set_folder_watch_previews_the_folder`;
  - `whats_new_navigation_round_trips` (`expect_navigation`).
  - The existing coverage test checks `LIBRARY_SECTIONS` on both sides and the `LibraryView` handler.
- [ ] **Step 2: Run** CI "App tests and doc tests". Expected: the new tests fail.
- [ ] **Step 3: Implement.** Register all five commands in all four places, and both tools.
- [ ] **Step 4: Run** CI "App tests and doc tests" and "Frontend Typecheck + Tests". Expected: they pass.
- [ ] **Step 5: Commit** "What's new: list, dismiss, folder watch, Add to library, and the assistant's view of it".

### Task 9: The What's new view model

**Files:**
- Create: `app/src/features/whatsNew/model.ts`.
- Test: `app/tests/whatsNew.test.ts`.

**Interfaces (produces):**
```ts
export type SectionState = 'loading' | 'error' | 'empty' | 'watching' | 'list';
export function sectionState(i: { loaded: boolean; error: string | null; items: number; justWatched: string[] }): SectionState;
export type Bucket = 'today' | 'this_week' | 'earlier';
export function bucketOf(foundAt: string, now: Date): Bucket;  // local days; the week starts Monday
export interface Row { key: string; kind: 'file' | 'paper' | 'file_group'; title: string; meta: string; isNew: boolean; itemIds: string[] }
export function rowsOf(items: FeedItem[], now: Date): { bucket: Bucket; rows: Row[] }[];
//   files of one folder on one day: more than 3 become one 'file_group' row "38 new files in Research Papers"
export const VISIBLE_ROWS = 5;
export function alertLine(a: AlertView, now: Date): { word: string; fact: string; fix: 'resume' | 'privacy' | null };
export function digestTitle(n: number, folder: string): string;   // mirrors Rust
export const EMPTY_COPY: string; export const LOCAL_ONLY_COPY: string;
```

- [ ] **Step 1: Write the failing tests:**
  - `loading is never shown as empty`;
  - `a folder just watched reads as watching, not empty`;
  - `buckets follow local days and a Monday week`;
  - `more than three files of one folder in a day are one group row`;
  - `alert lines for every state`. The expected lines:

    | `state` | Line |
    |---|---|
    | `active` | "Next check Mon 1 Dec, 09:00" |
    | `checking` | "Checking now…" |
    | `waiting_retry` | "Semantic Scholar didn't answer; trying again at 11:40" |
    | `held` | "Stopped after failing 3 weeks in a row", fix `resume` |
    | `offline` | "No internet connection; Shodh checks again when it's back" |
    | `blocked` | `LOCAL_ONLY_COPY`, fix `privacy` |
    | `off` | "Off" |
  - `the empty copy is the spec's`;
  - `digest titles match the backend` ("1 new file in X", "3 new files in X").
- [ ] **Step 2: Run** CI "Frontend Typecheck + Tests". Expected: the new tests fail.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** CI "Frontend Typecheck + Tests". Expected: they pass.
- [ ] **Step 5: Commit** "What's new: the section's states, buckets and alert lines".

### Task 10: The Library section, the alert dialog and the Inbox link

**Files:**
- Create: `app/src/features/whatsNew/WhatsNewSection.tsx`, `PaperAlertDialog.tsx`.
- Modify:
  - `app/src/features/library/LibraryView.tsx`: mount the section after `{unlistedCard}` and before the folder groups. It shows when `hasLibrary` or alerts exist. It carries the `data-agent-item` keys that Task 8's arm reveals.
  - `app/src/features/inbox/InboxButton.tsx`: Resume with `data.resume === 'paper_alert'` calls `paperAlertUpdate(jobId, {enabled: true})`. The repeat path is unchanged.
  - The folder card's menu (`FolderCard`) gains the toggle "Show new files in What's new".
- Test: `app/tests/whatsNew.test.ts` (source checks with `read()`).

- [ ] **Step 1: Write the failing source checks:**
  - `the section reserves its height`: the list container has a fixed `min-h` of `VISIBLE_ROWS` rows, and "Show all" expands only on click.
  - `no button in the section wraps`: no `flex-wrap`, and every button carries `whitespace-nowrap`.
  - `paper text is plain text`: no `ReactMarkdown` and no `dangerouslySetInnerHTML` in either file.
  - `the dialog shows the consent line before Turn on`: `consentLine(` is used in `PaperAlertDialog.tsx`, and the Turn on button is the only call site of `paperAlertCreate(`.
  - `the section carries the keys the navigation reveals`: `agentItemKey('library-section', 'whats_new')` and `agentItemKey('feed-item'` occur in `WhatsNewSection.tsx`.
  - `resume of a paper alert calls paper_alert_update`.
- [ ] **Step 2: Run** CI "Frontend Typecheck + Tests". Expected: the new checks fail.
- [ ] **Step 3: Implement** to the UX section below. Refresh on the `whats-new-changed` and `folder-sync` events. Mark seen after the section has been on screen for 1 s (IntersectionObserver).
- [ ] **Step 4: Run** CI "Frontend Typecheck + Tests". Expected: they pass.
- [ ] **Step 5: Commit** "Library: What's new, with new files, paper alerts and their consent".

---

## UX: Library → What's new

**Place and frame.**
- The section sits under the Library header and its notices, above the folder groups. "Add folder" keeps the page's primary-action slot (audit rule 1).
- **[+ Paper alert]** sits at the right of the section's heading row, like "Open graph".
- The list area always reserves `VISIBLE_ROWS` (5) row heights, so new items never push the folder cards down. Beyond 5 rows, **Show all (N)** expands in place, and only when clicked.

```
 WHAT'S NEW                                                   [+ Paper alert]
 Today
 •  kv-cache-survey.pdf          Research Papers · 10:41    [Open] [Add to table v] [x]
 •  38 new files in Datasets      Datasets · 09:12           [Browse]                [x]
 •  Gated Linear Attention for…  2026 · arXiv, OpenAlex · alert: linear attention
                                                             [Abstract] [Add to library v] [x]
 Earlier this week
    ...                                                                   Show all (12)
 Paper alerts
    linear attention KV cache    Next check Mon 1 Dec, 09:00            [Edit] [...]
```

- `•` marks unseen rows.
- **Rows:**
  - Title and meta truncate on one line, with the full text in the tooltip.
  - Actions sit in a fixed-width, `flex-nowrap` column.
  - A paper title is a link to its landing page (opened in the browser on click). [Abstract] expands the stored abstract inline and sends no request.
  - [Add to library v] lists the watched folders. It is absent when the API reported no open-access PDF, and the column keeps its width.

**States:**

| State | What shows |
|---|---|
| Loading | One muted line, "Loading what's new…" (never the empty copy). |
| Baseline (a folder just added) | "Watching *Research Papers*. Files added from now on show here." The first sync is never listed. |
| Nothing new | `EMPTY_COPY`, plus one button, [+ Paper alert] (when the policy allows it). |
| New items | The rows above, bucketed Today / Earlier this week / Earlier. |
| Alerts off | No alerts: the "Paper alerts" sub-list is absent and the empty copy offers the button. An alert switched off: its row reads **Off** with [Turn on]. |
| Local-only (or web access off) | [+ Paper alert] is disabled, with `LOCAL_ONLY_COPY` on one line under the heading and an [Open Privacy settings] link. Each alert row reads **Paused** with the same reason. Files keep showing: they never leave the computer. |
| API failure | The alert row reads **Retrying**: "Semantic Scholar didn't answer; trying again at 11:40. arXiv and OpenAlex were checked." Its papers already show. After the retries: "Checked arXiv and OpenAlex; Semantic Scholar didn't answer this week." |
| Offline | **Waiting**: "No internet connection; Shodh checks again when it's back." |
| Held | **Stopped**: "Failed 3 weeks in a row." [Resume]. The Attention item is the same. |
| Error (list failed) | "Couldn't load What's new: … [Try again]", with the last list kept below it. |

**The consent flow** (`PaperAlertDialog`, from [+ Paper alert], Edit, or a suggestion the assistant made):
1. "Papers about" `[ linear attention KV cache ]`. The words only; up to 200 characters.
2. The consent line updates live, word for word: *Shodh sends these words: linear attention KV cache to arXiv, OpenAlex and Semantic Scholar to look for new papers. Your files, your library and who you are stay on this computer.* (Q1, ruled)
3. One muted line: "The first check runs now. New papers show here and in your Inbox."
4. **[Cancel] [Turn on]**. Turn on is disabled while the words are empty. Under Local-only the dialog shows `LOCAL_ONLY_COPY` in place of the buttons and offers [Open Privacy settings].
5. Editing the words shows the consent line again and needs Turn on again. Switching an alert off and on needs no new consent, because the words did not change.
6. After saving, the alert row is highlighted and reads **Checking now…**.

**Digest timing** (Q4):
- **Files:** one Info item per folder per local day, updated by each run of that day. It is posted within seconds of the sync that found the files.
- **Papers:** every alert checks on **Mondays at 09:00** local time.
  - A Monday missed while the computer slept or Shodh was closed runs once on wake, after 2 minutes.
  - A run that found something posts one Info item, "4 new papers for “linear attention”". A run that found nothing posts none; the row says "Last checked Mon: nothing new".
- There are no desktop notifications for What's new.

**Inbox:** the items are Info (PR 2's `tabOf`). Open goes to Library → What's new. A hold is Attention, with [Resume].

---

## Acceptance tests (§13, PR 3) → tasks

| Acceptance | Delivered by |
|---|---|
| The first sync is a baseline | Task 1 `a_first_sync_is_a_baseline`, `an_interrupted_first_sync_stays_a_baseline` |
| Added files show, changed ones don't | Task 1 (`a_changed_file_is_not_new`, rename, forget), Task 5 `added_files_become_rows_and_one_digest` |
| De-dup across the APIs and against the library | Task 6 `the_same_paper_from_three_apis_is_one`, `new_papers_skip_the_library_and_earlier_items`, `a_paper_known_by_title_is_not_new_when_it_gains_a_doi` |
| The consent line names all three APIs | Task 6 and Task 7 (Rust and TS, same literal; tool preview) |
| Local-only refuses alerts | Task 7 (create, reword, run), Task 7 (tools), Task 8 (Add to library) |
| Fixture API replies only | Tasks 6–8: `FakeTransport` and recording transports over the `linear-attention` fixtures |
| One Info item per run | Task 5 (`two_syncs_on_one_day_update_one_digest`), Task 7 (`one_api_failing_…` updates one id) |
| `whats_new` navigation | Task 8 `whats_new_navigation_round_trips`, the coverage gate and the `LibraryView` arm; Task 10 renders the section |

## Departures from the spec

- **D1. A run's input is the feed, not its event's subjects** (§5.3, "subjects as its input").
  - `engine_run` has no input column, and `Engine::event` takes none.
  - The watcher writes feed rows when the report arrives. The handler posts what is unposted.
  - The engine's `pending` set already folds events that arrive while a run is in flight, so no one-minute coalescing is added.
- **D2. "Baseline" is per manifest, and it survives an interrupted first sync.** That takes the `added_baseline` flag. A manifest from an older build counts as past its baseline, so files that arrived while Shodh was closed show as new.
- **D3. "Added" excludes these, all beyond §7's wording:**
  - renames (same size and modification time as a file removed in the same sync);
  - files read again through `forget_file` (scan reader, chat uploads handed to a folder);
  - files that failed before.

  Without these, a tidy-up floods What's new (Q8).
- **D4. Field spelling on the wire.** `added_more` is serialised as `addedMore`. The existing fields keep their snake_case spelling, so no current reader breaks.
- **D5. One file digest per folder per local day**, updated by each run. Each run still posts or updates exactly one item, but a day of syncs makes one item, not one per sync (Q4).
- **D6. A watcher sync whose only change is files What's new reports posts no `indexing:` item.** Otherwise every new file shows twice in Info.
- **D7. Alerts have their own requests.** They are date-bounded and newest-first, one per API, with no OpenAlex title request. Each API's `checkedThrough` date lives in the job's `input_json`. `search_papers` asks for relevance with no date, so a weekly re-run would return the same classics.
- **D8. Retries, and "try again in an hour".** The engine gains `Handled::RetryAt`.
  - A failed API is retried alone, an hour later (or at its `Retry-After`), up to 3 times, counted by the handler (`run_checked.retries`). After that the run succeeds with what it found and names the API that didn't answer.
  - A run fails, and counts toward the hold, only when no API answered and the cause was not the connection.
  - Being offline ends `skipped`, so a laptop that was away never holds its alerts.
  - A catch-up run waits 2 minutes (§5.3), as `backoff` with code `after_wake`. It reads "Checking now…" and does not use up a retry. PR 1 deferred that rule.
  - The first check at turn-on is an `event` run, since the engine has no manual start.
- **D9. An alert is its `paper_alert` engine job.** There is no separate table, and the alert id is the job id.
- **D10. De-dup against the library is only as good as what Shodh knows of the library's papers:**
  - the citation scans (made when the paper graph is built);
  - arXiv ids in file names;
  - papers added from What's new.

  Without a graph build, a library PDF whose name carries no arXiv id can come back as new.
- **D11. `delete_paper_alert` is Write tier** (a local edit; §7 does not say). Resume is `paper_alert_update(enabled: true)`, as PR 1's D10 used a re-save.
- **D12. The first check runs when the alert is turned on**, over the last 14 days. §7 only says weekly (Q5).
- **D13. `feed_item` gains `job_id` and `posted_run_id`**, beyond §7's columns, for posting and deletion.
- **D14. A file row whose file is gone is hidden when listed**, not deleted by the sync. Retention removes it after 90 days.
- **D15. The section reserves five rows of height**, and Show all expands in place, so new items never move the folder cards ("things stay where they were").
- **D16. Every alert uses all three APIs.** There are no per-API switches in this PR (Q2).

## Open questions for the user (each with a recommendation)

- **Q1. Consent wording.** The spec's line ends with "Nothing else leaves your computer." Each service also sees the computer's IP address and that a Shodh app asked; the user agent names Shodh and its version.
  - **Recommended:** "Once a week, Shodh will send the words “linear attention KV cache” to arXiv, OpenAlex and Semantic Scholar to look for new papers. Your files, your library and who you are stay on this computer."
  - The plan ships the spec's line until you choose. Both are a single constant, pinned by the Rust and TS tests.
- **Q2. Which sources are on by default?** **Recommended:** all three, always, with no per-API switches in PR 3 (decision 6; minimal first). Per-API switches can come later if anyone asks.
- **Q3. Alert topics: from the library, or typed by the user?**
  - **Recommended:** typed by the user in PR 3. The consent line must show the exact words sent.
  - The assistant can suggest a topic from your library in chat, and `create_paper_alert` shows the consent line for your approval.
  - Library-derived chips in the dialog (methods from the paper graph, shown locally, sent only once accepted into the line) can follow.
- **Q4. Digest schedule.** **Recommended:**
  - files: one Inbox item per folder per day, updated as files arrive;
  - papers: every alert on Mondays at 09:00, one Info item per alert only when it found something;
  - no desktop notifications.

  The alternative is one combined Monday item for all alerts.
- **Q5. An alert's first check.** **Recommended:** at once when turned on, covering the last 14 days, so the user sees it work. The alternative is a silent baseline until the first Monday.
- **Q6. Rate limits.** **Recommended:**
  - at most **10 alerts**;
  - checks run one alert at a time;
  - arXiv requests at least **3 s** apart (its manual's rule), OpenAlex and Semantic Scholar at least **1 s** apart;
  - **25 results per API** per check, keeping at most **10 new papers** per run;
  - `Retry-After` honoured.

  That is at most 30 requests a week. It fits OpenAlex's keyless budget and needs no key.
- **Q7. Does a consented alert also obey "Web access for the agent" being off?** **Recommended:** yes, so one rule covers it: when either switch says no, nothing leaves. The row then says which switch, with a link to Privacy.
- **Q8. Renames.** **Recommended:** a file that appears with the same size and modification time as one removed in the same sync is a rename and is not shown. The cost: copying a file and deleting the original in one burst also reads as a rename.
- **Q9. Drop the "N indexed" item when What's new reports the same files (D6)?** **Recommended:** yes. Failures and removals still post it.
- **Q10. "Add to library" in this PR?** **Recommended:** yes, as §7 has it: open-access PDFs the API reported only, into a watched folder you pick, refused in Local-only mode. If you prefer the smallest PR, it can move to PR 4 and leave [Open] on the landing page only.

## Self-review notes

- **Spec coverage:**
  - §7 new files: Tasks 1 and 5. Paper alerts: Tasks 6 and 7.
  - Data: Task 2. Inbox: Tasks 2, 5 and 7.
  - Commands and tools: Tasks 7–8. UX: Tasks 9 and 10.
  - §12 item 4: Task 8 and Task 10.
  - §5.3's wake and offline rules are D8.
- **Types shared across tasks:**
  - `PaperApi`, `AlertInput`, `AlertView.state`, `FeedItem` (camelCase);
  - Inbox ids `whats_new:files:<source>:<yyyymmdd>`, `whats_new:papers:<job>:<run>` and `paper-alert-hold:<job>`;
  - `data.resume = 'paper_alert'`;
  - `LIBRARY_SECTIONS` ending in `whats_new`.
- **Not in PR 3:** playbooks triggered by `files_added` or `paper_found` (PR 4), the call ledger (alerts make no model calls), and library-derived topic chips (Q3).
