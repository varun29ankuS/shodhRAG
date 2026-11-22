# Live preview, phase 1 (embedded preview, floating chat layer, voice) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When Creator builds a page, Artifacts shows it live inside the main window, with the chat floating over it in the bottom-right corner. The user can type, dictate, send, steer, stop and approve there, and the page is never able to reach the app.

**Architecture:** The main window gets two child webviews.
- `preview-<id>` is built by the same lockdown code as today's preview window and is placed over a placeholder that the main webview reports.
- `dock` loads the app's `/dock-view` route. It renders the dock's panel from a snapshot that `main` publishes, and it forwards every user action back to `main`.
- `main` stays the only chat session, the only microphone holder and the only listener for agent events.
- Rust clamps every rectangle, hides both layers by default, and relays dock traffic to one webview label only.

**Tech Stack:** Tauri 2.12.5 with the `unstable` feature (`Window::add_child`, `WebviewBuilder`), WebView2 (Windows) and WKWebView (macOS), React and TypeScript. Tests are Rust unit tests (`cargo test --package shodh-rag-app`, run by CI's "App tests and doc tests" job) and Node strip-types tests (`node --experimental-strip-types --test app/tests/<name>.test.ts`, run by CI's "Frontend Typecheck + Tests" job).

**Spec:** `docs/superpowers/specs/2026-11-21-live-preview-in-artifacts-design.md` (phase 1, with voice moved into it on 2026-11-22).

## Global Constraints

- No local builds. `cargo build/test/check`, `npm install` and running the app happen in CI only. Each "Run" step below names the CI job whose log shows the result. `rustfmt` on changed files is allowed locally.
- Rust only for new tooling: no new Python, shell or other-language scripts. Workflow YAML steps may use the shells the workflows already use (bash, pwsh).
- Tauri stays at exactly `2.12.5`: `tauri = { version = "=2.12.5", features = ["tray-icon", "image-png", "unstable"] }`, with a comment that the multi-webview API is unstable and must be re-checked on every Tauri bump.
- The preview's locks hold in both places it is shown: `incognito(true)`, its own `data_directory` per switch state (`preview_window::data_folder`), `additional_browser_args(&browser_args(process_browser_args(), network))` (dead proxy `--proxy-server=127.0.0.1:9` while offline), `initialization_script_for_all_frames(preview::GUARD_SCRIPT)`, `on_navigation` limited to `navigation_allowed`, no new windows, no downloads, WebView2 permission denial and crash reporting, and devtools in debug builds only.
- No capability names `preview-*`. `dock` has exactly the `dock-window` capability.
- Both child webviews are hidden by default. Each is shown only while the latest report from `main` says the Artifacts preview pane is on screen and nothing of `main`'s own is above it.
- Mutex locks use `.unwrap_or_else(|e| e.into_inner())`, never a bare `.unwrap()` outside tests.
- No TODOs, placeholders, mocks in production code, or stubs. Commits have plain messages with no generated footers. Every change goes through the PR workflow.
- Agent-first: every command the UI gains has an agent tool or a stated exclusion in `app/src-tauri/agent-coverage.json`, added in the same task that registers the command (`agent_coverage.rs` fails otherwise).

## Review Focus

1. **A main dialog, menu or toast opens while the preview is embedded.** Both layers (preview and dock) must hide. The `dock` webview is above `main` too, so it would cover the bottom-right of a dialog. While they are hidden, `main` shows its in-page dock. The test is in Task 5 (`coveredBy`) and Task 8 (`layerVisible`).
2. **The text-size setting zooms `main`** (`app_settings::apply_text_size` calls `set_zoom`). With zoom `z`, a CSS-pixel rectangle from `getBoundingClientRect` is `z` times larger in logical pixels. A rectangle sent unscaled misplaces the page at every size other than 100%. The test is in Task 3 (`to_logical`).
3. **`main` reloads or crashes while the preview shows.** Nothing will send `covered: true` again. Rust must hide both layers when `main` starts a page load, and must never leave them on top. The test is in Task 3 (`EmbedState::main_reloaded`).
4. **An event that `main` should get alone also reaches `dock` or `preview-*`.** Every `emit_to` that names `MAIN_WINDOW` must target the `main` webview (`EventTarget::Webview { label: "main" }`), and nothing must use a window-wide target. The test is in Task 4 (a mock-runtime test listens on `dock` and asserts that it receives nothing).
5. **Dictation while focus is in the dock.** The transcript must land in `main`'s draft and echo back to the dock. Typing activity from dock edits must reach `typingActivity` in `main`, so the assistant's navigation waits as it does for in-page typing. The test is in Task 7 (`applyDockAction`).

---

## File Structure

