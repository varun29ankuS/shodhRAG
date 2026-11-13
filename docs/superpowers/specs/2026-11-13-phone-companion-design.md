# Shodh on Android: the phone companion (design)

Date: 2026-11-13. Status: draft, for the user's approval. Repo github.com/varun29ankuS/shodhRAG.
Where it goes when building starts: `docs/superpowers/specs/2026-11-13-phone-companion-design.md`.

## Intent

Make the personal Shodh library usable away from the desk. The user's own computer (a laptop or a desktop, at
home or at the office) keeps doing all the work: search, models, the agent. The Android app is a **remote** for
that computer. This comes before teams. Teams will reuse the same identity, pairing and transport layer (iroh),
so this is also the foundation for shared workspaces later.

Success: from a phone on mobile data, the user pairs once by QR, then asks a question about their library and
gets the same streamed, cited answer the desktop gives. They browse and open their documents, and approve or
deny what the agent asks to do. All of this is end-to-end encrypted, with no Shodh server.

## Goals (v1)

1. **Pairing by QR**, end-to-end encrypted over iroh. No account and no Shodh server.
2. **Ask on the go:** streamed answers with citation badges. The same conversations as the desktop: the phone
   sees the desktop's conversation history and can continue a conversation.
3. **Approvals:** the agent's approval cards (the same ones as the desktop) arrive on the phone with
   Allow / Deny.
4. **Library browsing (must have):** folders, documents and papers. Open a document, fetched on demand and not
   stored long-term on the phone. The citation badge opens the cited page or passage.
5. **Voice:** tap to talk using the phone's speech recognition. Answers are read aloud, with spoken citations
   ("Self-RAG, page 4"), on-device where the phone supports it. Hands-free back-and-forth is v1.1.
6. **When the computer is unreachable** (asleep, closed, offline):
   - the phone says so plainly;
   - questions are saved on the phone and sent when the computer comes back;
   - the answer then arrives.
7. **Notifications for approvals and finished answers, configurable:**
   - **Only while the app is open** (the default).
   - **Stay connected:** an Android foreground service with a permanent "Shodh is connected" notification.
   - **Push** through **UnifiedPush**. The user installs a distributor app such as ntfy, on the public server
     or their own. The desktop posts to the phone's endpoint directly.
   - Every notification carries **no content** (only "Shodh needs your approval" or "Your answer is ready").
     The details come over iroh when the app opens.
8. **"Stay available for my phone"** on the desktop: opt-in, active only while plugged in. It keeps the
   computer from sleeping so the phone can reach it.
9. **Lost phone:** both of these, as agreed:
   - On the desktop, Settings lists paired devices, and **Remove** revokes one immediately. Its key is rejected
     from then on.
   - On the phone, a PIN or fingerprint (biometric) is required to open the app. It's on by default and can be
     changed.

## Non-goals (v1)

- **Teams and shared workspaces:** the next project, which reuses this layer.
- **iPhone:** after Android. It needs the paid Apple Developer Program, and that same account also enables
  Mac notarization.
- **The Play Store:** the APK is installed directly first.
- **A copy of the library on the phone, or offline answers:** the phone stores no library. Opened documents
  are cached only temporarily.
- **Google push (FCM):** it needs a Shodh relay server, because the sending key can't ship in a desktop app.
- **Waking a sleeping computer remotely:** it can't be done reliably from outside its network, so it isn't
  promised.
- **Several computers paired to one phone:** one computer per phone in v1. My call; easy to extend later.

## Anti-goals (a failure even if it works)

- Anything that sends library content, questions or answers through a third party in readable form.
- A phone app that can do things the desktop's approval rules wouldn't allow. The phone is a remote: the
  desktop's modes, approval gate and safety rules stay in charge.
- A second, drifting copy of the UI. The phone reuses the desktop's React components.
- Silent failure when the computer is away. The user must always see the state: connected, asleep and
  waiting, or offline.

## Decided by the user

