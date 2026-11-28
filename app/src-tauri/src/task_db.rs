//! Tasks in `shodh.db` (migration 24): where [`crate::calendar_store::CalendarStore`] keeps
//! its tasks when the app gives it the database ([`use_database`]). Events stay in the
//! calendar file.
//!
//! - **The same model.** [`crate::calendar_store::CalendarData`] keeps every rule; a save
//!   ([`TaskDb::save`]) writes, in one transaction, the tasks a change touched, and one
//!   `task_change` row per field it changed, with who changed it ([`Actor`]).
//! - **Issue ids.** A task is numbered under its workspace's key when it is first stored
//!   (`THE-12`; `TASK-12` without a workspace) and keeps that id. A workspace's key is
//!   made from its name ([`key_from_name`]) and can be changed ([`TaskDb::set_issue_key`]);
//!   numbers are never reused.
//! - **Projects and labels.** A task's project name in a workspace is that workspace's
//!   project of that name (made when first named). Tags are shared labels, each with a
//!   colour, kept in the task's order.
//! - **The import** ([`TaskDb::import`]): the tasks of the calendar file, keeping their
//!   ids, in their order; a task already stored keeps the newer of the two.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use chrono::Utc;
use rusqlite::{params, OptionalExtension, Transaction};
use serde::Serialize;
use serde_json::{json, Value};
use shodh_rag::audit::{open_shared_connection, AuditKey, SharedConnection};

use crate::calendar_store::{Owner, OwnerKind, SubTask, TodoItem};

/// The key of tasks with no workspace.
pub const NO_WORKSPACE_KEY: &str = "TASK";
/// Longest issue key.
pub const MAX_KEY_CHARS: usize = 5;
/// The colours labels are given, in turn by name.
const LABEL_COLORS: [&str; 8] = [
    "#5e6ad2", "#26b5ce", "#4cb782", "#f2c94c", "#f2994a", "#eb5757", "#bb87fc", "#95a2b3",
];

/// Who made a change of a task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Actor {
    kind: &'static str,
    reference: Option<String>,
    reason: Option<String>,
}

impl Actor {
    /// The user, in the app.
    pub fn user() -> Self {
        Self::of("user", None)
    }
    /// The assistant, in the conversation `reference` names.
    pub fn agent(reference: Option<String>) -> Self {
        Self::of("agent", reference)
    }
    /// The user's phone.
    pub fn phone(device: Option<String>) -> Self {
        Self::of("phone", device)
    }
    /// An automation or a repeating task's run.
    pub fn automation(reference: Option<String>) -> Self {
        Self::of("automation", reference)
    }
    /// The app itself (a reminder that rang, a deleted workspace).
    pub fn system() -> Self {
        Self::of("system", None)
    }
    fn import() -> Self {
        Self::of("import", None)
    }
    fn of(kind: &'static str, reference: Option<String>) -> Self {
        Self {
            kind,
            reference,
            reason: None,
        }
    }
    /// The same actor, giving `reason` for the change.
    pub fn because(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }
}

/// One recorded change of a task.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskChange {
    pub at: String,
    pub actor: String,
    pub actor_ref: Option<String>,
    pub field: String,
    pub before: Value,
    pub after: Value,
    pub reason: Option<String>,
}

/// The tasks of `shodh.db`.
pub struct TaskDb {
    conn: SharedConnection,
}

impl std::fmt::Debug for TaskDb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskDb").finish_non_exhaustive()
    }
}

static DATABASES: OnceLock<Mutex<HashMap<PathBuf, Arc<TaskDb>>>> = OnceLock::new();

fn databases() -> std::sync::MutexGuard<'static, HashMap<PathBuf, Arc<TaskDb>>> {
    DATABASES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// From now on the calendar of `data_dir` keeps its tasks in `db`.
pub fn use_database(data_dir: &Path, db: Arc<TaskDb>) {
    databases().insert(data_dir.to_path_buf(), db);
}

/// The task database the calendar of `data_dir` uses, if the app gave it one.
pub fn database_of(data_dir: &Path) -> Option<Arc<TaskDb>> {
    databases().get(data_dir).cloned()
}

fn now() -> String {
    Utc::now().to_rfc3339()
}

fn db_error(e: impl std::fmt::Display) -> String {
    format!("the tasks database: {e}")
}

