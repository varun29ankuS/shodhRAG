//! The Slack connector's part B in the app: the channels indexed into Shodh workspaces
//! (their sync, card status and "Stop indexing and forget"), and the memories saved from
//! Slack (the owner's into the user's memories, everyone else's for their channel only).
//!
//! The sync itself is `shodh_rag::connectors::slack::index`; its state is in `shodh.db`
//! (`shodh_rag::connectors::slack::store`). Indexing makes no model call: the embeddings
//! are computed on this computer.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;
use serde_json::json;
use shodh_ontology::ExtractorKind;
use shodh_rag::audit::LOCAL_OWNER;
use shodh_rag::connectors::slack::content::{mentioned_users, readable_text, SlackMessage};
use shodh_rag::connectors::slack::ids;
use shodh_rag::connectors::slack::index::{
    backfill_progress, check_window_days, choose_channel, forget_channel, BotReader, EngineIndex,
    IndexEvents, SyncObserver, SyncTiming, SyncWorker, DEFAULT_WINDOW_DAYS, MAX_WINDOW_DAYS,
    MIN_WINDOW_DAYS,
};
use shodh_rag::connectors::slack::remember::{
    save_owner_memory, MemoryDraft, MemoryWriter, SaveError, SlackMemories,
};
use shodh_rag::connectors::slack::setup::{missing_index_scopes, missing_index_scopes_message};
use shodh_rag::connectors::slack::store::{
    ChannelState, Forgotten, IndexedChannel, NewIndexedChannel, SlackMemory, SlackMemoryKind,
    SlackStore,
};
use shodh_rag::connectors::slack::tokens::SlackBotToken;
use shodh_rag::connectors::slack::web::{Person, SlackApiError, SlackWeb};
use shodh_rag::connectors::slack::CancellationToken;
use shodh_rag::research::tables::calls::SystemClock;
use shodh_rag::statements::Scope;
use shodh_rag::user_memory::{Actor, MemoryContent, MemoryError, Origin, WriteAuthority};
use shodh_rag::workspaces::SourceKind as WorkspaceSourceKind;
use tauri::{AppHandle, Manager, State};

use super::slack::{
    audit_change, current_bot_token, emit_status, save, setup_error, status, workspace_ref,
    SlackInstallationPrefs, SlackState, SlackStatus, WorkspaceRef,
};
use super::slack_notices::{self, SlackNotice};
use crate::audit_commands::AuditState;
use crate::memory_commands::{MemoryState, APP_VERSION};
use crate::rag_commands::RagState;
use crate::workspace_commands::{broadcast_change, WorkspaceState};

/// What the card says about the cost of indexing.
pub const NO_MODEL_CALLS: &str =
    "Indexing runs on this computer: no model calls, no answers spent.";

fn rfc3339(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}

// ── Card ────────────────────────────────────────────────────────────

/// An indexed channel as the card shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexChannelView {
    pub id: String,
    pub name: String,
    pub shared: bool,
    pub private: bool,
    pub workspace: WorkspaceRef,
    /// `backfilling`, `up_to_date`, `paused`, `error`, `archived`, `removed`,
    /// `now_shared`, `forgetting`.
    pub state: &'static str,
    /// The reason, in plain words (an error, a pause, a stop).
    pub detail: Option<String>,
    pub threads: usize,
    /// Backfilling: about how many threads the window holds, and how long the rest takes.
    pub estimated_threads: Option<usize>,
    pub seconds_left: Option<u64>,
    pub last_sync_at: Option<String>,
    pub paused_until: Option<String>,
    pub window_days: u32,
    /// The first backfill finished (it may be paused or stopped since).
    pub backfill_done: bool,
}

/// The Slack member picked as the user.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlackOwner {
    pub user_id: String,
    pub name: String,
}

/// The indexing part of the card.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexStatus {
    /// `shodh.db` is open (indexing and memories need it).
    pub available: bool,
    pub channels: Vec<IndexChannelView>,
    pub window_days: u32,
    pub min_window_days: u32,
    pub max_window_days: u32,
    /// Scopes indexing lacks (answering may still work), and what to do about it.
    pub missing_scopes: Vec<String>,
    pub missing_scopes_message: Option<String>,
    /// Memories saved from Slack.
    pub memories: usize,
    pub owner: Option<SlackOwner>,
    pub cost_note: &'static str,
    /// The channels were read for this status (for the app only: an empty list from a
    /// failed read must not look like every channel gone, see `slack_notices`).
    #[serde(skip)]
    pub channels_read: bool,
}