| File | Responsibility |
|---|---|
| `app/src-tauri/src/creations/preview_window.rs` (modify) | Today's window. It gains `locked_webview` (the child-webview form of the same locks, through one macro) and `lock_platform` (the WebView2 handlers for any `Webview`). |
| `app/src-tauri/src/creations/preview_embed.rs` (create) | The embedded preview: `EmbedState`, rect clamping and zoom, show, hide and close, pop-out and show-here. |
| `app/src-tauri/src/dock_layer.rs` (create) | The `dock` child webview: building it in `main`'s environment, its bounds, and the snapshot and action relay. |
| `app/src-tauri/src/profile.rs` (modify) | `child_webview_defaults`, the `WebviewBuilder` twin of `webview_defaults`. |
| `app/src-tauri/src/creation_commands.rs` (modify) | `creation_preview_embed`, `creation_preview_place`, `creation_preview_pop_out`. |
| `app/src-tauri/src/acl_manifest.rs` (modify) | New rules that replace "window of its own"; the `dock-window` checks. |
| `app/src-tauri/permissions/dock-window.json`, `app/src-tauri/capabilities/dock.json` (create) | The dock's grant. |
| `app/src-tauri/agent-coverage.json` (modify) | Entries for every new command. |
| `app/src/lib/dockProtocol.ts` (create) | Pure module: `DockSnapshot`, `DockAction`, and parsing and size limits. |
| `app/src/lib/previewPlacement.ts` (create) | Pure module: `coveredBy`, `layerVisible`, overlay selectors. |
| `app/src/components/shell/DockPanel.tsx` (create) | The pill and the floating panel's markup, driven by a `DockModel`. It is split out of `ConversationDock`. |
| `app/src/components/shell/ConversationDock.tsx` (modify) | Builds a `DockModel` from the local hooks (behaviour unchanged) and renders `DockPanel`. |
| `app/src/features/dock/DockView.tsx` (create) | The `/dock-view` route: `DockModel` from the snapshot, actions to Rust. |
| `app/src/features/dock/useDockBridge.ts` (create) | Runs in `main`: publishes the snapshot and applies actions to the real session, draft and dictation. |
| `app/src/features/artifacts/EmbeddedPreview.tsx` (create) | The placeholder, rect and covered reports, and the Pop out and Show here toolbar. |
| `app/src/features/artifacts/ArtifactPage.tsx`, `ArtifactsView.tsx`, `api.ts`, `model.ts` (modify) | Use the embedded preview and show the page as soon as a build starts. |
| `app/src/features/agent/AgentComposer.tsx` (modify) | A `voice` prop so a compact composer can show the mic driven from outside. |
| `app/src/main.tsx` (modify) | The `/dock-view` route. |
| `.github/workflows/release.yml` (modify) | The Windows launch test, and the embedded-preview probe in both launch tests. |

**Decision recorded here (spec, "Left to the builder"):** the dock's entry is the `/dock-view` route of the existing bundle, as `/print-view` is, not a separate `dock.html`. That keeps one Vite entry and lets `acl_manifest.rs` compute the dock's module graph from roots, as it does for print.

**Deviation from the spec:** the spec says "`ConversationDock` stays unchanged, and only its provider differs". That cannot hold. `ConversationDock` reads webview-local stores besides `useChatSession` (`useDraft`/`composerDrafts`, `useImageAttachments`, `useChatFileScope`, `useQueuedQuestion`, `useSendOptions`), and the `dock` webview's JavaScript realm has none of `main`'s state. Task 6 therefore splits the markup into `DockPanel` and keeps `ConversationDock` as the in-page model builder. That is the smallest change that keeps a single panel component for both surfaces.

---

### Task 1: Spike (throwaway). Prove child webviews in `main`, two WebView2 environments, z-order and the dock capability

This task is **not merged**. It runs on branch `spike/preview-child-webviews` from `origin/master` as a draft PR titled "Spike (do not merge): child webviews in the main window". CI's `test` job is expected to fail on that branch, because the ACL gate forbids `.add_child(` and unlabelled webviews until Task 4. That failure is not a spike result.

**Files (spike branch only):**
- Modify: `app/src-tauri/Cargo.toml` (tauri `unstable`; `windows` feature `Win32_UI_WindowsAndMessaging` for the hit test)
- Modify: `app/src-tauri/src/creations/preview_window.rs` (`locked!` macro, `locked_webview`, `lock_platform`)
- Modify: `app/src-tauri/src/profile.rs` (`folder_id` to `pub(crate)`)
- Create: `app/src-tauri/src/spike_child_webviews.rs`
- Modify: `app/src-tauri/src/startup.rs` (`startup_ready` takes `AppHandle` and calls `spike_child_webviews::start`)
- Modify: `app/src-tauri/src/lib.rs` (module and the `spike_dock_report` command)
- Create: `app/src-tauri/permissions/dock-window.json` (allows `spike_dock_report`), `app/src-tauri/capabilities/dock.json` (`webviews: ["dock"]`, `local: true`, `["dock-window", "core:event:allow-listen", "core:event:allow-unlisten"]`)
- Modify: `app/src/main.tsx` (the `/dock-view` route renders `SpikeDockView`)
- Create: `app/src/features/dock/SpikeDockView.tsx`
- Modify: `.github/workflows/release.yml` (macOS launch passes the env var and greps; new `launch-windows` job)

**What runs, when `SHODH_SPIKE_CHILD_WEBVIEWS=1`.** It starts once, from `startup_ready`, so `main`'s environment is already running as it is in the real flow.
1. Make a real creation in the profile's data folder (`CreationStore::prepare`). Write a solid red `index.html` into its folder, grant it through `PREVIEWS.grant`, and build `preview_window::locked_webview(app, &opening)` with `network: false`. Add it with `main_window.add_child(builder, LogicalPosition(40, 120), LogicalSize(640, 420))`. On Windows, run `lock_platform` on it.
2. Build the `dock` child: `WebviewBuilder::new("dock", WebviewUrl::App("dock-view".into()))` with `process_browser_args()`, the profile's webview data folder and store id when separate, and `transparent(true)`. Add it at `LogicalPosition(480, 400)` with `LogicalSize(320, 240)`, overlapping the preview's bottom-right corner.
3. The dock page (`SpikeDockView`) paints solid green. Its left 40 px column is transparent. It calls `invoke('creation_preview_status')`, which is main-only and must be refused, and `invoke('plugin:path|resolve_directory', { directory: 11 })`, which main has and the dock must not. It then reports both outcomes through `spike_dock_report({ mainOnly, plugin })`.
4. Rust logs one line per fact, each prefixed `SPIKE`:
   - `SPIKE preview child added` and `SPIKE preview child loaded` (from `on_page_load` Finished)
   - `SPIKE dock child added` and `SPIKE dock report main-only=refused plugin=refused`
   - Windows only: `SPIKE browser pids main=<a> preview=<b> dock=<c>` (from `ICoreWebView2::BrowserProcessId`). `b != a` proves a second environment. `c == a` proves the dock shares `main`'s.
   - Windows only: `SPIKE hit-test overlap=dock preview-only=preview elsewhere=main`. This comes from `ChildWindowFromPointEx(main_hwnd, point, CWP_SKIPINVISIBLE)`, compared with each webview controller's `ParentWindow` container HWND, at the overlap centre, at a preview-only point and at a point outside both.
   - `SPIKE screen points overlap=(x,y) preview=(x,y) transparent=(x,y)`, the physical screen coordinates used for the screenshot check.
   - Then `SPIKE PASS`, or `SPIKE FAIL <what>` on the first fact that does not hold. Every `add_child`, `with_webview` and COM error is reported as its own `SPIKE FAIL` line.

