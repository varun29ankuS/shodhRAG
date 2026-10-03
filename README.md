<p align="center">
  <img src="app/public/shodh_logo_nobackground.svg" alt="Shodh" width="120" />
</p>

<h1 align="center">Shodh</h1>

<p align="center">
  <strong>Private document intelligence for teams: ask questions across your folders and get answers cited to the exact page.</strong>
</p>

<p align="center">
  <a href="#what-works-today">What works today</a> &middot;
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

**Shodh** (Sanskrit: शोध, "research") is a desktop app that reads the folders you point it at and answers questions about them. Each answer cites the file and page it came from.

- **Processing:** your documents are parsed, indexed and embedded on your computer.
- **Answers:** these come from the language model you choose. That can be a model running locally, or a cloud provider using your own API key.

> **Status: pre-release.** The core ask-your-files loop works. The enterprise-grade pieces are being built in the open and tracked in the [design spec](docs/superpowers/specs/2026-10-02-folder-sources-grounded-answers-design.md): the agent harness, folder sync, the knowledge graph, evaluation, and server mode. This README describes only what works today. Everything else is listed under [In progress](#in-progress).

## What works today

**Ask your files**
- Answers stream in with numbered citations.
- Click a citation to see the source passage. PDF citations carry page numbers.
- A run summary shows what actually happened: elapsed time, search queries issued, passages retrieved, and tools called.
- Conversations are saved locally, including their citations and run details.

**Documents**
- **Text documents:** PDF (text layer; scanned pages via Windows OCR), Word (`.docx`), PowerPoint (`.pptx`), Markdown, HTML, plain text and source code.
- **Spreadsheets:** `.xlsx`, `.xls`, `.xlsm`, `.xlsb`, `.ods`, plus CSV and TSV.
  - Sheets are indexed row by row, and every chunk repeats the header row. That way any single row can be retrieved on its own.
  - CSV encodings (UTF-8, UTF-16, Windows-1252) are detected automatically.
- **Failures:** if a file can't be indexed, the reason is recorded and logged instead of the file being skipped silently. Showing these in the Library is in progress.

**Retrieval**
- **Hybrid search:** dense vectors (multilingual E5, ONNX, on device) and BM25 keyword search (Tantivy), fused with reciprocal rank fusion.
- **Reranking:** a cross-encoder reorders the results.
- **Query rewriting:** an LLM rewrites and decomposes multi-part questions.

**Models**
- **Cloud:** OpenRouter, Anthropic, OpenAI, Google Gemini, xAI and Perplexity, using your own API key.
- **Local:** Ollama, or GGUF models run in-process through llama.cpp.
- **Pre-configuration:** set the model with environment variables, so IT can lock in the approved provider on managed machines. See [Configuration](#configuration).

**Calendar and tasks**
- Events and tasks share one Calendar view.
- Calendar items are indexed, so you can ask about them.

## In progress

Each item below has a section in the [design spec](docs/superpowers/specs/2026-10-02-folder-sources-grounded-answers-design.md) and lands through reviewed PRs.

| Area | What it brings |
|---|---|
| **Evaluation harness** | Measured retrieval and answer quality: CUAD contracts and synthetic invoices in CI, and your own folders locally. Numbers will be published here. |
| **Folder sync** | Folders stay in sync automatically. Changes made while the app was closed are caught on the next start, and renames are cheap. |
| **Document parsing** | Layout-aware parsing (tables, reading order, OCR), and chunks sized by tokens and aligned to the document's structure. |
| **Document viewer** | Clicking a citation opens the real document at the cited page with the passage highlighted. |
| **Records and totals** | Typed records (invoices, spreadsheet rows). Totals and counts are computed in code and report how many records they covered. |
| **Agent harness** | Agents in conversation, run by [oh-my-pi](https://github.com/can1357/oh-my-pi) over RPC. Shodh's tools are permission-checked in Rust. Sub-agents appear as live lanes. See [ADR 0001](docs/adr/0001-agent-harness-omp.md). |
| **Citation verification** | Each cited sentence is checked against its source. Unsupported claims are flagged. |
| **Knowledge graph** | Entities and relations extracted locally (rules plus GLiNER2), each with page-level provenance. Used for multi-hop retrieval. |
| **Security and governance** | Secrets in the OS keychain, an encrypted local database, a tamper-evident audit log, and a Local-only mode. |
| **Server mode** | The same core running headless, with SSO and per-document permissions enforced inside every query. |

## Quickstart

### Prerequisites

- [Rust](https://rustup.rs/) (stable) and [Node.js](https://nodejs.org/) 22 or later.
- Platform build tools:
  - **Windows:** [Microsoft C++ Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) and `protoc` (`choco install protoc`).
  - **macOS:** `xcode-select --install`
  - **Linux:** `sudo apt install build-essential libssl-dev libwebkit2gtk-4.1-dev protobuf-compiler`
- Search models (≈600 MB): on first run the app shows **Set up search** in Ask and Library and downloads the pinned, SHA-256-verified files itself. Debug builds install them into `models/` at the repository root (shared by every checkout of the repo); release builds use `<app data>/models`. `MODEL_PATH` overrides the location. Layout:
  - `models/multilingual-e5-base/`: `model_O4.onnx` and `tokenizer.json` from [intfloat/multilingual-e5-base](https://huggingface.co/intfloat/multilingual-e5-base/tree/main/onnx)
  - `models/ms-marco-MiniLM-L6-v2/`: `model_O4.onnx` and `tokenizer.json` from [cross-encoder/ms-marco-MiniLM-L6-v2](https://huggingface.co/cross-encoder/ms-marco-MiniLM-L6-v2)

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
| `SHODH_LLM_PROVIDER` | `openrouter`, `anthropic`, `openai`, `google`, `grok`, `ollama` |
| `SHODH_LLM_MODEL` | Provider model ID (optional), e.g. `anthropic/claude-haiku-4.5` |
| API key | The provider's usual variable: `OPENROUTER_API_KEY`, `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`, `XAI_API_KEY` |

Local models:
- **Ollama:** run `ollama pull <model>`, then set `SHODH_LLM_PROVIDER=ollama`.
- **GGUF:** select the file in **Settings → Models → Local**.

## Data and privacy

| Data | Where it goes |
|---|---|
| Your files, their text and their embeddings | Stay on this computer. Indexing makes no network calls. |
| Questions, and the passages retrieved to answer them | Sent to the model provider you configure. With a local model, nothing leaves the machine. |
| API keys | Stored in the OS credential store (Windows Credential Manager, macOS Keychain, Secret Service on Linux). They are never kept in the app's web storage or shown back in the UI. Keys saved by older builds are moved there on first launch. A key set through an environment variable applies to that session only. |

**Known gaps.** Each is tracked in the spec and will land as a reviewed PR.
- **Unencrypted index:** Shodh does not encrypt the local index (LanceDB, Tantivy). Use OS full-disk encryption (BitLocker or FileVault).
- **No messaging channels or Google Drive sync:** the old WhatsApp, Telegram and Discord bot backends and the Google Drive connector have been removed. They ran unauthenticated local HTTP servers. Messaging channels will return through a separate, authenticated gateway.
- **Stopping an answer:** it stops the display only. The provider call finishes in the background until the agent harness adds real cancellation.

## Architecture

```
shodhRAG/
├── crates/shodh-rag/        # Core library: ingestion, chunking, embeddings, hybrid search,
│   └── src/                 # reranking, chat engine, LLM providers
│       ├── processing/      # Parsers (PDF, Office, spreadsheets, CSV/TSV) and chunker
│       ├── search/          # Tantivy BM25 and fusion
│       ├── storage/         # LanceDB store
│       ├── embeddings/      # E5 via ONNX Runtime
│       ├── reranking/       # Cross-encoder reranker
│       ├── chat/            # Chat engine and streaming events
│       └── llm/             # Local (llama.cpp, ONNX) and HTTP providers
├── app/
│   ├── src/                 # React 19 + TypeScript frontend
│   │   ├── components/shell # Sidebar, Settings
│   │   └── features/ask     # Ask view, run chip, source preview, composer
│   └── src-tauri/           # Tauri commands (indexing, chat, LLM, calendar)
└── docs/
    ├── superpowers/specs/   # Design specs
    ├── superpowers/plans/   # Implementation plans
    └── adr/                 # Architecture decision records
```

**How a question is answered today:**
1. The question is rewritten if needed.
2. Hybrid search (vectors and BM25) finds candidate passages.
3. The candidates are reranked.
4. The top passages go to the model as context.
5. The answer streams back with `[n]` citations mapped to those passages.

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
| Vector store | LanceDB + Apache Arrow |
| Keyword search | Tantivy |
| Embeddings and reranking | multilingual E5 and an ms-marco MiniLM cross-encoder, via ONNX Runtime |
| Local LLM inference | llama.cpp (GGUF) |

## Contributing

1. **Branch and PR:** create a feature branch from `master`, open a PR, and merge only when CI is green.
2. **Production code only:** no TODOs, mocks, stubs or placeholders.
3. **Fix root causes:** never weaken a check to make CI pass.
4. **Keep PRs focused**, and explain *why* in commit messages.

Run these before opening a PR:

```bash
cargo fmt --all --check
cargo clippy --workspace
(cd app && npx tsc --noEmit)
```

## License

[Apache License 2.0](LICENSE). Copyright 2025 Shodh.
