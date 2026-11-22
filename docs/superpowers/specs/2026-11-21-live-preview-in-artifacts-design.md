# Live preview in Artifacts, with the chat floating over it

Status: approved by the user 2026-11-21 (option B; gaps closed in phase 3).
Voice moved to phase 1 by the user, 2026-11-22.

## Intent

When Creator builds a page, the user watches it take shape inside Shodh and improves it while it builds. They do that by typing in the chat and by pointing at the part they mean. Today they "can't tell" what it is doing:

- The build switches to Artifacts with the chat docked, but the page itself is not shown there.
- The page opens only on a click, in a separate window that takes the focus and covers the chat.

This is the first step of "the agent operates the whole app visibly" (agent-first UX): the assistant opens the view, builds, and the user steers.

## Goals

1. When a Creator build starts writing files, Artifacts shows the creation's page **live, inside the main window**. It reloads as files change, as today (at most every 1.5 s).
2. The chat **floats over the page** in the bottom-right corner, pill or expanded, exactly as on every other view (PR #191). It is **never covered by the page**. The page stays live while the chat is open and while the user types or dictates.
3. **Pointing.** The user presses **Point**, the element under the mouse is outlined, and a click attaches it to the chat box as a chip (`Pointing at: button "Sign up"`). The user types the request and sends. The assistant receives the element's description as page data.
4. **Pop out** moves the preview to its own window (today's window, unchanged) for a second screen.
5. Every guarantee of today's preview window holds for the embedded one: no app commands or events, offline unless the user turns internet on, no camera or microphone, navigation only within its own token's path, no downloads, no new windows.

## Non-goals

- Editing the page by hand in the preview, or drag-to-resize of elements.
- Pointing at more than one element per message. One chip; a new pick replaces it.
- Linux (not shipped).
- Changing what counts as an artifact. That is a separate question to the user.

## Anti-goals (a failure even if it works)

- The generated page can reach the app: any Tauri command, event, or the main webview's state.
- The chat, or an approval prompt, is hidden behind the page, so the user cannot steer or deny.
- Two chat states drift: the floating chat shows or sends something different from Ask.
- The preview is left showing over another view, a dialog or a menu.

## Constraints found in the code

- **The page must be drawn by its own webview, never in the app's page.** An iframe would run agent-written code in the same process as the app's privileged UI, and it could not be kept offline: the dead-proxy switch is per browser process (`preview_window::browser_args`).
- **A native child webview paints above the main webview's page.** Anything the app draws in that rectangle (the floating chat, menus, toasts, dialogs) is hidden under it. This is why the chat must be its own webview layered above the preview (option B, chosen by the user).
- **The ACL gate forbids this today.** `acl_manifest.rs` fails if `preview_window.rs` uses `.add_child(` or `WebviewBuilder::new(` ("the preview must be a window of its own"), or if any other file opens a webview without a label and a capability. This design replaces that rule with equally strict ones (below). It does not delete it.
- **Capabilities are scoped by webview label** (`capabilities/main.json`: `"webviews": ["main"]`). A child webview labelled `preview-*` gets nothing, as today's window does.
- **Tauri 2.12.5.** Child webviews (`Window::add_child`, `WebviewBuilder`) need Tauri's `unstable` cargo feature. That API can change in a Tauri minor release, so pin and watch it.
- Agent events go out with `app.emit(AGENT_EVENT, …)` to every webview. Only the main webview may act on them (below).

## Approaches considered

| | What | Trade-off |
|---|---|---|
| A | The page freezes to a still image while the chat is open | Small, but the page is not live while the user types. Rejected by the user. |
| **B (chosen)** | Three layers in the main window: the app, the live page, and the chat in its own webview on top | Everything live. Costs: a second chat surface kept in sync, a changed ACL rule, and Tauri's unstable multi-webview API. |
| C | Iframe in the app's page | Breaks isolation and offline. Ruled out. |

## Design

### Layers in the main window (bottom to top)

1. `main`: the app, as today.
2. `preview-<creationId>`: the creation's page.
   - Built with the same lockdown as the preview window: custom-protocol URL with its token, `incognito`, its own data directory and `browser_args` (dead proxy while offline), `initialization_script_for_all_frames(GUARD_SCRIPT)`, the same `on_navigation` rule, permission denial and crash reporting on Windows, devtools in debug only.
   - Shared with `preview_window.rs` through one builder function, so the two cannot drift.
3. `dock`: the floating chat, shown only while an embedded preview is visible. On every other view the existing in-page dock is used.

### Placing the preview

- Artifacts' preview pane renders an empty placeholder element. The main webview reports its rectangle to Rust (`creation_preview_embed { id, rect }`) on mount, on resize (ResizeObserver) and on scroll.
- Rust sets the child's bounds to the rectangle. Rust also clamps it to the window, so a bad rect cannot place the page over the title bar or outside the window.
- The main webview reports `covered: true` whenever something of its own must show above the page: a dialog, a menu or popover portal, the command palette, a toast over the rectangle. Rust hides the child until it reports `covered: false`.
  - One pure function decides "covered" from the open-overlay registry and the rectangles, and it is unit-tested.
  - Leaving Artifacts, switching creation, or minimising the window also hides it. Hidden is the default: if the main webview stops reporting, the page is hidden, never left on top.

### The floating chat layer (`dock` webview)

- **Separate entry.** A second entry page (`dock.html` → `dock.tsx`) renders the same `ConversationDock` component. It sits fixed at the bottom-right, and its bounds follow the pill or the expanded chat. The webview's background is transparent so the corners and shadow look as they do in-page.
- **Single source of truth: the main webview.** The dock webview does not run its own chat session.
  - Main → dock: on every change, main sends a snapshot of what `ConversationDock` reads (`messages` of the active conversation, `isStreaming`, the pending approval, `sideRun`, `streamingConversationId`, setup state, the draft) through Rust to the `dock` webview only (`emit_to("dock", …)`).
  - Dock → main: user actions (`send`, `steer`, `cancel`, `approve`, draft edits, start or stop dictation, minimise, Open in Ask, attach the pick) go through one Rust command. That command forwards them to `main` only, which runs them on its real session.
  - The dock's React tree gets a `RemoteChatSession` provider with the same interface as `useChatSession`. `ConversationDock` stays unchanged, and only its provider differs.
  - The dock webview ignores `agent_event`s itself. A test asserts that its entry does not subscribe to them.
- **Capability `dock-window`** (`webviews: ["dock"]`) grants only the commands the dock's modules name, computed by the same static check `acl_manifest.rs` already runs for `main-window` and `print-window`.

### Pointing

- **Point** (a button in the preview's toolbar, in the main webview, outside the page) puts the preview into pick mode. Rust records a fresh random nonce for that creation and evaluates `__shodhPick.start(nonce)` in the page. The pick script is part of `GUARD_SCRIPT`, so it is installed before the page's own scripts.
- The script outlines the element under the pointer. A click (or Enter) is swallowed, so the page's own handler does not run. The script then builds a description: tag, id, classes, a CSS selector path (at most 8 steps), visible text (at most 200 characters), the `outerHTML` start (at most 500 characters), and the box. It returns it by navigating to `shodh-pick://<token>/<nonce>?d=<urlencoded json>`.
- `on_navigation` refuses that navigation and passes the URL to Rust. Rust accepts it only while pick mode is on, with that creation's nonce, once. It then re-truncates every field, rejects anything else, and sends the pick to `main` and `dock` (`preview_picked`).
- A page that forges a pick only works during pick mode, and only replaces the user's chip with a description the user sees before sending. The description is page content: it goes in the prompt fenced as untrusted data, never as instructions, as Creator's `design_check` findings already are.
- Esc or Point again cancels. Pick mode ends after one pick.
- The chip opens the floating chat with focus in its box. The message carries the pick as attached context (`attachedContext`, as image text and snippets are today).

### Flow

1. The user sends a Creator request.
2. The first build step switches to Artifacts (existing `offer`), on the creation's preview pane. That pane already opens today (`ArtifactsView.tsx:88`). What changes is that the page shows in it at once: today `showPreview` is false and the pane shows only an "Open" card.
3. The page appears live, and the chat floats in the corner, expanded (existing `openDock`).
4. The user types or points, then sends. The page keeps reloading as the agent writes. **Pop out** moves it to the separate window, and the pane then says where it is.

## Security changes to the ACL gate (replacing the "window of its own" rule)

- The preview may be a child webview only through the shared lockdown builder. The test asserts each lock (`incognito`, `data_directory`, `additional_browser_args`, `on_navigation`, `initialization_script_for_all_frames`, permission and crash handlers) is present in that builder, and that no other code builds a `preview-*` webview.
- No capability names `preview-*`, as today.
- `dock` has exactly the `dock-window` capability. Its granted commands are exactly the ones its modules name. Those are the forwarding command and read-only commands that `main-window` also grants. The test lists the forwarding command by name: it is dock-only and only relays to `main`, which acts on what it receives.
- Rust bounds-clamps every rect it is sent, and the preview is hidden by default.

## Decisions made in review (my calls, open to correction)

- **Depends on PR #191** (the dock always floats) being merged first. This work builds on that dock.
- **Main window structure.** Spike first: add the child webviews to the existing `main` window (`tauri.conf.json` stays `["main"]`). Move to a `Window` holding several webviews only if the spike shows the child API misbehaves on a config `WebviewWindow`. That move touches startup, the tray and the macOS launch test, so it would come back to the user as a size change.
- **Pop out** closes the embedded page and opens today's window. The page reloads and loses its in-page state, which is acceptable for a page being built. A creation stays popped out until the user presses "Show here" in the pane. Only one preview shows at a time, as today (`SHOWN`).
- **Pick channel spike before phase 2.** On Windows, custom schemes are rewritten to `http://<scheme>.localhost` (`preview_window.rs:714`). A navigation to a new scheme may be treated as an external launch and never reach `on_navigation`. The spike checks whether a navigation within the preview's own origin to a reserved path (`/__shodh_pick/<nonce>?d=…`) reaches `on_navigation` on both platforms and can be refused there. Phase 2 is planned from what the spike shows.

## Phases (each its own PR, each usable)

1. **Embedded live preview with the floating-chat layer, and voice.** Covers goals 1, 2 (typing and dictating), 4 and 5 and the ACL changes. Without the chat layer, the chat would be covered, so they ship together.
   - The floating chat over the preview does: the last exchange and live transcript, typing, voice, send, steer, stop, approve or deny, minimise, and Open in Ask.
   - **Voice:** the dock's microphone button asks `main` to start or stop dictation. `main` holds the microphone (as today) and streams the transcript into the shared draft. The layer next to the generated page never gets microphone access.
   - What phase 3 adds (image paste, snippets, the setup card's buttons, file scope and queued-question chips) shows as "Open in Ask" until then.
2. **Pointing** (goal 3).
3. **Parity with the in-page dock** (the user asked for the gaps to be closed). Each item runs in `main` and is driven from the dock. The dock webview gains no device access and no new commands:
   - **Image paste:** the dock reads the pasted image, checks it against the same size and type limits as Ask, and hands it to `main`. `main` runs the existing image-text attachment.
   - **Snippets:** a drag between webviews isn't possible. Snippets get "Add to chat", which inserts into the shared draft, and that already works for the in-page dock.
   - **Setup card:** shown from the snapshot. Its buttons are actions forwarded to `main`.
   - **File scope and queued questions:** the chips are shown from the snapshot. Removing a chip is a forwarded action. The file check runs in `main` before sending, as today.
   - **Draft:** one draft, owned by `main`. The dock sends edits, and `main` echoes the draft back in the snapshot, so "Open in Ask" keeps what was typed.

## Verification

- Unit tests (Node, `app/tests`): the rect clamp, the covered decision, the snapshot and action protocol (round trip, unknown actions refused), the dictation start and stop actions and the transcript reaching the shared draft, pick validation (nonce, once, truncation, rejection), and the dock entry not subscribing to agent events.
- Rust tests: the ACL gate's new rules, pick URL parsing and validation, and bounds clamping.
- CI: the macOS launch test opens a creation preview embedded and checks that the child webview exists and is hidden when Artifacts is left.
- By hand, on the user's Windows machine and the Mac:
  - The page stays live while typing.
  - Dictating from the floating chat fills its box, and the dock webview is never asked for, or given, the microphone.
  - The chat and approvals are never covered.
  - Menus, dialogs and toasts are never hidden.
  - Offline really is offline (the network tab is empty with internet off).
  - Pop out works.
  - Pointing attaches the right element.

## Risks

- **Tauri `unstable` multi-webview.** Its API or behaviour may change in a minor release. Pin Tauri, and note the dependency in `Cargo.toml`.
- **WebView2 environments.** Two environments in one window: the preview uses its own data directory and arguments, as today's window does. If WebView2 refuses differing arguments in one window, phase 1 stops and comes back to the user. The page is never run with the app's environment.
- **Transparency of the `dock` webview** on Windows and macOS (corners and shadow). The fallback is a square-cornered dock, which still works.
- **Keeping the snapshot small.** Only the active conversation's last exchange and live transcript are sent, as `ConversationDock` shows them, not the whole history.

## Left to the builder

The names of the commands, events and entry files. Throttling of rect reports. The exact toolbar layout of the preview pane, following the existing Artifacts page style.
