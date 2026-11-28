use std::path::Path;
use std::sync::Arc;

use super::*;
use crate::calendar_store::{CalendarData, CalendarStore, NewTask, TaskPatch, PRE_TASKS_FILE};
use shodh_rag::workspaces::{NewWorkspace, WorkspaceAuthor, WorkspaceStore};

/// The calendar file of an older version: every kind of task it held.
const BEFORE: &str = include_str!("../tests/fixtures/calendar_data_before_tasks_db.json");

struct Env {
    dir: tempfile::TempDir,
    db: Arc<TaskDb>,
}

impl Env {
    fn store(&self) -> CalendarStore {
        CalendarStore::with_tasks(self.dir.path(), self.db.clone())
    }
    fn file(&self) -> std::path::PathBuf {
        self.dir.path().join(crate::calendar_store::CALENDAR_FILE)
    }
}

fn database(dir: &Path) -> Arc<TaskDb> {
    let path = dir.join("shodh.db");
    let workspaces = WorkspaceStore::open(&path, None).unwrap();
    workspaces
        .create_with_id(
            "ws-thesis",
            &NewWorkspace {
                name: "Thesis".into(),
                description: String::new(),
                template: None,
                icon: None,
                color: None,
                instructions: None,
            },
            WorkspaceAuthor::User,
        )
        .unwrap();
    Arc::new(TaskDb::open(&path, None).unwrap())
}

/// A profile with the older version's calendar file and the app database.
fn env_with_file() -> Env {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(crate::calendar_store::CALENDAR_FILE),
        BEFORE,
    )
    .unwrap();
    let db = database(dir.path());
    Env { dir, db }
}

fn without_stored_ids(mut tasks: Vec<TodoItem>) -> Vec<TodoItem> {
    for task in &mut tasks {
        task.key = None;
        task.project_id = None;
    }
    tasks
}

#[test]
fn every_task_moves_to_the_database_with_the_same_meaning() {
    let e = env_with_file();
    let old: CalendarData = serde_json::from_str(BEFORE).unwrap();
    let data = e.store().load().unwrap();
    // Every field of every task, in their order: ids, tags (now labels), checklists,
    // reminders fired and snoozed, a repeat's copy, an old status and an unreadable
    // reminder kept as written, a project name without a workspace kept as the name.
    assert_eq!(without_stored_ids(data.tasks.clone()), old.tasks);
    assert_eq!(data.events, old.events, "events stay");
    // Numbered under their workspace's key, in their order.
    let keys: Vec<&str> = data.tasks.iter().filter_map(|t| t.key.as_deref()).collect();
    assert_eq!(keys, ["THE-1", "TASK-1", "THE-2", "TASK-2"]);
    // The workspace's project name is now its project.
    assert!(data.tasks[0].project_id.is_some());
    assert_eq!(
        data.tasks[1].project_id, None,
        "no workspace, no project entity"
    );
    // The tasks left the file only once stored; the original is kept.
    let left: CalendarData =
        serde_json::from_str(&std::fs::read_to_string(e.file()).unwrap()).unwrap();
    assert!(left.tasks.is_empty());
    assert_eq!(left.events, old.events);
    assert_eq!(
        std::fs::read_to_string(e.dir.path().join(PRE_TASKS_FILE)).unwrap(),
        BEFORE
    );
    // Each import is in the task's history.
    let history = e.db.history(&old.tasks[0].id).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].actor, "import");
    assert_eq!(history[0].field, "imported");
    // Read again: the same tasks, the same keys.
    let again = e.store().load().unwrap();
    assert_eq!(again.tasks, data.tasks);
}

