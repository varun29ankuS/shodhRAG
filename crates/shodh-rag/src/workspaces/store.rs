//! SQLite storage of workspaces in the shared `shodh.db`.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use chrono::{SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};

use super::templates::{template, BLANK};
use super::{
    check_id, clean_color, clean_description, clean_icon, clean_instructions, clean_name,
    clean_note, normalize_path, AddReport, InstructionVersion, NewSource, NewWorkspace,
    SourceCounts, SourceKind, Workspace, WorkspaceAuthor, WorkspaceDetail, WorkspaceError,
    WorkspacePatch, WorkspaceResult, WorkspaceSource, MAX_LABEL_CHARS, MAX_REF_CHARS,
    MAX_SOURCES_PER_CALL,
};
use crate::audit::{open_shared_connection, AuditKey};

const COLUMNS: &str = "w.id, w.name, w.description, w.icon, w.color, w.template, w.pinned, \
    w.archived, w.created_at, w.updated_at, w.last_active_at, \
    (SELECT IFNULL(MAX(version), 0) FROM workspace_instructions i WHERE i.workspace_id = w.id), \
    (SELECT count(*) FROM workspace_sources s WHERE s.workspace_id = w.id AND s.kind = 'folder'), \
    (SELECT count(*) FROM workspace_sources s WHERE s.workspace_id = w.id AND s.kind = 'file'), \
    (SELECT count(*) FROM workspace_sources s WHERE s.workspace_id = w.id AND s.kind = 'snippet'), \
    (SELECT count(*) FROM workspace_sources s WHERE s.workspace_id = w.id AND s.kind = 'paper')";

/// Workspaces, in `shodh.db`.
///
/// Read-then-write transactions are `IMMEDIATE`: other components commit to the same
/// database, and a deferred transaction upgraded after their commit would fail with
/// `SQLITE_BUSY_SNAPSHOT`, which the busy timeout does not retry.
pub struct WorkspaceStore {
    conn: Mutex<Connection>,
}

impl std::fmt::Debug for WorkspaceStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkspaceStore").finish_non_exhaustive()
    }
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn corrupt(what: &str, value: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("unknown {what} {value:?}"),
        )),
    )
}

fn count(r: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u32> {
    let n: i64 = r.get(index)?;
    Ok(u32::try_from(n).unwrap_or(u32::MAX))
}

fn read_workspace(r: &rusqlite::Row<'_>) -> rusqlite::Result<Workspace> {
    Ok(Workspace {
        id: r.get(0)?,
        name: r.get(1)?,
        description: r.get(2)?,
        icon: r.get(3)?,
        color: r.get(4)?,
        template: r.get(5)?,
        pinned: r.get::<_, i64>(6)? != 0,
        archived: r.get::<_, i64>(7)? != 0,
        created_at: r.get(8)?,
        updated_at: r.get(9)?,
        last_active_at: r.get(10)?,
        instructions_version: count(r, 11)?,
        source_counts: SourceCounts {
            folders: count(r, 12)?,
            files: count(r, 13)?,
            snippets: count(r, 14)?,
            papers: count(r, 15)?,
        },
    })
}

fn read_source(r: &rusqlite::Row<'_>) -> rusqlite::Result<WorkspaceSource> {
    let kind: String = r.get(0)?;
    let added_by: String = r.get(4)?;
    Ok(WorkspaceSource {
        kind: SourceKind::parse(&kind).ok_or_else(|| corrupt("source kind", &kind))?,
        reference: r.get(1)?,
        label: r.get(2)?,
        path: r.get(3)?,
        added_by: WorkspaceAuthor::parse(&added_by).ok_or_else(|| corrupt("author", &added_by))?,
        added_at: r.get(5)?,
    })
}

fn read_version(r: &rusqlite::Row<'_>) -> rusqlite::Result<InstructionVersion> {
    let author: String = r.get(2)?;
    Ok(InstructionVersion {
        version: count(r, 0)?,
        text: r.get(1)?,
        author: WorkspaceAuthor::parse(&author).ok_or_else(|| corrupt("author", &author))?,
        note: r.get(3)?,
        created_at: r.get(4)?,
    })
}

fn workspace_in(conn: &Connection, id: &str) -> WorkspaceResult<Workspace> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM workspaces w WHERE w.id = ?1"),
        params![id],
        read_workspace,
    )
    .optional()?
    .ok_or_else(|| WorkspaceError::NotFound(id.to_string()))
}

fn current_instructions(conn: &Connection, id: &str) -> WorkspaceResult<(u32, String)> {
    Ok(conn
        .query_row(
            "SELECT version, text FROM workspace_instructions WHERE workspace_id = ?1
             ORDER BY version DESC LIMIT 1",
            params![id],
            |r| Ok((count(r, 0)?, r.get::<_, String>(1)?)),
        )
        .optional()?
        .unwrap_or((0, String::new())))
}

