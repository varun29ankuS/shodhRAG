# Evidence table (Elicit parity, part 1): design

Status: design approved in chat on 2026-10-20. This written spec is waiting for review.
Scope: v1 (the table) is specified in full. v2 (screening and table summaries) is specified to the
level the v1 data model must support, and gets its own plan after v1 ships.

## 1. Goal

A researcher picks papers from their library and asks questions as columns ("Method", "Dataset",
"Sample size", "Is the code released?"). Shodh fills one row per paper. Every cell holds:

- an answer;
- the verbatim quote it rests on;
- the page, and a highlight in the PDF.

Success criteria:

1. **Every cell can be checked in one click.** A cell is either backed by a quote that occurs word for
   word in the paper, or it is visibly marked as unverified or not found. A model's claim is never
   shown as a fact without its quote.
2. **It works on the free quota.**
   - One model call per paper per fill, at most two for very long papers.
   - Cached, so re-opening a table or adding a row costs only the new work.
   - Before a run, the user sees how many calls it needs.
   - Runs continue across days and restarts.
3. **Typed columns** (number, yes/no, pick-list) can be sorted and filtered, so v2 screening is a
   view over the same table, not a redesign.
4. **A published accuracy number.** A hand-checked set of 10 papers × 5 columns is run and its
   accuracy is stated in the README.

Non-goals for v1:

- searching outside the library (that is the Explore tab, a separate spec);
- automatic alerts;
- LaTeX export (with Zotero);
- tables across workspaces;
- collaboration.

## 2. Parity map

| Elicit | Shodh | Where |
|---|---|---|
| Chat with papers | exists | — |
| Upload PDFs | Library | — |
| Extract data into a table | evidence table | v1 (this spec) |
| Screening by criteria | yes/no columns, include/exclude with reasons, counts | v2 (this spec) |
| Summarise a table (report) | a synthesis in which every sentence cites cells | v2 (this spec) |
| Export | CSV, Markdown (v1); BibTeX/LaTeX (with Zotero) | v1 / later |
| Find papers | Explore tab (arXiv, Semantic Scholar, OpenAlex) | separate spec |
| Alerts | Automations | later |
| Published accuracy | hand-checked extraction eval | v1 |

Not copied:

- Elicit's own paper index;
- per-credit pricing;
- wording that claims a paper is silent ("not mentioned"). Retrieval can miss text, so Shodh says
  "not found in the passages searched".

## 3. Where it lives

- **UI:** Workspace → a new **Tables** tab, next to Sources and Chats
  (`app/src/features/workspaces/WorkspacePage.tsx`, `WORKSPACE_TAB_LABELS`).
- **Tables per workspace:** a workspace can hold several tables.
- **Rows:** the table's papers are chosen from the workspace's library sources. "All PDFs in this
  workspace" is the default.
- **Storage:** `shodh.db`, the encrypted (SQLCipher) research database
  (`crates/shodh-rag/src/research/db.rs`), in a new schema version.
  - Cells hold quotes from the user's papers, so they belong with the other research data, under the
    same key.
  - They are not in LanceDB.
- **Code:**
  - backend: a new module `crates/shodh-rag/src/research/tables/` (model, store, fill, validate);
  - Tauri commands: `app/src-tauri/src/table_commands.rs`;
  - frontend: `app/src/features/tables/`.

## 4. Data model

```
evidence_table   id, workspace_id, name, created_at, updated_at
table_column     id, table_id, position, question, kind, options_json, instructions,
                 created_at, updated_at, revision
table_row        id, table_id, position, document_id (library identity of the paper), added_at
table_cell       table_id, row_id, column_id,
                 status, value_json, quote, chunk_id, page, bbox_json,
                 model, column_revision, paper_hash, filled_at, edited_at, error,
                 PRIMARY KEY (row_id, column_id)
table_fill_job   id, table_id, created_at, state, planned_calls, done_calls,
                 next_attempt_at, last_error
fill_cache       paper_hash, column_key, model, response_json, created_at,
                 PRIMARY KEY (paper_hash, column_key, model)
```

**`kind`** is one of:

| Kind | Value | Example |
|---|---|---|
| `text` | a string of at most 500 chars | "contrastive pre-training on LAION-400M" |
| `number` | a decimal string plus an optional unit, verbatim from the quote | "0.912", "48 GB" |
| `yes_no` | `yes` or `no` | "yes" |
| `choice` | one of `options_json` (2–12 options, set by the user) | "RCT" |

**`status`** is one of:

| Status | Meaning |
|---|---|
| `empty` | never filled |
| `queued` | waiting in a fill job |
| `verified` | the quote occurs in the paper and the value passed its type check |
| `unverified` | the model answered, but the quote or the type check failed; the reason is in `error` |
| `not_found` | the model reported no answer in the passages it was given |
| `edited` | the user's value; never overwritten by a fill |
| `failed` | the call failed (network, quota, parse); retried by the job |