/// A workspace's issue key made from its name: the initials of its first three words, or
/// the first three letters of one word, in capitals (*Thesis* -> `THE`, *Reading group* ->
/// `RG`). `TASK` when the name has no letters.
pub fn key_from_name(name: &str) -> String {
    let words: Vec<String> = name
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| w.chars().next().is_some_and(|c| c.is_ascii_alphabetic()))
        .map(|w| w.to_ascii_uppercase())
        .collect();
    let key: String = match words.as_slice() {
        [] => String::new(),
        [one] => one.chars().take(3).collect(),
        many => many
            .iter()
            .take(3)
            .filter_map(|w| w.chars().next())
            .collect(),
    };
    if key.is_empty() {
        NO_WORKSPACE_KEY.to_string()
    } else {
        key
    }
}

/// An issue key as given: 2 to 5 capital letters or digits, starting with a letter.
pub fn check_key(key: &str) -> Result<String, String> {
    let key = key.trim().to_ascii_uppercase();
    let fits = (2..=MAX_KEY_CHARS).contains(&key.chars().count())
        && key.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && key
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
    if !fits {
        return Err(format!(
            "An issue key is 2 to {MAX_KEY_CHARS} letters or digits, starting with a letter, not {key:?}."
        ));
    }
    if key == NO_WORKSPACE_KEY {
        return Err(format!(
            "{NO_WORKSPACE_KEY} is the key of tasks without a workspace."
        ));
    }
    Ok(key)
}

fn label_color(name: &str) -> &'static str {
    let sum: usize = name.bytes().map(usize::from).sum();
    LABEL_COLORS[sum % LABEL_COLORS.len()]
}

const TASK_COLS: &str = "id, issue_key, number, title, description, status, priority, due, \
     reminder, reminder_fired_at, snoozed_until, owner_kind, owner_ref, workspace_id, \
     project_id, project_name, parent_id, subtasks_json, source, source_ref, created_at, \
     updated_at, completed_at";

/// What a save records of a task: the fields people change, by name, as `FieldChange`
/// names them.
fn recorded(task: &TodoItem) -> Vec<(&'static str, Value)> {
    vec![
        ("title", json!(task.title)),
        ("notes", json!(task.description)),
        ("due", json!(task.due_date)),
        ("priority", json!(task.priority)),
        ("status", json!(task.status)),
        ("tags", json!(task.tags)),
        ("project", json!(task.project)),
        ("workspace", json!(task.workspace_id)),
        ("reminder", json!(task.reminder)),
        (
            "subtasks",
            json!(task
                .subtasks
                .iter()
                .map(|s| json!({ "title": s.title, "completed": s.completed }))
                .collect::<Vec<_>>()),
        ),
        ("parent", json!(task.parent_id)),
        ("owner", json!(task.owner)),
    ]
}

fn record(
    tx: &Transaction<'_>,
    task_id: &str,
    actor: &Actor,
    field: &str,
    before: &Value,
    after: &Value,
    at: &str,
) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO task_change (task_id, at, actor, actor_ref, field, before_json, after_json, reason)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            task_id,
            at,
            actor.kind,
            actor.reference,
            field,
            before.to_string(),
            after.to_string(),
            actor.reason
        ],
    )?;
    Ok(())
}

impl TaskDb {
    /// Opens the tasks of `shodh.db` at `path`, applying pending migrations.
    pub fn open(path: &Path, key: Option<&AuditKey>) -> Result<Self, String> {
        Ok(Self {
            conn: open_shared_connection(path, key).map_err(db_error)?,
        })
    }

