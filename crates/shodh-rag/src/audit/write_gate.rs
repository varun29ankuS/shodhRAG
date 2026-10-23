//! One in-process write turn per database file.
//!
//! Several components keep their own connection to `shodh.db` (the audit writer, the
//! statement dynamics, the inboxes, the research and evidence tables, the visuals, the
//! workspaces and the model catalog cache). SQLite lets one connection write at a time,
//! and a connection that finds the write lock taken does not queue: its busy handler
//! sleeps and tries again until its busy timeout runs out. A thread that commits back to
//! back can therefore keep another writer of the same process out until that writer
//! fails with `SQLITE_BUSY`.
//!
//! Every connection opened by [`super::open_shared_connection`] and the audit writer take
//! a turn on the database's [`WriteGate`] around each write transaction. Turns are served
//! in the order they were asked for (a ticket lock), so writers of one process wait for
//! each other in order and never poll. The busy timeout is left to cover other processes.
//!
//! Rules that keep the gate free of deadlocks:
//! - A turn lasts for one synchronous closure ([`SharedConnection::write`]): it cannot be
//!   held across an `.await` and cannot outlive the transaction.
//! - A thread holds at most one turn. Asking for a second one (a write started inside a
//!   write) is a bug: debug builds panic, release builds return an error instead of
//!   waiting forever. Blocking on the audit writer inside a write is refused the same way.
//! - Reads never take a turn. In debug builds a shared connection is `query_only` outside
//!   its turns, so a write that skips the gate fails in tests.

use std::cell::Cell;
use std::collections::HashMap;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, LazyLock, Mutex, MutexGuard, Weak};

use rusqlite::{ffi, Connection, Transaction, TransactionBehavior};

/// The ticket lock behind the turns on one database file.
#[derive(Default)]
pub(crate) struct WriteGate {
    tickets: Mutex<Tickets>,
    turn: Condvar,
}

#[derive(Default)]
struct Tickets {
    /// The ticket the next writer to ask gets.
    next: u64,
    /// The ticket whose writer has the turn.
    serving: u64,
}

/// Gates by canonical database path. Entries are weak: a gate lives as long as a
/// connection to its file does.
static GATES: LazyLock<Mutex<HashMap<PathBuf, Weak<WriteGate>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

thread_local! {
    /// Whether this thread holds a turn on any gate.
    static HOLDING: Cell<bool> = const { Cell::new(false) };
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Whether the calling thread is inside a write turn.
pub(crate) fn holding_write_turn() -> bool {
    HOLDING.with(Cell::get)
}

/// The error for a misuse that would otherwise deadlock: a panic in debug builds, an
/// `SQLITE_MISUSE` error in release builds.
pub(crate) fn misuse(what: &str) -> rusqlite::Error {
    if cfg!(debug_assertions) {
        panic!("{what}");
    }
    rusqlite::Error::SqliteFailure(ffi::Error::new(ffi::SQLITE_MISUSE), Some(what.to_string()))
}

const NESTED_WRITE: &str =
    "a write to shodh.db was started inside another write on the same thread";

impl WriteGate {
    /// The gate of the database file at `path`, shared by every handle to that file in
    /// this process. Call it once the connection is open, so the file exists and resolves
    /// to its canonical path.
    pub(crate) fn for_database(path: &Path) -> std::io::Result<Arc<Self>> {
        let key = std::fs::canonicalize(path)?;
        let mut gates = lock(&GATES);
        if let Some(gate) = gates.get(&key).and_then(Weak::upgrade) {
            return Ok(gate);
        }
        gates.retain(|_, gate| gate.strong_count() > 0);
        let gate = Arc::new(Self::default());
        gates.insert(key, Arc::downgrade(&gate));
        Ok(gate)
    }

    /// Wait for this thread's turn, in the order turns were asked for.
    fn enter(&self) -> rusqlite::Result<Turn<'_>> {
        if holding_write_turn() {
            return Err(misuse(NESTED_WRITE));
        }
        let mut tickets = lock(&self.tickets);
        let mine = tickets.next;
        tickets.next = tickets.next.wrapping_add(1);
        while tickets.serving != mine {
            tickets = self.turn.wait(tickets).unwrap_or_else(|e| e.into_inner());
        }
        drop(tickets);
        HOLDING.with(|h| h.set(true));
        Ok(Turn { gate: self })
    }

    /// Turns asked for and not yet finished, the current one included.
    #[cfg(test)]
    pub(crate) fn pending(&self) -> u64 {
        let tickets = lock(&self.tickets);
        tickets.next.wrapping_sub(tickets.serving)
    }
}