**Freshness keys:**
- `column_key` is the SHA-256 of the normalised `question`, `kind`, `options_json` and
  `instructions`. Editing a column changes its key, so the cells refill on the next run and the
  old cache entries stay unused.
- `paper_hash` is the library's content hash of the file. A changed PDF refills its row.
- `revision` on a column, matched against `column_revision` on a cell, marks cells that are stale
  but still shown.

**Forget and delete:**
- Deleting a table removes its rows, cells, jobs and its cache entries.
- Removing a paper from the library removes its rows' cells. This hooks into the library-removal
  path from #60.

## 5. Filling a paper

**Per paper, for the columns whose cell is `empty`, `failed` or stale:**

**1. Local retrieval, no model call.**
- For each column, run the existing hybrid search restricted to the paper (`source_path` filter,
  `rag_engine.rs:2080`). The query is the column question plus its instructions; take the top 4
  passages.
- Take the union across columns and remove duplicates by chunk id.
- Always include the paper's first chunk, which holds the title and abstract.

**2. Budget.**
- Passages are added in rank order, round-robin across columns, until about 6,000 tokens (the
  tokenizer already used for chunking).
- If the columns' top passages don't fit, the columns are split into at most 2 calls.
- Beyond that, the remaining columns get the passages that fit and are marked as such.

**3. One model call.**
- The prompt gives:
  - numbered passages, each with its page;
  - the columns as a JSON list: id, question, kind, options, instructions.
- It asks for strict JSON:
  `{"cells":[{"column":"<id>","answer":<value|null>,"quote":"<verbatim>","passage":<n>}]}`.
  `answer: null` means not found.
- The user's chat model is used, unless a table model is set in the table's settings.
- The request uses the provider path that already exists. Structured output (`response_format`) is
  not used: no provider path supports it yet, and adding it is shared work with memory phase (ii).

**4. Tolerant parse.**
- Strip fences and `<think>` blocks, and take the first balanced `{…}`.
- Unknown keys are ignored.
- One retry is allowed, and only if the envelope can't be parsed at all. The retry is counted
  against the job's calls.

**5. Validation, deterministic; the model never has the last word.**
- **Quote:**
  - After normalising both (Unicode NFKC, collapsed whitespace, hyphenation at line breaks joined),
    `quote` must occur in passage `passage`.
  - If it occurs in a different passage given in the same call, that passage is used.
  - If it occurs in none, the cell is `unverified` with error `quote_not_in_passages`.
  - `chunk_id`, `page` and `bbox` come from the matched passage, using the same highlight data
    citations use.
- **Type:**
  - `number`: the value's digits must occur in the quote, the same rule as results extraction
    (`research/results/numbers.rs`). Nothing is computed, rounded or converted.
  - `yes_no`: the answer is `yes` or `no`, case-folded.
  - `choice`: the answer is one of the options, case-folded with exact match; otherwise
    `unverified`.
  - `text`: non-empty and at most 500 chars.
- **`answer: null`** → `not_found`, shown as "Not found in the passages searched".
- **The grounding check:** a `verified` cell then goes through the existing answer-grounding check,
  the one used for citations in answers, applied to the pair (answer, quote).
  - A contradiction downgrades the cell to `unverified` with error `quote_does_not_support`.
  - This is local scoring, not a model call: `harness/grounding/verify.rs::support`, backed by the
    local NLI model in `reranking/nli.rs`.

**6. Store** the cell, and the raw response in `fill_cache`.

**Numeric results already extracted.** For `number` columns, Result statements already extracted
from the paper's tables (`research/results`) are offered as passages too. These are the accepted
ones only. Their cell boxes are used for the highlight.

## 6. Budget, queue and resume

- **Before a run**, the dialog says: "Filling 23 papers × 4 columns needs about 25 model calls.
  Shodh has made 12 calls today."
  - Shodh doesn't know the provider's remaining quota, so it shows its own count from a shared
    per-day call counter, not a guess about the provider.
- **Daily cap:** a run never spends more than a per-day table cap in Settings, 30 calls by default.
  This leaves room for chat on a 50-a-day free key.
