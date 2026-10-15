<p align="center">
  <img src="app/public/shodh_logo_nobackground.svg" alt="Shodh" width="120" />
</p>

<h1 align="center">Shodh</h1>

<p align="center">
  <strong>Ask questions across your papers and folders and get answers cited to the exact page. Your documents stay on your computer.</strong>
</p>

<p align="center">
  <a href="#download">Download</a> &middot;
  <a href="#what-works-today">What works today</a> &middot;
  <a href="#retrieval-quality">Retrieval quality</a> &middot;
  <a href="#in-progress">In progress</a> &middot;
  <a href="#quickstart">Quickstart</a> &middot;
  <a href="#data-and-privacy">Data &amp; privacy</a> &middot;
  <a href="#architecture">Architecture</a> &middot;
  <a href="#contributing">Contributing</a>
</p>

<p align="center">
  <a href="https://github.com/varun29ankuS/shodhRAG/blob/master/LICENSE"><img src="https://img.shields.io/badge/license-Apache%202.0-blue.svg" alt="License" /></a>
  <img src="https://img.shields.io/badge/tauri-2-24C8D8.svg" alt="Tauri" />
  <img src="https://img.shields.io/badge/react-19-61DAFB.svg" alt="React" />
  <img src="https://img.shields.io/badge/status-pre--release-orange.svg" alt="Status: pre-release" />
</p>

---

**Shodh** (Sanskrit: शोध, "research") is a desktop app that reads the folders you point it at and answers questions about them. Each answer cites the file and page it came from, and each cited statement is checked against its source.

- **Processing:** your documents are parsed, OCR'd, indexed and embedded on your computer.
- **Answers:** these come from the language model you choose. That can be a model running locally (Ollama or LM Studio), or a cloud provider using your own API key or subscription.