    /// Every task, in the calendar's order.
    pub fn load(&self) -> Result<Vec<TodoItem>, String> {
        let conn = self.conn.read().map_err(db_error)?;
        let mut labels: HashMap<String, Vec<String>> = HashMap::new();
        {
            let mut stmt = conn
                .prepare(
                    "SELECT l.task_id, d.name FROM task_label l JOIN task_label_def d
                       ON d.id = l.label_id ORDER BY l.task_id, l.position",
                )
                .map_err(db_error)?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .map_err(db_error)?;
            for row in rows {
                let (task, name) = row.map_err(db_error)?;
                labels.entry(task).or_default().push(name);
            }
        }
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {TASK_COLS} FROM task ORDER BY position, created_at, id"
            ))
            .map_err(db_error)?;
        let rows = stmt
            .query_map([], |r| {
                let subtasks: String = r.get(17)?;
                let owner_kind: String = r.get(11)?;
                Ok(TodoItem {
                    id: r.get(0)?,
                    key: Some(format!(
                        "{}-{}",
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?
                    )),
                    title: r.get(3)?,
                    description: r.get(4)?,
                    status: r.get(5)?,
                    priority: r.get(6)?,
                    due_date: r.get(7)?,
                    reminder: r.get(8)?,
                    reminder_fired_at: r.get(9)?,
                    snoozed_until: r.get(10)?,
                    owner: Owner {
                        kind: OwnerKind::parse(&owner_kind).unwrap_or_default(),
                        reference: r.get(12)?,
                    },
                    workspace_id: r.get(13)?,
                    project_id: r.get(14)?,
                    project: r.get(15)?,
                    parent_id: r.get(16)?,
                    subtasks: serde_json::from_str::<Vec<SubTask>>(&subtasks).unwrap_or_default(),
                    source: r.get(18)?,
                    source_ref: r.get(19)?,
                    created_at: r.get(20)?,
                    updated_at: r.get(21)?,
                    completed_at: r.get(22)?,
                    tags: Vec::new(),
                })
            })
            .map_err(db_error)?;
        let mut tasks = Vec::new();
        for row in rows {
            let mut task = row.map_err(db_error)?;
            task.tags = labels.remove(&task.id).unwrap_or_default();
            tasks.push(task);
        }
        Ok(tasks)
    }

    /// Writes what a change did: `before` is the calendar's tasks as loaded, `after` as the
    /// change left them. New tasks are numbered, projects and labels are made as named,
    /// and `after` is given the ids and keys it was stored with. One transaction.
    pub fn save(
        &self,
        before: &[TodoItem],
        after: &mut [TodoItem],
        actor: &Actor,
    ) -> Result<(), String> {
        let at = now();
        self.conn
            .write(|tx| -> rusqlite::Result<()> {
                let kept: HashSet<&str> = after.iter().map(|t| t.id.as_str()).collect();
                for gone in before.iter().filter(|t| !kept.contains(t.id.as_str())) {
                    delete_task(tx, &gone.id)?;
                    record(
                        tx,
                        &gone.id,
                        actor,
                        "deleted",
                        &json!(gone.title),
                        &Value::Null,
                        &at,
                    )?;
                }
                let old: HashMap<&str, (usize, &TodoItem)> = before
                    .iter()
                    .enumerate()
                    .map(|(i, t)| (t.id.as_str(), (i, t)))
                    .collect();
                for (position, task) in after.iter_mut().enumerate() {
                    let previous = old.get(task.id.as_str()).copied();
                    if previous.is_some_and(|(p, t)| p == position && t == &*task) {
                        continue;
                    }
                    store_task(tx, task, position, previous.map(|(_, t)| t), actor, &at)?;
                }
                Ok(())
            })
            .map_err(db_error)
    }

    /// Imports the tasks of the calendar file, keeping their ids and their order after the
    /// tasks already stored. A task stored already keeps the newer of its two versions.
    /// Returns how many were written.
    pub fn import(&self, tasks: &[TodoItem]) -> Result<usize, String> {
        let at = now();
        let actor = Actor::import();
        self.conn
            .write(|tx| -> rusqlite::Result<usize> {
                let mut next: i64 =
                    tx.query_row("SELECT COALESCE(MAX(position) + 1, 0) FROM task", [], |r| {
                        r.get(0)
                    })?;
                let mut written = 0;
                for task in tasks {
                    let stored: Option<(String, i64)> = tx
                        .query_row(
                            "SELECT updated_at, position FROM task WHERE id = ?1",
                            params![task.id],
                            |r| Ok((r.get(0)?, r.get(1)?)),
                        )
                        .optional()?;
                    let mut task = task.clone();
                    task.key = None;
                    task.project_id = None;
                    let position = match &stored {
                        Some((updated, _)) if updated.as_str() >= task.updated_at.as_str() => {
                            continue
                        }
                        Some((_, position)) => *position,
                        None => {
                            let p = next;
                            next += 1;
                            p
                        }
                    };
                    let position = usize::try_from(position).unwrap_or(0);
                    store_task(tx, &mut task, position, None, &actor, &at)?;
                    written += 1;
                }
                Ok(written)
            })
            .map_err(db_error)
    }

    /// The task's recorded changes, oldest first.
    pub fn history(&self, task_id: &str) -> Result<Vec<TaskChange>, String> {
        let conn = self.conn.read().map_err(db_error)?;
        let mut stmt = conn
            .prepare(
                "SELECT at, actor, actor_ref, field, before_json, after_json, reason
                   FROM task_change WHERE task_id = ?1 ORDER BY id",
            )
            .map_err(db_error)?;
        let parse = |text: Option<String>| {
            text.and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or(Value::Null)
        };
        let rows = stmt
            .query_map(params![task_id], |r| {
                Ok(TaskChange {
                    at: r.get(0)?,
                    actor: r.get(1)?,
                    actor_ref: r.get(2)?,
                    field: r.get(3)?,
                    before: parse(r.get(4)?),
                    after: parse(r.get(5)?),
                    reason: r.get(6)?,
                })
            })
            .map_err(db_error)?;
        rows.collect::<Result<_, _>>().map_err(db_error)
    }

    /// The issue key of `workspace_id`'s new tasks (made from its name when it has none).
    pub fn issue_key(&self, workspace_id: Option<&str>) -> Result<String, String> {
        self.conn
            .write(|tx| key_for(tx, workspace_id))
            .map_err(db_error)
    }

    /// Changes the key of `workspace_id`'s new tasks. Tasks numbered already keep their
    /// ids. Refused when another workspace has that key.
    pub fn set_issue_key(&self, workspace_id: &str, key: &str) -> Result<String, String> {
        let key = check_key(key)?;
        let taken: Option<String> = {
            let conn = self.conn.read().map_err(db_error)?;
            conn.query_row(
                "SELECT workspace_id FROM task_issue_key WHERE issue_key = ?1",
                params![key],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_error)?
        };
        if taken.as_deref().is_some_and(|w| w != workspace_id) {
            return Err(format!("Another workspace's tasks are numbered {key}-…"));
        }
        self.conn
            .write(|tx| {
                tx.execute(
                    "INSERT INTO task_issue_key (workspace_id, issue_key) VALUES (?1, ?2)
                     ON CONFLICT (workspace_id) DO UPDATE SET issue_key = ?2",
                    params![workspace_id, key],
                )
            })
            .map_err(db_error)?;
        Ok(key)
    }

    /// Links `task_id` to `target` (`blocks`, `relates`, `duplicate_of`, …). A `blocks`
    /// link that would make a task wait for itself is refused, naming the chain.
    pub fn link(
        &self,
        task_id: &str,
        kind: &str,
        target: &str,
        actor: &Actor,
    ) -> Result<(), String> {
        if task_id == target {
            return Err("A task cannot be linked to itself.".to_string());
        }
        let titles: HashMap<String, String> =
            self.load()?.into_iter().map(|t| (t.id, t.title)).collect();
        let title = |id: &str| titles.get(id).cloned().unwrap_or_else(|| id.to_string());
        if kind == "blocks" {
            if let Some(chain) = self.blocking_chain(target, task_id)? {
                let mut names: Vec<String> = vec![title(task_id)];
                names.extend(chain.iter().map(|id| title(id)));
                return Err(format!(
                    "That would make {} wait for itself ({}).",
                    title(task_id),
                    names.join(" -> ")
                ));
            }
        }
        let at = now();
        self.conn
            .write(|tx| -> rusqlite::Result<()> {
                tx.execute(
                    "INSERT OR IGNORE INTO task_link (task_id, kind, target, created_at, created_by)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![task_id, kind, target, at, actor.kind],
                )?;
                record(
                    tx,
                    task_id,
                    actor,
                    "link",
                    &Value::Null,
                    &json!({ "kind": kind, "target": target }),
                    &at,
                )
            })
            .map_err(db_error)
    }

    /// The tasks `from` blocks, followed until `to` (`from`, …, `to`), if `from` waits on
    /// a chain that reaches `to`.
    fn blocking_chain(&self, from: &str, to: &str) -> Result<Option<Vec<String>>, String> {
        let conn = self.conn.read().map_err(db_error)?;
        let mut stmt = conn
            .prepare("SELECT task_id, target FROM task_link WHERE kind = 'blocks'")
            .map_err(db_error)?;
        let edges: Vec<(String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(db_error)?
            .collect::<Result<_, _>>()
            .map_err(db_error)?;
        let mut stack: Vec<Vec<String>> = vec![vec![from.to_string()]];
        let mut seen: HashSet<String> = HashSet::new();
        while let Some(path) = stack.pop() {
            let last = path.last().cloned().unwrap_or_default();
            if last == to {
                return Ok(Some(path));
            }
            if !seen.insert(last.clone()) {
                continue;
            }
            for (_, next) in edges.iter().filter(|(a, _)| *a == last) {
                let mut longer = path.clone();
                longer.push(next.clone());
                stack.push(longer);
            }
        }
        Ok(None)
    }

    /// Deletes a project. Its tasks stay in their workspace with no project, each with a
    /// recorded change. Returns how many tasks it had.
    pub fn delete_project(&self, project_id: &str, actor: &Actor) -> Result<usize, String> {
        let at = now();
        let actor = actor.clone().because("project deleted");
        self.conn
            .write(|tx| -> rusqlite::Result<usize> {
                let mut stmt =
                    tx.prepare("SELECT id, project_name FROM task WHERE project_id = ?1")?;
                let tasks: Vec<(String, Option<String>)> = stmt
                    .query_map(params![project_id], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<Result<_, _>>()?;
                drop(stmt);
                for (id, name) in &tasks {
                    tx.execute(
                        "UPDATE task SET project_id = NULL, project_name = NULL, updated_at = ?2
                          WHERE id = ?1",
                        params![id, at],
                    )?;
                    record(tx, id, &actor, "project", &json!(name), &Value::Null, &at)?;
                }
                tx.execute(
                    "DELETE FROM task_project WHERE id = ?1",
                    params![project_id],
                )?;
                Ok(tasks.len())
            })
            .map_err(db_error)
    }

    /// Deletes the projects of a deleted workspace (its tasks have left it already).
    pub fn delete_workspace_projects(&self, workspace_id: &str) -> Result<usize, String> {
        self.conn
            .write(|tx| {
                tx.execute(
                    "DELETE FROM task_project WHERE workspace_id = ?1",
                    params![workspace_id],
                )
            })
            .map_err(db_error)
    }
}

/// The key `workspace_id`'s new tasks are numbered under, made and kept on first use.
fn key_for(tx: &Transaction<'_>, workspace_id: Option<&str>) -> rusqlite::Result<String> {
    let Some(workspace_id) = workspace_id else {
        return Ok(NO_WORKSPACE_KEY.to_string());
    };
    let known: Option<String> = tx
        .query_row(
            "SELECT issue_key FROM task_issue_key WHERE workspace_id = ?1",
            params![workspace_id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(key) = known {
        return Ok(key);
    }
    let name: Option<String> = tx
        .query_row(
            "SELECT name FROM workspaces WHERE id = ?1",
            params![workspace_id],
            |r| r.get(0),
        )
        .optional()?;
    let base = key_from_name(name.as_deref().unwrap_or(""));
    let base = if base == NO_WORKSPACE_KEY {
        "WS".to_string()
    } else {
        base
    };
    let mut key = base.clone();
    let mut n = 2;
    loop {
        let taken: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM task_issue_key WHERE issue_key = ?1)",
            params![key],
            |r| r.get(0),
        )?;
        if !taken {
            break;
        }
        key = format!("{}{n}", &base[..base.len().min(MAX_KEY_CHARS - 1)]);
        n += 1;
    }
    tx.execute(
        "INSERT INTO task_issue_key (workspace_id, issue_key) VALUES (?1, ?2)",
        params![workspace_id, key],
    )?;
    Ok(key)
}

/// The next number under `key`: never one given before.
fn next_number(tx: &Transaction<'_>, key: &str) -> rusqlite::Result<i64> {
    let used: i64 = tx.query_row(
        "SELECT COALESCE(MAX(number), 0) FROM task WHERE issue_key = ?1",
        params![key],
        |r| r.get(0),
    )?;
    let counted: Option<i64> = tx
        .query_row(
            "SELECT next FROM issue_counter WHERE issue_key = ?1",
            params![key],
            |r| r.get(0),
        )
        .optional()?;
    let number = counted.unwrap_or(1).max(used + 1);
    tx.execute(
        "INSERT INTO issue_counter (issue_key, next) VALUES (?1, ?2)
         ON CONFLICT (issue_key) DO UPDATE SET next = ?2",
        params![key, number + 1],
    )?;
    Ok(number)
}

/// The project named `name` in `workspace_id`, made when it is first named.
fn project_for(
    tx: &Transaction<'_>,
    workspace_id: &str,
    name: &str,
    actor: &Actor,
    at: &str,
) -> rusqlite::Result<String> {
    let found: Option<String> = tx
        .query_row(
            "SELECT id FROM task_project WHERE workspace_id = ?1 AND name = ?2",
            params![workspace_id, name],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = found {
        return Ok(id);
    }
    let id = format!("proj-{}", uuid::Uuid::new_v4());
    tx.execute(
        "INSERT INTO task_project (id, workspace_id, name, created_by, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
        params![id, workspace_id, name, actor.kind, at],
    )?;
    Ok(id)
}

/// The label named `name`, made with its colour when first used.
fn label_for(tx: &Transaction<'_>, name: &str, at: &str) -> rusqlite::Result<String> {
    let found: Option<String> = tx
        .query_row(
            "SELECT id FROM task_label_def WHERE name = ?1",
            params![name],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = found {
        return Ok(id);
    }
    let id = format!("label-{}", uuid::Uuid::new_v4());
    tx.execute(
        "INSERT INTO task_label_def (id, name, color, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![id, name, label_color(name), at],
    )?;
    Ok(id)
}

fn delete_task(tx: &Transaction<'_>, id: &str) -> rusqlite::Result<()> {
    tx.execute("DELETE FROM task_label WHERE task_id = ?1", params![id])?;
    tx.execute(
        "DELETE FROM task_link WHERE task_id = ?1 OR target = ?1",
        params![id],
    )?;
    tx.execute("DELETE FROM task WHERE id = ?1", params![id])?;
    Ok(())
}

/// Writes `task` at `position` (insert or update), with its project and labels, and
/// records what changed since `previous` (all of it, as created, when there is none).
fn store_task(
    tx: &Transaction<'_>,
    task: &mut TodoItem,
    position: usize,
    previous: Option<&TodoItem>,
    actor: &Actor,
    at: &str,
) -> rusqlite::Result<()> {
    let stored: Option<(String, i64)> = tx
        .query_row(
            "SELECT issue_key, number FROM task WHERE id = ?1",
            params![task.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let (key, number) = match stored {
        Some(found) => found,
        None => {
            let key = key_for(tx, task.workspace_id.as_deref())?;
            let number = next_number(tx, &key)?;
            (key, number)
        }
    };
    let project_id = match (&task.workspace_id, &task.project) {
        (Some(workspace), Some(name)) => Some(project_for(tx, workspace, name, actor, at)?),
        _ => None,
    };
    let subtasks = serde_json::to_string(&task.subtasks)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    let position = i64::try_from(position).unwrap_or(i64::MAX);
    tx.execute(
        &format!(
            "INSERT INTO task (position, {TASK_COLS})
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
                     ?18, ?19, ?20, ?21, ?22, ?23, ?24)
             ON CONFLICT (id) DO UPDATE SET position = ?1, title = ?5, description = ?6,
                 status = ?7, priority = ?8, due = ?9, reminder = ?10, reminder_fired_at = ?11,
                 snoozed_until = ?12, owner_kind = ?13, owner_ref = ?14, workspace_id = ?15,
                 project_id = ?16, project_name = ?17, parent_id = ?18, subtasks_json = ?19,
                 source = ?20, source_ref = ?21, created_at = ?22, updated_at = ?23,
                 completed_at = ?24"
        ),
        params![
            position,
            task.id,
            key,
            number,
            task.title,
            task.description,
            task.status,
            task.priority,
            task.due_date,
            task.reminder,
            task.reminder_fired_at,
            task.snoozed_until,
            task.owner.kind.as_str(),
            task.owner.reference,
            task.workspace_id,
            project_id,
            task.project,
            task.parent_id,
            subtasks,
            task.source,
            task.source_ref,
            task.created_at,
            task.updated_at,
            task.completed_at,
        ],
    )?;
    tx.execute(
        "DELETE FROM task_label WHERE task_id = ?1",
        params![task.id],
    )?;
    let mut placed: HashSet<String> = HashSet::new();
    for (i, name) in task.tags.iter().enumerate() {
        let label = label_for(tx, name, at)?;
        if placed.insert(label.clone()) {
            tx.execute(
                "INSERT INTO task_label (task_id, label_id, position) VALUES (?1, ?2, ?3)",
                params![task.id, label, i64::try_from(i).unwrap_or(i64::MAX)],
            )?;
        }
    }
    task.key = Some(format!("{key}-{number}"));
    task.project_id = project_id;
    match previous {
        None => record(
            tx,
            &task.id,
            actor,
            if actor.kind == "import" {
                "imported"
            } else {
                "created"
            },
            &Value::Null,
            &json!(task.title),
            at,
        )?,
        Some(previous) => {
            for ((field, before), (_, after)) in recorded(previous).into_iter().zip(recorded(task))
            {
                if before != after {
                    record(tx, &task.id, actor, field, &before, &after, at)?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "task_db_tests.rs"]
mod tests;
