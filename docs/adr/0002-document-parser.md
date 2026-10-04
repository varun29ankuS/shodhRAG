# ADR 0002: Document parser — layout heuristics on pdf_oxide

- **Date:** 2026-10-04
- **Status:** Accepted
- **Context:** spec §6 (parsing, chunking, citations) and §3a (ort alignment); the user's main workflow is reading research papers.

## Problem

On the research-paper corpus (15 PDFs, 367 pages, 44 MB, plus one `.tex` file), the old pipeline had four failures:

1. **No page numbers on any citation.** Every PDF was read twice:
   - `pdf_extract` produced the whole-document text, with no pages.
   - The lopdf content-stream parser was used for per-page text. It returned **0 characters on all 367 pages**: it cannot decode font encodings, hex strings or `TJ` arrays split across lines.
   - So every page section was empty. The chunker fell back to unpaged sliding windows: 760 chunks, 0% paged.
2. **RoboTTT.pdf rejected as "scanned".** Its ToUnicode CMap has a 6-byte `bfrange` destination. `pdf_extract` panics on that (`adobe-cmap-parser`: "bad length of hexstring"). lopdf returned nothing, and the OCR fallback found nothing. pypdf extracts its text fine.
3. **No structure.** Chunks were 1,750-character windows. They cut through tables, theorems and equations, and carried no section path.
4. **Text-quality defects.**
   - The arXiv margin stamp was glued into chunk text.
   - `pdf_extract` dropped 44% of the words of 2406.06484 (word recall 0.559).

## Candidates

Each candidate was run by the same harness on the same 15 PDFs and scored by one script against pypdf's per-page text. The harness and scorer were kept outside the repository; the method is described below.

- **Word recall:** multiset recall of lower-cased words of 2+ characters.
- **Precision:** the same measure in the other direction. It penalises duplicated or garbage text.
- **Bigram:** recall of adjacent word pairs within pypdf's lines. This is a reading-order proxy. pypdf follows content-stream order, so it is a reference, not ground truth.

| Candidate | Word recall mean (min) | Precision | Bigram | Blocks with page | Headings | Tables | Speed (ms/page) | Models | License |
|---|---|---|---|---|---|---|---|---|---|
| Old: pdf_extract + lopdf | 0.891 (**0.000**, RoboTTT) | 0.916 | 0.880 | **0%** | 0 | 0 | 13.4 (release) | none | MIT |
| docling.rs 1.91 ML pipeline (layout_heron int8 + TableFormer, OCR off) | 0.946 (0.869) | 0.961 | 0.930 | 92% | 480 | 75 | **690** (release); 4.4 s/page on Forman-Ricci | 69 MB layout + 144 MB TableFormer | MIT |
| docling.rs text layer (no models) | 0.819 (0.572) | 0.979 | 0.745 | 100% | 0 | 0 | 14.4 (release) | none | MIT |
| pdf_oxide 0.3.78 `extract_structured` as-is | 0.966 (0.900) | 0.972 | 0.941 | 100% | 0 (corpus is untagged) | 68 | 9.6 (release) | none | MIT/Apache-2.0 |
| **Chosen:** shodh layout heuristics on pdf_oxide spans | 0.961 (0.902) | 0.971 | 0.937 | **100%** | 455 | 37 (only on captioned pages) | **11.3 (debug; pdf_oxide at opt-level 3)** | none | MIT/Apache-2.0 |

Block kinds the chosen parser produced on the corpus:

| Kind | Count |
|---|---|
| paragraph | 5,420 |
| equation | 1,313 |
| reference_entry | 1,237 |
| heading | 455 |
| list_item | 186 |
| figure (caption) | 114 |
| code | 82 |
| footnote | 75 |
| table | 37 |
| theorem / lemma / … | 30 |
| title | 12 |
| proof | 9 |
| definition | 8 |

**Not run, with reasons:**