#[test]
fn a_crash_between_the_import_and_the_file_rewrite_makes_no_second_copy() {
    let e = env_with_file();
    let old: CalendarData = serde_json::from_str(BEFORE).unwrap();
    // The import committed, then the app stopped before the file was rewritten.
    e.db.import(&old.tasks).unwrap();
    let data = e.store().load().unwrap();
    assert_eq!(data.tasks.len(), old.tasks.len(), "no duplicates");
    assert_eq!(without_stored_ids(data.tasks), old.tasks);
    let left: CalendarData =
        serde_json::from_str(&std::fs::read_to_string(e.file()).unwrap()).unwrap();
    assert!(left.tasks.is_empty(), "the rewrite is finished");
}

#[test]
fn a_task_edited_in_the_file_meanwhile_keeps_its_newer_version() {
    let e = env_with_file();
    let mut old: CalendarData = serde_json::from_str(BEFORE).unwrap();
    e.db.import(&old.tasks).unwrap();
    // The database could not be opened for a while: the task was changed in the file.
    old.tasks[1].title = "Pay rent and deposit".into();
    old.tasks[1].updated_at = "2026-11-25T08:00:00+00:00".into();
    std::fs::write(e.file(), serde_json::to_string(&old).unwrap()).unwrap();
    let data = e.store().load().unwrap();
    assert_eq!(data.tasks[1].title, "Pay rent and deposit");
    assert_eq!(data.tasks[1].key.as_deref(), Some("TASK-1"), "the same id");
}

#[test]
fn a_failed_import_leaves_the_file_as_it_was_and_the_tasks_in_it() {
    let e = env_with_file();
    let conn = rusqlite::Connection::open(e.dir.path().join("shodh.db")).unwrap();
    conn.execute_batch("DROP TABLE task;").unwrap();
    drop(conn);
    let old: CalendarData = serde_json::from_str(BEFORE).unwrap();
    let data = e.store().load().unwrap();
    assert_eq!(data.tasks, old.tasks, "read from the file");
    assert_eq!(std::fs::read_to_string(e.file()).unwrap(), BEFORE);
}

#[test]
fn every_change_is_recorded_with_who_made_it() {
    let e = env_with_file();
    let store = e.store();
    let id = store
        .update_as(&Actor::agent(Some("conversation:c-7".into())), |d| {
            d.insert_task(NewTask {
                title: "Draft related work".into(),
                workspace_id: Some("ws-thesis".into()),
                ..NewTask::default()
            })
            .map(|t| t.id)
        })
        .unwrap();
    store
        .update(|d| {
            d.patch_task(
                &id,
                &TaskPatch {
                    priority: Some("urgent".into()),
                    status: Some("canceled".into()),
                    ..TaskPatch::default()
                },
            )
        })
        .unwrap();
    let task = store.load().unwrap().task(&id).unwrap().clone();
    assert_eq!(task.key.as_deref(), Some("THE-3"));
    assert!(task.completed_at.is_some(), "canceled is closed");
    let history = e.db.history(&id).unwrap();
    let fields: Vec<(&str, &str)> = history
        .iter()
        .map(|c| (c.actor.as_str(), c.field.as_str()))
        .collect();
    assert_eq!(
        fields,
        [
            ("agent", "created"),
            ("user", "priority"),
            ("user", "status")
        ]
    );
    assert_eq!(history[0].actor_ref.as_deref(), Some("conversation:c-7"));
    assert_eq!(
        history[1].before,
        json!("none"),
        "new tasks start at No priority"
    );
    assert_eq!(history[1].after, json!("urgent"));
}