- **The job:**
  - It is a `table_fill_job` row. Runs fill papers in row order.
  - They run in the background with the existing notification on finish or "needs you".
  - A quota error (HTTP 429 or the provider's daily-limit error) pauses the job until the next day.
  - It survives restarts: on launch, queued jobs resume.
- **Cancel:** stops after the current call. Finished cells stay.
- **Cache:**
  - Re-running a column whose key didn't change for a paper whose hash didn't change uses
    `fill_cache` and makes no call.
  - Adding a row costs only that row; adding a column costs one call per paper.

## 7. UI

**Tables tab:**
- A list of the workspace's tables.
- "New table" offers columns from presets: Title, Year, Method, Dataset, Main result, Limitations.
  They can be renamed or removed.

**Grid:**
- **Rows:** papers. The first column is the paper's title, with a link that opens the paper.
- **Cells:**
  - the value, with a small status mark for `unverified`, `not_found`, `edited` and `failed`;
  - stale cells are dimmed until refilled.
- **Sort and filter:**
  - Sort by any typed column; numbers sort numerically and ignore units.
  - Filter by `yes_no` / `choice` values and by status.
- **Keyboard and screen readers:** the grid is a real table with headers, and arrow keys move
  between cells.

**Cell panel:**
- Click a cell, and a side panel shows:
  - the value;
  - the quote, with the matched words marked;
  - the page;
  - **Open in PDF**, which opens the viewer at the highlight;
  - the model and date;
  - for an unverified cell, the reason in plain words.
- Actions: **Edit** (the user's value, status `edited`), **Refill this cell**, **Clear**.

**Columns:**
- "Add column": question, kind (Text / Number / Yes–No / Pick-list with options) and optional
  instructions ("report the largest dataset only").
- Column menu: edit, move, delete, refill.

**Fill:**
- The "Fill" button opens the budget dialog (§6).
- Progress appears in the tab and in the activity tray.

**Export:**
- **CSV:** value columns, plus an optional `<column> quote` and `<column> page`.
- **Markdown:** a table, with footnotes holding the quotes and pages.

## 8. v2 (needs no change to the v1 schema)

**Screening:**
- A table can mark `yes_no` / `choice` columns as **criteria**, each with a rule: include when yes /
  exclude when yes / must be one of these options.
- Each row gets a decision: included, excluded or undecided, with the deciding cells as its reason.
- A user's override of the decision is recorded with their reason.
- The counts are shown as a flow: papers → screened → excluded (by reason) → included.
- Stored as `table_screening(row_id, decision, reason_json, decided_by, decided_at)`.

**Summarise this table:**
- An answer, through the normal agent path, over the included rows' verified cells, given as the
  sources.
- Every sentence cites the cells it rests on, and the citation checker treats cells as passages.

## 9. Testing and accuracy

**Rust unit tests:**
- normalisation and the verbatim quote match (hyphenation, ligatures, Unicode);
- the type rules for each kind;
- the tolerant parse, including fences, `<think>`, trailing prose and unknown keys;
- the budget split (1 call / 2 calls / over budget);
- cache hits on an unchanged key or hash and misses on a changed column or file;
- job resume after restart, quota pause and cancel;
- table deletion and library removal deleting cells.

**Fill tests on recorded model responses:**
- Real responses captured once into fixtures, for these cases:
  - a correct answer;
  - a paraphrased quote → `unverified`;
  - a number not in its quote;
  - a choice outside the options;
  - a null answer;
  - an unparsable envelope.
- No live model calls in CI.

**Frontend tests:** grid sorting and filtering, cell status rendering, and the export format.

**Accuracy eval:**
- **Set:** 10 open-access papers from the existing retrieval-eval corpus × 5 columns (one per kind,
  plus a second text column), with values and quotes checked by hand.
- **Runner:** a script that fills them with a named free model, spread over days within the quota,
  and resumable.
- **Reported numbers:**
  - verified-and-correct;
  - verified-but-wrong (the number that matters);
  - unverified;
  - not found when an answer exists.
- **Published:** the result goes in the README with the model name and date.

## 10. Risks

- **Retrieval misses.** The answer is in the paper but not in the top passages, so the cell shows
  `not_found`. This is mitigated by the honest wording and the per-cell Refill, which can widen to
  the top 8. The eval measures how often it happens.
- **A verbatim quote of the wrong thing.** The quote exists, but the answer misreads it. The
  grounding check catches some cases; the eval's "verified but wrong" rate measures the rest. The
  one-click quote is the user's defence.
- **Free-model JSON quality.** The tolerant parse and one retry handle most failures. What's left
  becomes `failed` with its reason, retried by the job and never shown as data.
- **The quota shared with chat.** The per-day table cap (30) protects chat. On a paid key the cap
  can be raised in Settings.
- **Scanned PDFs.** Retrieval works through OCR text, and highlights use OCR boxes where they exist.
  Where they don't, the cell shows the page without a highlight.

## 11. Order of work (for the plan)

1. Schema, store, model and the validation rules, with their tests.
2. The fill pipeline (retrieval, budget, call, parse, validate, cache), with recorded-response tests.
3. Job queue, quota pause, resume and notifications.
4. Tables tab, grid, cell panel, column dialog, budget dialog.
5. Export; the eval set and runner.
6. v2: screening, then table summaries (a separate plan).