/// A held turn; dropping it (also while unwinding) passes the turn to the next ticket.
struct Turn<'g> {
    gate: &'g WriteGate,
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        HOLDING.with(|h| h.set(false));
        let mut tickets = lock(&self.gate.tickets);
        tickets.serving = tickets.serving.wrapping_add(1);
        drop(tickets);
        self.gate.turn.notify_all();
    }
}

/// In debug builds, makes `conn` refuse writes outside a turn.
pub(crate) fn guard_reads(conn: &Connection) -> rusqlite::Result<()> {
    if cfg!(debug_assertions) {
        conn.pragma_update(None, "query_only", true)?;
    }
    Ok(())
}

/// Lifts `query_only` for one turn in debug builds and restores it on drop.
struct Writable<'c>(&'c Connection);

impl<'c> Writable<'c> {
    fn new(conn: &'c Connection) -> rusqlite::Result<Self> {
        if cfg!(debug_assertions) {
            conn.pragma_update(None, "query_only", false)?;
        }
        Ok(Self(conn))
    }
}

impl Drop for Writable<'_> {
    fn drop(&mut self) {
        if let Err(e) = guard_reads(self.0) {
            tracing::error!(target: "shodh::audit", error = %e, "could not make a shodh.db connection read-only again");
        }
    }
}

/// Runs `f` in one `IMMEDIATE` transaction on `conn` during a turn on `gate`, and commits
/// it. An error from `f` rolls the transaction back. The turn ends when this returns.
pub(crate) fn write_in_turn<T, E>(
    conn: &Connection,
    gate: &WriteGate,
    f: impl FnOnce(&Transaction<'_>) -> Result<T, E>,
) -> Result<T, E>
where
    E: From<rusqlite::Error>,
{
    // Dropped in reverse: the transaction (rolled back unless committed), then
    // `query_only`, then the turn.
    let _turn = gate.enter()?;
    let _writable = Writable::new(conn)?;
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let value = f(&tx)?;
    tx.commit()?;
    Ok(value)
}

/// A component's connection to `shodh.db`: reads through [`read`](Self::read), writes
/// through [`write`](Self::write), which takes the database's write turn.
pub struct SharedConnection {
    conn: Mutex<Connection>,
    gate: Arc<WriteGate>,
}

impl std::fmt::Debug for SharedConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedConnection").finish_non_exhaustive()
    }
}

impl SharedConnection {
    pub(crate) fn new(conn: Connection, gate: Arc<WriteGate>) -> rusqlite::Result<Self> {
        guard_reads(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            gate,
        })
    }

    /// The connection, for reads. It takes no turn; in debug builds any write through
    /// it fails (`query_only`).
    pub fn read(&self) -> ReadConnection<'_> {
        if cfg!(debug_assertions) && holding_write_turn() {
            panic!("shodh.db was read through another connection inside a write");
        }
        ReadConnection(lock(&self.conn))
    }

    /// Runs `f` in one `IMMEDIATE` transaction during this database's write turn and
    /// commits it; an error from `f` rolls it back. `f` must only use the transaction it
    /// is given: no other store, no blocking audit call, no `.await`.
    pub fn write<T, E>(&self, f: impl FnOnce(&Transaction<'_>) -> Result<T, E>) -> Result<T, E>
    where
        E: From<rusqlite::Error>,
    {
        let conn = lock(&self.conn);
        write_in_turn(&conn, &self.gate, f)
    }

    #[cfg(test)]
    pub(crate) fn gate(&self) -> &Arc<WriteGate> {
        &self.gate
    }
}

/// A shared connection held for reads.
pub struct ReadConnection<'a>(MutexGuard<'a, Connection>);

