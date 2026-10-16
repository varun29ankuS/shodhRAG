# Visual explanations: visual-first answers, "go deeper" windows and interactive pages

Status: design, awaiting review (2026-10-16).

## 1. Goal

Ask Shodh to explain anything and the explanation comes mainly as visuals. The AI marks
the parts that deserve their own explanation. Clicking a mark opens a nested window that
explains that part, also visual-first, which may hold marks of its own. When a visual
cannot show the idea, or the user asks for something to use (a calculator, a "what if"
model), the AI writes a small interactive page that runs in a sandbox.

Decided with the user:

- **The AI proposes and the user opens.** Nothing opens or runs without a click.
- **Scope is anything the user asks**, with or without documents.
- **Both approaches:**
  - A: typed visuals with "go deeper" marks;
  - C: model-written interactive pages, used only where typed visuals cannot do the job.

Success means:

- An explanation question gets a visual answer in one LLM call.
- Every visual element shows where it came from.
- Drilling down costs one call per window opened, and none for windows never opened.
- No interactive page can reach the network, the app's IPC, storage or keys, or send a
  question without the user's click.

## 2. What exists (built on, not rebuilt)

- **Visual renderers (`app/src/features/ask/visual/`).** Answers carry typed fences:
  `diagram`, `chart`, `plot`, `simulation`, `svg`, `figure`, `derivation`, `symbols`, plus
  KaTeX equations and tables.
  - The model writes a spec; Shodh validates it and draws it. The model never writes drawing
    code; `diagramSpec.ts` follows GitDiagram.
  - Diagram nodes already carry `cite` and `path` and have "Ask about this".
- **Nested windows (`app/src/features/focus/`).**
  - `focusStack.ts`: levels with push, back, forward and jump, up to `MAX_DEPTH`.
  - `FocusOverlay` and `SideThread`: a side discussion about one object.
  - `ExplorationMap`: the tree of side discussions.
  - `carriedPassages.ts`: a side thread inherits the parent answer's cited passages under the
    same numbers.
- **The visual gallery (`crates/shodh-rag/src/visuals/`, `app/src/features/visuals/`).**
  Every visual becomes a versioned object with refine, pin and export.
- **The grounding check.** Sentences are checked against the retrieved passages; numbers are
  checked against the passage text; the result shows as "Grounded n/m".
- **Sandboxed frames.** `WebSources.tsx` shows web pages in `sandbox=""` (no scripts). No
  frame runs model-written code today.

What is missing:

- The AI choosing the presentation on purpose.
- The AI marking what deserves its own window.
- Source labels on visuals other than diagrams.
- A script-running sandbox.

## 3. Visual-first answers (A)

### 3.1 Choosing the presentation

The research system prompt gains a visual-first section. The guidance is that the visual is
the body of the answer, and text is captions and glue.

| The question is about | Lead with |
|---|---|
| How something works, parts and flow | `diagram` (flow, architecture, sequence, layers) |
| How quantities relate | `plot` or `chart` |
| Change over time, dynamics | `simulation` or a `timeline` diagram |
| A formula or derivation | `derivation` / equation walk-through |
| Comparing options | table or `quadrant` diagram |
| Something to use or adjust | interactive page (Section 5) |
| A fact, a definition, writing | text (no visual forced) |

No router model and no extra call: the choice is made in the answer call. The guidance also
says to use one main visual per answer, and at most two.

### 3.2 "Go deeper" marks

- **The field.** Any visual element may carry `deeper`: one prepared question, at most 160
  characters, that would explain this element on its own.
- **Where marks go (v1):**
  - diagram nodes;
  - chart series;
  - plot and simulation parameters;
  - derivation steps;
  - symbol entries (equation terms).

  Diagram edges, single chart points and plot curves have no selectable element yet and
  come later.
- **In text,** a mark is written `[[shown text|prepared question]]`.
- **Validation (pure modules, Node unit tests):**
  - at most 4 marks per answer;
  - the question is non-empty and within the length cap;
  - the mark must sit on an element that exists;
  - extra marks are dropped, keeping the first 4 in document order, and the drop is logged.
- **Rendering.**
  - A marked diagram node gets a small glow, and its node panel offers "Go deeper".
  - Other visuals list their marks as a row of "Go deeper" chips under the visual.
  - Marked text gets a dotted underline.
  - Hovering shows the prepared question.
  - Marks are keyboard reachable: Tab moves through them, Enter opens.
- **Opening.** A click pushes a level on the focus stack. The side thread starts with the
  prepared question already asked. It carries:
  - the element;
  - its parent visual's spec;
  - the parent answer's cited passages, through `carriedPassages`.
- **Inside the window.** The answer follows the same visual-first rules and may hold marks.
  Depth is limited by `MAX_DEPTH`; at the limit, marks render without the glow and do
  not open.
- **Cost.** One call per window opened. No call is made until the click.

### 3.3 Saving

Visuals in nested windows are captured into the gallery like any other answer visual.
Their origin is recorded as the side answer, as captures are today.