- **xberg 1.3.3.** Its PDF backend, `xberg-native-pdf`, has a dependency list identical to pdf_oxide's, so it adds nothing over pdf_oxide here. It also brings a large optional surface: OCR engines, LLM clients, a web server.
- **pdfium-render.** It needs the native Pdfium DLL shipped and loaded at runtime. pdf_oxide fills the same role (positioned glyph runs, fonts, tables) in pure Rust.

## Decision

PDFs are parsed by `processing::pdf_layout`. Positioned text runs come from pdf_oxide. The reading order is pdf_oxide's structure-tree order for trustworthy tagged PDFs, else column-aware XY-cut. The default top-to-bottom order interleaves the two columns of papers line by line; the Diffusing Blame paper showed this.

Runs become lines, and lines become typed blocks by geometric and font statistics:

- **Headings:** size relative to the body font, plus bold or numbering. Levels come from numbering depth, or from size ranks when a document has no numbering.
- **Title:** the largest heading on page 1.
- **Display equations:** math-symbol density plus offset from the column edge, or an equation number.
- **Code:** monospace fonts, excluding URLs.
- **Footnotes:** small type in the bottom 30% of the page.
- **List items** and **captions.**
- **Bibliography entries:** split at `[n]` markers or at hanging-indent starts.
- **Tables:** pdf_oxide's detector. It runs only on pages with a "Table N" caption, unless the document has no captions at all; detection is about 75% of pdf_oxide's per-page cost.
- **Running headers, footers, page numbers and the arXiv margin stamp** are removed.

Semantic kinds are assigned in `document_model` from text: theorem-likes, definitions, proofs, figure captions, references, and table captions attached to their tables. Every block carries its page, a PDF-point bounding box (bottom-left origin) and its section path.

**docling.rs is not adopted now**, for these reasons:

- **Speed.** It is about 72× slower per page than pdf_oxide in release builds (690 vs 9.6 ms/page). Debug builds would be far slower again, because its page renderer is pure Rust.
- **Equations.** It emitted no formula items on this corpus.
- **Model files.** It adds about 213 MB of models.
- **Configuration.** It locates models through process-wide environment variables (`DOCLING_RS_MODELS_DIR`, `DOCLING_LAYOUT_ONNX`, …) or paths relative to the working directory or executable. There is no constructor that takes a path, so a desktop app must mutate its environment before any thread starts.
- **Concurrency.** `Pipeline::convert` takes `&mut self`.

Its layout model is clearly better on hard pages, for tables and figure regions. The ort upgrade to 2.0.0-rc.13 in this change keeps it adoptable as a later "hard page" path: for example, pages where the heuristic finds a table caption but no table.

## Fallbacks and scanned PDFs

1. The layout parser runs first. Any panic inside it is caught, and reading order is retried per page.
2. If the layout parser cannot open the file, `pdf_extract` provides unpaged text. A panic there is caught too.
3. A PDF counts as scanned only when its whole text layer is essentially empty: under 200 characters in total and under 8 per page. It is then OCR'd page by page with Windows OCR, which keeps page numbers. One extractor failing never makes a PDF "scanned". The RoboTTT regression test builds a PDF with the panicking CMap.

## Index compatibility

- **No storage schema change.** The LanceDB schema is unchanged. Structured chunks add these keys to the existing `metadata_json`: `page_start`/`page_end`, `bboxes`, `section_path`, `block_kinds`, `unit_kind` and `chunker = structure-v1`.
- **Existing indexes keep working.** Their chunks remain searchable and unpaged.
- **Re-indexing replaces old chunks.** Re-indexing a file replaces its chunks. It also deletes rows stored under older spellings of its path: verbatim, forward-slash, and the previous normalization.
- **Page numbers and highlighting need a re-index.** A library gets page numbers, sections and box highlighting only after its folder is re-indexed.

## Consequences