**CI evidence:**
- **macOS:** `launch-macos` passes `--env SHODH_SPIKE_CHILD_WEBVIEWS=1` to `open`. After ready, it waits up to 30 s for `SPIKE PASS` or `SPIKE FAIL` in the log, prints every `SPIKE` line, fails on `SPIKE FAIL`, and uploads a `screencapture -x` image as an artifact. That image is a human check, not a gate.
- **Windows:** a new `launch-windows` job (`needs: [plan, build]`, `runs-on: windows-latest`, 20 minutes) does the following:
  - downloads `shodh-windows-x64` and installs it with `/S`;
  - starts `Shodh.exe` with `SHODH_SPIKE_CHILD_WEBVIEWS=1` and waits up to 120 s for `Main window ready` in `%APPDATA%\com.shodh.rag-app\logs\shodh.log`, then up to 30 s for `SPIKE PASS` or `SPIKE FAIL`;
  - prints the `SPIKE` lines, then reads the screen pixels at the logged points with `System.Drawing` (`CopyFromScreen`). It expects green at the overlap, red at the preview-only point, and records the transparent point's colour without gating on it;
  - uploads the screenshot, then stops the process.
- **Watch the fast jobs first.** CI's `check` job (`cargo check --workspace --locked`) shows a compile error long before the 50 to 130 minute release build. Fix it before the build is spent.

- [ ] **Step 1:** Create the worktree `C:/wt/preview-spike` on `spike/preview-child-webviews` from `origin/master`.
- [ ] **Step 2:** Make the changes listed above, with `rustfmt` on the changed Rust files.
- [ ] **Step 3:** Commit, push, and open the draft PR. Its body states what it proves, that it must not be merged, and that the `test` job's ACL failure is expected.
- [ ] **Step 4:** Wait for `Check + Clippy`. On a compile error, read the job log and fix it (a fix attempt).
- [ ] **Step 5:** Wait for `Release builds / Launch test (macOS)` and `Release builds / Launch test (Windows)`. Expected: both print `SPIKE PASS`, and the Windows pixel check prints `overlap=green preview=red`.
- [ ] **Step 6:** If the macOS release build runs about 2 hours with no output (a known CI problem on PRs), record that as no result rather than a failure.

**Spike gate:** at most three fix attempts on the spike branch. If either launch test prints `SPIKE FAIL`, or the Windows pixel check fails, stop and bring the log lines and options back to the user. Do not start Task 2. The likely failures and their options:
- WebView2 refuses the second environment in one window. The options are a separate borderless owned window tracking the pane, or giving up the embedded page on Windows.
- The dock paints under the preview. The option is re-ordering with `SetWindowPos(HWND_TOP)` or `reparent`.
- `add_child` misbehaves on a config `WebviewWindow`. The option is the spec's `Window` with several webviews, which is a size change for the user.

---

### Task 2: One lockdown for the preview window and the embedded preview

**Files:**
- Modify: `app/src-tauri/Cargo.toml:37` (tauri `=2.12.5` with `unstable`, plus the comment)
- Modify: `app/src-tauri/src/creations/preview_window.rs:317-408` (`locked!` macro; `locked_builder` and `locked_webview` both use it; `lock_platform`)
- Modify: `app/src-tauri/src/profile.rs` (`child_webview_defaults`; `folder_id` to `pub(crate)`)
- Test: `app/src-tauri/src/creations/preview_window.rs` (tests module)

**Interfaces:**
- Produces: `pub(crate) fn locked_webview<R: Runtime>(app: &AppHandle<R>, opening: &Opening<'_>) -> tauri::webview::WebviewBuilder<R>`, which is hidden by default via `.focused(false)`. The caller adds it and sets bounds.
- Produces: `#[cfg(windows)] pub(crate) fn lock_platform<R: Runtime>(app: &AppHandle<R>, label: &str, webview: &tauri::Webview<R>) -> Result<(), String>` (permission denial and `ProcessFailed` reporting through `tell_main`). `open` uses it too, through `window.as_ref()` (the `Webview` of the `WebviewWindow`).
- Produces: `pub fn child_webview_defaults<R: Runtime>(builder: WebviewBuilder<R>) -> WebviewBuilder<R>` in `profile.rs`, the same args, data dir and store id as `webview_defaults`.
- The locks are written once, inside `macro_rules! locked { ($builder:expr, $app:expr, $opening:expr) => { ... } }`. Both builder types have the same method names, and the `on_page_load` and `on_download` closure arguments differ only in type.

- [ ] **Step 1: Write the failing test** `the_locked_child_builds_on_its_own_address`. It uses the mock app (`tauri::test::mock_builder`, as in `the_locked_window_builds_on_its_own_address`). It builds a plain `tauri::window::WindowBuilder::new(&app, "host")`, calls `add_child(locked_webview(app.handle(), &opening), LogicalPosition::new(0., 0.), LogicalSize::new(100., 100.))`, and asserts `webview.label() == label` and `webview.url() == address`.
- [ ] **Step 2: Write the failing test** `child_webviews_get_the_main_windows_environment` in `profile.rs`. It asserts that `child_webview_defaults` on the normal profile adds `process_browser_args()` and no data dir. Expose `WebviewBuilder`'s attributes through `tauri::webview::WebviewBuilder::new(..)` plus a `#[cfg(test)]` helper that returns the args and data dir the profile chose, as pure values: `fn child_defaults_for(profile: &Profile) -> (String, Option<PathBuf>)`, which `child_webview_defaults` calls.
- [ ] **Step 3: Implement** the macro, `locked_webview`, `lock_platform` and `child_webview_defaults`, and pin Tauri.
- [ ] **Step 4: Run** CI "App tests and doc tests". Expected: both new tests pass, and every existing `preview_window` test still passes. The ACL test is still red only if a lock string moved. Update the lock list in `acl_manifest.rs` in this task to point at the macro's text, which keeps the same strings.
- [ ] **Step 5: Commit** with the message "Preview: one lockdown for the preview window and an embedded preview".

