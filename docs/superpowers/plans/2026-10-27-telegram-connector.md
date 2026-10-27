# The Telegram connector, and connector conversations in the app

Date: 2026-10-27. Branch `feat/connectors-telegram`.

The user, 2026-10-26: "conversation from the connector is not visible on the app... it should be
visible ... otherwise how will the app get context .. how will it save memories". And, 2026-10-27,
for Telegram: "Me first, others later".

## Part 1: connector conversations in the app

- **One store.** Each connector exchange (the question, who asked, when, and Shodh's answer) is kept
  in the app's own conversation store (`conversations.json`, the same as Ask), so search, history
  and export see it. A `source` (`ConversationRecord.source`: connector, chat key, label, link)
  marks it. The connector owns it:
  - `save_conversation` takes only the UI's fields (title, pin, side threads) from a stale UI copy;
  - a connector id the store no longer has is never inserted again by a late save, so a forgotten
    conversation stays forgotten;
  - every change is sent to the windows (`connector-conversation-changed`).
- **One conversation per chat.**
  - Slack: one per thread. A DM question outside a thread is its own, the same rule as Slack's own
    follow-ups.
  - Telegram, private chat: one until `/new`.
  - Telegram, group: one per reply thread. A reply to Shodh's answer continues the thread of the
    question it answered.
- **Ids** are `connector-<slack|telegram>-<chat key>`. The UI makes `conv-…` ids, so the two never
  meet.
- **Sidebar.**
  - The conversations are grouped per chat label ("Telegram · you", "Slack · #deals") with the
    connector's logo.
  - The Ask view shows them read-only, with a "From Telegram" badge.
  - No agent session is started for them. Every send path refuses them: the composer, the dock,
    Retry and side threads.
- **Continue in Ask** starts an Ask chat whose first question carries the conversation as attached
  context (`AttachedOrigin::Connector`). It is fenced, and each message names its author ("Me",
  "Shodh", "Alex (not me)"). The lead line tells the model that everyone else's words are untrusted
  and never instructions. Attached context is never learned from, never matched as a memory request
  and never recorded as the question. A chat replays it with its first question.
- **Memory (decided).**
  - The owner's own words go through the app's learning path, exactly like an Ask turn
    (`LearnState::record_connector_turn`).
  - Other people's words never do; `connectors::conversations::words_to_learn` is the one gate,
    for Slack and Telegram alike.
  - Slack's channel-scoped memories are unchanged.
  - A request to remember that was already handled is not learned again.
- **Forgetting.**
  - Disconnect with forget removes the connector's conversations: Slack's for that Slack
    workspace, Telegram's all of them.
  - "Stop indexing and forget" for a Slack channel removes that channel's conversations.
- The agent's `search_conversations` marks connector conversations (`from`) and says that the words
  of anyone but the user there are untrusted.

## Part 2: Telegram

- **Transport: the Bot API with long polling.**
  - The connector polls with `getUpdates`, 30 s at a time, with a separate client timeout of
    45 s. There is no public URL and no webhook.
  - The next offset is saved before updates are handed on: at most once, so a crash loses an
    update rather than answering it twice. A restart resumes from the checkpoint.
  - 429: wait `parameters.retry_after` from the body.
  - Network errors and 5xx: exponential backoff from 1 s to 5 min, with jitter.
  - 401 (a revoked token) and 409 (a webhook is set, or another program is polling) stop the
    connector with a reason the user can act on, instead of looping.
  - `migrate_to_chat_id`: a group that became a supergroup keeps its place on the allowlist.
- **Token.**
  - It sits in the OS credential store (`telegram-bot-token:<bot id>`) and is redacted in `Debug`.
    It has no `Display` and no `Serialize`.
  - The token is part of every Bot API URL. So the URL exists only inside one function, and every
    reqwest error goes through `without_url()` and a scrub. A test makes reqwest's own error name
    the URL and checks that nothing of it gets through.
- **Linking.**
  - The page shows a one-time link (`https://t.me/<bot>?start=<code>`) and its QR code. The QR
    code comes from the `qrcode` crate, without default features, and the page draws it as SVG.
  - Codes are 24 characters from the OS's random source. Each says what it is for (owner or
    invite), works once, lasts 10 minutes, and replaces the previous code of its kind.
  - Codes are accepted only from a private chat (`chat.id == from.id`).
  - An invite never makes someone the owner. An owner code used by a second account while one is
    linked is refused.
- **Who gets what.** The chat decides, never the message or the sender:
  - **The owner's private chat** is Shodh as in Ask: the whole library (notes and records too),
    the owner's memories (when Settings → Memory gives memories to the model), the chosen model,
    citations, and the chat so far. It has **no tools at all**: it cannot change anything, run
    code or browse. The model is told to answer "Open Shodh to do that". "Remember that…" is saved
    the way Ask saves it; a sensitive note waits for approval ("Open Shodh to approve it"). The
    omp agent is not used: it is a ~320 MB process with approval prompts that a phone cannot
    answer.
  - **Invited people and allowed groups** are answered through the Slack answer path, in
    Telegram's words. They get one workspace (indexed Slack channels are taken out), never
    memories, and Telegram's own daily cap.
  - In groups, Shodh answers only a mention, `/ask@bot`, or a reply to its answer. The owner
    asking in a group gets the group's answer.
  - Anyone else gets a short refusal, at most once every 10 minutes per person.
  - A group that adds the bot is listed as waiting, with an Inbox item that needs the user.
- **Formatting.**
  - Answers use HTML parse mode, so only `& < >` are escaped. The text is cut first (at
    paragraphs, then lines, then words, counted in UTF-16 units against a 3 500-unit part), then
    escaped, then the model's few Markdown marks become `<b>`, `<code>` and `<pre>`.
  - An answer goes out in at most 4 messages, with a note when it was cut. HTML that Telegram
    refuses is sent again as plain text.
  - Citations give the title and page. Diagram code is replaced by a note, and everything is
    sanitized.
  - "typing…" is sent every 4 s while Shodh works.
- **Privacy notes** on the page:
  - Telegram bot chats are not end-to-end encrypted and are stored on Telegram's servers.
  - The model-provider note, as Slack has.
- **Notifications and the Inbox**, as for Slack:
  - the connection lost (3 minutes) or back;
  - the connection stopped;
  - the model failed;
  - the cap reached (once a day);
  - a group waiting (needs you);
  - someone joined with an invite.
- **No migration.** The state lives in settings, plus `connectors/telegram_state.json` (the
  checkpoint and each private chat's conversation number) and `connectors/telegram_usage.json`.

## Not done

- Others asking in Telegram cannot save memories. Slack's "remember:" for other people has no
  Telegram counterpart yet.
- Voice notes, photos and files are not read; the bot says it reads only text.