- **Dependencies.** `pdf_oxide = "=0.3.78"` is added. `office_oxide` is pinned to `=0.1.10`, because 0.1.11+ adds a struct field that pdf_oxide 0.3.78 does not initialise. Lift both pins together.
- **Dev builds.** `[profile.dev.package.pdf_oxide] opt-level = 3` keeps parsing at about 11 ms/page.
- **Other formats.** LaTeX and Markdown get the same block model, from `\section`, theorem environments, `equation`, `tabular`, `\bibitem`, and ATX headings / fences / pipe tables. They have no page numbers.
- **Known gaps.** Wrapped multi-line display equations can split into several equation blocks. The chunker re-attaches them to their context. Table detection by the heuristics alone is weaker than TableFormer (37 tables vs docling's 75); see "Table model" below.

## Table model (amendment, 2026-10-04)

**Problem.** Result extraction read 0 values from the 37 tables the heuristics found in the corpus. Refusals: no dataset named 14, header not recovered 7, no method column 9, merged columns or numberless rows 7.

**Decision.** A hybrid path. The heuristic parser still parses every page. Pages are flagged as table candidates (`pdf_layout::candidate_cues`) by any of:

- a `Table N` caption;
- three or more nearby lines split into cells that are mostly numbers;
- cells starting at the same x positions in three or more columns over four or more lines;
- three or more horizontal rules on a page with a numeric row;
- a table found by the heuristic detector.

Only on those pages, docling.rs 1.91 (`docling-pdf`, MIT) renders the page. Its Heron layout detector (int8) finds the table regions, and its TableFormer port (fp16-weight encoder, int8 decoder) predicts cells with row and column spans and header tags. `processing::table_structure` resolves the cells:

- a spanning header names every column under it;
- a spanning value is kept once;
- multi-row headers are flattened as `Parent / Child`;
- section rows become group labels;
- units are read from the headers.

A model table replaces the heuristic tables of its page. Pages where the model finds none keep them.

**Models.** `embeddings::model_store::table_model_artifacts`: five files from docling.rs's `models-v1` release, about 212 MB. Each is pinned by size and SHA-256 (GitHub's published digest), because a release tag can be re-pointed. They are downloaded only when the user installs them in Settings. Licences: Heron weights Apache-2.0; TableFormer weights CDLA-Permissive-2.0 / Apache-2.0; exports and runtime MIT. Without the models the heuristics are used, and the Results report says table extraction quality is reduced.

**Configuration.** docling.rs reads model paths only from process environment variables (`DOCLING_LAYOUT_ONNX`, `DOCLING_TABLEFORMER_*`). `TableModel::load` sets them only for the duration of the load, under a process-wide lock, and restores them afterwards. `DOCLING_RS_NO_GRAPH_CACHE` keeps docling from writing optimized graph copies into the user's home directory. This is sound on Windows, where `SetEnvironmentVariableW` is serialized by the OS. On other platforms a C library calling `getenv` concurrently could observe a torn update, so the model is not loaded there (`TableModelError::UnsupportedPlatform`) and the heuristics are used. The models take `&mut self`, so pages are processed one at a time behind a mutex.

**Latency.** Indexing stays on the fast path. A PDF with candidate pages is queued after its chunks are stored. A background worker re-parses it with the model and replaces its chunks only when the model structured a table and the file's size and modification time are unchanged (`table_refinement`). Papers that were scanned for results before are then re-extracted.

**Measured on the corpus (15 PDFs, 367 pages; debug build, docling-pdf and image crates at opt-level 3):**

| | Heuristics only | Hybrid (model on candidate pages) |
|---|---|---|
| Candidate pages | — | 101 of 367; they cover 52 of the 54 pages where the layout model, run on every page, finds a table |
| Table blocks | 37 | 70 (69 structured by the model) |
| Result values stored | 6 | 774 |
| Parse time, whole corpus | 4.1 s (11 ms/page; candidate detection included) | +211.8 s in the background. Per candidate page: median 0.87 s, p90 2.6 s, max 40.7 s (a 745-cell page-sized table) |

A spot check of 50 values from the arXiv papers against the rendered pages found the following:

- value, page, dataset and metric correct for all 50;
- 3 method labels wrong: the label carries a `Figure 1(a)` cell from a neighbouring label column.

**Not adopted:** docling.rs's full pipeline on every page. It is about 70× slower per page, and the heuristic parser is better on equations and reading order.