/// A source as stored: kind, reference (paths normalised so one file is one source),
/// label and path.
fn clean_source(
    source: &NewSource,
) -> WorkspaceResult<(SourceKind, String, String, Option<String>)> {
    let reference = source.reference.trim();
    if reference.is_empty() || reference.chars().count() > MAX_REF_CHARS {
        return Err(WorkspaceError::Invalid(format!(
            "a source reference must be 1 to {MAX_REF_CHARS} characters"
        )));
    }
    let path = source
        .path
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string);
    if path
        .as_ref()
        .is_some_and(|p| p.chars().count() > MAX_REF_CHARS)
    {
        return Err(WorkspaceError::Invalid("a source path is too long".into()));
    }
    let (reference, path) = match source.kind {
        // A file is identified by its path.
        SourceKind::File => (
            normalize_path(reference),
            Some(path.unwrap_or_else(|| reference.to_string())),
        ),
        SourceKind::Folder => {
            if path.is_none() {
                return Err(WorkspaceError::Invalid(
                    "a folder source needs the folder's path".into(),
                ));
            }
            (reference.to_string(), path)
        }
        SourceKind::Snippet => {
            if path.is_none() {
                return Err(WorkspaceError::Invalid(
                    "a snippet source needs the path of its file".into(),
                ));
            }
            (reference.to_string(), path)
        }
        SourceKind::Paper => (reference.to_string(), path),
    };
    let label: String = source
        .label
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let label = if label.is_empty() {
        reference.clone()
    } else {
        label
    };
    let label: String = label.chars().take(MAX_LABEL_CHARS).collect();
    Ok((source.kind, reference, label, path))
}

fn insert_version(
    tx: &Transaction<'_>,
    id: &str,
    version: u32,
    text: &str,
    author: WorkspaceAuthor,
    note: Option<&str>,
    at: &str,
) -> WorkspaceResult<()> {
    tx.execute(
        "INSERT INTO workspace_instructions(workspace_id, version, text, author, note, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, version, text, author.as_str(), note, at],
    )?;
    Ok(())
}

