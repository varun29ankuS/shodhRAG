# Shodh features

This is the full list of what Shodh does today, grouped by area. It describes the `master` branch, so a few items may be newer than the [latest release](https://github.com/varun29ankuS/shodhRAG/releases/latest). For a short overview, see the [README](../README.md).

- [Ask your files](#ask-your-files)
- [Citation check](#citation-check)
- [Document viewer](#document-viewer)
- [Library and folders](#library-and-folders)
- [Documents and file types](#documents-and-file-types)
- [Scanned documents](#scanned-documents)
- [Search and retrieval](#search-and-retrieval)
- [Research papers](#research-papers)
- [Workspaces](#workspaces)
- [Memory](#memory)
- [Tasks and calendar](#tasks-and-calendar)
- [Inbox, Activity and notifications](#inbox-activity-and-notifications)
- [Assistant tools and Code mode](#assistant-tools-and-code-mode)
- [Recall: your coding sessions](#recall-your-coding-sessions)
- [Connectors and the phone](#connectors-and-the-phone)
- [Models](#models)
- [Privacy controls and app](#privacy-controls-and-app)
- [Data and privacy: what leaves your computer](#data-and-privacy-what-leaves-your-computer)
- [Known limitations](#known-limitations)
- [Retrieval quality](#retrieval-quality)
- [Configuration](#configuration)
- [Building from source](#building-from-source)
- [Architecture](#architecture)
- [Tech stack](#tech-stack)

## Ask your files

- Answers stream in with numbered citations. Click a citation to open the source at the cited passage.
- The question goes to an agent that searches your library, opens documents and, when allowed, searches the web. Each step is shown as it runs, for example "8 passages from 3 files · 1.2s". A finished answer folds them into one line, such as "Worked for 12s · 5 steps · 3 sources".
- **Stop** (or Esc) cancels the run.
- **Visuals:** answers can include diagrams, charts, timelines and tables, and the model is asked to lead with one when it fits the question. Marked parts of an answer offer **Go deeper**, which asks a prepared follow-up in a side pane only when you click it.
- **Reasoning:** **Think before answering** (**Settings → Search**, off by default) lets the model reason first. When a model streams its reasoning, a **Reasoning** button under the answer shows it.
- **Ask about this:** select text in an answer or a document and click **Ask about this** (Ctrl+Shift+A). This opens a side discussion that keeps the parent answer's passages under the same numbers. It can post a summary back to the main conversation, with its citations written out as "(side-discussion source: file, p. N)".
- **Long chats:** conversations render only the messages on screen. **Ctrl+F** (Cmd+F on macOS) finds text in the whole conversation.
- **Saving:** a finished answer is saved as soon as it is final, and pending saves are written before the app quits. If the app is killed while an answer is being checked, the answer comes back marked as not checked.
- **Drafts:** each conversation keeps its unsent question while you switch views, for the session.
- **Images:** text read from a pasted or dropped image is attached to your next question as a removable chip. It goes to the model next to your question, not as part of it.
- **Model errors:** when the provider fails, the answer ends as an error, not as an answer. Any partial text is kept and marked Incomplete. The error card says what happened in plain words (for example a timeout, a rejected key or a rate limit), with a **Retry** button and **Details from the provider**. Keys and tokens are masked in those details.

## Citation check

- Every answer is checked on this computer, statement by statement, against the passages it cites. No language model judges the answer.
- The cross-encoder reranker scores each statement against its passage, together with number and quote checks. A quote of four or more words must appear in the cited text word for word.
- **Answer checking model:** an optional local model, about 96 MB, installed from **Settings → Search**.
- Unsupported statements are flagged inline, for example "not found in the cited source" or "source [n] does not exist". A chip shows the score, such as "Grounded 4/5".
- When another passage of the same answer supports a flagged statement, the flag offers **Cite N instead**. The correction is recorded in Activity first; if it cannot be recorded, nothing is changed. A corrected citation is marked "citation corrected by you".
- Flagged citations are not passed on when an answer becomes context for a follow-up or a side discussion.
- **Re-check flagged statements** (on by default) asks the model once to cite the right passage, correct the statement or remove it. If the model cited passages without searching, Shodh runs the search itself first.

## Document viewer

- **PDFs** open at the cited page with the passage outlined and scrolled into view.
- **Word, PowerPoint, Markdown, HTML, text and code** open as extracted text with the passage highlighted.
- **Spreadsheets and CSV** open as a grid.
- The model, the citation check and the viewer see the same passage text, and it always contains the part that search matched.

## Library and folders

- **Folder sync:** add a folder in the Library (**Add folder**) and it stays in sync:
  - A file watcher picks up new, changed and deleted files.
  - Changes made while the app was closed are caught on the next start.
  - An interrupted sync resumes where it stopped.
  - A rename is handled as a removal plus a new file.
- **Files already indexed:** adding a folder takes over its unchanged, already indexed files instead of reading them again. Indexed files that are in no listed folder are shown in the Library, with **Add these folders**.
- **Names:** files and folders are shown in their case on disk.
- **Removing a source** removes only that source, not a folder whose path merely starts the same way. The removal is recorded first, holds across a restart, and stops that source's sync or indexing.
- **Unreadable folders:** if a subfolder cannot be read, its files are kept and the card shows a sync error.
- **Failures:** the Library lists each file that could not be indexed, grouped by a plain reason, for example "Protected with a password", "Damaged or incomplete" or "This file type is not supported".
- **Secrets are kept out:** files that may hold passwords or keys (such as `.env` files and key files) are never indexed, and passwords or keys found inside other files are redacted before they are stored.
- **One-time upgrade:** existing libraries are re-chunked once with the current parser. The card shows "Updating index for improved text extraction" while it runs, and files that failed before are tried once more.

## Documents and file types

- **Text documents:** PDF, Word (`.docx`, and older `.doc`), PowerPoint (`.pptx`, and older `.ppt`), OpenDocument text and slides (`.odt`, `.odp`), RTF, Markdown, LaTeX, HTML (`.html`, `.htm`), e-books (`.epub`), emails (`.eml`), notebooks (`.ipynb`), logs, plain text and source code.
  - PDFs are parsed with their layout: headings, paragraphs, lists, tables and footnotes. A PDF's printed title becomes the document title, so a file named `1706.03762v7.pdf` is found by its title.
  - Word and PowerPoint text is read with an XML parser, so XML markup is not indexed and entities such as `&amp;` are decoded. Word tables become table sections.
  - Text in any script is chunked, including Chinese, Japanese, Devanagari, Cyrillic and Arabic.
  - HTML pages in any script are read without scripts, styles or comments, and their entities are decoded.
  - PDF, scan, LaTeX and Markdown chunks are sized in tokens and never cross a heading.
- **Tables in PDFs:** an optional table-structure model (Windows, about 212 MB, **Settings → Search**) re-reads table pages in the background.
- **Spreadsheets:** `.xlsx`, `.xls`, `.xlsm`, `.xlsb`, `.ods`, plus CSV and TSV. Sheets are indexed row by row, and every chunk repeats the header row.
- **Size limits:** text files up to 50 MB, spreadsheets and Office files up to 100 MB, PDFs up to 2,000 pages. A larger file is reported as "too large to index" and is not read.

## Scanned documents

- Scanned PDF pages and images (PNG, JPEG, BMP, TIFF, GIF, WebP) are read with PP-OCRv6. The OCR model is installed with the search models.
- OCR runs in a child process at below-normal priority on half the cores, so the computer stays responsive. Progress reads "Reading scanned pages: 12 of 240".
- OCR results are cached per file, so an unchanged scan is not read twice.
- When OCR cannot read some pages, it keeps the rest and names the pages it missed. When it reads nothing, it says why.
- On Windows, Windows OCR is the fallback while the PP-OCRv6 model is missing. macOS uses PP-OCRv6 only.

## Search and retrieval

- **Hybrid search:** dense vectors (multilingual E5, ONNX, on device) and BM25 keyword search (Tantivy), fused with reciprocal rank fusion.
- **Reranking:** a cross-encoder reorders the results.
- **Graphics card:** the search model runs on a graphics card when there is a usable one (DirectML on Windows, Core ML on Apple silicon), otherwise on the CPU, and the System status panel says which and why. Turn off **Use the graphics card for search** there to use the CPU only.

## Research papers

- **Paper graph:** the Library's paper graph shows how your papers cite each other, read from their bibliographies.
- **Compare results:** numeric results are read from the tables of papers. Every value is cited, and low-confidence values wait for your review.
- **Snippets:** passages you keep from your reading are collected in the Library.

## Workspaces

- A workspace is a named set of sources (folders, files, snippets, papers) with instructions. Its chats search only those sources unless you ask for the whole library.
- Snippets and results made in the Library are visible to workspace chats, limited to that workspace's files.
- Instructions are versioned, with history.
- Templates: Blank, Literature review, Grant proposal, Paper writing, Client audit.
- Deleting a workspace keeps its memories, snippets, results, tasks and chats; they move out of the workspace.

## Memory

- Start a message with "Remember …" (or "Keep in mind …", "From now on …") and Shodh files a note card for you to accept, without a model call.
- Shodh also suggests memories from your conversations, within a daily cap of model calls (10 by default). You accept or dismiss each one.
- **Settings → Memory** lists them, with search, edit, pin, history, export and forget. Forget erases a memory and all its earlier versions: from the memory store and its older versions on disk, from suggestions that quoted it, and from new Activity entries, which record ids and counts, never the words. The conversation where you said it, and Activity entries recorded before forget existed, still hold the words.

## Tasks and calendar

- **Tasks** shows tasks and events as a list or a calendar, with reminders. A task can belong to a workspace, and each workspace has a task board.
- The **New** menu (beside New chat, or Ctrl/Cmd+Shift+N) creates a chat, workspace, task or event, or adds a folder, from any view.
- Tasks and events are indexed, so you can ask about them, and the assistant can create and update them.

## Inbox, Activity and notifications

- **Inbox** collects what waits on you, such as approvals, memory suggestions and reminders, and background work that finished or failed.
- **Activity** shows usage and the audit log of questions, tool calls, approvals, retrievals, answers and setting changes. The log can be filtered and exported (JSONL, CSV), and it has a retention setting. Retention deletes only events older than the window, even after the system clock jumps.
  - Each event is hash-chained to the one before it.
  - **Verify integrity** reports the first event that was edited, deleted or reordered.
- **Desktop notifications** on Windows and macOS for reminders, approvals that wait for you, finished work (answers, indexing, extracted results) and problems (failed indexing or sync, a failed background model call, a data file that had to be recovered).
  - They appear only while the Shodh window is not in front. They name titles and counts, never document or answer text.
  - **Settings → Notifications** has a switch per category, **Send a test notification**, and **Open system notification settings**.
  - Routine folder syncs stay quiet, including the one-time update of an existing library after an upgrade; indexing you start, and new failures, are announced.

## Assistant tools and Code mode

- The assistant runs on [oh-my-pi](https://github.com/can1357/oh-my-pi) (`omp`), a pinned agent runtime driven over RPC. See [ADR 0001](adr/0001-agent-harness-omp.md).
  - **Install agent runtime** downloads it once from GitHub. Shodh checks its SHA-256 before every launch.
- Every tool runs in Rust and is permission-checked. Each answer has a budget of tool calls, and writes and deletions wait for your approval.
- **Built-in tools:** library search and documents, web search and pages, paper search, tasks and calendar, memory, workspaces, conversations, settings and exports.
- **Web lookups after private content:** once a conversation has read your documents, memories or other private content, a web search, paper search or page fetch that could carry that content out asks first. You can allow web search, or one site, for the rest of that conversation.
- **More tools:** add MCP servers and skills in **Settings → Tools & connections**.
- **Code mode:** the **Research | Code** switch in Ask lets a conversation read, change and run commands in a workspace's code folder.
  - File tools are confined to that folder: Shodh checks the path the tool will actually open and refuses ambiguous ones. Files that can run programs (`.git`, editor tasks, hooks, CI workflows, `package.json` and similar) are never changed without asking, and `.git` cannot be edited. Shell commands cannot be confined by path, so each one needs approval unless you allow it exactly.
  - Each workspace has a mode: **Ask** (asks before every change), **Accept edits**, **Plan** or **Auto**.
  - In a git repository, changes go on a branch of their own and can be discarded.
  - On macOS, commands run in macOS's sandbox. On Windows they run with your account's access, after your approval (see [Known limitations](#known-limitations)).

## Recall: your coding sessions

- Shodh can index your own Claude Code and Codex session transcripts, on this computer, so you and the assistant can search what you worked on before. It is off until you set it up in the Library.
- Transcripts are read where they are and never copied or changed. Secrets in them are redacted before anything is indexed, shown or sent to a model.
- Recall is a separate index: answers in Ask, Slack and Telegram cannot reach it.

## Connectors and the phone

- **Slack:** answers questions people ask the Shodh bot, with citations from the one workspace you selected, and can index the channels you choose. It connects out over Slack's Socket Mode; it opens no port on this computer.
- **Telegram:** answers your messages to the bot, and those of people and groups you allow, polled from this computer over the Telegram Bot API.
- **GitHub:** a connector with per-repository permissions for the repositories you allow.
- **Shodh for Android** (Android 7 or later, 64-bit phones): pairs with Shodh on your computer over an encrypted link (**Settings → Phone**). From the phone you ask questions, which Shodh on your computer answers from its library, and see your conversations, tasks and the approvals that wait for you. The phone and the computer must run the same version. It is installed from the release's APK, not from Google Play.

What each of these sends, and to whom, is listed under [Data and privacy](#data-and-privacy-what-leaves-your-computer).

## Models

- **Cloud with your API key:** OpenRouter, Anthropic, OpenAI, Google Gemini and xAI.
- **Cloud with a subscription:** sign in with Claude Pro/Max, ChatGPT Plus/Pro, GitHub Copilot or Google Gemini.
- **Local:** Ollama or LM Studio. The assistant needs tool calling, so GGUF files loaded in-process cannot answer questions.
- No model is set up out of the box: you choose one in **Settings → Models**.
- **Pre-configuration:** set the model with environment variables, so IT can lock in the approved provider on managed machines. See [Configuration](#configuration).

## Privacy controls and app

- **Local-only mode** (**Settings → Privacy**) turns off web tools, cloud models and the update check: the assistant may only use a local model.
- **Web access:** the assistant's web access can be turned off on its own.
- **API keys** are kept in the OS credential store. A removed key is never sent again: answers running on it stop and say so.
- **Crash-safe data:** conversations, settings, the calendar and app state are written through a temporary file, keeping the previous version as a backup. A damaged file falls back to its backup and is set aside, never overwritten.
- **Background:** Shodh can keep running in the tray and start when you log in (**Settings → General**). On macOS, clicking the Dock icon brings the window back.
- **Updates:** Shodh checks for updates a few minutes after start and then once a day. An update is downloaded and installed only when you choose to. Updates are signed, and one whose signature does not verify is refused. You can turn the check off in **Settings → Privacy**, and it never runs in Local-only mode.

## Data and privacy: what leaves your computer

| Data | Where it goes |
|---|---|
| Your files, their text, OCR results and embeddings | Stay on this computer. Indexing makes no network calls. |
| Questions, and the passages retrieved to answer them | Sent to the model provider you choose, together with workspace instructions and any memories recalled for that answer. Memory suggestions are learned by sending your messages to that model. With a local model, and web access off (or Local-only mode on), questions and passages stay on this computer. |
| Web searches and pages | When web access is on, web searches go through OpenRouter's web plugin, Gemini with Google Search, or your SearXNG (`SHODH_SEARXNG_URL`), and pages are fetched directly. Off in Local-only mode. |
| Paper lookups | Building the paper graph online looks papers up on OpenAlex, only when you ask for it and the web is allowed. |
| Downloads | Models come from Hugging Face and the agent runtime from GitHub, each pinned and checked by SHA-256. The update check contacts GitHub once a day; it is off in Local-only mode, and you can turn it off yourself. |
| Agent runtime | With a cloud provider, the pinned runtime was also observed fetching public model metadata from its catalog (`catalog.stencil.so`). |
| Notifications | Desktop notifications carry titles and counts only, never document or answer text. |
| Phone link | Only once a phone is paired or a pairing is started, and never in Local-only mode. The computer and the phone connect over iroh (QUIC, encrypted end to end, dialled by public key), directly where they can and otherwise through n0's public relays, which forward only encrypted traffic. Each side also publishes and looks up its address through n0's address lookup, under its public key (`crates/shodh-link/src/lib.rs`). |
| Phone notifications | Only when you turn on notifications on the phone, and never in Local-only mode: the computer posts a one-word, content-free wake-up to the UnifiedPush address the phone registered (its push server sees the post, not what it is about). The details come over the link when you open the app (`app/src-tauri/src/phone_link/push.rs`). |
| Slack (when connected) | Over Slack's Socket Mode from this computer. The questions people ask the bot, and the channels you choose to index, come from Slack; answers, with citations from the one workspace you selected, go back to Slack, with paths, user names and ids removed. |
| Telegram (when connected) | Over the Telegram Bot API, polled from this computer. Your messages to the bot, and those of people and groups you allow, come from Telegram; answers go back to the same chat. |
| GitHub (when connected) | Calls to GitHub's API with the Shodh GitHub App's user token, for the repositories you allowed and only what you let it do there (read or change code, pull requests, issues). |
| API keys | Stored in the OS credential store (Windows Credential Manager, macOS Keychain, Secret Service on Linux). They are never kept in the app's web storage or shown back in the UI. A key set through an environment variable applies to that session only. Subscription sign-ins are held by the agent runtime's own account store, in Shodh's app data folder. |
| Audit log | Kept in `shodh.db` on this computer. |
| Graphics card search (Windows) | Windows GPU search uses Microsoft's DirectML (`DirectML.dll`, installed with Shodh). Its licence says it "may collect information about you and your use of the software, and send that to Microsoft", and that you can opt out of many of these scenarios but not all. Turn off "Use the graphics card for search" (System status panel) and Shodh does not use DirectML: search runs on the CPU and stays fully local. The DLL is still loaded with the app, as it has been since ONNX Runtime was added; it is not called. On macOS, graphics card search uses Apple's Core ML, part of macOS. |

## Known limitations

- **Unsigned installers:** the Windows installer and the macOS app are not signed with a publisher certificate, so the system warns before the first launch (the [README](../README.md#download) has the steps).
- **No Intel Mac build:** macOS builds are for Apple silicon only.
- **Unencrypted storage:** released builds do not encrypt the local index (LanceDB, Tantivy) or the app database (`shodh.db`). The app database can use SQLCipher only as a build option that released builds do not use. Use OS full-disk encryption (BitLocker or FileVault).
- **Audit chain without a key:** the hash chain shows edited, deleted or reordered events. Someone who can write to `shodh.db` could still rebuild the whole chain.
- **Agent runtime egress:** the pinned runtime's catalog fetch cannot be turned off (see [ADR 0001](adr/0001-agent-harness-omp.md)).
- **Code mode commands on Windows run as you:** on Windows, approved shell commands run with your user account's permissions, without a sandbox (with time and memory limits, and without Shodh's API keys in their environment). On macOS they run in macOS's sandbox: no network, changes only in the project folder (its `.git` folder included) and the conversation's scratch folder, and nothing of the rest of your home folder, your keychain or Shodh's data is read. Files outside your home folder, such as other drives, stay readable there.
- **Provider keys in Code mode:** Shodh does not pass API keys or tokens to commands. On Windows a command runs as you, so it could still reach a key through your account (for example the credential store); on macOS the sandbox closes the keychain and Shodh's data.
- **Chunking:** Word, PowerPoint, HTML and plain text are chunked with sliding windows, not by their structure.
- **Answer quality:** no answer-quality baseline is published; only retrieval is measured in CI (below).
- **No local bot servers or Google Drive sync:** the old WhatsApp, Telegram and Discord bot backends and the Google Drive connector have been removed; they ran unauthenticated local HTTP servers. Today's Slack and Telegram connectors open no port: each connects out from this computer.

## Retrieval quality

[`crates/shodh-eval`](../crates/shodh-eval) measures retrieval on the app's real indexing and search code, with no language model involved. The **Retrieval eval** CI job re-indexes both committed sets on every PR. Retrieval is deterministic, so the job fails on any drop below the committed baseline.

| Set | What it is | Questions |
|---|---|---|
| Synthetic | Fictional contracts and invoices (paged PDFs with tables), research notes and policies | 50, 5 of them unanswerable |
| Scanned | Pages of those contracts and invoices as image-only scans, plus text distractors, read with PP-OCRv6 | 34, 5 of them unanswerable |

Baselines ([synthetic](../crates/shodh-eval/data/synthetic/baseline.json), [scanned](../crates/shodh-eval/data/scanned/baseline.json); recorded 2026-10-11, top 10, reranking on):

| Metric | Synthetic | Scanned |
|---|---|---|
| hit@1 (right document first) | 0.978 | 1.000 |
| MRR | 0.989 | 1.000 |
| page_hit@1 (right page first) | 0.939 | 1.000 |
| passage_hit@1 (chunk holding the expected passage first) | 0.947 | 1.000 |
| hit, page_hit and passage_hit @5 and @10 | 1.000 | 1.000 |
| Files indexed | 1.000 | 1.000 |

These are small fictional sets. They catch regressions; they do not predict quality on your documents. Answer quality (`shodh-eval answers`) needs a language model, so it does not run in CI.

## Configuration

### Choosing a model

Choose a model in **Settings → Models**, or pre-configure it with environment variables. The environment variables are ignored unless `SHODH_LLM_PROVIDER` is set.

| Variable | Values |
|---|---|
| `SHODH_LLM_PROVIDER` | `openrouter`, `anthropic`, `openai`, `google` (or `gemini`), `grok` (or `xai`), `ollama` |
| `SHODH_LLM_MODEL` | Provider model ID (optional), e.g. `anthropic/claude-haiku-4.5` |
| API key | The provider's usual variable: `OPENROUTER_API_KEY`, `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY` (or `GOOGLE_API_KEY`), `XAI_API_KEY` |

A key saved in the app is used before a key from an environment variable.

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

## Building from source

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

## Architecture

```
shodhRAG/
├── crates/
│   ├── shodh-rag/              # Core library
│   │   └── src/
│   │       ├── processing/     # Parsers (PDF layout, Office XML, spreadsheets, CSV/TSV), OCR, chunkers, size limits
│   │       ├── search/         # Tantivy BM25 and fusion
│   │       ├── storage/        # LanceDB store
│   │       ├── embeddings/     # E5 via ONNX Runtime, pinned model downloads
│   │       ├── reranking/      # Cross-encoder reranker
│   │       ├── harness/        # omp sidecar, host tools, citation check, provider errors, web, MCP, Code mode
│   │       ├── folder_sync.rs  # Folder manifests and sync
│   │       ├── research/       # Paper graph, paper results
│   │       ├── workspaces/     # Workspaces and templates
│   │       ├── user_memory/    # Memories and the learner
│   │       ├── recall/         # Index of your coding-session transcripts
│   │       ├── audit/          # Hash-chained audit log
│   │       └── llm/            # Provider settings and local providers
│   ├── shodh-link/             # Phone link (iroh) and the phone's update check
│   ├── shodh-ontology/         # Typed statements behind memories, snippets and results
│   └── shodh-eval/             # Retrieval and answer evaluation
├── app/
│   ├── src/                    # React 19 + TypeScript frontend
│   │   ├── components/shell    # Sidebar, Settings
│   │   └── features/           # ask, agent, focus (side discussions), library, workspaces, tasks,
│   │                           # activity, audit, inbox, memory, notifications, modelPicker, setup
│   └── src-tauri/              # Tauri commands, agent tools, file watcher, notifications, updater
├── mobile/                     # Shodh for Android (Tauri)
├── site/                       # The project website (GitHub Pages)
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
5. The citation check verifies each cited statement and flags the unsupported ones.

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