#[test]
fn numbers_are_never_given_twice_and_a_new_key_keeps_old_ids() {
    let e = env_with_file();
    let store = e.store();
    let data = store.load().unwrap();
    let last = data.tasks[2].id.clone();
    assert_eq!(data.tasks[2].key.as_deref(), Some("THE-2"));
    store.update(|d| d.delete_task(&last).map(|_| ())).unwrap();
    let next = store
        .update(|d| {
            d.insert_task(NewTask {
                title: "Plot results".into(),
                workspace_id: Some("ws-thesis".into()),
                ..NewTask::default()
            })
        })
        .unwrap();
    assert_eq!(
        store.load().unwrap().task(&next.id).unwrap().key.as_deref(),
        Some("THE-3"),
        "THE-2 was deleted, and is not given again"
    );
    assert_eq!(e.db.set_issue_key("ws-thesis", "thz").unwrap(), "THZ");
    let new = store
        .update(|d| {
            d.insert_task(NewTask {
                title: "Write up".into(),
                workspace_id: Some("ws-thesis".into()),
                ..NewTask::default()
            })
        })
        .unwrap();
    let data = store.load().unwrap();
    assert_eq!(data.task(&new.id).unwrap().key.as_deref(), Some("THZ-1"));
    assert_eq!(data.tasks[0].key.as_deref(), Some("THE-1"), "old ids stay");
    assert!(
        e.db.set_issue_key("ws-other", "THZ").is_err(),
        "one workspace per key"
    );
    assert!(check_key("1AB").is_err());
    assert!(check_key("TASK").is_err());
    assert_eq!(key_from_name("Reading group"), "RG");
    assert_eq!(key_from_name("Thesis"), "THE");
    assert_eq!(key_from_name("—"), NO_WORKSPACE_KEY);
}

#[test]
fn a_blocking_cycle_is_refused_naming_it() {
    let e = env_with_file();
    let data = e.store().load().unwrap();
    let (a, b) = (data.tasks[0].id.clone(), data.tasks[1].id.clone());
    e.db.link(&a, "blocks", &b, &Actor::user()).unwrap();
    let refused = e.db.link(&b, "blocks", &a, &Actor::user()).unwrap_err();
    assert_eq!(
        refused,
        "That would make Pay rent wait for itself (Pay rent -> Run baselines -> Pay rent)."
    );
    e.db.link(&b, "relates", &a, &Actor::user()).unwrap();
}

#[test]
fn deleting_a_project_keeps_its_tasks_and_says_why() {
    let e = env_with_file();
    let data = e.store().load().unwrap();
    let project = data.tasks[0].project_id.clone().unwrap();
    assert_eq!(e.db.delete_project(&project, &Actor::user()).unwrap(), 1);
    let after = e.store().load().unwrap();
    assert_eq!(after.tasks.len(), data.tasks.len(), "every task stays");
    assert_eq!(after.tasks[0].project, None);
    assert_eq!(after.tasks[0].workspace_id.as_deref(), Some("ws-thesis"));
    let last = e.db.history(&data.tasks[0].id).unwrap().pop().unwrap();
    assert_eq!(last.reason.as_deref(), Some("project deleted"));
}

#[test]
fn a_sub_issue_stays_in_its_parents_workspace_and_never_under_itself() {
    let e = env_with_file();
    let store = e.store();
    let data = store.load().unwrap();
    let (parent, child, elsewhere) = (
        data.tasks[0].id.clone(),
        data.tasks[2].id.clone(),
        data.tasks[1].id.clone(),
    );
    let sub = |parent: &str| TaskPatch {
        parent_id: Some(Some(parent.to_string())),
        ..TaskPatch::default()
    };
    store
        .update(|d| d.patch_task(&child, &sub(&parent)))
        .unwrap();
    let cycle = store
        .update(|d| d.patch_task(&parent, &sub(&child)))
        .unwrap_err();
    assert_eq!(
        cycle.to_string(),
        "That would make Run baselines a sub-issue of itself."
    );
    let across = store
        .update(|d| d.patch_task(&elsewhere, &sub(&parent)))
        .unwrap_err();
    assert!(across.to_string().contains("in its parent's workspace"));
    // Moved out of the workspace, it leaves its parent and its project behind.
    store
        .update(|d| {
            d.patch_task(
                &parent,
                &TaskPatch {
                    workspace_id: Some(None),
                    ..TaskPatch::default()
                },
            )
        })
        .unwrap();
    let after = store.load().unwrap();
    assert_eq!(after.task(&parent).unwrap().project, None);
    assert_eq!(after.task(&child).unwrap().parent_id, None);
}