/// The card's state word for an indexed channel at `now`.
pub fn channel_state_word(channel: &IndexedChannel, now: DateTime<Utc>) -> &'static str {
    match channel.state {
        ChannelState::Forgetting => "forgetting",
        ChannelState::Archived => "archived",
        ChannelState::Removed => "removed",
        ChannelState::NowShared => "now_shared",
        ChannelState::Error => "error",
        ChannelState::Active if channel.paused_at(now) => "paused",
        ChannelState::Active if !channel.backfill_done => "backfilling",
        ChannelState::Active => "up_to_date",
    }
}

/// The indexing part of the card for `prefs`.
pub async fn index_status(
    app: &AppHandle,
    prefs: Option<&SlackInstallationPrefs>,
    window_days: u32,
) -> IndexStatus {
    let state = app.state::<SlackState>();
    let mut status = IndexStatus {
        window_days,
        min_window_days: MIN_WINDOW_DAYS,
        max_window_days: MAX_WINDOW_DAYS,
        cost_note: NO_MODEL_CALLS,
        ..IndexStatus::default()
    };
    let Some(prefs) = prefs else {
        return status;
    };
    status.owner = prefs.owner.clone();
    let missing = missing_index_scopes(&prefs.installation.scopes);
    if !missing.is_empty() {
        status.missing_scopes_message = Some(missing_index_scopes_message(&missing));
        status.missing_scopes = missing;
    }
    let Ok(store) = state.store().await else {
        return status;
    };
    status.available = true;
    let team = prefs.installation.team_id.clone();
    let read = {
        let store = store.clone();
        tokio::task::spawn_blocking(move || {
            Ok::<_, String>((
                store.channels(&team).map_err(|e| e.to_string())?,
                store.memories(&team).map_err(|e| e.to_string())?.len(),
            ))
        })
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r)
    };
    let (channels, memories) = match read {
        Ok(found) => found,
        Err(e) => {
            tracing::warn!(target: "shodh::slack", error = %e, "the indexed Slack channels could not be read");
            return status;
        }
    };
    status.memories = memories;
    status.channels_read = true;
    let now = Utc::now();
    let pace = SyncTiming::default().pace;
    for channel in channels {
        let progress = backfill_progress(&channel, pace);
        let word = channel_state_word(&channel, now);
        status.channels.push(IndexChannelView {
            workspace: workspace_ref(app, &channel.workspace_id).await,
            id: channel.channel_id.clone(),
            name: channel.name.clone(),
            shared: channel.shared,
            private: channel.private,
            state: word,
            detail: channel.error.clone(),
            threads: channel.threads,
            estimated_threads: progress.estimated_total,
            seconds_left: progress.estimated_seconds_left,
            last_sync_at: channel.last_sync_at.map(rfc3339),
            paused_until: channel
                .paused_until
                .filter(|until| *until > now)
                .map(rfc3339),
            window_days: channel.window_days,
            backfill_done: channel.backfill_done,
        });
    }
    status
}

// ── The sync ────────────────────────────────────────────────────────

/// Tells the card, and removes from the app what a forgotten channel leaves outside the
/// connector: its folder source in its workspace, and its memories in the user's memories.
struct AppSyncObserver {
    app: AppHandle,
}

#[async_trait]
impl SyncObserver for AppSyncObserver {
    fn changed(&self) {
        emit_status(&self.app);
    }

    async fn remove_from_workspace(&self, channel: &IndexedChannel) -> Result<(), String> {
        let workspace = channel.workspace_id.clone();
        let space = channel.space_id();
        match self
            .app
            .state::<WorkspaceState>()
            .run(move |s| s.remove_source(&workspace, WorkspaceSourceKind::Folder, &space))
            .await
        {
            Ok(_) => {
                broadcast_change(&self.app, Some(&channel.workspace_id));
                Ok(())
            }
            // The workspace is gone: so is the source.
            Err(e) if e.code == "not_found" => Ok(()),
            Err(e) => Err(format!(
                "#{} could not be removed from its workspace ({})",
                channel.name, e.message
            )),
        }
    }

    async fn forget_memories(&self, memory_ids: &[String]) -> Result<(), String> {
        forget_memories(&self.app, memory_ids).await
    }
}

/// Forgets `ids` in the user's memories. One already gone (forgotten in Settings, or by
/// an earlier try) is done; any other failure stops with the error, so the caller keeps
/// what links the rest and can try again.
async fn forget_memories(app: &AppHandle, ids: &[String]) -> Result<(), String> {
    if ids.is_empty() {
        return Ok(());
    }
    let memory = app.state::<MemoryState>();
    let service = memory
        .service()
        .await
        .map_err(|e| format!("the memories from Slack could not be forgotten ({e})"))?;
    let mut result = Ok(());
    for id in ids {
        match service.forget(id, None, &Actor::ui()).await {
            Ok(_) | Err(MemoryError::NotFound(_)) => {}
            Err(e) => {
                result = Err(format!("a memory from Slack could not be forgotten ({e})"));
                break;
            }
        }
    }
    memory.changed();
    result
}