---

### Task 3: The embedded preview in Rust (place, clamp, hide by default, pop out)

**Files:**
- Create: `app/src-tauri/src/creations/preview_embed.rs`
- Modify: `app/src-tauri/src/creations/mod.rs` (`pub mod preview_embed;`)
- Modify: `app/src-tauri/src/creations/preview_window.rs` (`Shown` gains `place: Place`, where `Place::{Window, Embedded}`; `close` also closes an embedded one; `on_window_event` hides the embedded preview when `main` is minimised)
- Modify: `app/src-tauri/src/creation_commands.rs` (three commands), `app/src-tauri/src/lib.rs` (register them; `on_page_load` of `main` calls `preview_embed::main_reloaded`)
- Modify: `app/src-tauri/permissions/main-window.json`, `app/src-tauri/agent-coverage.json`
- Test: `app/src-tauri/src/creations/preview_embed.rs` (tests module)

**Interfaces:**
- Consumes: `locked_webview`, `lock_platform` (Task 2).
- Produces:
  - `pub struct CssRect { pub x: f64, pub y: f64, pub width: f64, pub height: f64 }` (serde camelCase).
  - `pub struct Placement { pub rect: Option<CssRect>, pub covered: bool }`.
  - `pub fn to_logical(rect: CssRect, zoom: f64) -> CssRect`.
  - `pub fn clamp(rect: CssRect, window: (f64, f64), top_reserve: f64) -> Option<CssRect>`, where `None` means hide. It rejects NaN, infinite or negative sizes and anything under 40 × 40 after clamping, and keeps the rectangle inside `[0, width] × [top_reserve, height]`.
  - `pub struct EmbedState` with these methods, each returning a `Visibility::{Shown(CssRect), Hidden}`:
    - `place(&mut self, label: &str, placement: Placement)` (a stale label is ignored);
    - `main_reloaded(&mut self)`;
    - `minimised(&mut self, yes: bool)`.
  - The commands, all granted to `main-window` only:
    - `creation_preview_embed(app, id: String, version: Option<u32>, network: bool, dark: bool) -> CreationResult<PreviewInfo>`, which creates the child hidden;
    - `creation_preview_place(app, label: String, placement: Placement) -> CreationResult<()>`, which multiplies by the current text-size zoom and clamps;
    - `creation_preview_pop_out(app, label: String) -> CreationResult<PreviewInfo>`, which closes the embedded child and opens today's window with the same grant inputs. The pane's "Show here" calls `creation_preview_embed` again.
  - Event `PREVIEW_EVENT` keeps its four states, and `PreviewEvent` gains `place: "window" | "embedded"`.

- [ ] **Step 1: Write the failing tests:**
  - `a_rect_is_scaled_by_the_text_zoom`: `to_logical({10,20,300,200}, 1.25) == {12.5,25,375,250}`.
  - `a_rect_is_clamped_inside_the_window_below_the_title_bar`: `clamp({-50,0,5000,5000}, (1400.,900.), 40.) == Some({0,40,1400,860})`.
  - `a_bad_rect_hides_the_page`: NaN, infinite, a negative width, or 30 × 30 each give `None`.
  - `hidden_until_main_says_visible`: a fresh `EmbedState` after embed is `Hidden`; `place(label, {rect, covered:false})` is `Shown`; `covered:true` is `Hidden`; `rect: None` is `Hidden`.
  - `a_reload_of_main_hides_the_page`: `Shown`, then `main_reloaded()`, is `Hidden` until the next `place`.
  - `minimising_hides_and_restoring_waits_for_main`: after `minimised(false)` the state stays `Hidden` until a new `place`.
  - `a_stale_label_is_ignored`.
- [ ] **Step 2: Run** CI "App tests". Expected: FAIL (`preview_embed` does not exist).
- [ ] **Step 3: Implement** `preview_embed.rs`, the commands, the grants in `main-window.json`, and the `agent-coverage.json` entries:
  - `creation_preview_embed` maps to `{ "tool": "open_artifact" }`;
  - `creation_preview_place` is `{ "excluded": "Geometry the Artifacts pane reports while it is on screen; the agent shows a preview with open_artifact and never places one." }`;
  - `creation_preview_pop_out` is `{ "excluded": "Moving the page to a separate window is the user's screen layout; open_artifact shows the preview in Artifacts." }`.
- [ ] **Step 4: Run** CI "App tests". Expected: PASS, including `agent_coverage` and `every_app_command_is_granted_to_exactly_the_windows_that_call_it`.
- [ ] **Step 5: Commit** with the message "Preview: show it inside Artifacts, placed by the pane and hidden by default".

---

### Task 4: The `dock` layer in Rust and the ACL gate's new rules

**Files:**
- Create: `app/src-tauri/src/dock_layer.rs`
- Create: `app/src-tauri/permissions/dock-window.json`, `app/src-tauri/capabilities/dock.json`
- Modify: `app/src-tauri/src/lib.rs` (module, commands), `app/src-tauri/permissions/main-window.json`, `app/src-tauri/agent-coverage.json`
- Modify: `app/src-tauri/src/acl_manifest.rs:798-1000` and its runtime test at `1149`
- Modify: every `emit_to` that targets `MAIN_WINDOW` (`preview_window::tell_main` and the results of `grep -rn "EventTarget::WebviewWindow\|emit_to(MAIN_WINDOW\|emit_to(\"main\"" app/src-tauri/src`), so each one targets `EventTarget::Webview { label: MAIN_WINDOW }`
- Test: `app/src-tauri/src/dock_layer.rs`, `app/src-tauri/src/acl_manifest.rs`