impl WorkspaceStore {
    /// Opens `shodh.db` at `path` with the audit database key (if the database is
    /// encrypted), applying any pending migrations.
    pub fn open(path: &Path, key: Option<&AuditKey>) -> WorkspaceResult<Self> {
        let conn = open_shared_connection(path, key)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Workspaces, pinned first, then most recently active or changed. Archived ones only
    /// when `include_archived`.
    pub fn list(&self, include_archived: bool) -> WorkspaceResult<Vec<Workspace>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM workspaces w
             WHERE ?1 OR w.archived = 0
             ORDER BY w.pinned DESC, MAX(IFNULL(w.last_active_at, ''), w.updated_at) DESC, w.name"
        ))?;
        let rows = stmt
            .query_map(params![include_archived], read_workspace)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// One workspace.
    pub fn get(&self, id: &str) -> WorkspaceResult<Workspace> {
        let id = check_id(id)?;
        workspace_in(&self.lock(), id)
    }

    /// One workspace with its current instructions and its sources.
    pub fn detail(&self, id: &str) -> WorkspaceResult<WorkspaceDetail> {
        let id = check_id(id)?;
        let conn = self.lock();
        let workspace = workspace_in(&conn, id)?;
        let (_, instructions) = current_instructions(&conn, id)?;
        let mut stmt = conn.prepare(
            "SELECT kind, ref, label, path, added_by, added_at FROM workspace_sources
             WHERE workspace_id = ?1 ORDER BY kind, label COLLATE NOCASE, ref",
        )?;
        let sources = stmt
            .query_map(params![id], read_source)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(WorkspaceDetail {
            workspace,
            instructions,
            sources,
        })
    }

    /// Creates a workspace (with a new id) from `new`, filling icon, colour and
    /// instructions from its template where not given.
    pub fn create(
        &self,
        new: &NewWorkspace,
        author: WorkspaceAuthor,
    ) -> WorkspaceResult<Workspace> {
        let id = format!("ws-{}", uuid::Uuid::new_v4());
        self.create_with_id(&id, new, author)
    }

    /// Creates a workspace with a given id (the import of a legacy space keeps the space
    /// id, so what was scoped to it stays attached). Fails when the id is taken.
    pub fn create_with_id(
        &self,
        id: &str,
        new: &NewWorkspace,
        author: WorkspaceAuthor,
    ) -> WorkspaceResult<Workspace> {
        let id = check_id(id)?;
        let template_id = new
            .template
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .unwrap_or(BLANK);
        let template = template(template_id)
            .ok_or_else(|| WorkspaceError::Invalid(format!("unknown template {template_id:?}")))?;
        let name = clean_name(&new.name)?;
        let description = clean_description(&new.description)?;
        let icon = clean_icon(new.icon.as_deref().unwrap_or(template.icon))?;
        let color = clean_color(new.color.as_deref().unwrap_or(template.color))?;
        let (instructions, instructions_author) = match &new.instructions {
            Some(text) => (clean_instructions(text)?, author),
            None => (
                clean_instructions(template.instructions)?,
                WorkspaceAuthor::Template,
            ),
        };
        let at = now();
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let taken: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM workspaces WHERE id = ?1)",
            params![id],
            |r| r.get(0),
        )?;
        if taken {
            return Err(WorkspaceError::Invalid(format!(
                "a workspace with id {id} already exists"
            )));
        }
        tx.execute(
            "INSERT INTO workspaces(id, name, description, icon, color, template, pinned,
                                    archived, created_at, updated_at, last_active_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, 0, ?7, ?7, NULL)",
            params![id, name, description, icon, color, template.id, at],
        )?;
        if !instructions.is_empty() {
            insert_version(&tx, id, 1, &instructions, instructions_author, None, &at)?;
        }
        tx.commit()?;
        workspace_in(&conn, id)
    }

    /// Changes name, description, icon, colour, pin or archive state.
    pub fn update(&self, id: &str, patch: &WorkspacePatch) -> WorkspaceResult<Workspace> {
        let id = check_id(id)?;
        if patch.is_empty() {
            return Err(WorkspaceError::Invalid("nothing to change".into()));
        }
        let name = patch.name.as_deref().map(clean_name).transpose()?;
        let description = patch
            .description
            .as_deref()
            .map(clean_description)
            .transpose()?;
        let icon = patch.icon.as_deref().map(clean_icon).transpose()?;
        let color = patch.color.as_deref().map(clean_color).transpose()?;
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute(
            "UPDATE workspaces SET
                name = IFNULL(?2, name),
                description = IFNULL(?3, description),
                icon = IFNULL(?4, icon),
                color = IFNULL(?5, color),
                pinned = IFNULL(?6, pinned),
                archived = IFNULL(?7, archived),
                updated_at = ?8
             WHERE id = ?1",
            params![
                id,
                name,
                description,
                icon,
                color,
                patch.pinned.map(i64::from),
                patch.archived.map(i64::from),
                now()
            ],
        )?;
        if changed == 0 {
            return Err(WorkspaceError::NotFound(id.to_string()));
        }
        tx.commit()?;
        workspace_in(&conn, id)
    }

    /// Records new instructions as the next version. `expected_version` (the version the
    /// editor started from) refuses an edit made over a newer one. Returns the version
    /// now current and whether it is new (`false` when the text did not change).
    pub fn set_instructions(
        &self,
        id: &str,
        text: &str,
        author: WorkspaceAuthor,
        note: Option<&str>,
        expected_version: Option<u32>,
    ) -> WorkspaceResult<(InstructionVersion, bool)> {
        let id = check_id(id)?;
        let text = clean_instructions(text)?;
        let note = clean_note(note)?;
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        workspace_in(&tx, id)?;
        let (current, current_text) = current_instructions(&tx, id)?;
        if let Some(expected) = expected_version {
            if expected != current {
                return Err(WorkspaceError::Stale { expected, current });
            }
        }
        if current_text == text {
            let unchanged = tx
                .query_row(
                    "SELECT version, text, author, note, created_at FROM workspace_instructions
                     WHERE workspace_id = ?1 AND version = ?2",
                    params![id, current],
                    read_version,
                )
                .optional()?;
            return match unchanged {
                Some(version) => Ok((version, false)),
                None => Err(WorkspaceError::Invalid(
                    "the workspace has no instructions yet; write some first".into(),
                )),
            };
        }
        let at = now();
        let version = current + 1;
        insert_version(&tx, id, version, &text, author, note.as_deref(), &at)?;
        tx.execute(
            "UPDATE workspaces SET updated_at = ?2 WHERE id = ?1",
            params![id, at],
        )?;
        tx.commit()?;
        Ok((
            InstructionVersion {
                version,
                text,
                author,
                note,
                created_at: at,
            },
            true,
        ))
    }

    /// The current instructions and their version (0 and empty when none).
    pub fn instructions(&self, id: &str) -> WorkspaceResult<(u32, String)> {
        let id = check_id(id)?;
        let conn = self.lock();
        workspace_in(&conn, id)?;
        current_instructions(&conn, id)
    }

    /// Every instruction version, newest first.
    pub fn instruction_history(&self, id: &str) -> WorkspaceResult<Vec<InstructionVersion>> {
        let id = check_id(id)?;
        let conn = self.lock();
        workspace_in(&conn, id)?;
        let mut stmt = conn.prepare(
            "SELECT version, text, author, note, created_at FROM workspace_instructions
             WHERE workspace_id = ?1 ORDER BY version DESC",
        )?;
        let rows = stmt
            .query_map(params![id], read_version)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Adds sources; ones the workspace already has are counted, not duplicated.
    pub fn add_sources(
        &self,
        id: &str,
        sources: &[NewSource],
        added_by: WorkspaceAuthor,
    ) -> WorkspaceResult<AddReport> {
        let id = check_id(id)?;
        if sources.is_empty() {
            return Err(WorkspaceError::Invalid("no sources to add".into()));
        }
        if sources.len() > MAX_SOURCES_PER_CALL {
            return Err(WorkspaceError::Invalid(format!(
                "{} sources in one call; add at most {MAX_SOURCES_PER_CALL} at a time",
                sources.len()
            )));
        }
        let cleaned = sources
            .iter()
            .map(clean_source)
            .collect::<WorkspaceResult<Vec<_>>>()?;
        let at = now();
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        workspace_in(&tx, id)?;
        let mut report = AddReport::default();
        for (kind, reference, label, path) in cleaned {
            let inserted = tx.execute(
                "INSERT OR IGNORE INTO workspace_sources
                    (workspace_id, kind, ref, label, path, added_by, added_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    id,
                    kind.as_str(),
                    reference,
                    label,
                    path,
                    added_by.as_str(),
                    at
                ],
            )?;
            if inserted == 1 {
                report.added += 1;
            } else {
                report.already += 1;
            }
        }
        if report.added > 0 {
            tx.execute(
                "UPDATE workspaces SET updated_at = ?2 WHERE id = ?1",
                params![id, at],
            )?;
        }
        tx.commit()?;
        Ok(report)
    }

    /// Removes one source. Returns whether the workspace had it.
    pub fn remove_source(
        &self,
        id: &str,
        kind: SourceKind,
        reference: &str,
    ) -> WorkspaceResult<bool> {
        let id = check_id(id)?;
        let reference = match kind {
            SourceKind::File => normalize_path(reference),
            _ => reference.trim().to_string(),
        };
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        workspace_in(&tx, id)?;
        let removed = tx.execute(
            "DELETE FROM workspace_sources WHERE workspace_id = ?1 AND kind = ?2 AND ref = ?3",
            params![id, kind.as_str(), reference],
        )?;
        if removed > 0 {
            tx.execute(
                "UPDATE workspaces SET updated_at = ?2 WHERE id = ?1",
                params![id, now()],
            )?;
        }
        tx.commit()?;
        Ok(removed > 0)
    }

    /// Records that a chat of the workspace asked something now.
    pub fn touch(&self, id: &str) -> WorkspaceResult<()> {
        let id = check_id(id)?;
        let changed = self.lock().execute(
            "UPDATE workspaces SET last_active_at = ?2 WHERE id = ?1",
            params![id, now()],
        )?;
        if changed == 0 {
            return Err(WorkspaceError::NotFound(id.to_string()));
        }
        Ok(())
    }

    /// Deletes a workspace, its instruction history and its source list (never the
    /// sources themselves). Returns whether it existed.
    pub fn delete(&self, id: &str) -> WorkspaceResult<bool> {
        let id = check_id(id)?;
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "DELETE FROM workspace_sources WHERE workspace_id = ?1",
            params![id],
        )?;
        tx.execute(
            "DELETE FROM workspace_instructions WHERE workspace_id = ?1",
            params![id],
        )?;
        let removed = tx.execute("DELETE FROM workspaces WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(removed > 0)
    }

    /// Ids of workspaces that hold `kind` `reference` (e.g. a folder about to be removed).
    pub fn holding(&self, kind: SourceKind, reference: &str) -> WorkspaceResult<Vec<String>> {
        let reference = match kind {
            SourceKind::File => normalize_path(reference),
            _ => reference.trim().to_string(),
        };
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT workspace_id FROM workspace_sources WHERE kind = ?1 AND ref = ?2
             ORDER BY workspace_id",
        )?;
        let rows = stmt
            .query_map(params![kind.as_str(), reference], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        Ok(rows)
    }

    /// A bookkeeping value (e.g. whether the legacy import ran).
    pub fn state(&self, key: &str) -> WorkspaceResult<Option<String>> {
        Ok(self
            .lock()
            .query_row(
                "SELECT value FROM workspace_state WHERE key = ?1",
                params![key],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Sets a bookkeeping value.
    pub fn set_state(&self, key: &str, value: &str) -> WorkspaceResult<()> {
        self.lock().execute(
            "INSERT INTO workspace_state(key, value, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![key, value, now()],
        )?;
        Ok(())
    }
}
