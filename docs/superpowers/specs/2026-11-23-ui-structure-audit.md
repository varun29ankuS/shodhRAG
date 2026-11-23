# UI structure audit (2026-11-23)

Build: PR #198 (live preview phase 1), Windows, 1400×900 at 100%, a separate test profile
(`C:\shodh-readme\profile`, 7 CC-BY papers). Read over the WebView2 debug port: every sidebar view, Ask empty,
working and failed, and the floating chat open and closed. The positions below are CSS pixels measured from
the DOM, not estimates. The screenshots are in `docs/images/ui-audit/`.

The question asked: what moves, what changes size, and where each kind of action lives. The rules are in
memory "UI: sturdy, unambiguous": one obvious next action per state, every state visible, no hidden modes,
things stay where they were, nothing silently disappears.

## Findings

### A. Each view builds its own page frame

| View | Title | Title left, top | Primary action |
|---|---|---|---|
| Artifacts | H1 | 344, 74 | none; the search box sits at a section's right (1048, 163) |
| Library | H1 | 339, 74 | "Add folder", title row right (1182, 74) |
| Workspaces | **H2** | 344, 74 | "New workspace", title row right (1153, 74) |
| Connectors | **H2** | 344, 74 | none; a Connect per card |
| Tasks | H1 | **272, 108** | an inline add row (1045, 175); the List/Calendar switch sits **above** the title (275, 65) |
| Activity | H1 | **309**, 74 | an export toolbar halfway down the page (883–1325, 382) |
| Settings | H1 | 260, 74, plus a sub-nav; the content starts at 508 | none |

- There are five different left edges and two title tops. Moving between views moves the title, and the eye
  has to look for it again.
- "Add something" lives in a different place in every view: the title row, an inline row, a card, or nowhere.
- Workspaces and Connectors use H2 for the page title, so the heading outline (and screen readers) is
  inconsistent.

### B. The top bar changes from view to view

- Ask: Inbox (1078), More (1114), Instructions (1148), Sources.
- Every other view: Inbox (**1204**), Sources.
- The Inbox bell moves 126 px whenever the user leaves Ask.

### C. The Ask message box jumps

- A new chat has the box in the middle (y 345). After the first send it moves to the bottom (y 690).
- The things the user just used (mode, model, sources, Send) all move 345 px. "Send" becomes "Interrupt" in the
  same place, which is right.

### D. The floating chat covers controls

- The closed pill ("Assistant Ctrl+J", 1228–1384 × 844–884) sits over page content. Nothing reserves its
  space:
  - Library: it covers the **Open graph** button (1186, 861).
  - Activity: it covers the event table's Conversation column.
  - Settings → Models: it covers the LM Studio and Ollama text.