impl Deref for ReadConnection<'_> {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::sync::Barrier;
    use std::thread;
    use std::time::Duration;

    use rusqlite::ErrorCode;
    use serde_json::json;

    use super::*;
    use crate::audit::{open_shared_connection, AuditEventType, AuditLog, AuditRecord};

    fn shared(path: &Path) -> SharedConnection {
        open_shared_connection(path, None).unwrap()
    }

    /// `conn` gives up at once when SQLite reports the write lock taken, so a write of it
    /// that meets another writer of the process inside SQLite fails instead of waiting.
    fn impatient(conn: &SharedConnection) {
        conn.read().busy_timeout(Duration::ZERO).unwrap();
    }

    fn put(conn: &SharedConnection, key: &str) -> rusqlite::Result<usize> {
        conn.write(|tx| {
            tx.execute(
                "INSERT INTO audit_settings(key, value) VALUES (?1, 'x')",
                [key],
            )
        })
    }

    /// Keys of the `audit_settings` rows the tests wrote, in commit order.
    fn keys_in_order(conn: &SharedConnection) -> Vec<String> {
        let conn = conn.read();
        let mut stmt = conn
            .prepare("SELECT key FROM audit_settings WHERE key LIKE 't-%' ORDER BY rowid")
            .unwrap();
        let keys = stmt
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<Vec<String>, _>>()
            .unwrap();
        keys
    }

    /// Spins until `n` turns on `gate` are held or queued. No sleeping: the count is exact
    /// and only grows while the caller holds the current turn.
    fn wait_for_pending(gate: &WriteGate, n: u64) {
        while gate.pending() < n {
            thread::yield_now();
        }
    }

    #[test]
    fn handles_to_one_file_share_a_gate_and_other_files_do_not() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shodh.db");
        let a = shared(&path);
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let b = shared(&dir.path().join("sub").join("..").join("shodh.db"));
        assert!(Arc::ptr_eq(a.gate(), b.gate()));
        assert!(Arc::ptr_eq(
            a.gate(),
            &WriteGate::for_database(&path).unwrap()
        ));

        let other_dir = tempfile::tempdir().unwrap();
        let other = shared(&other_dir.path().join("shodh.db"));
        assert!(!Arc::ptr_eq(a.gate(), other.gate()));

        // A turn held on one file does not hold up a write to another.
        let (entered, holding) = mpsc::channel();
        let (release, released) = mpsc::channel::<()>();
        thread::scope(|s| {
            let a = &a;
            s.spawn(move || {
                a.write(|tx| {
                    tx.execute(
                        "INSERT INTO audit_settings(key, value) VALUES ('t-a', 'x')",
                        [],
                    )?;
                    entered.send(()).unwrap();
                    released.recv().unwrap();
                    Ok::<_, rusqlite::Error>(())
                })
                .unwrap();
            });
            holding.recv().unwrap();
            assert_eq!(put(&other, "t-other").unwrap(), 1);
            release.send(()).unwrap();
        });
        assert_eq!(keys_in_order(&b), ["t-a"]);
    }

    #[test]
    fn without_the_gate_a_second_writer_of_the_process_is_refused_busy() {
        // What the gate removes: two connections of one process, the second with no busy
        // budget left, meet inside SQLite and the second fails with SQLITE_BUSY.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shodh.db");
        drop(shared(&path));
        let holder = Connection::open(&path).unwrap();
        let victim = Connection::open(&path).unwrap();
        victim.busy_timeout(Duration::ZERO).unwrap();
        holder.execute_batch("BEGIN IMMEDIATE").unwrap();
        let refused = victim
            .execute(
                "INSERT INTO audit_settings(key, value) VALUES ('t-v', 'x')",
                [],
            )
            .unwrap_err();
        assert_eq!(refused.sqlite_error_code(), Some(ErrorCode::DatabaseBusy));
        holder.execute_batch("COMMIT").unwrap();
    }

    #[test]
    fn with_the_gate_a_second_writer_waits_for_its_turn_instead_of_failing_busy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shodh.db");
        let holder = shared(&path);
        let victim = shared(&path);
        impatient(&victim);
        let gate = Arc::clone(holder.gate());
        let (entered, holding) = mpsc::channel();
        thread::scope(|s| {
            let (holder, gate) = (&holder, &gate);
            s.spawn(move || {
                holder
                    .write(|tx| {
                        tx.execute(
                            "INSERT INTO audit_settings(key, value) VALUES ('t-h', 'x')",
                            [],
                        )?;
                        entered.send(()).unwrap();
                        // Commit only once the victim is queued behind this turn, so its
                        // write is asked for while SQLite's write lock is held here.
                        wait_for_pending(gate, 2);
                        Ok::<_, rusqlite::Error>(())
                    })
                    .unwrap();
            });
            holding.recv().unwrap();
            assert_eq!(put(&victim, "t-v").unwrap(), 1);
        });
        assert_eq!(keys_in_order(&victim), ["t-h", "t-v"]);
    }

    #[test]
    fn a_writer_that_asks_again_at_once_goes_behind_the_one_waiting() {
        // The starvation case: a thread that commits and at once writes again must not
        // overtake a writer that was already waiting.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shodh.db");
        let busy = shared(&path);
        let waiting = shared(&path);
        let gate = Arc::clone(busy.gate());
        let (entered, holding) = mpsc::channel();
        thread::scope(|s| {
            let (busy, gate) = (&busy, &gate);
            s.spawn(move || {
                busy.write(|tx| {
                    tx.execute(
                        "INSERT INTO audit_settings(key, value) VALUES ('t-1', 'x')",
                        [],
                    )?;
                    entered.send(()).unwrap();
                    wait_for_pending(gate, 2);
                    Ok::<_, rusqlite::Error>(())
                })
                .unwrap();
                put(busy, "t-3").unwrap();
            });
            holding.recv().unwrap();
            put(&waiting, "t-2").unwrap();
        });
        assert_eq!(keys_in_order(&busy), ["t-1", "t-2", "t-3"]);
    }

    #[test]
    fn back_to_back_commits_and_the_audit_writer_never_starve_a_writer_without_busy_budget() {
        // The shape of the CI failure: one thread commits fsynced transactions back to back
        // while the audit writer and another component write the same file. The victim has
        // no busy budget at all, so it only succeeds because writers of the process never
        // meet inside SQLite.
        const HAMMER: usize = 200;
        const VICTIM: usize = 40;
        const EVENTS: usize = 40;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shodh.db");
        let log = AuditLog::open(&path, None).unwrap();
        let hammer = shared(&path);
        let victim = shared(&path);
        impatient(&victim);
        let start = Barrier::new(3);
        thread::scope(|s| {
            s.spawn(|| {
                start.wait();
                for i in 0..HAMMER {
                    put(&hammer, &format!("t-hammer-{i}")).unwrap();
                }
            });
            s.spawn(|| {
                start.wait();
                for i in 0..EVENTS {
                    log.submit(AuditRecord::new(
                        AuditEventType::Question,
                        json!({ "text": format!("question {i}") }),
                    ));
                }
            });
            start.wait();
            for i in 0..VICTIM {
                if let Err(e) = put(&victim, &format!("t-victim-{i}")) {
                    panic!("write {i} of the victim failed: {e}");
                }
            }
        });
        // A waited-for append is a barrier: the writer has handled every earlier submit.
        log.append(AuditRecord::new(AuditEventType::Question, json!({})))
            .unwrap();
        assert_eq!(log.unrecorded(), None);
        assert_eq!(keys_in_order(&victim).len(), HAMMER + VICTIM);
        let events: i64 = victim
            .read()
            .query_row("SELECT count(*) FROM audit_events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(events, EVENTS as i64 + 1);
    }

    #[test]
    fn a_failed_or_panicking_write_rolls_back_and_passes_the_turn_on() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shodh.db");
        let conn = shared(&path);
        let failed = conn.write(|tx| {
            tx.execute(
                "INSERT INTO audit_settings(key, value) VALUES ('t-gone', 'x')",
                [],
            )?;
            Err::<(), _>(rusqlite::Error::InvalidQuery)
        });
        assert!(matches!(failed, Err(rusqlite::Error::InvalidQuery)));
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            conn.write(|tx| -> rusqlite::Result<()> {
                tx.execute(
                    "INSERT INTO audit_settings(key, value) VALUES ('t-gone-too', 'x')",
                    [],
                )?;
                panic!("the closure panics mid-write");
            })
        }));
        assert!(panicked.is_err());
        assert!(!holding_write_turn());
        assert_eq!(conn.gate().pending(), 0);
        // Another handle gets the next turn at once, and neither write was kept.
        assert_eq!(put(&shared(&path), "t-after").unwrap(), 1);
        assert_eq!(keys_in_order(&conn), ["t-after"]);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn a_write_outside_a_turn_is_refused_in_debug_builds() {
        let dir = tempfile::tempdir().unwrap();
        let conn = shared(&dir.path().join("shodh.db"));
        let refused = conn
            .read()
            .execute(
                "INSERT INTO audit_settings(key, value) VALUES ('t-x', 'x')",
                [],
            )
            .unwrap_err();
        assert_eq!(refused.sqlite_error_code(), Some(ErrorCode::ReadOnly));
        // A turn lifts it for its own transaction only.
        assert_eq!(put(&conn, "t-y").unwrap(), 1);
        let after = conn
            .read()
            .execute("DELETE FROM audit_settings WHERE key = 't-y'", [])
            .unwrap_err();
        assert_eq!(after.sqlite_error_code(), Some(ErrorCode::ReadOnly));
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "inside another write")]
    fn a_write_inside_a_write_panics_instead_of_deadlocking() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shodh.db");
        let outer = shared(&path);
        let inner = shared(&path);
        let _ = outer.write(|_| put(&inner, "t-nested"));
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "read through another connection inside a write")]
    fn a_read_of_another_connection_inside_a_write_panics_instead_of_deadlocking() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shodh.db");
        let outer = shared(&path);
        let other = shared(&path);
        let _ = outer.write(|_| {
            other
                .read()
                .query_row("SELECT count(*) FROM audit_settings", [], |r| {
                    r.get::<_, i64>(0)
                })
        });
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "use AuditLog::submit there")]
    fn awaiting_the_audit_writer_inside_a_write_panics_instead_of_deadlocking() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shodh.db");
        let log = AuditLog::open(&path, None).unwrap();
        let conn = shared(&path);
        let _ = conn.write(|_| {
            log.append(AuditRecord::new(AuditEventType::Question, json!({})))
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
        });
    }

    #[test]
    fn a_queued_audit_event_inside_a_write_is_recorded_after_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shodh.db");
        let log = AuditLog::open(&path, None).unwrap();
        let conn = shared(&path);
        conn.write(|tx| {
            tx.execute(
                "INSERT INTO audit_settings(key, value) VALUES ('t-w', 'x')",
                [],
            )?;
            log.submit(AuditRecord::new(AuditEventType::Question, json!({})));
            Ok::<_, rusqlite::Error>(())
        })
        .unwrap();
        assert_eq!(
            log.append(AuditRecord::new(AuditEventType::Question, json!({})))
                .unwrap(),
            2
        );
        assert_eq!(log.unrecorded(), None);
        assert_eq!(keys_in_order(&conn), ["t-w"]);
    }
}