**Interfaces:**
- Consumes: `child_webview_defaults` (Task 2), `EmbedState` visibility (Task 3). The dock is shown exactly when the embedded preview is `Shown`.
- Produces:
  - `pub const DOCK_WEBVIEW: &str = "dock";`
  - `pub const DOCK_SNAPSHOT_EVENT: &str = "dock-snapshot";` (to `dock` only)
  - `pub const DOCK_ACTION_EVENT: &str = "dock-action";` (to `main` only)
  - `pub const MAX_RELAY_BYTES: usize = 262_144;`
  - `pub fn dock_bounds(panel: (f64, f64), window: (f64, f64), edge: f64) -> CssRect`, bottom-right with a 16 px edge, clamped.
  - `dock_layer_publish(webview: tauri::Webview, app, snapshot: serde_json::Value) -> Result<(), String>`. It is granted to `main-window`. It refuses a caller other than `main` and anything over `MAX_RELAY_BYTES`, and emits to `dock`.
  - `dock_layer_action(webview: tauri::Webview, app, action: serde_json::Value) -> Result<(), String>`. It is granted to `dock-window`. It refuses a caller other than `dock` and anything over the limit, and emits to `main`. It does not interpret the action; `main` validates it (Task 5).
  - `dock_layer_size(webview: tauri::Webview, app, width: f64, height: f64) -> Result<(), String>`. It is granted to `dock-window` and sets the dock's bounds through `dock_bounds`.
  - `fn build(app) -> Result<Webview, String>` creates `dock` hidden, in `main`'s environment, with `transparent(true)` and `devtools(cfg!(debug_assertions))`. It denies every permission on both platforms with `.on_permission_request(|_, _| PermissionResponse::Deny)`, a function named `deny_permissions` in 2.12.5 (WebView2's `PermissionRequested`, and WKUIDelegate's `requestMediaCapturePermission`). The dock shares `main`'s profile and origin, so a microphone grant that `main` saved could otherwise carry over to it. Tauri documents that a saved choice skips the handler, so the dock's module graph must also never name `getUserMedia`, `SpeechRecognition` or `webkitSpeechRecognition`. Task 6's test asserts that.
  - The ACL rules:
    - `DOCK_ROOTS = ["features/dock/DockView.tsx", "contexts/ThemeContext.tsx", "components/ErrorBoundary.tsx"]`.
    - `dock-window` allows exactly the commands named in the dock-only modules, plus `dock_layer_action` and `dock_layer_size`. `DOCK_ONLY_COMMANDS = ["dock_layer_action", "dock_layer_size"]` may appear in no other permission.
    - `capabilities_name_their_windows…` accepts `dock` as a third webview label.
    - The preview rules become:
      - `preview_window.rs` may use `.add_child(` and `WebviewBuilder::new(` only inside `locked!` and `preview_embed.rs`;
      - every lock string is present in the `locked!` macro body;
      - no file other than `preview_window.rs`, `preview_embed.rs`, `dock_layer.rs`, `pdf_export.rs` and `lib.rs` matches `opens_windows`;
      - `dock_layer.rs` builds exactly one webview, labelled `DOCK_WEBVIEW`;
      - `dock_layer.rs` contains `deny_permissions`.

- [ ] **Step 1: Write the failing tests:**
  - `the_dock_sits_bottom_right_inside_the_window`: `dock_bounds((380.,470.), (1400.,900.), 16.) == {1004,414,380,470}`. A panel taller than the window clamps to `height - 2*edge`.
  - `relay_refuses_the_wrong_caller_and_big_payloads`: a pure `fn relay_allowed(caller: &str, expected: &str, bytes: usize) -> Result<(), String>`.
  - `main_events_reach_main_alone` (mock runtime): build `main` plus a `dock` child. Listen for `PREVIEW_EVENT` and `DOCK_ACTION_EVENT` on `dock` and assert that it got none after `tell_main` and `dock_layer_action`.
  - The ACL updates: `the_compiled_acl_refuses_every_other_window_and_origin` gains a `dock` webview that may call `dock_layer_action` and is refused `creation_preview_status`, `dock_layer_publish` and `plugin:path|resolve_directory`. The `main` webview is refused `dock_layer_action`.
- [ ] **Step 2: Run** CI "App tests". Expected: FAIL.
- [ ] **Step 3: Implement** the module, the permission and capability files, the ACL rules and the event-target fixes. Add these `agent-coverage.json` exclusions:
  - `dock_layer_publish`: "Mirrors the chat into the floating layer over the preview; the agent's own actions already show in that chat."
  - `dock_layer_action`: "The floating layer's relay of the user's own clicks and keys to the main window; never the agent's."
  - `dock_layer_size`: "The floating layer's size follows its pill or panel; layout, not an action."
- [ ] **Step 4: Run** CI "App tests". Expected: PASS.
- [ ] **Step 5: Commit** with the message "Dock layer: the floating chat's webview over the preview, with its own capability".

---

### Task 5: Snapshot, action and placement rules (pure TypeScript)

**Files:**
- Create: `app/src/lib/dockProtocol.ts`, `app/src/lib/previewPlacement.ts`
- Test: `app/tests/dockProtocol.test.ts`, `app/tests/previewPlacement.test.ts`

