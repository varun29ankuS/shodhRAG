# Shodh on Android (phone companion): implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task by task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** an Android app that pairs with the user's own Shodh desktop over iroh and gives streamed, cited answers,
conversations, library browsing, approvals, voice and notifications. The desktop does all the work.

**Architecture:**
- **Saving.** Conversations are first saved by the desktop backend (PR 1).
- **Bridge.** A new crate `shodh-link` carries pairing, identity and an allowlisted bridge that forwards the
  desktop's own Tauri commands and its `agent_event` stream over iroh (PR 2).
- **App.** A new Tauri v2 Android app in `mobile/` reuses the desktop's React components through an `invoke`
  shim (PRs 3–6).

**Tech stack:**
- Rust: Tauri 2.12 (desktop + Android), iroh.
- Frontend: React 19, Tailwind 3, lucide-react.
- Android: a Kotlin Tauri plugin (voice), and UnifiedPush.
- CI: GitHub Actions (Android SDK/NDK, emulator).

**Spec:** `docs/superpowers/specs/2026-11-13-phone-companion-design.md`, moved into the repo in PR 1.

## Global constraints

- No local builds. CI is the only verifier.
- One agent at a time.
- PR workflow, branch → PR → CI gate green → merge.
- No commit or PR footers.
- Rust preferred; Kotlin only where Android requires it (the voice plugin, the foreground service).
- UI rules:
  - text 12px or larger;
  - plain words;
  - icons left of labels;
  - touch targets of 44px or more;
  - one column on the phone;
  - shortcut hints from the registry (desktop).
- The phone and desktop must be the same Shodh version. On a mismatch the phone shows "Update Shodh on your
  phone", with a one-tap APK update.
- Notification payloads carry no content. A UnifiedPush endpoint URL is a secret: never logged.
- The phone may approve Ask and Creator steps only. Code mode commands, edits and "allow for the rest of this
  conversation" are refused from a remote device on the desktop side.
- Secret-shaped test values are built at runtime, because of push protection.

## Review focus (the failure modes no single task's happy path covers)

1. **The phone drops off mid-answer** (tunnel, app backgrounded). The full answer must still be saved and shown
   when it reconnects. *Test in Task 1.3 and Task 2.4.*
2. **Desktop and phone send to the same conversation within a second.** No turn is lost and the order is
   kept. *Test in Task 1.2.*
3. **A removed or stolen phone retries with its old key, or a replayed pairing QR is used.** It must be
   refused, with no partial access. *Test in Task 2.2.*
4. **The desktop goes to sleep with questions queued, and the phone app is killed before it wakes.** The queue
   survives and sends in order. *Test in Task 4.2.*
5. **The phone asks for a Code-mode approval or calls a command outside the allowlist.** It's refused on the
   desktop, with a plain message. *Test in Task 2.3.*

---

## PR 1: the desktop backend saves conversations (prerequisite)

Branch `feat/backend-saves-conversations`.

**Why:** today `app/src/hooks/useConversations.ts:146` saves whole records through `save_conversation`, and the
agent transcript is reduced in TS (`app/src/features/agent/reducer.ts:555` `reduceTranscript`). Two devices
saving whole records lose turns.

### Task 1.1: a Rust transcript reducer, kept equal to the TS one

**Files:**
- Create `app/src-tauri/src/agent_transcript.rs`.
- Create the fixtures `app/tests/fixtures/transcripts/*.json`. Each is an agent_event list plus the expected
  reduced transcript, and each is generated from real recorded synthetic sessions, never user data.
- Test the same fixtures in both languages: Rust in `agent_transcript.rs`, TS in
  `app/tests/transcriptParity.test.ts`.

**Interfaces:**
- Produces `pub fn reduce_transcript(events: &[AgentEventEnvelope]) -> serde_json::Value`, the same JSON shape
  `ConversationMessage.transcript` stores today.

- [ ] Write the failing test: `fixtures_reduce_to_the_saved_transcript`, i.e. for each fixture
  `reduce_transcript(events) == expected` (Rust), and `reduceTranscript` over the same events gives `expected`
  (TS).