## 4. Source labels on every element

### 4.1 Labels

Each element (node, edge, series, point, curve, parameter, step, term, table cell or row)
carries `source`:

| Value | Meaning | Rendering |
|---|---|---|
| `{ "cite": n }` | passage [n] of this answer | solid; [n] opens the source viewer at the outlined passage |
| `"general"` | the model's general knowledge | dashed outline, "general knowledge" in the legend |
| `"calculated"` | computed from other labelled elements | ƒ mark; hovering lists the inputs |
| `"illustrative"` | example values, not facts | striped, "illustrative" written on the visual |
| `"user"` | a value the user gave in this chat | plain |

The diagram's existing `cite` field becomes shorthand for `{ "cite": n }`.

### 4.2 Rules

- **A missing `source` means `general`.** Nothing reads as document-backed by default.
- **`cite` must name a passage retrieved for this answer.** The diagram check is extended to
  every visual type. A failing element is flagged "source not in this answer" and is not
  hidden.
- **A number on a cited element must appear in the cited passage.** This uses the grounding
  check's number matching. A failing element is flagged "not found in [n]".
- **Counting.** The "Grounded n/m" chip counts cited visual elements alongside sentences.
- **Legend.** Each visual gets a one-line legend written by Shodh, for example
  "6 from your documents · 2 general knowledge · 1 illustrative".

## 5. Interactive pages (C)

### 5.1 When

- The AI writes an `app` fence only when:
  - the user asks for something to use or adjust; or
  - no typed visual can show the idea.
- The prompt states this, and the evaluation (Section 7) measures how often the AI follows
  it.

### 5.2 Format

```app
{ "title": "Loan repayment",
  "inputs": [ { "id": "rate", "label": "Interest rate", "source": { "cite": 2 }, "value": 8.5, "unit": "%" },
              { "id": "years", "label": "Term", "source": "user", "value": 20 } ],
  "html": "...", "css": "...", "js": "..." }
```

- The total size is capped at 64 KB.
- v1 allows no external scripts, fonts or images: plain JS, SVG and canvas only.
- `inputs` is the provenance manifest. Shodh validates and checks it as in Section 4, and
  passes it to the page as its initial values.

### 5.3 Protocol: MCP Apps

The page and Shodh talk using the View–Host protocol of MCP Apps (SEP-1865, specification
2026-01-26): JSON-RPC 2.0 over `postMessage`. Shodh is the host.

Supported in v1:

- **`ui/initialize`, `ui/notifications/initialized`:** the handshake. Host context carries
  the theme and the display mode.
- **`ui/notifications/tool-input`:** the validated `inputs` manifest, which is the page's
  data.
- **`ui/notifications/size-changed`:** the page's height, clamped to the window.
- **`ui/notifications/host-context-changed`:** theme or display-mode changes.
- **`ui/message`:** the page proposes a follow-up question with its current values.
  - Shodh **always** requires the user's consent; the spec leaves this optional for hosts.
  - The message becomes a pending chip under the page, for example
    "Ask: what if the rate is 9%?".
  - The chip is part of Shodh's UI, outside the frame. Clicking it opens a nested window
    (as in 3.2) that asks the question with the values attached.
  - Each page has at most one pending chip; a new message replaces the old one.
- **`ui/request-display-mode`:** `inline` (in the answer) or `fullscreen` (a focus window).
  `pip` is not supported.

Not supported in v1, answered with a JSON-RPC "method not found" error:

- `tools/call` and `resources/read`: pages get no tools.
- `ui/open-link`.
- `ui/update-model-context`.

Following the standard leaves room to render MCP Apps from external MCP servers later,
through Settings → Tools & connections. That is a separate decision with its own review.

### 5.4 Sandbox

- **Double frame, as MCP Apps prescribes for web hosts.**
  - The outer frame is a sandbox proxy served from its own origin, a Tauri custom protocol
    that is not the app's origin.
  - The inner frame is the page, with `sandbox="allow-scripts allow-same-origin"`.
    `allow-same-origin` here is the proxy's origin, never the app's.
  - `allow-forms`, `allow-popups`, `allow-top-navigation`, `allow-modals` and
    `allow-downloads` are not set.
- **CSP of the page:** the MCP Apps default with no domains declared:
  `default-src 'none'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'none'`.
- **IPC.** The sandbox origin is not listed in any Tauri capability, so IPC calls from it are
  refused.
- **The app's CSP.** `frame-src` in `tauri.conf.json` allows only `blob:` today. It gains the
  sandbox origin and nothing else; `connect-src` is unchanged.
- **Isolation gate (must pass before anything else in C is built).** An automated test loads
  a hostile page into the real app (CDP, separate profile). The page tries to:
  - call `window.__TAURI_INTERNALS__` / `invoke`;
  - `fetch` the internet, the IPC endpoint and `http://ipc.localhost`;
  - read `localStorage` / `indexedDB` of the app origin and `parent.document`;
  - open a popup and navigate the top frame;
  - send 100 `ui/message` requests in a loop.

  Pass means every attempt fails and exactly one pending chip exists. If the test cannot be
  made to pass on Windows (WebView2) and macOS (WKWebView), C does not ship.