**Interfaces:**
- Produces in `dockProtocol.ts`:
  - `interface DockSnapshot { v: 1; conversationId: string | null; title: string; lastUser: { content: string } | null; transcript: TranscriptState | null; isStreaming: boolean; busyElsewhere: boolean; sideRunLabel: string | null; readOnlyConnector: string | null; draft: string; dictating: boolean; voiceSupported: boolean; open: boolean; unread: number; theme: 'light' | 'dark'; zoom: number }`. Only the active conversation's last exchange, as `ConversationDock` shows it.
  - `type DockAction = { kind: 'send' } | { kind: 'steer' } | { kind: 'cancel' } | { kind: 'approve'; id: string; allow: boolean } | { kind: 'draft'; text: string } | { kind: 'dictate'; on: boolean } | { kind: 'open'; open: boolean } | { kind: 'openInAsk' } | { kind: 'close' }`.
  - `parseDockAction(value: unknown): DockAction | null` refuses unknown kinds and extra fields, `draft` longer than 20 000 characters, and an `approve` id that is not 1 to 128 printable characters.
  - `parseDockSnapshot(value: unknown): DockSnapshot | null`.
  - `snapshotOf(input: SnapshotInput): DockSnapshot`, where `SnapshotInput` is the session fields `ConversationDock` reads today.
  - `MAX_SNAPSHOT_BYTES = 262144`, the same as Rust's `MAX_RELAY_BYTES`.