- **Android first, built with Tauri v2 mobile.** Whisker (pre-alpha) was considered and rejected.
- **iroh** for the connection, now and for teams.
- Laptop and desktop both, at home or office. Questions queue while the computer sleeps.
- Notifications configurable: Off (open only) / Stay connected / UnifiedPush. No Google push in v1.
- Lost phone: both device removal on the desktop and a PIN/biometric lock on the phone.
- APK installed directly first. Google developer verification is expected in India in 2027; the Play Store
  comes later.
- **Same repo.**

## Architecture (the parts decided here; the rest is left to the builder)

**Layout**
- `app/`: the desktop app, unchanged except for a new "phone link" module.
- `mobile/`: a new small Tauri v2 project with an Android target and its own `src-tauri`. It doesn't depend
  on `shodh-rag`, ONNX or the models.
- `crates/shodh-link`: the protocol both apps depend on. It holds the message types, the version, and the
  pairing and identity code.
- Shared React components move into a folder both frontends import. The builder picks the mechanism, such as
  a workspace package or a path alias.

**Conversations are saved by the desktop backend (a prerequisite, decided).** Today the desktop's React UI keeps
conversations in memory and saves whole records with `save_conversation` (`app/src/hooks/useConversations.ts`).
If the phone drove a chat, the phone would be the one saving the answer. Answers would then be lost when the
phone drops mid-reply, and two devices saving whole records would overwrite each other's turns. So saving moves
into the desktop backend, the way connector (Slack) conversations already are:
- the agent session appends each turn as it completes;
- both frontends only read and display.

This is its own PR, done before the bridge.

**Both devices in one conversation (decided):** both see the same live stream, and either can send the next
message. The desktop runs one message at a time per conversation, so there are no conflicts.

**The bridge, the key decision.** The phone doesn't get a new API. It forwards an **allowlisted subset** of the
desktop's existing Tauri commands and events over iroh:
- **Commands:** for example `agent_start`, `agent_send`, `agent_approve` (see `agent_session_commands.rs`),
  conversation list/load, library listing, document open/page fetch.
- **Events:** the `agent_event` stream, filtered to the sessions the phone is viewing, approval requests
  included.

**Which approvals the phone may grant (decided).** The allowlist is per command **and** per approval kind:
- **The phone may approve Ask and Creator steps:** searches, web fetches, notes, GitHub posts and similar.
- **Code mode shell commands, file edits and "allow for the rest of this conversation"** are shown on the phone
  as "Approve this on your computer", with no Allow button. The desktop refuses them from a remote device even
  if asked.
- A lost or borrowed phone must never be able to run commands on the computer.
- On the phone, an `invoke` shim sends these over the iroh connection, so the reused React components work
  unchanged.
- The allowlist lives in `shodh-link` and is checked **on the desktop**. Anything not on the list is refused.
- Every call is attributed to the device (an audit line in Activity, e.g. "from Pixel 8").
- The desktop's own approval gate and modes apply exactly as for local use.

**Identity and pairing**
- Each install has an iroh node key, stored in the OS keystore: the Android Keystore on the phone, the
  existing secure storage on the desktop.
- The desktop shows a QR with its node id, relay hint and a one-time pairing secret. The secret expires in
  minutes and is used once.
- The phone connects and proves the secret. Both sides store each other's key, and from then on only those
  keys are accepted.
- The desktop lists paired devices with **Remove**.

**Transport**
- iroh QUIC: direct where possible, otherwise through a relay, which only forwards encrypted traffic.
- Default to n0's public relays. A "custom relay" setting comes later.

**Versioning (decided).** v1 requires the phone and desktop to be the **same Shodh version**, because the bridge
forwards the desktop's own commands.
- A mismatch shows "Update Shodh on your phone" (or on the desktop) instead of failing oddly.
- The phone has a **one-tap update**: it downloads the matching signed APK from the GitHub release, verifies
  it, and hands it to Android's installer.
- A version range with fixed wire types is later.

