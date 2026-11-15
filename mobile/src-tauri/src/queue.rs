//! The phone's outbox for the window: questions asked, files sent and edits
//! made while the computer is asleep or away, kept sealed in order
//! (`shodh_link::outbox`) and sent by the window when the computer is back
//! (mobile/src/lib/outbox.ts). Every change is saved before it is reported,
//! and the window is told ([`QUEUE_EVENT`]).

use std::path::Path;

use serde_json::Value;
use shodh_link::allowlist::allowed;
use shodh_link::outbox::{Outbox, QueueState, QueuedItem, Work};
use shodh_link::sealed::SealedFile;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::OnceCell;

use crate::link::now_ms;
use crate::store::Store;

/// The queue changed (no payload: the window reads it again).
pub const QUEUE_EVENT: &str = "outbox://changed";

const OUTBOX_FILE: &str = "outbox.sealed";

/// Managed state: the outbox, opened on first use.
#[derive(Default)]
pub struct Queue {
    outbox: OnceCell<Outbox<SealedFile>>,
}

impl Queue {
    pub async fn outbox(&self, app: &AppHandle) -> Result<&Outbox<SealedFile>, String> {
        let store = app.state::<Store>();
        self.outbox
            .get_or_try_init(|| async {
                let storage = store.get(app).await?;
                Outbox::open(storage.sealed(OUTBOX_FILE)).map_err(|e| e.to_string())
            })
            .await
    }
}

fn changed(app: &AppHandle) {
    if let Err(e) = app.emit(QUEUE_EVENT, ()) {
        tracing::warn!("The window was not told the queue changed: {e}");
    }
}

/// What may be queued: an edit only of a command the phone may call, and a
/// file only from the app's own storage (a copy made when it was added).
fn check(work: &Work, own: &Path) -> Result<(), String> {
    match work {
        Work::Question(q) => {
            if q.text.trim().is_empty() || q.conversation_id.trim().is_empty() {
                return Err("A question needs its words and its conversation.".into());
            }
            Ok(())
        }
        Work::Edit(edit) => {
            if !allowed(&edit.command) {
                return Err("Your phone can't do this. Use your computer.".into());
            }
            Ok(())
        }
        Work::File(file) => {
            let path = Path::new(&file.local_path);
            if !path.is_absolute()
                || path
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
                || !path.starts_with(own)
            {
                return Err("Only a copy Shodh made on this phone can be sent.".into());
            }
            Ok(())
        }
    }
}

#[tauri::command]
pub async fn outbox_list(
    app: AppHandle,
    queue: State<'_, Queue>,
) -> Result<Vec<QueuedItem>, String> {
    Ok(queue.outbox(&app).await?.list())
}

#[tauri::command]
pub async fn outbox_add(
    app: AppHandle,
    id: String,
    work: Work,
    queue: State<'_, Queue>,
    store: State<'_, Store>,
) -> Result<QueuedItem, String> {
    let storage = store.get(&app).await?;
    check(&work, storage.cache())?;
    let item = queue
        .outbox(&app)
        .await?
        .add(&id, work, now_ms())
        .map_err(|e| e.to_string())?;
    changed(&app);
    Ok(item)
}

#[tauri::command]
pub async fn outbox_mark(
    app: AppHandle,
    id: String,
    state: QueueState,
    error: Option<String>,
    queue: State<'_, Queue>,
) -> Result<QueuedItem, String> {
    let item = queue
        .outbox(&app)
        .await?
        .mark(&id, state, error, now_ms())
        .map_err(|e| e.to_string())?;
    changed(&app);
    Ok(item)
}

#[tauri::command]
pub async fn outbox_remove(
    app: AppHandle,
    id: String,
    queue: State<'_, Queue>,
) -> Result<Value, String> {
    let removed = queue
        .outbox(&app)
        .await?
        .remove(&id)
        .map_err(|e| e.to_string())?;
    if let Work::File(file) = &removed.work {
        // The copy made when the file was added is not needed any more.
        let _ = std::fs::remove_file(&file.local_path);
    }
    changed(&app);
    Ok(Value::Null)
}