- Produces in `previewPlacement.ts`:
  - `interface Box { x: number; y: number; width: number; height: number }`
  - `const OVERLAY_SELECTORS: readonly string[]` (`[role="dialog"][aria-modal="true"]`, `[role="menu"]`, `[role="listbox"]`, `[data-radix-popper-content-wrapper]`, `[data-sonner-toast]`, `[cmdk-root]`), which excludes the in-page dock (`#dock-composer`'s panel).
  - `coveredBy(pane: Box, overlays: readonly Box[], modalOpen: boolean): boolean`: any open modal, or any overlay intersecting the pane or the dock's corner box.
  - `layerVisible(state: { onArtifacts: boolean; paneShown: boolean; poppedOut: boolean; covered: boolean; windowVisible: boolean }): boolean`.

- [ ] **Step 1: Write the failing tests:**
  - `dockProtocol.test.ts`:
    - `every action round-trips` (each kind through `JSON.parse(JSON.stringify(a))` and `parseDockAction` deep-equals `a`);
    - `unknown or malformed actions are refused` (`{kind:'eval'}`, `{kind:'send', extra:1}`, `{kind:'draft', text: 'x'.repeat(20001)}`, `{kind:'approve', id:'', allow:true}`, `null`, `'send'`);
    - `the snapshot carries only the last exchange` (a three-exchange session gives `lastUser.content` of the third, and `JSON.stringify(snapshot).length` stays under `MAX_SNAPSHOT_BYTES` for a 200-step transcript fixture from `app/tests/fixtures/transcripts`);
    - `dictation state travels in the snapshot`.
  - `previewPlacement.test.ts`:
    - `a menu over the pane covers it`;
    - `a toast away from the pane and the dock corner does not`;
    - `any modal dialog covers it wherever it is`;
    - `hidden unless on Artifacts, pane shown, not popped out, not covered and the window visible` (each flag flipped alone gives `false`).
- [ ] **Step 2: Run** `node --experimental-strip-types --test app/tests/dockProtocol.test.ts app/tests/previewPlacement.test.ts`. Expected: FAIL (modules missing).
- [ ] **Step 3: Implement** both modules.
- [ ] **Step 4: Run** the same command. Expected: PASS. CI "Frontend Typecheck + Tests" is green.
- [ ] **Step 5: Commit** with the message "Dock layer: the snapshot and action protocol and when the layers show".

---

### Task 6: `DockPanel` split, `/dock-view` route, and the mic in a compact composer

**Task 6 revised 2026-11-22: shared transcript through a host interface, user decision (b).** The in-page chat's `Transcript` imported main-only providers (`MessageContentRenderer → FocusFrame → FocusContext → ChatSessionContext`, `RuntimeCard → RuntimeSetupContext → useAgentSession`, which names `AGENT_EVENT`). The dock may load neither, and `useRuntimeSetup` threw without its provider. The user chose to untangle them and keep one transcript for both surfaces. The task is two commits:
- **6a.** The transcript tree reads focus, the answer engine's setup and code review through host modules (`focus/focusHost.tsx`, `setup/runtimeSetupHost.ts`, `code/codeReviewHost.ts`), which main's providers fill unchanged. The agent API and types move out of `useAgentSession.ts` (now the event stream only) into `agent/agentApi.ts`. `SettingsSection` moves to `memory/navigation.ts`. `AgentComposer` takes dictation as a `voice` prop. `tests/transcriptBoundary.test.ts` pins the boundary, and Ask behaves exactly as before.
- **6b.** The dock renders the same `Transcript` through `DockPanel`. It provides these hosts from its snapshot and forwards their buttons to main as `DockAction`s. When `fitTranscript` drops older blocks, the dock says so, with a control that opens them in Ask.

**Files:**
- Create: `app/src/components/shell/DockPanel.tsx`, `app/src/features/dock/DockView.tsx`
- Modify: `app/src/components/shell/ConversationDock.tsx` (builds a `DockModel` and renders `DockPanel`), `app/src/features/agent/AgentComposer.tsx`, `app/src/main.tsx`
- Test: `app/tests/dockView.test.ts`, and the existing `app/tests/dockMode.test.ts` still passes

**Interfaces:**
- Consumes: `DockSnapshot`, `DockAction`, `parseDockSnapshot` (Task 5); the Rust commands and events `dock_layer_action`, `dock_layer_size`, `dock-snapshot` (Task 4).
- Produces:
  - `interface DockModel { snapshot: DockSnapshot; onAction(a: DockAction): void; composerExtras?: React.ReactNode; setupCard?: React.ReactNode }`. The in-page dock passes its attachment, file chips and `SetupCard` as `composerExtras` and `setupCard`; the remote dock passes none, which is phase 3's gap.
  - `DockPanel(props: { model: DockModel; onExpandAsk(): void; fixed: boolean })`.
  - `AgentComposer` no longer imports `useVoiceInput`. It takes `voice?: { supported: boolean; listening: boolean; toggle(): void }` and shows the mic whenever `voice?.supported`, compact or not. Its callers that dictate today (Ask's composer, via `grep -rn "<AgentComposer" app/src`) call `useVoiceInput(getText, setText)` themselves and pass the result, so their behaviour is unchanged. The in-page dock passes nothing, as today. That keeps `useVoiceInput` out of the dock's module graph.
  - `main.tsx` renders `<DockView />` on `path === '/dock-view'`, inside `ThemeProvider` and `ErrorBoundary`, as print does.
  - `DockView` subscribes to `dock-snapshot` only. It reports its panel size with `dock_layer_size` from a ResizeObserver. Every control calls `dock_layer_action`. It handles Ctrl+J and Esc locally by sending `open` and `cancel` or `approve` actions. In the dock, "Open in Ask" sends `openInAsk`.

- [ ] **Step 1: Write the failing test** `app/tests/dockView.test.ts`:
  - `the dock entry never listens to agent events`: walk `DockView.tsx`'s static imports with the same regex `acl_manifest.rs` uses, and assert that no module in the graph names `agent_event` or `AGENT_EVENT`, or imports `features/ask/ChatSessionContext`;
  - `the dock entry calls only its own commands`: its graph names exactly `dock_layer_action` and `dock_layer_size`;
  - `the dock entry never touches a device`: no module in its graph names `getUserMedia`, `SpeechRecognition`, `webkitSpeechRecognition` or `useVoiceInput`. The mic button is the `voice` prop, driven by `dictating` from the snapshot;
  - `main.tsx routes /dock-view to DockView`.
- [ ] **Step 2: Run** `node --experimental-strip-types --test app/tests/dockView.test.ts`. Expected: FAIL.
- [ ] **Step 3: Implement** the split. The in-page behaviour must be identical, and the existing `dockMode.test.ts` source assertions must still pass. Update them only where they read markup that moved to `DockPanel.tsx`, keeping each assertion's meaning. Add the route and the `voice` prop.
- [ ] **Step 4: Run** both test files and CI "Frontend Typecheck + Tests" and "App tests" (`acl_manifest` now computes the `dock-window` grant from `DockView`'s graph). Expected: PASS.
- [ ] **Step 5: Commit** with the message "Dock: one panel for the in-page dock and the floating layer, with voice in compact mode".

---

### Task 7: `main` drives the layer: snapshot out, actions in, voice

**Files:**
- Create: `app/src/features/dock/useDockBridge.ts`, `app/src/features/dock/dockActions.ts`
- Modify: `app/src/App-SplitView.tsx:2017` (while the layer is active, mount the bridge instead of the in-page `ConversationDock`)
- Test: `app/tests/dockActions.test.ts`

**Interfaces:**
- Consumes: `snapshotOf`, `parseDockAction` (Task 5); `useChatSession`, `useDraft`/`composerDrafts`, `useVoiceInput`, `typingActivity`.
- Produces:
  - `applyDockAction(action: DockAction, target: DockTarget): void`, where `DockTarget = { send(): void; steer(text: string): void; cancel(): void; approve(id: string, allow: boolean): void; setDraft(text: string): void; dictation: { listening: boolean; toggle(): void }; setOpen(open: boolean): void; openInAsk(): void; close(): void; typed(): void }`.
  - `draft` calls `setDraft` and `typed()`; `typed()` is `typingActivity.touch()`.
  - `dictate` toggles only when `on !== listening`.
  - `send` runs the same submit path as the in-page dock, so it is moved from `ConversationDock` into `dockActions.ts`'s `submitDockDraft(session, draft, options)` and shared by both surfaces.
  - `useDockBridge({ active: boolean })` publishes `snapshotOf(...)` with `dock_layer_publish` at most once per animation frame while `active`, and listens for `dock-action`.

- [ ] **Step 1: Write the failing test** `app/tests/dockActions.test.ts` with a recording `DockTarget`:
  - `a draft edit sets main's draft and counts as typing`;
  - `dictate on starts dictation once and dictate off stops it` (`{on:true}` twice gives one toggle);
  - `send steer cancel approve reach the session` (approve passes `id` and `allow` through);
  - `an action while nothing is streaming cannot steer` (`steer` with `isStreaming=false` is ignored).
- [ ] **Step 2: Run** `node --experimental-strip-types --test app/tests/dockActions.test.ts`. Expected: FAIL.
- [ ] **Step 3: Implement.** The dictation that `main` runs writes into `main`'s draft (`useVoiceInput(getDraft, setDraft)`), and the next snapshot carries the text to the dock. `voiceSupported` in the snapshot is `main`'s `useVoiceInput().supported`.
- [ ] **Step 4: Run** the test and CI "Frontend Typecheck + Tests". Expected: PASS.
- [ ] **Step 5: Commit** with the message "Dock layer: the main window runs the floating chat, including dictation".

---

### Task 8: Artifacts shows the page inside the window

**Files:**
- Create: `app/src/features/artifacts/EmbeddedPreview.tsx`
- Modify: `app/src/features/artifacts/ArtifactPage.tsx` (the preview pane renders `EmbeddedPreview`; the open-window card is now the popped-out state), `ArtifactsView.tsx:84-92` (a build start also shows the page: `showPreview` is true when `target.pane === 'preview'` or the navigation is a Creator build's `offer`), `api.ts` (`embedPreview`, `placePreview`, `popOutPreview`), `model.ts` (`PreviewEvent.place`)
- Test: `app/tests/artifacts.test.ts` (extend)

**Interfaces:**
- Consumes: `creation_preview_embed`, `creation_preview_place`, `creation_preview_pop_out` (Task 3); `coveredBy`, `layerVisible`, `OVERLAY_SELECTORS` (Task 5); `useDockBridge` (Task 7). The layer is active exactly when `layerVisible` is true.
- Produces: `EmbeddedPreview({ creationId, version, network, label, onPoppedOut })`. It reports `{ rect, covered }` on mount, on ResizeObserver, on scroll, on window resize, and on DOM mutations of `document.body` (for overlays), coalesced to one report per animation frame. On unmount it reports `{ rect: null, covered: true }`. It has a toolbar with Reload, Pop out and the network switch (today's), and in the popped-out state it shows "Showing in its own window", "Bring to front" and "Show here".
- The 1.5 s live reload while a build writes (`ArtifactPage.tsx:177`) keeps calling `creation_preview_reload` with the embedded label.

- [ ] **Step 1: Write the failing tests** in `artifacts.test.ts`:
  - `a build start opens the page itself, not just the pane` (the selection reducer extracted from `ArtifactsView` as `selectionFor(target, prev)` in `model.ts` gives `showPreview: true` for a Creator offer);
  - `the embedded preview reports hidden when it unmounts` (source assertion: `EmbeddedPreview.tsx` calls `placePreview` with `rect: null, covered: true` in its effect cleanup);
  - `pop out leaves a way back` (`previewShowingText` for `place: 'window'` names "Show here").
- [ ] **Step 2: Run** `node --experimental-strip-types --test app/tests/artifacts.test.ts`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the test and CI "Frontend Typecheck + Tests". Expected: PASS.
- [ ] **Step 5: Commit** with the message "Artifacts: the page shows inside the window as Creator builds it".

---

### Task 9: Agent-first. `open_artifact` opens the embedded preview visibly

**Files:**
- Modify: `app/src-tauri/src/agent_tools/artifacts.rs` (description and test), `app/src-tauri/src/agent_session_commands.rs:1598` (the Creator system text says the page shows inside Artifacts with the chat over it)
- Test: `app/src-tauri/src/agent_tools/artifacts.rs::open_artifact_shows_a_version_and_pane_visibly` (extend)

**Interfaces:**
- Consumes: the `artifact` navigation target. Task 8 makes `pane: "preview"` open the embedded page.
- `agent-coverage.json` was updated in Tasks 3 and 4 for every new command. This task re-runs `agent_coverage` as its gate.

- [ ] **Step 1: Extend the test.** The navigation emitted for `pane: "preview"` carries `openDock: true`, so the floating chat opens over the page and the user sees the agent's action. The tool's `text_for_model` says the page is shown inside Artifacts.
- [ ] **Step 2: Run** CI "App tests". Expected: FAIL on `openDock`.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** CI "App tests". Expected: PASS, including `agent_coverage`.
- [ ] **Step 5: Commit** with the message "Agent: open_artifact shows the page inside Artifacts with the chat over it".

---

### Task 10: CI. Both launch tests open an embedded preview

**Files:**
- Modify: `.github/workflows/release.yml` (`launch-macos` and the `launch-windows` job, which the spike proved and which is kept permanently, with the same structure and the Windows paths)
- Create: `app/src-tauri/src/launch_probe.rs`
- Modify: `app/src-tauri/src/startup.rs`, `app/src-tauri/src/lib.rs`, `scripts/release/ci-gate.mjs` if its job list is enumerated
- Test: `app/src-tauri/src/launch_probe.rs` (tests module)

**Interfaces:**
- Produces: `pub fn probe_requested(env_value: Option<&OsStr>, profile: &Profile) -> bool`. It is true only when `SHODH_LAUNCH_PROBE=embedded-preview` **and** the profile is separate (`SHODH_DATA_DIR` set), so the normal profile and a user's real data never run it.
- The probe:
  1. creates a fixed sample creation from `include_str!("../tests/fixtures/launch_probe.html")` in the separate profile;
  2. calls the same code as `creation_preview_embed` and a `place` with a fixed rect;
  3. logs `Launch probe: preview embedded label=<l> shown=true`;
  4. simulates leaving Artifacts (`place` with `rect: None`), then logs `Launch probe: preview hidden=true`.
- Both launch jobs set `SHODH_DATA_DIR` to a temporary folder and `SHODH_LAUNCH_PROBE=embedded-preview`, and wait for both lines. On macOS, the existing `open --env` gets two more `--env` values, and the data-folder assertion follows `SHODH_DATA_DIR`.

**Decision for the user:** this ships a launch-only probe in release builds, gated by two environment variables and a separate profile. The alternative, a CI-only cargo feature, would mean the launch test no longer runs the binary testers get. This plan recommends the probe.

- [ ] **Step 1: Write the failing test** `the_probe_runs_only_on_a_separate_profile_and_by_name`: `(Some("embedded-preview"), separate)` gives true; `(Some("embedded-preview"), normal)`, `(Some("1"), separate)` and `(None, separate)` give false.
- [ ] **Step 2: Run** CI "App tests". Expected: FAIL.
- [ ] **Step 3: Implement** the probe and the two jobs.
- [ ] **Step 4: Run** CI "Release builds / Launch test (macOS)" and "(Windows)". Expected: both print the two probe lines, and the CI gate counts `launch-windows`.
- [ ] **Step 5: Commit** with the message "CI: the launch tests embed a preview and hide it again, on macOS and Windows".

---

## By hand, before merging the last task (the user's Windows machine and the Mac)

- The page stays live while typing and while dictating from the floating chat.
- The chat and approvals are never covered.
- Menus, dialogs and toasts are never hidden; the layers hide and the in-page dock shows.
- With internet off, the preview's network tab is empty.
- Pop out works, and so does Show here.
- At text sizes 90% and 125%, the page fills exactly the pane.
- The dock never shows a microphone permission prompt. Dictation runs in `main`.

## Self-review notes

- **Spec coverage:**
  - goal 1 is Tasks 3 and 8;
  - goal 2 (typing and dictating) is Tasks 4 to 7;
  - goal 4 is Tasks 3 and 8;
  - goal 5 is Tasks 2 and 4;
  - the ACL changes are Task 4;
  - the agent-first requirement is Task 9;
  - verification is Tasks 3 to 8 (unit) and Task 10 (CI);
  - goal 3 (pointing) is phase 2 and out of scope.
- **Types:** these names are shared across tasks and must not drift: `CssRect`, `Placement`, `EmbedState`, `DOCK_WEBVIEW`, `MAX_RELAY_BYTES` = `MAX_SNAPSHOT_BYTES` = 262 144, `DockSnapshot`, `DockAction`, `DockModel`, `applyDockAction`, `useDockBridge`.