/// Starts the sync of `prefs` until `cancel`; returns where its live events go. `None`
/// (logged) when `shodh.db` cannot be opened: answering still works.
pub(super) async fn start_sync(
    app: &AppHandle,
    prefs: &SlackInstallationPrefs,
    web: &SlackWeb,
    token: &SlackBotToken,
    cancel: CancellationToken,
) -> Option<Arc<IndexEvents>> {
    let state = app.state::<SlackState>();
    let store = match state.store().await {
        Ok(store) => store,
        Err(e) => {
            tracing::warn!(target: "shodh::slack", error = %e, "Slack channels cannot be indexed");
            return None;
        }
    };
    let team = prefs.installation.team_id.clone();
    let wake = state.wake(&team);
    let worker = SyncWorker::new(
        &team,
        &prefs.installation.bot_user_id,
        Arc::new(BotReader {
            web: web.clone(),
            token: token.clone(),
        }),
        Arc::new(EngineIndex {
            rag: app.state::<RagState>().rag.clone(),
        }),
        store.clone(),
        Arc::new(SystemClock),
        Arc::new(AppSyncObserver { app: app.clone() }),
        state.gate(&team),
        wake.clone(),
        SyncTiming::default(),
    );
    tauri::async_runtime::spawn(worker.run(cancel));
    Some(Arc::new(IndexEvents::new(
        &team,
        store,
        Arc::new(SystemClock),
        wake,
    )))
}

// ── Memories from Slack ─────────────────────────────────────────────

/// Where memories from one Slack workspace go.
pub(super) struct AppMemories {
    pub app: AppHandle,
    pub team_id: String,
    pub bot_user_id: String,
    pub data_dir: PathBuf,
    pub web: SlackWeb,
    pub token: SlackBotToken,
}

impl AppMemories {
    async fn prefs(&self) -> Result<Option<SlackInstallationPrefs>, String> {
        super::slack::installation_prefs(&self.data_dir, &self.team_id).await
    }

    async fn store(&self) -> Result<Arc<SlackStore>, String> {
        self.app.state::<SlackState>().store().await
    }

    /// The row of `draft` (not saved yet).
    fn memory(&self, draft: &MemoryDraft, owner: bool) -> SlackMemory {
        SlackMemory {
            id: format!("slackmem-{}", uuid::Uuid::new_v4()),
            team_id: self.team_id.clone(),
            channel_id: draft.channel.clone(),
            channel_name: draft.channel_name.clone(),
            author_id: draft.author_id.clone(),
            author_name: draft.author_name.clone(),
            text: draft.text.clone(),
            kind: draft.kind,
            message_ts: draft.message_ts.clone(),
            permalink: draft.permalink.clone(),
            saved_at: draft.at,
            owner,
            memory_id: None,
        }
    }
}

/// Writes the owner's memories from Slack into the user's memories.
struct OwnerWriter {
    app: AppHandle,
}

#[async_trait]
impl MemoryWriter for OwnerWriter {
    async fn write(&self, memory: &SlackMemory) -> Result<Option<String>, String> {
        let source = ids::memory_source(&memory.team_id, &memory.channel_id, &memory.message_ts);
        remember_in_memories(
            &self.app,
            &source,
            memory.kind,
            &memory.text,
            WriteAuthority::SlackOwner {
                team_id: memory.team_id.clone(),
                user_id: memory.author_id.clone(),
            },
        )
        .await
    }

    async fn erase(&self, id: &str) -> Result<(), String> {
        forget_memories(&self.app, &[id.to_string()]).await
    }
}

/// The Actor of a memory written from Slack.
fn slack_actor() -> Actor {
    Actor {
        via: "slack",
        principal: LOCAL_OWNER.to_string(),
        conversation_id: None,
        profile_id: None,
        run_id: None,
    }
}

/// Writes `text` into the user's memories with a Slack origin. Returns the id of the memory
/// this write added; `None` when the same note was in the user's memories already: that
/// one is the user's (from Settings, or another channel), and nothing from Slack links to
/// it, so forgetting what came from Slack never removes it.
async fn remember_in_memories(
    app: &AppHandle,
    source: &str,
    kind: SlackMemoryKind,
    text: &str,
    authority: WriteAuthority,
) -> Result<Option<String>, String> {
    let memory = app.state::<MemoryState>();
    let service = memory.service().await?;
    let extractor = match kind {
        SlackMemoryKind::Text => ExtractorKind::User,
        SlackMemoryKind::Thread => ExtractorKind::Llm,
    };
    let origin = Origin::from_slack(source, extractor, APP_VERSION, authority);
    let outcome = service
        .remember(
            MemoryContent::Note {
                text: text.to_string(),
            },
            Scope::Global,
            &origin,
            &slack_actor(),
        )
        .await
        .map_err(|e| e.to_string())?;
    memory.changed();
    Ok(outcome.created_id().map(str::to_string))
}