> **Status: pre-release (0.0.1).** Asking with citations, folder sync, scanned documents, the document viewer and the grounding check work today. Server mode, encryption of the local index and the paid features are not built yet. This README describes only what works today. Everything else is listed under [In progress](#in-progress).

## Download

Get the latest installer from [Releases](https://github.com/varun29ankuS/shodhRAG/releases/latest):

- **Windows 10 (1903+) and 11, 64-bit:** `Shodh_<version>_x64-setup.exe`
- **macOS 13.3+ on Apple Silicon:** `Shodh_<version>_aarch64.dmg`. There is no Intel Mac build yet.

The installers are not code-signed yet:

- **Windows:** SmartScreen shows "Windows protected your PC". Click **More info**, then **Run anyway**.
- **macOS:** the first open is blocked. Open **System Settings → Privacy & Security** and click **Open Anyway**, or run `xattr -dr com.apple.quarantine /Applications/Shodh.app`.

Shodh checks for updates once a day and installs one only when you click it. You can turn the check off in **Settings → Privacy**.

## Pricing

The core of Shodh is free and open source (Apache-2.0), and it stays that way: asking with citations, the Library, scanned documents, reasoning and long chats.

**Pro** and **Team** plans are coming. They will add structured extraction (tables of facts across many documents, every value cited), automations, connectors such as Zotero, and shared team workspaces. Paid features will never lock you out of your own library.

## What works today

**Ask your files**
- Answers stream in with numbered citations. Click a citation to open the source at the cited passage.
- The question goes to an agent that searches your library, opens documents and, when allowed, searches the web. Each step is shown as it runs (for example "8 passages from 3 files · 1.2s"). A finished answer folds them into one line such as "Worked for 12s · 5 steps · 3 sources".
- **Stop** (or Esc) cancels the run, including the model call in progress.
- **Reasoning:** when the model streams its reasoning, a **Reasoning** button under the answer shows it on demand. It is saved with the conversation. It is not checked against your sources, and the panel says so.
- **Ask about this:** select text in an answer, a side answer or a document and click **Ask about this** (Ctrl+Shift+A). This opens a side discussion. It keeps the passages the parent answer cited under the same numbers, and it can post a summary back to the main conversation.
- **Long chats:** conversations render only the messages on screen. **Ctrl+F** (Cmd+F on macOS) finds text in the whole conversation, including messages not on screen.
- Conversations are saved locally with their citations, steps, checks and side discussions.
- In a library of papers, the starter questions are about methods, results, datasets and baselines.

**Grounding check**
- Every answer is checked on this computer, statement by statement, against the passages it cites. No language model judges the answer.
- By default the cross-encoder reranker scores each statement against its passage, together with number and quote checks. A quote of four or more words must appear in the cited text word for word.
- **Answer checking model:** an optional local model, about 96 MB, installed from **Settings → Search**. It also catches reversed or swapped facts.
- Unsupported statements are flagged inline. Examples: "not found in the cited source", "no source", "source [n] does not exist". A chip shows the score, such as "Grounded 4/5".
- **Re-check flagged statements** (on by default) asks the model once to cite the right passage, correct the statement or remove it.

**Document viewer**
- **PDFs** open at the cited page with the passage outlined and scrolled into view.
- **Word, PowerPoint, Markdown, HTML, text and code** open as extracted text with the passage highlighted.
- **Spreadsheets and CSV** open as a grid with the cited rows named.

**Folders and documents**
- **Folder sync:** add a folder in the Library (**Add folder**) and it stays in sync:
  - A file watcher picks up new, changed and deleted files.
  - Changes made while the app was closed are caught on the next start.
  - An interrupted sync resumes where it stopped.
  - A rename is handled as a removal plus a new file, so the file is indexed again.
- **Text documents:** PDF, Word (`.docx`), PowerPoint (`.pptx`), Markdown, LaTeX, HTML, plain text and source code.
  - PDFs are parsed with their layout: headings, paragraphs, lists, tables, footnotes and bibliography.
  - PDF, scan, LaTeX and Markdown chunks are sized in tokens and never cross a heading.
- **Tables in PDFs:** an optional table-structure model (Windows, about 212 MB, **Settings → Search**) re-reads table pages in the background.
- **Spreadsheets:** `.xlsx`, `.xls`, `.xlsm`, `.xlsb`, `.ods`, plus CSV and TSV.
  - Sheets are indexed row by row, and every chunk repeats the header row. That way any single row can be retrieved on its own.
  - CSV encodings are detected automatically: UTF-8, UTF-16 with a byte-order mark, and Windows-1252.
- **Failures:** the Library lists each file that could not be indexed, grouped by a plain reason, for example "Protected with a password", "Damaged or incomplete" or "This file type is not supported". Failed files are tried again once they change.

**Scanned documents**
- Scanned PDF pages and images (PNG, JPEG, BMP, TIFF) are read with PP-OCRv6. The OCR model is installed with the search models.
- OCR runs in a separate low-priority process on half the cores, so the computer stays responsive. Progress reads "Reading scanned pages: 12 of 240".
- OCR results are cached per file, so an unchanged scan is not read twice.
- On Windows, Windows OCR is the fallback while the PP-OCRv6 model is missing. macOS uses PP-OCRv6 only.

**Retrieval**
- **Hybrid search:** dense vectors (multilingual E5, ONNX, on device) and BM25 keyword search (Tantivy), fused with reciprocal rank fusion.
- **Reranking:** a cross-encoder reorders the results.
- **Multi-part questions** are split into parts by rules, and the agent can search again with other wording.
- **Papers:** the Library's **Graph** builds a citation graph of your papers: which cite each other, and their methods, datasets and authors. For questions about papers and citations, the graph also ranks search results.
- **Compare results:** numeric results are read from the tables of papers. Every value is cited to its cell, and low-confidence values wait for your review.

**Workspaces**
- A workspace is a named set of sources (folders, files, snippets, papers) with instructions. Its chats search only those sources unless you ask for the whole library.
- Instructions are versioned, with history and a diff.
- Templates: Blank, Literature review, Grant proposal, Paper writing, Client audit.

**Memory**
- Shodh suggests memories from your conversations, and you accept or dismiss them.
- **Settings → Memory** lists them, with search, edit, pin, forget, history and export. **Use memories in answers** controls whether they are recalled into answers.

**Tasks and calendar**
- **Tasks** shows tasks and events as a list or a calendar, with reminders.
- Tasks and events are indexed, so you can ask about them, and the assistant can create and update them.

**Inbox and Activity**
- **Inbox** collects what waits on you, such as approvals, memory suggestions and reminders, and background work that finished or failed.
- **Activity** shows usage and the audit log of questions, tool calls, approvals, retrievals, answers and setting changes. The log can be filtered and exported (JSONL, CSV), and it has a retention setting.
  - Each event is hash-chained to the one before it.
  - **Verify integrity** reports the first event that was edited, deleted or reordered.

**Assistant tools**
- The assistant runs on [oh-my-pi](https://github.com/can1357/oh-my-pi) (`omp`), a pinned agent runtime driven over RPC. See [ADR 0001](docs/adr/0001-agent-harness-omp.md).
  - **Install agent runtime** downloads it once from GitHub. Shodh checks its SHA-256 before every launch.
  - The hash is pinned in the source on Windows. On macOS it is taken from the release's checksum file.
- Every tool runs in Rust and is permission-checked:
  - Each answer has a budget of tool calls.
  - By default, writes and deletions wait for your approval. MCP tools can be set to run automatically.
- **Built-in tools:**
  - Library search and documents
  - Web search and pages
  - Paper search
  - Tasks and calendar
  - Memory
  - Workspaces, conversations and settings
  - Exports
- **More tools:** add MCP servers and skills in **Settings → Tools & connections**.
- **Code mode:** the **Research | Code** switch in Ask lets a conversation read, change and run commands in a workspace's code folder.
  - File tools are confined to that folder. Shell commands cannot be confined by path.
  - Each edit and command asks first, unless you choose a level that applies edits by themselves or runs allowlisted commands.
  - In a git repository, changes go on a branch of their own and can be discarded.

**Models**
- **Cloud with your API key:** OpenRouter, Anthropic, OpenAI, Google Gemini and xAI.
- **Cloud with a subscription:** sign in with Claude Pro/Max, ChatGPT Plus/Pro, GitHub Copilot or Google Gemini.
- **Local:** Ollama or LM Studio. The assistant needs tool calling, so GGUF files loaded in-process cannot answer questions.
- **Pre-configuration:** set the model with environment variables, so IT can lock in the approved provider on managed machines. See [Configuration](#configuration).

**Privacy controls and app**
- **Local-only mode** (**Settings → Privacy**) blocks web tools, cloud models, remote MCP servers, online paper lookups and the update check.
- **Web access:** the assistant's web access can be turned off on its own.
- **API keys** are kept in the OS credential store.
- **Updates** are signed, checked once a day, and installed only when you click.
- **Background:** Shodh can keep running in the tray and start with Windows (**Settings → General**).

## Retrieval quality

[`crates/shodh-eval`](crates/shodh-eval) measures retrieval on the app's real indexing and search code, with no language model involved. The **Retrieval eval** CI job re-indexes both committed sets on every PR. Retrieval is deterministic, so the job fails on any drop below the committed baseline.

| Set | What it is | Questions |
|---|---|---|
| Synthetic | Fictional contracts and invoices (paged PDFs with tables), research notes and policies | 50, 5 of them unanswerable |
| Scanned | Eleven pages of those contracts and invoices as image-only scans (150–300 dpi, skewed, speckled, three table pages) plus text distractors, read with PP-OCRv6 | 34, 5 of them unanswerable |

Baselines ([synthetic](crates/shodh-eval/data/synthetic/baseline.json), [scanned](crates/shodh-eval/data/scanned/baseline.json); recorded 2026-10-11, top 10, reranking on):

| Metric | Synthetic | Scanned |
|---|---|---|
| hit@1 (right document first) | 0.978 | 1.000 |
| MRR | 0.989 | 1.000 |
| page_hit@1 (right page first) | 0.939 | 1.000 |
| passage_hit@1 (chunk holding the expected passage first) | 0.947 | 1.000 |
| hit, page_hit and passage_hit @5 and @10 | 1.000 | 1.000 |
| Files indexed | 1.000 | 1.000 |

These are small fictional sets. They catch regressions; they do not predict quality on your documents. For your own folder, `shodh-eval generate` writes questions with a language model you choose, and `shodh-eval retrieval` scores them. Generating sends passages to that model, so use a local provider for private folders. Questions and reports stay in a git-ignored local folder.

Answer quality is measured by `shodh-eval answers`: fact recall, citation correctness, grounding verdicts and correct refusals. It needs a language model, so it does not run in CI, and no baseline has been published yet.

## In progress

Nothing below works yet. The larger items have a section in the [design spec](docs/superpowers/specs/2026-10-02-folder-sources-grounded-answers-design.md) and land through reviewed PRs.

| Area | What it brings |
|---|---|
| **Server mode** | The same core running headless, with SSO and per-document permissions enforced inside every query. |
| **Encryption at rest** | Encryption of the local index (LanceDB, Tantivy). The app database (`shodh.db`) can already use SQLCipher with a key in the OS keychain, but only as a build option that released builds do not use yet. |
| **Records and totals** | Typed records (invoices, spreadsheet rows). Totals and counts are computed in code and report how many records they covered. |
| **Knowledge graph beyond papers** | Entities and relations from any document (rules plus GLiNER2), each with page-level provenance, used for multi-hop retrieval. |
| **Sub-agents** | A delegate tool that runs restricted child sessions, shown as live lanes. See [ADR 0001](docs/adr/0001-agent-harness-omp.md). |
| **Structure-aware chunks for every format** | Word, PowerPoint, HTML and plain text still use sliding windows. |
| **Answer-quality baseline** | Published answer metrics for the committed sets. |
| **Pro: structured extraction** | Tables of facts across many documents, every value cited. |
| **Pro: automations** | Questions and tasks that run on a schedule or a trigger. |
| **Pro: connectors** | Sources such as Zotero. |
| **Code signing** | Signed installers for Windows and macOS. |

## Quickstart

### Prerequisites

- [Rust](https://rustup.rs/) (stable) and [Node.js](https://nodejs.org/) 22 or later.
- Platform build tools:
  - **Windows:** [Microsoft C++ Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) and `protoc` (`choco install protoc`).
  - **macOS:** `xcode-select --install`
  - **Linux** (not built in CI): `sudo apt install build-essential libssl-dev libwebkit2gtk-4.1-dev protobuf-compiler`
- **Search models** (about 650 MB with the OCR model): on first run the app shows **Set up search** in Ask and Library and downloads the pinned, SHA-256-verified files itself.
  - Debug builds install them into `models/` at the root of the checkout; release builds use `<app data>/models`.
  - Setting `MODEL_PATH` overrides the location (and the install target).
  - Layout:
    - `models/multilingual-e5-base/`: `model_O4.onnx` and `tokenizer.json` from [intfloat/multilingual-e5-base](https://huggingface.co/intfloat/multilingual-e5-base/tree/main/onnx)
    - `models/ms-marco-MiniLM-L6-v2/`: `model_O4.onnx` and `tokenizer.json` from [cross-encoder/ms-marco-MiniLM-L6-v2](https://huggingface.co/cross-encoder/ms-marco-MiniLM-L6-v2)
    - `models/pp-ocrv6/`: the PP-OCRv6 small detection and recognition models
  - The answer checking model (`nli-deberta-v3-xsmall/`) and the table model (`docling-tables/`) are optional installs from **Settings → Search**.
- **Agent runtime:** installed from the app (**Install agent runtime**), or set `SHODH_OMP_PATH` to an `omp` binary.

### Run in development

```bash
git clone https://github.com/varun29ankuS/shodhRAG.git
cd shodhRAG/app
npm ci

# Terminal 1: frontend dev server
npx vite --port 5173 --strictPort

# Terminal 2: desktop app (reuses the dev server above)
npx tauri dev --config '{"build":{"beforeDevCommand":""}}'
```

On Windows PowerShell, pass the config as `--config "{\"build\":{\"beforeDevCommand\":\"\"}}"`.

The first build compiles the Rust workspace and takes several minutes. Later builds are incremental.

### Production build

```bash
cd app
npm run tauri build
```

## Configuration

### Choosing a model

Choose a model in **Settings → Models**, or pre-configure it with environment variables. The environment variables are ignored unless `SHODH_LLM_PROVIDER` is set.

| Variable | Values |
|---|---|
| `SHODH_LLM_PROVIDER` | `openrouter`, `anthropic`, `openai`, `google` (or `gemini`), `grok` (or `xai`), `ollama` |
| `SHODH_LLM_MODEL` | Provider model ID (optional), e.g. `anthropic/claude-haiku-4.5` |
| API key | The provider's usual variable: `OPENROUTER_API_KEY`, `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`, `XAI_API_KEY` |

Local models:
- **Ollama:** run `ollama pull <model>`, then set `SHODH_LLM_PROVIDER=ollama` or pick it in **Settings → Models**.
- **LM Studio:** load a model in LM Studio and pick it under **Settings → Models → Run locally**.

### Separate profile (demo or testing)

Set `SHODH_DATA_DIR` to an absolute folder to run a separate profile:
- Its library, settings, chats and WebView storage live in that folder.
- Its saved API keys use a credential-store service of their own.
- It runs beside your normal instance instead of switching to it.
- It cannot change the start-with-Windows entry.

Search models are downloaded into that folder. To reuse an existing download, also set `MODEL_PATH` (installs then write there).

Run one instance per folder. WebView storage is separated on Windows, Linux and macOS 14+.

## Data and privacy

| Data | Where it goes |
|---|---|
| Your files, their text, OCR results and embeddings | Stay on this computer. Indexing makes no network calls. |
| Questions, and the passages retrieved to answer them | Sent to the model provider you choose, together with workspace instructions and any memories recalled for that answer. Memory suggestions are learned by sending conversation text to that model. With Ollama or LM Studio, and web access off (or Local-only mode on), nothing leaves this computer. |
| Web searches and pages | When web access is on (the default), web searches go through OpenRouter's web plugin, Gemini with Google Search, or your SearXNG (`SHODH_SEARXNG_URL`), and pages are fetched directly. Off in Local-only mode. |
| Paper lookups | Building the citation graph online looks papers up on OpenAlex. Only when you ask for it and the web is allowed. |
| Downloads | Models come from Hugging Face and the agent runtime from GitHub, each pinned and checked by SHA-256. The update check contacts GitHub once a day. It is off in Local-only mode, and you can turn it off yourself. |
| Agent runtime | With a cloud provider, the pinned runtime was also observed fetching public model metadata from its catalog (`catalog.stencil.so`). |
| API keys | Stored in the OS credential store (Windows Credential Manager, macOS Keychain, Secret Service on Linux). They are never kept in the app's web storage or shown back in the UI. Keys saved by older builds are moved there on first launch. A key set through an environment variable applies to that session only. Subscription sign-ins are held by the agent runtime's own account store, in Shodh's app data folder. |
| Audit log | Kept in `shodh.db` on this computer. Document snippets in it are limited to 300 characters of text you were already shown. |

**Known gaps.** Each is tracked in the spec and will land as a reviewed PR.
- **Unencrypted storage:** released builds do not encrypt the local index (LanceDB, Tantivy) or the app database (`shodh.db`). Use OS full-disk encryption (BitLocker or FileVault).
- **Audit chain without a key:** the hash chain shows edited, deleted or reordered events. Someone who can write to `shodh.db` could still rebuild the whole chain.
- **Agent runtime egress:** the pinned runtime's catalog fetch cannot be turned off yet (see [ADR 0001](docs/adr/0001-agent-harness-omp.md)).
- **Code mode commands run as you:** approved shell commands run with your user account's permissions, without a sandbox.
- **No messaging channels or Google Drive sync:** the old WhatsApp, Telegram and Discord bot backends and the Google Drive connector have been removed. They ran unauthenticated local HTTP servers. Messaging channels will return through a separate, authenticated gateway.

## Architecture

```
shodhRAG/
├── crates/
│   ├── shodh-rag/              # Core library
│   │   └── src/
│   │       ├── processing/     # Parsers (PDF layout, Office, spreadsheets, CSV/TSV), OCR, chunkers
│   │       ├── search/         # Tantivy BM25, fusion, citation-graph ranking
│   │       ├── storage/        # LanceDB store
│   │       ├── embeddings/     # E5 via ONNX Runtime, pinned model downloads
│   │       ├── reranking/      # Cross-encoder reranker
│   │       ├── harness/        # omp sidecar, host tools, grounding check, web, MCP, Code mode
│   │       ├── folder_sync.rs  # Folder manifests and sync
│   │       ├── research/       # Citation graph, paper results
│   │       ├── workspaces/     # Workspaces and templates
│   │       ├── user_memory/    # Memories
│   │       ├── audit/          # Hash-chained audit log
│   │       └── llm/            # Provider settings and local providers
│   ├── shodh-ontology/         # Typed statements behind memories, snippets and results
│   └── shodh-eval/             # Retrieval and answer evaluation
├── app/
│   ├── src/                    # React 19 + TypeScript frontend
│   │   ├── components/shell    # Sidebar, Settings
│   │   └── features/           # ask, agent, focus (side discussions), library, workspaces,
│   │                           # tasks, activity, memory, modelPicker, setup
│   └── src-tauri/              # Tauri commands, agent tools, file watcher, updater
└── docs/
    ├── superpowers/specs/      # Design specs
    ├── superpowers/plans/      # Implementation plans
    └── adr/                    # Architecture decision records
```

**How a question is answered:**
1. The question goes to an `omp` agent session with Shodh's tools. Workspace instructions and recalled memories are added.
2. The model calls `search_documents`. Hybrid search (vectors and BM25) finds candidate passages, and the cross-encoder reranks them.
3. It may open documents, search again, or use other tools. Writes wait for approval.
4. The answer streams back with `[n]` citations mapped to the numbered passages.
5. The grounding check verifies each cited statement and flags the unsupported ones.

### Using the core library

```toml
[dependencies]
shodh-rag = { path = "crates/shodh-rag" }
```

```rust
use std::collections::HashMap;
use std::path::Path;
use shodh_rag::{RAGConfig, RAGEngine};

let mut config = RAGConfig::default();
config.data_dir = "./shodh-data".into();
config.embedding.model_dir = "./models".into();
config.embedding.use_e5 = true;
config.embedding.dimension = 768;

let mut engine = RAGEngine::new(config).await?;
engine.add_document_from_file(Path::new("contract.pdf"), HashMap::new()).await?;
let results = engine.search("When does the agreement renew?", 10).await?;
```

## Tech stack

| Layer | Technology |
|---|---|
| Desktop shell | Tauri 2 |
| Frontend | React 19, TypeScript 5.8, Vite 6, Tailwind CSS, Geist (bundled) |
| Core | Rust |
| Agent runtime | oh-my-pi (`omp`), pinned, over JSONL RPC |
| Vector store | LanceDB + Apache Arrow |
| Keyword search | Tantivy |
| Embeddings and reranking | multilingual E5 and an ms-marco MiniLM cross-encoder, via ONNX Runtime |
| OCR | PP-OCRv6 (ONNX Runtime); Windows OCR as the Windows fallback |
| App database | SQLite (`shodh.db`) |

## Contributing

1. **Branch and PR:** create a feature branch from `master`, open a PR, and merge only when CI is green.
2. **Production code only:** no TODOs, mocks, stubs or placeholders.
3. **Fix root causes:** never weaken a check to make CI pass.
4. **Keep PRs focused**, and explain *why* in commit messages.

Run these before opening a PR (CI runs the same checks, plus the tests and the retrieval eval):

```bash
cargo fmt --all --check
cargo clippy --workspace -- -D clippy::correctness -D clippy::suspicious -D clippy::perf
(cd app && npx tsc --noEmit && npm test)
```

## License

[Apache License 2.0](LICENSE). Copyright 2025 Shodh.