- The open chat (1004–1384 × 414–884) covers the Library's coding-sessions card text and Open graph.
- Floating by design (#191) is fine. Covering a control is not.

### E. One conversation, two sets of controls

- The Ask message box has: image, Dictate, Mode, Model, Research/Code, Sources, Send.
- The floating chat has: Reply, Send, and the header buttons (Open in Ask, Minimise, Close).
- To change the mode, the model or the sources, the user has to leave the floating chat. The difference
  isn't explained, so it reads as missing.

### F. Status shown twice, and the two copies disagree

- Model: the sidebar pill "gemma-4-31b-it" has a **red** dot. The message box chip "Google: Gemma 4 31B (free)"
  has a **blue** dot. Both describe the same model.
- Sources: "All sources · 7 files" shows in the top bar on every view and again in the message box.

### G. A known failure is shown only after sending

- The test profile's OpenRouter key was rejected, and the sidebar dot was already red.
- The message box still offered Send with no notice. The user found out only after sending: the "Your OpenRouter
  key was rejected" card, with Fix key and Change model.
- The failure card itself is the bar: a short state and a one-click fix. It comes one step late.

### H. Labels wrap inside buttons

- On the Library folder card, "Browse files" and "Re-index" each break onto two lines, so the card's button row
  is taller than its siblings and looks broken.

### I. Too much text

- Library, Artifacts, Activity and Workspaces open with a sentence or a paragraph per section. Library's coding
  sessions card is about 90 words before its button. This is already queued for the copy pass (memory "UI:
  sturdy, unambiguous"). It is listed here because it pushes each view's controls down by different amounts.

### J. The code review panel ("Review this")

Opened from an HTML block in an answer; model Claude Sonnet 5.5 through a Claude (Pro/Max) sign-in.

- **It covers the conversation.** The panel takes the right half (720–1400 px) over Ask:
  - the answer is cut mid-word;
  - the message box is cut at "Claude S…";
  - the status line underneath still shows the previous answer's model.
- **The primary button is cramped.** "Review this" wraps onto two lines inside an orange button that also
  holds the shortcut (Ctrl+Shift+E).
- **The presets read as tabs, but act as a single choice.**
  - Algorithm, Security, Performance, Simpler version, Architecture and Your question look like tabs.
  - "Your question" is selected on open, with an empty box under it.
  - Picking another preset hides the box, so the user can't tell whether their question will be sent.
- **It offers a model that can't answer, then fails.**
  - The line beside the button says it sends to claude-sonnet-5-5.
  - After about 10 s, a red paragraph says subscription models answer only through the assistant in Ask.
  - The failure has no fix button (switch model, use a key) and no short state. It is finding G again.
- **No result layout.** Below the controls, half the panel is empty, with no hint of what a review returns or
  where.
- **Mixed type.** The title row mixes a bold label with a monospace subtitle ("Code from the answer (html)"),
  and the editor's label is a lowercase "snippet".

### K. Ask says it made something it can't make

The user asked, in **Ask** mode, "build me a small artifact" and then "html page is fine".
- The model printed an HTML code block and wrote "You can now view it in the Artifacts tab".
- It then opened Artifacts, which was empty.
- Asked "i dont see any html page there", it said it "will create a proper artifact now", and again made nothing.

The cause:
- Ask has no tool that builds or saves a page; only Creator mode does.
- The prompt never says what the mode cannot do, and nothing offers the switch.

The user's request was an explicit make. Under the 2026-11-21 artifacts rule, that belongs in Artifacts.

### What already works

- The sidebar is identical on every view.
- Send and Interrupt share one place.
- The failure card has a state and its fixes.
- Connectors are logo cards with one action each.
- The Inbox and Sources sit in the same top-right corner off Ask.

## The layout system this asks for

1. **One page frame** for every view except Ask:
   - one left edge and one max width;
   - the H1 always at the same top;
   - a **primary-action slot** at the title row's right, always reserved: "Add folder", "New workspace", "Add
     task", "Export", "New creation";
   - an optional **toolbar row** under the title for view switches (Tasks List/Calendar), filters and
     search.
   - Settings keeps its sub-nav inside the frame.
2. **One top bar:**
   - the view title on the left;
   - on the right, a fixed group, Sources then Inbox, at the same x on every view;
   - view-specific actions (Ask's More and Instructions) sit in a reserved slot to the left of that group, so
     nothing to their right moves.
3. **The Ask message box always sits at the bottom.** In an empty chat, the suggestions go above it.
4. **A reserved corner for the chat.** Every page frame keeps a bottom gutter the height of the pill (56 px), so
   the pill never covers a control. The open chat may cover content (it floats by design) but never a page's
   primary-action slot.
5. **The floating chat gets the same controls row as Ask** (Dictate, Mode, Model, Sources), as compact
   chips. One conversation, one set of controls.
6. **One model state:**
   - the sidebar pill and the message box chip read one source and show the same colour and word;
   - a known failure (rejected key, rate limit with no fallback) shows before sending, as a one-line state
     above the message box with its fix button;
   - Send stays enabled, because a different model may work.
7. **Buttons never wrap.** Labels are `white-space: nowrap` with a minimum width; a row that doesn't fit
   moves into an overflow menu.
8. **Panels never cover the conversation they belong to.** The review panel docks beside the chat; the chat
   narrows rather than being covered.
9. **The review panel** is laid out as follows:
   - one row of preset chips, which can combine with "Your question", and the question box always visible;
   - a compact primary button, with the shortcut in the tooltip;
   - a result area with its own empty, working, done and failed states;
   - a failed state with a short line plus "Switch model". A model that can't serve the review is never offered:
     the panel picks the first one that can, or says none can, before sending.
10. **Making things from Ask:**
    - every non-Creator mode's prompt states that it cannot build apps, pages or games;
    - an explicit make request gets a one-click "Build in Creator" card that resends the request in Creator
      (turning on Code mode first, as #189 does);
    - the assistant never says something is in Artifacts unless a tool saved it.


Each rule gets a test:
- frame and top-bar geometry checked per view in the frontend tests (the same rects as this audit: title left
  and top, slot positions);
- the composer's position in an empty versus an active chat;
- no control intersects the pill's rect, in the Windows launch test with the existing hit-test probe.

## Screenshots

`00-start` Ask with an answer · `12-ask-empty` new chat · `13-ask-working-1` working ·
`13-ask-working-3` failed (key rejected) · `02-artifacts` · `03-library` · `06-tasks` · `07-activity` ·
`08-settings` · `11-lib-chat` the floating chat open over Library.