#[async_trait]
impl SlackMemories for AppMemories {
    async fn owner(&self) -> Result<Option<String>, String> {
        Ok(self.prefs().await?.and_then(|p| p.owner).map(|o| o.user_id))
    }

    async fn channel_name(&self, channel: &str, direct: bool) -> String {
        if direct {
            return "a direct message".to_string();
        }
        let known = self.prefs().await.ok().flatten().and_then(|p| {
            p.answer_channels
                .iter()
                .find(|c| c.id == channel)
                .map(|c| c.name.clone())
        });
        match known {
            Some(name) => format!("#{name}"),
            None => match self.web.channel_info(&self.token, channel).await {
                Ok(info) if !info.name.is_empty() => format!("#{}", info.name),
                _ => "a Slack channel".to_string(),
            },
        }
    }

    async fn permalink(&self, channel: &str, ts: &str) -> Option<String> {
        self.web.permalink(&self.token, channel, ts).await.ok()
    }

    /// The thread as text. Slack's rate limit fails it (nothing is saved, the asker is
    /// told), rather than a summary of names Slack would not give.
    async fn thread_text(&self, channel: &str, thread_ts: &str) -> Result<String, String> {
        let mut messages: Vec<SlackMessage> = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..5 {
            let page = self
                .web
                .replies(&self.token, channel, thread_ts, cursor.as_deref())
                .await
                .map_err(|e| e.to_string())?;
            messages.extend(page.messages);
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        let mut seen = BTreeSet::new();
        messages.retain(|m| seen.insert(m.ts.clone()) && m.indexed(&self.bot_user_id));
        let mut people: BTreeSet<String> = BTreeSet::new();
        for m in &messages {
            if let Some(user) = &m.user {
                people.insert(user.clone());
            }
            people.extend(mentioned_users(&m.text));
        }
        let mut names = HashMap::new();
        for user in people {
            match self.web.user_info(&self.token, &user).await {
                Ok(member) => {
                    names.insert(user, member.name);
                }
                Err(e @ SlackApiError::RateLimited { .. }) => return Err(e.to_string()),
                Err(e) => {
                    tracing::debug!(target: "shodh::slack", error = %e, "a Slack name could not be read");
                }
            }
        }
        Ok(messages
            .iter()
            .map(|m| {
                let who = m
                    .user
                    .as_ref()
                    .and_then(|u| names.get(u))
                    .cloned()
                    .unwrap_or_else(|| "Someone".to_string());
                format!("{who}: {}", readable_text(m.text.trim(), &names))
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }

    async fn save_owner(&self, draft: &MemoryDraft) -> Result<(), String> {
        let store = self.store().await?;
        let writer = OwnerWriter {
            app: self.app.clone(),
        };
        save_owner_memory(&store, &writer, self.memory(draft, true)).await?;
        emit_status(&self.app);
        Ok(())
    }

    async fn save_channel(&self, draft: &MemoryDraft) -> Result<(), SaveError> {
        let store = self.store().await.map_err(SaveError::Failed)?;
        let memory = self.memory(draft, false);
        let saved = SlackNotice::MemorySaved {
            memory_id: memory.id.clone(),
            channel: memory.channel_name.clone(),
            author: memory.author_name.clone(),
        };
        tokio::task::spawn_blocking(move || store.save_memory(&memory))
            .await
            .map_err(|e| SaveError::Failed(e.to_string()))?
            .map_err(SaveError::from)?;
        emit_status(&self.app);
        // The owner sees what others store for their channels, in the Inbox.
        let team = match self.prefs().await {
            Ok(Some(prefs)) => prefs.installation.team_name,
            _ => "Slack".to_string(),
        };
        slack_notices::tell(&self.app, &self.team_id, &team, &saved);
        Ok(())
    }
}

// ── Commands ────────────────────────────────────────────────────────

/// A memory saved from Slack, as the card's "From Slack" list shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlackMemoryView {
    pub id: String,
    pub channel_name: String,
    pub author_name: String,
    pub text: String,
    /// `text` or `thread`.
    pub kind: &'static str,
    pub permalink: Option<String>,
    pub saved_at: String,
    /// Saved by the owner (into the user's memories).
    pub owner: bool,
    /// In the user's memories (the owner's, or promoted). An owner's note the user had
    /// already is in them without a link: it stays when this one is deleted.
    pub in_memories: bool,
}

fn memory_view(memory: SlackMemory) -> SlackMemoryView {
    SlackMemoryView {
        in_memories: memory.owner || memory.memory_id.is_some(),
        id: memory.id,
        channel_name: memory.channel_name,
        author_name: memory.author_name,
        text: memory.text,
        kind: memory.kind.as_str(),
        permalink: memory.permalink,
        saved_at: rfc3339(memory.saved_at),
        owner: memory.owner,
    }
}

async fn blocking<T, F>(store: Arc<SlackStore>, f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&SlackStore) -> shodh_rag::connectors::slack::store::SlackStoreResult<T>
        + Send
        + 'static,
{
    tokio::task::spawn_blocking(move || f(&store).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

/// The installation the card manages and the store.
async fn current(state: &SlackState) -> Result<(SlackInstallationPrefs, Arc<SlackStore>), String> {
    let settings = state.settings().await?;
    let current = settings
        .connectors
        .slack
        .current()
        .cloned()
        .ok_or_else(|| "Connect a Slack workspace first.".to_string())?;
    Ok((current, state.store().await?))
}

/// Starts indexing a channel the bot is in, into Shodh workspace `workspace_id`; a
/// channel shared with other organizations needs `confirm_shared`. Choosing it again
/// (another workspace, or confirming sharing) keeps what was indexed and resumes. Whether
/// it is shared is read from Slack now. User-only.
#[tauri::command]
pub async fn slack_index_channel(
    app: AppHandle,
    state: State<'_, SlackState>,
    workspaces: State<'_, WorkspaceState>,
    audit: State<'_, AuditState>,
    channel_id: String,
    workspace_id: String,
    confirm_shared: bool,
) -> Result<SlackStatus, String> {
    let (_, prefs, token) = current_bot_token(&state).await?;
    let missing = missing_index_scopes(&prefs.installation.scopes);
    if !missing.is_empty() {
        return Err(missing_index_scopes_message(&missing));
    }
    let workspace_id = workspace_id.trim().to_string();
    let lookup = workspace_id.clone();
    let detail = workspaces
        .run(move |s| s.detail(&lookup))
        .await
        .map_err(|e| e.message)?;
    if detail.workspace.archived {
        return Err("That workspace is archived. Restore it, or choose another.".into());
    }
    let web = SlackWeb::new().map_err(|e| e.to_string())?;
    let channels = web
        .bot_channels(&token)
        .await
        .map_err(|e| setup_error("bot token", e))?;
    let Some(channel) = channels.into_iter().find(|c| c.id == channel_id.trim()) else {
        return Err(
            "Shodh is not in that channel. Invite it there (/invite @shodh), then reload the list."
                .into(),
        );
    };
    if channel.shared && !confirm_shared {
        return Err(format!(
            "#{} is shared with other organizations: their messages would be stored on this computer. Confirm to index it.",
            channel.name
        ));
    }
    let store = state.store().await?;
    let team = prefs.installation.team_id.clone();
    let window = state.settings().await?.connectors.slack.index_window();
    let new = NewIndexedChannel {
        team_id: team.clone(),
        channel_id: channel.id.clone(),
        name: channel.name.clone(),
        workspace_id: workspace_id.clone(),
        shared: channel.shared,
        private: channel.private,
        window_days: window,
    };
    let space = ids::space_id(&team, &channel.id);
    let label = format!("#{} (Slack)", channel.name);
    let workspace_store = workspaces.store().await.map_err(|e| e.message)?;
    // The workspace's source first, then the channel (taken back if the channel cannot
    // be saved): a channel never indexes into a space no workspace holds.
    let chosen = tokio::task::spawn_blocking(move || {
        choose_channel(&store, &workspace_store, &new, &label, Utc::now())
    })
    .await
    .map_err(|e| e.to_string())??;
    broadcast_change(&app, Some(&workspace_id));
    if let Some(old) = chosen.before.filter(|b| b.workspace_id != workspace_id) {
        let old_workspace = old.workspace_id.clone();
        let old_space = space.clone();
        if let Err(e) = workspaces
            .run(move |s| s.remove_source(&old_workspace, WorkspaceSourceKind::Folder, &old_space))
            .await
        {
            if e.code != "not_found" {
                tracing::warn!(target: "shodh::slack", error = %e.message, "a Slack channel could not be removed from its old workspace");
            }
        }
        broadcast_change(&app, Some(&old.workspace_id));
    }
    state.wake(&team).notify_one();
    audit_change(
        &audit,
        "slack_index_channel",
        json!({
            "channel": channel.name,
            "workspace": detail.workspace.name,
            "sharedWithOtherOrganizations": channel.shared,
            "windowDays": window,
        }),
    );
    status(&app).await
}

/// Forgets everything of `channel`: its documents, sync state, checkpoints, thread
/// follow-ups and Slack memories (those in the user's memories too), and its source in
/// its workspace. The channel shows "Forgetting" at once; the sync gate is then taken
/// (the step holding it stops at its next thread), so no sync step adds a thread back
/// meanwhile. A failure leaves it being forgotten, which the sync tries again.
pub(super) async fn forget_indexed_channel(
    app: &AppHandle,
    store: Arc<SlackStore>,
    channel: &IndexedChannel,
) -> Result<Forgotten, String> {
    let (team, id) = (channel.team_id.clone(), channel.channel_id.clone());
    blocking(store.clone(), move |s| {
        s.set_state(&team, &id, ChannelState::Forgetting, None)
    })
    .await?;
    emit_status(app);
    let state = app.state::<SlackState>();
    let gate = state.gate(&channel.team_id);
    let _turn = gate.urgent_turn().await;
    // The sync may have forgotten it while this waited.
    let (team, id) = (channel.team_id.clone(), channel.channel_id.clone());
    let Some(current) = blocking(store.clone(), move |s| s.channel(&team, &id)).await? else {
        return Ok(Forgotten::default());
    };
    let index = EngineIndex {
        rag: app.state::<RagState>().rag.clone(),
    };
    let observer = AppSyncObserver { app: app.clone() };
    forget_channel(&store, &index, &observer, &current).await
}

/// "Stop indexing and forget" for one channel. User-only.
#[tauri::command]
pub async fn slack_forget_channel(
    app: AppHandle,
    state: State<'_, SlackState>,
    audit: State<'_, AuditState>,
    channel_id: String,
) -> Result<SlackStatus, String> {
    let (prefs, store) = current(&state).await?;
    let team = prefs.installation.team_id.clone();
    let id = channel_id.trim().to_string();
    let channel = blocking(store.clone(), move |s| s.channel(&team, &id))
        .await?
        .ok_or_else(|| "That channel is not indexed.".to_string())?;
    emit_status(&app);
    let forgotten = forget_indexed_channel(&app, store, &channel).await?;
    // Shodh's answers in the channel, as the app keeps them, go too.
    let conversations = super::conversations::forget(
        &app,
        super::conversations::Connector::Slack,
        Some(super::slack::slack_channel_prefix(
            &channel.team_id,
            &channel.channel_id,
        )),
    )
    .await
    .map_err(|e| {
        format!("The channel's indexed content was forgotten, but Shodh's answers there could not be removed from the app ({e}). Try again.")
    })?;
    audit_change(
        &audit,
        "slack_forget_channel",
        json!({
            "channel": channel.name,
            "threads": forgotten.threads,
            "memories": forgotten.memories,
            "conversations": conversations,
        }),
    );
    status(&app).await
}

/// Forgets every channel and memory of Slack workspace `team` (Disconnect with forget).
pub(super) async fn forget_team(app: &AppHandle, team: &str) -> Result<Forgotten, String> {
    let state = app.state::<SlackState>();
    let store = state.store().await?;
    let channels = {
        let team = team.to_string();
        blocking(store.clone(), move |s| s.channels(&team)).await?
    };
    let mut all = Forgotten::default();
    for channel in &channels {
        let forgotten = forget_indexed_channel(app, store.clone(), channel).await?;
        all.channels += forgotten.channels;
        all.threads += forgotten.threads;
        all.memories += forgotten.memories;
    }
    // Memories saved in channels that were never indexed, and the follow-ups: the user's
    // memories first, then the rows that link them.
    let linked = {
        let team = team.to_string();
        blocking(store.clone(), move |s| s.linked_memories(&team, None)).await?
    };
    forget_memories(app, &linked).await?;
    let rest = {
        let team = team.to_string();
        blocking(store, move |s| s.forget_team(&team)).await?
    };
    // A memory saved while this ran (its row was read after `linked`).
    let late: Vec<String> = rest
        .memory_ids
        .iter()
        .filter(|m| !linked.contains(m))
        .cloned()
        .collect();
    forget_memories(app, &late).await?;
    all.memories += rest.memories;
    // Shodh's answers in this Slack workspace, as the app keeps them.
    super::conversations::forget(
        app,
        super::conversations::Connector::Slack,
        Some(super::slack::slack_team_prefix(team)),
    )
    .await
    .map_err(|e| format!("Shodh's answers in Slack could not be removed from the app ({e})"))?;
    Ok(all)
}

/// Sets how far back a channel's first backfill reaches (channels chosen from now on).
/// User-only.
#[tauri::command]
pub async fn slack_set_index_window(
    app: AppHandle,
    state: State<'_, SlackState>,
    audit: State<'_, AuditState>,
    days: u32,
) -> Result<SlackStatus, String> {
    check_window_days(days)?;
    let settings = save(&state, move |prefs| {
        prefs.index_window_days = days;
        Ok(())
    })
    .await?;
    audit_change(&audit, "slack_set_index_window", json!({ "days": days }));
    crate::app_settings::broadcast(&app, &settings);
    status(&app).await
}

/// The full members of the Slack workspace, for the card's "This is me" pick. User-only.
#[tauri::command]
pub async fn slack_list_people(state: State<'_, SlackState>) -> Result<Vec<Person>, String> {
    let (_, prefs, token) = current_bot_token(&state).await?;
    SlackWeb::new()
        .map_err(|e| e.to_string())?
        .people(&token, &prefs.installation.team_id)
        .await
        .map_err(|e| setup_error("bot token", e))
}

/// Picks the Slack member who is the user (`None`: nobody): their `remember:` goes into
/// the user's memories. Checked against the workspace's members now. User-only.
#[tauri::command]
pub async fn slack_set_owner(
    app: AppHandle,
    state: State<'_, SlackState>,
    audit: State<'_, AuditState>,
    user_id: Option<String>,
) -> Result<SlackStatus, String> {
    let (_, prefs, token) = current_bot_token(&state).await?;
    let owner = match user_id
        .map(|u| u.trim().to_string())
        .filter(|u| !u.is_empty())
    {
        Some(user) => {
            let people = SlackWeb::new()
                .map_err(|e| e.to_string())?
                .people(&token, &prefs.installation.team_id)
                .await
                .map_err(|e| setup_error("bot token", e))?;
            let person = people.into_iter().find(|p| p.id == user).ok_or_else(|| {
                "That person is not a full member of the Slack workspace.".to_string()
            })?;
            Some(SlackOwner {
                user_id: person.id,
                name: person.name,
            })
        }
        None => None,
    };
    let team = prefs.installation.team_id.clone();
    let picked = owner.clone();
    let settings = save(&state, move |slack| {
        if let Some(i) = slack.find_mut(&team) {
            i.owner = picked;
        }
        Ok(())
    })
    .await?;
    audit_change(
        &audit,
        "slack_set_owner",
        json!({ "owner": owner.as_ref().map(|o| o.name.clone()) }),
    );
    crate::app_settings::broadcast(&app, &settings);
    status(&app).await
}

/// The memories saved from Slack, newest first. User-only.
#[tauri::command]
pub async fn slack_memories(state: State<'_, SlackState>) -> Result<Vec<SlackMemoryView>, String> {
    let (prefs, store) = current(&state).await?;
    let team = prefs.installation.team_id.clone();
    Ok(blocking(store, move |s| s.memories(&team))
        .await?
        .into_iter()
        .map(memory_view)
        .collect())
}

/// Puts a memory someone saved for one Slack channel into the user's memories (it stays
/// the channel's too). User-only: the user's click is the authority.
#[tauri::command]
pub async fn slack_promote_memory(
    app: AppHandle,
    state: State<'_, SlackState>,
    audit: State<'_, AuditState>,
    id: String,
) -> Result<Vec<SlackMemoryView>, String> {
    let (prefs, store) = current(&state).await?;
    let lookup = id.clone();
    let memory = blocking(store.clone(), move |s| s.memory(&lookup))
        .await?
        .filter(|m| m.team_id == prefs.installation.team_id)
        .ok_or_else(|| "That memory is gone.".to_string())?;
    if memory.memory_id.is_some() {
        return Err("That memory is in your memories already.".into());
    }
    let source = ids::memory_source(&memory.team_id, &memory.channel_id, &memory.message_ts);
    let Some(memory_id) = remember_in_memories(
        &app,
        &source,
        memory.kind,
        &memory.text,
        WriteAuthority::SlackPromoted,
    )
    .await?
    else {
        return Err("The same note is in your memories already.".into());
    };
    let keep = id.clone();
    let link = memory_id.clone();
    if let Err(e) = blocking(store, move |s| s.set_memory_id(&keep, &link)).await {
        // Unlinked, it could never be forgotten with the channel: take it back.
        if let Err(undo) = forget_memories(&app, std::slice::from_ref(&memory_id)).await {
            tracing::warn!(target: "shodh::slack", error = %undo, "a promoted memory whose link failed could not be taken back");
        }
        return Err(e);
    }
    audit_change(
        &audit,
        "slack_promote_memory",
        json!({ "channel": memory.channel_name, "author": memory.author_name }),
    );
    emit_status(&app);
    slack_memories(state).await
}

/// Deletes a memory saved from Slack (from the user's memories too when it is there).
/// User-only.
#[tauri::command]
pub async fn slack_delete_memory(
    app: AppHandle,
    state: State<'_, SlackState>,
    audit: State<'_, AuditState>,
    id: String,
) -> Result<Vec<SlackMemoryView>, String> {
    let (prefs, store) = current(&state).await?;
    let lookup = id.clone();
    let memory = blocking(store.clone(), move |s| s.memory(&lookup))
        .await?
        .filter(|m| m.team_id == prefs.installation.team_id)
        .ok_or_else(|| "That memory is gone.".to_string())?;
    // The user's memory first: the row is what links it, so it goes only after.
    if let Some(memory_id) = &memory.memory_id {
        forget_memories(&app, std::slice::from_ref(memory_id)).await?;
    }
    blocking(store, move |s| s.delete_memory(&id)).await?;
    audit_change(
        &audit,
        "slack_delete_memory",
        json!({ "channel": memory.channel_name, "author": memory.author_name }),
    );
    emit_status(&app);
    slack_memories(state).await
}

/// The default window, for settings written before part B.
pub fn default_window_days() -> u32 {
    DEFAULT_WINDOW_DAYS
}

/// Where `shodh.db` is, for opening the store (`None`: the audit log could not open it).
pub type Database = Option<(PathBuf, Option<shodh_rag::audit::AuditKey>)>;

/// Opens the store at `database` off the async workers.
pub async fn open_store(database: &Database) -> Result<Arc<SlackStore>, String> {
    let (path, key) = database.clone().ok_or_else(|| {
        "Slack indexing needs the app database (shodh.db), which could not be opened; see the audit page."
            .to_string()
    })?;
    let store = tokio::task::spawn_blocking(move || {
        SlackStore::open(Path::new(&path), key.as_ref()).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(Arc::new(store))
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn channel(state: ChannelState) -> IndexedChannel {
        IndexedChannel {
            team_id: "T1".into(),
            channel_id: "C1".into(),
            name: "deals".into(),
            workspace_id: "w1".into(),
            shared: false,
            private: false,
            window_days: 90,
            state,
            backfill_latest: None,
            backfill_cursor: None,
            backfill_done: false,
            oldest_seen: None,
            newest_ts: None,
            paused_until: None,
            error: None,
            permalink_base: None,
            last_sync_at: None,
            last_poll_at: None,
            last_slim_at: None,
            poll_latest: None,
            poll_cursor: None,
            poll_newest: None,
            slim_latest: None,
            started_at: Utc::now(),
            threads: 0,
        }
    }

    #[test]
    fn the_card_words_follow_the_channel() {
        let now = Utc.with_ymd_and_hms(2026, 10, 24, 9, 0, 0).unwrap();
        let mut c = channel(ChannelState::Active);
        assert_eq!(channel_state_word(&c, now), "backfilling");
        c.paused_until = Some(now + chrono::Duration::seconds(30));
        assert_eq!(channel_state_word(&c, now), "paused");
        c.paused_until = Some(now - chrono::Duration::seconds(30));
        assert_eq!(
            channel_state_word(&c, now),
            "backfilling",
            "a pause that ended"
        );
        c.backfill_done = true;
        assert_eq!(channel_state_word(&c, now), "up_to_date");
        for (state, word) in [
            (ChannelState::Archived, "archived"),
            (ChannelState::Removed, "removed"),
            (ChannelState::NowShared, "now_shared"),
            (ChannelState::Error, "error"),
            (ChannelState::Forgetting, "forgetting"),
        ] {
            assert_eq!(channel_state_word(&channel(state), now), word);
        }
    }

    #[test]
    fn a_memory_view_says_where_it_lives_and_never_more() {
        let view = memory_view(SlackMemory {
            id: "m1".into(),
            team_id: "T1".into(),
            channel_id: "C1".into(),
            channel_name: "#deals".into(),
            author_id: "U2".into(),
            author_name: "Bob".into(),
            text: "Standup at 10".into(),
            kind: SlackMemoryKind::Thread,
            message_ts: "1.1".into(),
            permalink: Some("https://acme.slack.com/archives/C1/p11".into()),
            saved_at: Utc.with_ymd_and_hms(2026, 10, 24, 9, 0, 0).unwrap(),
            owner: false,
            memory_id: None,
        });
        assert!(!view.in_memories && !view.owner);
        assert_eq!(view.kind, "thread");
        let text = serde_json::to_string(&view).unwrap();
        assert!(text.contains("\"channelName\":\"#deals\""));
        assert!(!text.contains("U2"), "no Slack user ids: {text}");
        assert!(!text.contains("T1\""));
    }

    #[test]
    fn the_cost_note_says_no_model_calls() {
        assert!(NO_MODEL_CALLS.contains("no model calls"));
        assert_eq!(default_window_days(), 90);
    }
}