### 5.5 Failures

- The proxy forwards the page's `error` and `unhandledrejection` events to Shodh as a
  notification.
- Shodh shows "This tool failed: \<message\>" with a **Repair** button.
- Repair is one LLM call carrying the error and the page source, run only on click. There is
  no automatic retry.
- A page that does not complete `ui/initialize` within 5 s is shown as failed with the same
  Repair action.

### 5.6 Saving and export

- Pages are captured into the gallery as kind `app`, versioned like other visuals; Repair
  and refine create new versions.
- Export writes one standalone HTML file with the page, its inputs and the CSP meta tag.

## 6. Order of work (one agent at a time, one PR each)

1. **Visual-first prompt and marks (Section 3).** About 2 days.
   - Tests: mark parsing and validation (Node).
   - CDP check in a demo profile: a mark opens a nested window with the prepared question;
     no LLM request is sent before the click.
2. **Source labels (Section 4).** About 1.5 days.
   - Tests: an unknown `cite` is flagged; a cited number missing from its passage is
     flagged; a missing `source` renders as general; the Grounded count includes visual
     elements.
3. **Interactive pages (Section 5).** About 2.5 days.
   - First the isolation gate: the proxy origin plus the hostile-page test.
   - Then the protocol subset, the consent chip, failures and Repair, gallery and export.
   - Tests: the hostile page (Section 5.4); a protocol conformance test per supported method;
     the 64 KB cap; the 5 s initialize timeout.
4. **Free-model evaluation (Section 7).** About 30–45 calls over two days.

## 7. Evaluation

Run on the free model the user runs, on 15 questions:

- 5 how-it-works;
- 5 about quantities;
- 5 "make me a tool".

Recorded per answer:

- the visual renders;
- the chosen presentation fits the question (judged against the table in 3.1);
- source labels are present and correct;
- marks are useful, meaning the prepared question is answerable and not a repeat;
- for pages, it works on first render.

If pages work first time in fewer than 60% of the tool questions, `app` stays behind a
"beta" label in Settings and is off by default. The numbers go in the PR description.

## 8. Not in scope

- The AI opening windows by itself, and guided multi-step tours.
- External libraries inside pages.
- `tools/call` from pages, and rendering external MCP servers' Apps.
- 3D and maps.
- A separate router model.

## 9. Risks

- **Free models write weaker specs and pages than frontier models.** Typed specs are
  validated with errors shown (as today); pages get Repair and the beta gate.
- **Marks may be noisy or obvious.** The cap of 4 plus the evaluation check on mark
  usefulness. Prompt guidance: mark what a newcomer would not know, not what the answer
  already explains.
- **General knowledge presented visually still looks authoritative.** The dashed rendering
  and the legend are the mitigation; the label cannot be omitted, because a missing label
  is `general`.
- **WebView2 injects scripts into frames.** This is why the isolation gate comes before any
  other work on C.

## 10. References

- **Bret Victor, *Explorable Explanations* (2011).** Reactive documents, explorable examples
  and contextual information on demand; the basis for marks and pages.
  https://worrydream.com/ExplorableExplanations/
- **Jiang, Rayan, Dow, Xia, *Graphologue: Exploring Large Language Model Responses with
  Interactive Diagrams*, UIST 2023.** LLM answers turned into node-link diagrams in real time,
  with user control of diagram complexity. https://arxiv.org/abs/2305.11473
- **Suh, Min, Palani, Xia, *Sensecape: Enabling Multilevel Exploration and Sensemaking with
  Large Language Models*, UIST 2023.** Expanding concepts into sub-levels, plus hierarchy and
  canvas views; the closest precedent for "go deeper". https://arxiv.org/abs/2305.11483
- **Google Research, *Generative UI: LLMs are Effective UI Generators* (2025).**
  Model-written interactive pages preferred over markdown in most comparisons; the case
  for C. https://research.google/blog/generative-ui-a-rich-custom-visual-interactive-user-experience-for-any-prompt/
- **MCP Apps, SEP-1865 (specification 2026-01-26).** Sandboxed `ui://` HTML views,
  JSON-RPC over `postMessage`, a default CSP with no network; the protocol for Section 5.
  https://modelcontextprotocol.io/seps/1865-mcp-apps-interactive-user-interfaces-for-mcp
- **CopilotKit, *Open Intelligent UI* (MIT).**
  - Borrowed: presentation chosen per turn, provenance labels inside the UI, follow-ups
    only on deliberate clicks.
  - Not borrowed: its paid router and its code.

  https://github.com/CopilotKit/OpenIntelligentUI
- **Andy Matuschak, stacked notes / sliding panes.** Nested context kept visible along the
  path, as the focus stack's breadcrumbs and exploration map do.
  https://notes.andymatuschak.org