**The offline queue**
- Saved on the phone. When the connection returns, it's sent in order.
- Each queued question shows its state: waiting, sent, answered.

**Push (UnifiedPush)**
- The phone registers its endpoint with the desktop over iroh.
- The desktop POSTs only a content-free wake-up to that endpoint.
- The endpoint URL is treated as a secret: stored securely and never logged.

**The voice plugin**
- A small Kotlin Tauri plugin wraps Android's `SpeechRecognizer` and `TextToSpeech`, preferring on-device.
- It's the only non-Rust/TS code, and Tauri requires it for Android APIs.
- Spoken citations come from the citation metadata, not from parsing the text.

**UI**
- The user's rules apply:
  - text 12px or larger;
  - plain words;
  - icons left of labels;
  - touch targets of 44px or more;
  - a one-column layout.
- Screens:
  - Ask (chat);
  - Conversations;
  - Library (folders → documents → reader);
  - Approvals;
  - Settings (pairing, lock, notifications).
- Connection state is always visible in the header.

**Agent-first.** The desktop agent can show "Pair a phone" (open the QR view) and list paired devices. It
**can't** remove a device or change phone notification settings without the user's approval.

**CI and release**
- An Android build job (SDK + NDK) in CI. It's path-scoped per #155: it runs when `mobile/`, `shodh-link` or
  the shared components change, and always in nightly and release runs.
- The APK is signed with a release key held in a GitHub secret, like the TAURI_SIGNING secrets. The APK is
  attached to the GitHub release.
- An Android emulator smoke test launches the app and reaches its first screen, like the macOS launch test.

## Tests the build must include

- **Protocol:**
  - a command outside the allowlist is refused on the desktop;
  - a removed device's key is refused;
  - an expired or reused pairing secret is refused;
  - a version mismatch gives the update message.
- **End to end:** two in-process iroh nodes (desktop side + phone side) pair, ask, stream an answer with
  citations, answer an approval, open a document page. Deterministic, with no sleeps as synchronisation.
- **Offline queue:**
  - questions asked while disconnected are sent in order after reconnecting;
  - a quit while queued doesn't lose them.
- **Push:** the payload contains no content, and the endpoint URL is never logged.
- **Approval gate:** an action requested from the phone in Ask mode is still gated exactly as on the desktop.
  A Code mode command, edit or "allow for conversation" approval sent from the phone is refused by the desktop.
- **Saving:**
  - a phone that disconnects mid-answer still finds the full answer saved;
  - two devices sending to one conversation lose no turns.

## Left to the builder

- The exact allowlist, starting from what the five screens need.
- How shared React components are packaged.
- How the voice plugin is structured inside Tauri's Android plugin model.
- Caching details for opened documents (size cap, eviction).

## Effort (honest estimate)

| Part | Rough size |
|---|---|
| Conversation saving moves to the desktop backend (prerequisite PR) | 1–2 days |
| `shodh-link`: types, version, pairing, identity, allowlist bridge on the desktop | 1 day |
| One-tap APK update | 0.5 day |
| `mobile/` Tauri Android project, invoke shim, the five screens laid out for phone | 1–1.5 days |
| Library browsing + document reader on phone | 0.5–1 day |
| Offline queue, connection states, "Stay available" | 0.5 day |
| Notifications: foreground service + UnifiedPush | 1 day |
| Voice plugin (Kotlin) + spoken citations | 1 day |
| Device removal, PIN/biometric lock | 0.5 day |
| Android CI job, signing, emulator smoke test | 0.5–1 day |
| Review and fix round (like every PR so far) | 1 day |

Total: about **9–11 working days** for the full v1, in about 6–7 PRs, done one at a time.

**The slice** (now about 3 days, because backend saving comes first) is a working version with:
- QR pairing;
- ask with streamed cited answers;
- conversations;
- library browsing (read-only list, with opening a document);
- connection state.

It ships as a sideloaded APK, without voice, without notifications when the app is closed, and without the
emulator test.