- [ ] Implement `reduce_transcript`, covering steps, the task list (with `blocked`, from #166), usage and cited
  passages.
- [ ] Commit.

### Task 1.2: the session appends turns to the store

**Files:**
- Modify `app/src-tauri/src/agent_session_commands.rs`.
- Modify `app/src-tauri/src/conversation_commands.rs`: add `ConversationStore::append_turn`.

**Interfaces:**
- Produces `ConversationStore::append_turn(&self, conversation_id: &str, message: ConversationMessage) -> Result<ConversationRecord, String>`.
  It's an atomic read-modify-write under the existing file lock.
- Produces the event `"conversation_changed" { id }`, emitted after every append.

- [ ] Write the failing test `two_senders_lose_no_turns`: two tasks append to one conversation concurrently;
  every message is present, in submission order.
- [ ] Implement it. The user turn is appended when `agent_send` accepts it. The assistant turn is appended at
  the session's final event, with the `reduce_transcript` output, `metadata` and `run`.
- [ ] Commit.

### Task 1.3: the UI reads and stops writing whole records for agent answers

**Files:**
- Modify `app/src/hooks/useConversations.ts`, and the callers that save after an answer.

- [ ] Write the failing test: `an_answer_is_saved_without_the_window` (Rust: the session finishes with no UI
  save call, and the store has the answer). TS: the hook reloads on `conversation_changed` and doesn't call
  `save_conversation` for agent turns.
- [ ] Implement it. Title, pin, workspace and mode edits still go through `save_conversation`, as field
  updates. Message lists are not overwritten from the UI.
- [ ] Commit, open the PR "Conversations are saved by the app, not the window", get CI green, then merge.

## PR 2: `shodh-link`, i.e. pairing, identity and the bridge (desktop side)

Branch `feat/shodh-link`.

### Task 2.1: the crate, versions and messages

**Files:**
- Create `crates/shodh-link/{Cargo.toml,src/lib.rs,src/messages.rs}`.

**Interfaces:**
- `pub const LINK_VERSION: &str = env!("CARGO_PKG_VERSION")`. It must equal the app version, because of the
  lockstep rule.
- `pub enum Request { Invoke { command: String, args: serde_json::Value, call_id: u64 }, Subscribe { session_ids: Vec<String> }, Hello { version: String, device_name: String } }`.
- `pub enum Response { Result { call_id: u64, value: Result<serde_json::Value, String> }, Event { name: String, payload: serde_json::Value }, VersionMismatch { desktop: String } }`.
- Framing: length-prefixed JSON on one iroh bidirectional stream per connection, with a 4 MiB frame cap.

- [ ] Test: round-trip serde for every variant; an over-cap frame is refused.
- [ ] Implement, and commit.

### Task 2.2: identity and pairing

**Files:**
- Create `crates/shodh-link/src/pairing.rs`.
- Desktop: `app/src-tauri/src/phone_link.rs`, plus the commands `phone_pairing_start`, `phone_devices`,
  `phone_device_remove`.

**Interfaces:**
- `PairingOffer { node_id, relay_url, secret: [u8; 32], expires_at }`, encoded as the QR string
  `shodh-pair:1:<base32>`.
- Secret lifetime: 5 minutes, single use.
- `PairedDevice { node_id, name, paired_at, last_seen }`, stored in the desktop's secure storage.

- [ ] Tests:
  - `an_expired_pairing_secret_is_refused`;
  - `a_used_pairing_secret_is_refused`;
  - `a_removed_device_is_refused` (its next connection is closed before any request);
  - `an_unknown_node_is_refused`.
- [ ] Test `the_agent_can_open_pairing_but_not_remove_a_device` (agent-first): an agent tool shows the
  pairing QR view; removal needs the user's approval; both are recorded in agent-coverage.json.
- [ ] Implement it: the node key is kept in the OS secure storage on the desktop and in the Android Keystore
  on the phone.
- [ ] Commit.

### Task 2.3: the allowlisted bridge

**Files:**
- Create `crates/shodh-link/src/allowlist.rs`.
- Modify `app/src-tauri/src/phone_link.rs` (dispatch to the real command handlers, and filter `agent_event`
  per subscription).

**Interfaces:**
- `pub fn allowed(command: &str) -> bool`. The list starts from the five screens: `agent_start`, `agent_send`,
  `agent_approve`, conversation list/load, library listing, document open/page fetch. The builder finalises
  it, and every entry carries a comment saying why it's there.
- `pub fn remote_may_approve(kind: &ApprovalKind) -> bool`: false for Code-mode commands, file edits and
  allow-for-conversation.

- [ ] Tests:
  - `a_command_outside_the_allowlist_is_refused`;
  - `a_remote_code_approval_is_refused` (the gate stays pending, and the phone gets "Approve this on your
    computer");
  - `events_of_unsubscribed_sessions_are_not_sent`;
  - `each_remote_call_is_recorded_in_activity_with_the_device_name`.
- [ ] Implement, and commit.

### Task 2.4: end to end, with two in-process nodes

- [ ] Test `pair_ask_stream_approve_open`: pair; start a session; receive streamed tokens and a citation;
  answer an Ask approval; open a document page; disconnect mid-answer, reconnect, and the full answer is in
  the store. No sleeps: use channels or barriers.
- [ ] Test `a_version_mismatch_says_update`.
- [ ] Open the PR "Phone link: pairing and a safe bridge to the desktop", get CI green, then merge.

## PR 3: the Android app and its look (the slice ships after this PR)

Branch `feat/mobile-app`.

### Task 3.1: the project, the shim and the CI job

**Files:**
- Create `mobile/` (a Tauri v2 project, Android target), `mobile/src-tauri` (depends on `shodh-link` and iroh;
  not `shodh-rag`), and `mobile/src/lib/invoke.ts`.
- Shared components: move the chat message, citation badge, approval card and library list into
  `app/src/shared/` and import them from both apps. The builder picks a path alias or a workspace package.
- CI: add an `android` job to `.github/workflows/ci.yml` (path-scoped per #155: `mobile/`, `crates/shodh-link`,
  `app/src/shared`), always in nightly and release runs, building a debug APK.

- [ ] Test: Vitest `invoke_shim_forwards_and_routes_events`, with a fake link transport.
- [ ] Test: CI emulator smoke `the_app_reaches_its_pair_screen`. Like the macOS launch test: install the APK,
  launch it, wait for a real ready log line, stay up 15 s, no crash in logcat.
- [ ] Implement, and commit.

### Task 3.2: the five screens, and making the app look great

**Files:**
- `mobile/src/screens/{Pair,Ask,Conversations,Library,Reader,Approvals,Settings}.tsx`
- `mobile/src/theme.css`

**Design direction (decided):**
- **Same visual language as the desktop**, with no new brand: Shodh's existing tokens (colours, type, radius),
  light and dark, following the system.
- **A phone-native layout:**
  - a bottom tab bar (Ask, Library, Approvals) with icons left of or above the labels;
  - thumb-reach primary actions;
  - 44px targets;
  - body text 16px, and 12px or more everywhere;
  - safe-area insets;
  - an edge-to-edge reader.
- **A calm, readable answer view:**
  - the streamed text has no layout jumps;
  - citation badges are tappable chips that open the passage in the Reader at the cited page;
  - a skeleton shows while waiting, never a bare spinner;
  - the connection state lives in the header as a small dot plus plain words ("Connected", "Your computer is
    asleep: questions will be sent when it wakes").
- **Motion that rivals iOS (user, 2026-11-13: "without animation nothing will catch anyone's eye"):**
  - **Springs, not easing curves:** every transition is physics-based, interruptible and velocity-aware. Use
    the `motion` library (Framer Motion) for React; the builder confirms its licence (MIT) and bundle size.
  - **Gesture-driven:**
    - swipe from the edge to go back, with the page following the finger;
    - pull to refresh with a rubber band;
    - bottom sheets that drag and snap (approvals, workspace picker, citation preview);
    - swipe actions on lists (Conversations: pin and delete).
  - **Shared-element transitions:**
    - a citation chip expands into the Reader at the cited page;
    - a conversation row expands into the chat;
    - a library item expands into the document.
  - **Streaming that feels alive:** tokens fade in per word, never jumping the layout. Citation chips pop in
    with a small spring when their source arrives. Skeletons shimmer while waiting.
  - **Haptics** (official Tauri haptics plugin): light ticks on snap points, a success pulse when an approval
    is sent, a warning pulse when it's refused.
  - **Performance budget:**
    - animate only `transform` and `opacity` (GPU-composited); never animate layout properties;
    - 60 fps, and 120 fps on high-refresh phones;
    - add a lint or test that fails on animations of layout properties;
    - run a real-device check on a mid-range Android phone by the user before PR 3 merges. Emulator frame
      rates aren't representative.
  - **Accessibility:** with Android's "Remove animations" on, every transition becomes an instant cross-fade
    and gestures still work.
  - **Motion never delays information:** text and answers appear as fast as they arrive, and animation
    decorates, never waits.
- **Empty states and errors in plain words,** each with one next step.
- **Process:**
  - use the frontend-design skill for the visual pass;
  - CI uploads emulator screenshots of every screen in light and dark as artifacts;
  - the user reviews those screenshots before merge. The PR doesn't merge until the user approves the look.

- [ ] Test (Vitest): each screen renders its empty, loading, connected and asleep states; no text under
  12px (a computed-style check in jsdom on the theme tokens); every interactive element has 44px or more
  (style contract).
- [ ] Test (emulator): screenshots of all seven screens, light and dark, are uploaded as the artifact
  `mobile-screens`.
- [ ] Implement, commit, open the PR "Shodh for Android: ask, conversations and library", get CI green, have
  the user approve the screenshots, then merge.

## PR 4: the offline queue, connection states, "Stay available", one-tap update

### Task 4.1: "Stay available for my phone" (desktop)
- [ ] Test: the setting keeps the computer awake only while on AC power and only when enabled. Use the OS
  power APIs on Windows and macOS, with an injectable power source in the test.
- [ ] Implement, and commit.

### Task 4.2: the offline queue (phone)
- [ ] Test `queued_questions_survive_a_kill_and_send_in_order`: a persisted queue file is replayed after
  restart; states go waiting → sent → answered.
- [ ] Implement, and commit.

### Task 4.3: one-tap update
- [ ] Test: on a version mismatch, the phone downloads the matching APK from the GitHub release, refuses one
  whose signature or hash doesn't match the release, then hands it to the installer.
- [ ] Implement, open the PR, get CI green, merge.

## PR 4b: send files from the phone (user-approved 2026-11-13)

**How a file comes in:** Android's "Share → Shodh", an Add button in Library (the file picker), and camera
photos (OCR'd on the desktop).

**Where it goes:** the user picks per send. The default is the **"From my phone"** inbox; the other choice is a
**specific workspace** (the list comes from the desktop).

**On the desktop:** the file goes over iroh-blobs on the same connection, with resumable transfers. It's
indexed through the normal upload path with every existing safety rule (secret files blocked, size limits,
legacy Office in the guarded worker, skipped reasons), and recorded in Activity as "added from <device>". It's
queued on the phone while the computer is asleep.

**Small extra:** "Send to phone" on a document, for offline reading on the phone.

- [ ] Tests:
  - `a_file_from_the_phone_lands_in_the_chosen_workspace` (and in the inbox by default);
  - `an_interrupted_transfer_resumes`;
  - `a_secret_file_from_the_phone_is_blocked_like_any_upload`;
  - `a_file_over_the_size_limit_is_refused_with_the_reason`;
  - `queued_files_send_after_the_computer_wakes`.
- [ ] Implement, open the PR, get CI green, merge.

## PR 5: notifications (Stay connected + UnifiedPush) and the lost-phone lock

- [ ] Test: the push payload has no content fields; the endpoint URL never appears in logs (a log-capture
  test); the foreground service starts only when "Stay connected" is chosen.
- [ ] Test: the PIN/biometric lock is required on open by default; a wrong PIN 5 times locks for 30 s.
- [ ] Implement it: Kotlin only for the foreground service, using the official Tauri biometric plugin.
- [ ] Open the PR, get CI green, merge.

## PR 6: voice

- [ ] Kotlin Tauri plugin `shodh-voice`: `listen() -> String` (on-device recognition preferred) and
  `speak(text)`.
- [ ] Test: spoken citations come from citation metadata ("Self-RAG, page 4"), never from parsing the text.
  This is a unit test on the TS formatter.
- [ ] Open the PR, get CI green, merge.

## Release

The slice ships as 0.0.12 after PR 3. The full v1 ships after PR 6. The APK is signed with a release key held
in a GitHub secret and attached to the GitHub release.
