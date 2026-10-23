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
//! Rules that keep the gate free of deadlocks, checked in every build. Breaking one returns
//! an `SQLITE_MISUSE` error at once instead of waiting forever:
//! - A turn lasts for one synchronous closure ([`SharedConnection::write`]): it cannot be
//!   held across an `.await` and cannot outlive the transaction.
//! - A thread uses one shared connection at a time. Inside a write it may not write or read
//!   through another handle (or the same one); while it holds a read guard it may not
//!   write or take another read guard. Nor may it wait for the audit writer in either case,
//!   since the writer needs a turn to answer.
//! - A writer takes the turn before its connection's mutex. Readers never take a turn, so a
//!   writer waiting for its turn does not hold up readers of its store, and the thread that
//!   holds the turn only ever waits for a mutex held by a reader, which finishes without
//!   waiting for anything.
//!
//! Outside its turns a shared connection is `query_only`, so a write that skips the gate
//! fails instead of racing the other writers.
//!
//! Another process holding the database's write lock is covered by the busy timeout. When
//! the writer holding the turn runs out of it, the writers already queued behind it fail at
//! once with the same `SQLITE_BUSY` instead of each waiting out the timeout in turn. That
//! covers writers already queued at that moment. A thread that writes again afterwards (the
//! audit writer working through its queue one event at a time) asks for a new turn and
//! waits out the timeout once more, so N queued audit events against a lock another
//! process keeps take N busy timeouts.

use std::cell::Cell;
use std::collections::HashMap;
use std::io;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex, MutexGuard, Weak};

use rusqlite::{ffi, Connection, ErrorCode, Transaction, TransactionBehavior};

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
    /// Tickets below this one were queued when a turn ran out of busy timeout against
    /// another process; they fail at once when their turn comes.
    busy_before: u64,
}

/// Gates by database key ([`gate_key`]). Entries are weak: a gate lives as long as a
/// connection to its file does.
static GATES: LazyLock<Mutex<HashMap<PathBuf, Weak<WriteGate>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Whether the canonical-path fallback has been logged.
static FALLBACK_LOGGED: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// Whether this thread holds a turn on any gate.
    static HOLDING: Cell<bool> = const { Cell::new(false) };
    /// Read guards ([`ReadConnection`]) alive on this thread.
    static READING: Cell<u32> = const { Cell::new(0) };
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn holding_write_turn() -> bool {
    HOLDING.with(Cell::get)
}

fn reading() -> bool {
    READING.with(Cell::get) > 0
}

const NESTED_WRITE: &str =
    "a write to shodh.db was started inside another write on the same thread";
const WRITE_WHILE_READING: &str =
    "a write to shodh.db was started while the same thread holds a read of it";
const READ_IN_WRITE: &str = "shodh.db was read through a shared connection inside a write";
const NESTED_READ: &str =
    "shodh.db was read through a shared connection while the same thread holds another read";
const AWAIT_IN_DATABASE_USE: &str = "an audit write was awaited inside a read or write of \
     shodh.db; the audit writer needs a write turn to answer (use AuditLog::submit there)";
const OTHER_PROCESS_BUSY: &str = "shodh.db is locked by another process: the write queued \
     ahead of this one waited out the busy timeout";

/// A use of the gate that would deadlock if it went ahead: logged and returned as an
/// `SQLITE_MISUSE` error.
fn misuse(what: &str) -> rusqlite::Error {
    tracing::error!(target: "shodh::audit", "{what}");
    rusqlite::Error::SqliteFailure(ffi::Error::new(ffi::SQLITE_MISUSE), Some(what.to_string()))
}

fn may_write() -> rusqlite::Result<()> {
    if holding_write_turn() {
        return Err(misuse(NESTED_WRITE));
    }
    if reading() {
        return Err(misuse(WRITE_WHILE_READING));
    }
    Ok(())
}

fn may_read() -> rusqlite::Result<()> {
    if holding_write_turn() {
        return Err(misuse(READ_IN_WRITE));
    }
    if reading() {
        return Err(misuse(NESTED_READ));
    }
    Ok(())
}

/// Refuses to wait for the audit writer on a thread that holds a turn or a read guard: the
/// writer needs a turn, which may wait for that turn or for the guard's connection.
pub(crate) fn may_wait_for_audit_writer() -> rusqlite::Result<()> {
    if holding_write_turn() || reading() {
        return Err(misuse(AWAIT_IN_DATABASE_USE));
    }
    Ok(())
}

fn is_busy(error: &rusqlite::Error) -> bool {
    matches!(
        error.sqlite_error_code(),
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
    )
}

/// The registry key of the database file at `path`: its canonical path, so every handle
/// to one file shares a gate. Called once the connection is open, so the file exists.
fn gate_key(path: &Path) -> PathBuf {
    key_from(path, std::fs::canonicalize(path))
}

/// [`gate_key`] given the result of canonicalizing `path`. When the volume cannot report
/// a canonical path, the absolute path is used (logged once). Every caller goes through
/// this one function, and canonicalizing one path on one volume either works or fails for
/// every handle, so two handles to the same file get the same kind of key.
fn key_from(path: &Path, canonical: io::Result<PathBuf>) -> PathBuf {
    match canonical {
        Ok(canonical) => canonical,
        Err(e) => {
            if !FALLBACK_LOGGED.swap(true, Ordering::Relaxed) {
                tracing::warn!(target: "shodh::audit", path = %path.display(), error = %e, "shodh.db has no canonical path; its write gate is keyed by the absolute path");
            }
            std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
        }
    }
}

impl WriteGate {
    /// The gate of the database file at `path`, shared by every handle to that file in
    /// this process.
    pub(crate) fn for_database(path: &Path) -> Arc<Self> {
        let key = gate_key(path);
        let mut gates = lock(&GATES);
        if let Some(gate) = gates.get(&key).and_then(Weak::upgrade) {
            return gate;
        }
        gates.retain(|_, gate| gate.strong_count() > 0);
        let gate = Arc::new(Self::default());
        gates.insert(key, Arc::downgrade(&gate));
        gate
    }

    /// Wait for this thread's turn, in the order turns were asked for.
    fn enter(&self) -> rusqlite::Result<Turn<'_>> {
        may_write()?;
        let mut tickets = lock(&self.tickets);
        let mine = tickets.next;
        tickets.next += 1;
        while tickets.serving != mine {
            tickets = self.turn.wait(tickets).unwrap_or_else(|e| e.into_inner());
        }
        if mine < tickets.busy_before {
            tickets.serving += 1;
            drop(tickets);
            self.turn.notify_all();
            return Err(rusqlite::Error::SqliteFailure(
                ffi::Error::new(ffi::SQLITE_BUSY),
                Some(OTHER_PROCESS_BUSY.to_string()),
            ));
        }
        drop(tickets);
        HOLDING.with(|h| h.set(true));
        Ok(Turn { gate: self })
    }

    /// Turns asked for and not yet finished, the current one included.
    #[cfg(test)]
    pub(crate) fn pending(&self) -> u64 {
        let tickets = lock(&self.tickets);
        tickets.next - tickets.serving
    }
}

/// A held turn; dropping it (also while unwinding) passes the turn to the next ticket.
struct Turn<'g> {
    gate: &'g WriteGate,
}

impl Turn<'_> {
    /// Passes `error` through. When it is a busy error, another process held the write lock
    /// past this turn's busy timeout, so the turns already queued fail at once.
    fn note(&self, error: rusqlite::Error) -> rusqlite::Error {
        if is_busy(&error) {
            let mut tickets = lock(&self.gate.tickets);
            tickets.busy_before = tickets.next;
        }
        error
    }
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        HOLDING.with(|h| h.set(false));
        let mut tickets = lock(&self.gate.tickets);
        tickets.serving += 1;
        drop(tickets);
        self.gate.turn.notify_all();
    }
}

/// Makes `conn` refuse writes outside a turn.
pub(crate) fn guard_reads(conn: &Connection) -> rusqlite::Result<()> {
    conn.pragma_update(None, "query_only", true)
}

/// Lifts `query_only` for one turn and restores it on drop.
struct Writable<'c>(&'c Connection);

impl<'c> Writable<'c> {
    fn new(conn: &'c Connection) -> rusqlite::Result<Self> {
        conn.pragma_update(None, "query_only", false)?;
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

/// Runs `f` in one `IMMEDIATE` transaction on `conn` during `turn`, and commits it. An
/// error from `f` rolls the transaction back.
fn run_in_turn<T, E>(
    conn: &Connection,
    turn: Turn<'_>,
    f: impl FnOnce(&Transaction<'_>) -> Result<T, E>,
) -> Result<T, E>
where
    E: From<rusqlite::Error>,
{
    // Locals drop before `turn`: the transaction (rolled back unless committed), then
    // `query_only`, then the turn.
    let _writable = Writable::new(conn)?;
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
        .map_err(|e| turn.note(e))?;
    let value = f(&tx)?;
    tx.commit().map_err(|e| turn.note(e))?;
    Ok(value)
}

/// Runs `f` in one `IMMEDIATE` transaction on `conn` during a turn on `gate`, and commits
/// it. For a connection this thread owns outright (the audit writer's); shared ones go
/// through [`SharedConnection::write`].
pub(crate) fn write_in_turn<T, E>(
    conn: &Connection,
    gate: &WriteGate,
    f: impl FnOnce(&Transaction<'_>) -> Result<T, E>,
) -> Result<T, E>
where
    E: From<rusqlite::Error>,
{
    let turn = gate.enter()?;
    run_in_turn(conn, turn, f)
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

    /// The connection, for reads. It takes no turn, and any write through it fails
    /// (`query_only`). Refused inside a write and while this thread holds another read.
    pub fn read(&self) -> rusqlite::Result<ReadConnection<'_>> {
        may_read()?;
        let guard = lock(&self.conn);
        READING.with(|r| r.set(r.get() + 1));
        Ok(ReadConnection(guard))
    }

    /// Runs `f` in one `IMMEDIATE` transaction during this database's write turn and
    /// commits it; an error from `f` rolls it back. `f` must only use the transaction it
    /// is given: no other store, no blocking audit call, no `.await`. Refused inside
    /// another write and while this thread holds a read guard.
    pub fn write<T, E>(&self, f: impl FnOnce(&Transaction<'_>) -> Result<T, E>) -> Result<T, E>
    where
        E: From<rusqlite::Error>,
    {
        // The turn comes first and the connection second: a writer waiting for its turn
        // does not hold this store's readers up.
        let turn = self.gate.enter()?;
        let conn = lock(&self.conn);
        run_in_turn(&conn, turn, f)
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

impl Drop for ReadConnection<'_> {
    fn drop(&mut self) {
        READING.with(|r| r.set(r.get().saturating_sub(1)));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc;
    use std::sync::Barrier;
    use std::thread;
    use std::time::Duration;

    use serde_json::json;

    use super::*;
    use crate::audit::{open_shared_connection, AuditError, AuditEventType, AuditLog, AuditRecord};

    fn shared(path: &Path) -> SharedConnection {
        open_shared_connection(path, None).unwrap()
    }

    /// `conn` gives up at once when SQLite reports the write lock taken, so a write of it
    /// that meets another writer of the process inside SQLite fails instead of waiting.
    fn impatient(conn: &SharedConnection) {
        conn.read().unwrap().busy_timeout(Duration::ZERO).unwrap();
    }

    fn put(conn: &SharedConnection, key: &str) -> rusqlite::Result<usize> {
        conn.write(|tx| {
            tx.execute(
                "INSERT INTO audit_settings(key, value) VALUES (?1, 'x')",
                [key],
            )
        })
    }

    fn record() -> AuditRecord {
        AuditRecord::new(AuditEventType::Question, json!({}))
    }

    /// Keys of the `audit_settings` rows the tests wrote, in commit order.
    fn keys_in_order(conn: &SharedConnection) -> Vec<String> {
        let conn = conn.read().unwrap();
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
    /// and only grows while the current turn is held.
    fn wait_for_pending(gate: &WriteGate, n: u64) {
        while gate.pending() < n {
            thread::yield_now();
        }
    }

    fn assert_misuse(result: rusqlite::Result<impl std::fmt::Debug>, what: &str) {
        match result {
            Err(e) => {
                assert_eq!(e.sqlite_error_code(), Some(ErrorCode::ApiMisuse), "{e}");
                assert!(e.to_string().contains(what), "{e}");
            }
            Ok(v) => panic!("expected the misuse error {what:?}, got {v:?}"),
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
        assert!(Arc::ptr_eq(a.gate(), &WriteGate::for_database(&path)));

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
    fn a_path_without_a_canonical_form_is_keyed_by_its_absolute_path() {
        let path = Path::new("data").join("sub").join("..").join("shodh.db");
        let failed = key_from(&path, Err(io::Error::other("no final path on this volume")));
        assert!(failed.is_absolute(), "{}", failed.display());
        assert_eq!(failed, std::path::absolute(&path).unwrap());
        // The same input always gives the same key.
        assert_eq!(
            failed,
            key_from(&path, Err(io::Error::other("no final path on this volume")))
        );
        let canonical = std::env::temp_dir().join("shodh.db");
        assert_eq!(key_from(&path, Ok(canonical.clone())), canonical);
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
        log.append(record()).unwrap();
        assert_eq!(log.unrecorded(), None);
        assert_eq!(keys_in_order(&victim).len(), HAMMER + VICTIM);
        let events: i64 = victim
            .read()
            .unwrap()
            .query_row("SELECT count(*) FROM audit_events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(events, EVENTS as i64 + 1);
    }

    #[test]
    fn a_reader_of_a_store_is_not_held_up_by_its_writer_waiting_for_the_turn() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shodh.db");
        let holder = shared(&path);
        let store = shared(&path);
        let gate = Arc::clone(holder.gate());
        let (entered, holding) = mpsc::channel();
        let (release, released) = mpsc::channel::<()>();
        thread::scope(|s| {
            let (holder, store) = (&holder, &store);
            s.spawn(move || {
                holder
                    .write(|tx| {
                        tx.execute(
                            "INSERT INTO audit_settings(key, value) VALUES ('t-h', 'x')",
                            [],
                        )?;
                        entered.send(()).unwrap();
                        released.recv().unwrap();
                        Ok::<_, rusqlite::Error>(())
                    })
                    .unwrap();
            });
            holding.recv().unwrap();
            let writer = s.spawn(move || put(store, "t-s"));
            wait_for_pending(&gate, 2);
            // The store's writer is queued; its readers still get the connection.
            let rows: i64 = store
                .read()
                .unwrap()
                .query_row(
                    "SELECT count(*) FROM audit_settings WHERE key LIKE 't-%'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(rows, 0);
            release.send(()).unwrap();
            assert_eq!(writer.join().unwrap().unwrap(), 1);
        });
        assert_eq!(keys_in_order(&store), ["t-h", "t-s"]);
    }

    static RELEASE_HOLDER: AtomicBool = AtomicBool::new(false);
    static QUEUED_BUSY_WAITS: AtomicUsize = AtomicUsize::new(0);

    /// The turn holder's busy handler: keeps waiting until the test lets it give up.
    fn hold_until_released(_: i32) -> bool {
        while !RELEASE_HOLDER.load(Ordering::SeqCst) {
            thread::yield_now();
        }
        false
    }

    /// The queued writers' busy handler: counts any wait inside SQLite.
    fn count_queued_wait(_: i32) -> bool {
        QUEUED_BUSY_WAITS.fetch_add(1, Ordering::SeqCst);
        false
    }

    #[test]
    fn writers_queued_behind_a_turn_that_another_process_kept_out_fail_busy_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shodh.db");
        let holder = shared(&path);
        let queued: Vec<SharedConnection> = (0..2).map(|_| shared(&path)).collect();
        holder
            .read()
            .unwrap()
            .busy_handler(Some(hold_until_released))
            .unwrap();
        for conn in &queued {
            conn.read()
                .unwrap()
                .busy_handler(Some(count_queued_wait))
                .unwrap();
        }
        let gate = Arc::clone(holder.gate());
        // Another process holds the write lock.
        let other_process = Connection::open(&path).unwrap();
        other_process.execute_batch("BEGIN IMMEDIATE").unwrap();
        thread::scope(|s| {
            let held = s.spawn(|| put(&holder, "t-held"));
            wait_for_pending(&gate, 1);
            let waiting: Vec<_> = queued
                .iter()
                .enumerate()
                .map(|(i, conn)| s.spawn(move || put(conn, &format!("t-queued-{i}"))))
                .collect();
            wait_for_pending(&gate, 3);
            RELEASE_HOLDER.store(true, Ordering::SeqCst);
            let held = held.join().unwrap().unwrap_err();
            assert_eq!(held.sqlite_error_code(), Some(ErrorCode::DatabaseBusy));
            for waiter in waiting {
                let error = waiter.join().unwrap().unwrap_err();
                assert_eq!(error.sqlite_error_code(), Some(ErrorCode::DatabaseBusy));
                assert!(error.to_string().contains("another process"), "{error}");
            }
        });
        // They gave up without waiting inside SQLite at all.
        assert_eq!(QUEUED_BUSY_WAITS.load(Ordering::SeqCst), 0);
        // Writers that ask after that are not affected.
        other_process.execute_batch("COMMIT").unwrap();
        let late = shared(&path);
        assert_eq!(put(&late, "t-late").unwrap(), 1);
        assert_eq!(keys_in_order(&late), ["t-late"]);
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

    #[test]
    fn a_write_outside_a_turn_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let conn = shared(&dir.path().join("shodh.db"));
        let refused = conn
            .read()
            .unwrap()
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
            .unwrap()
            .execute("DELETE FROM audit_settings WHERE key = 't-y'", [])
            .unwrap_err();
        assert_eq!(after.sqlite_error_code(), Some(ErrorCode::ReadOnly));
    }

    #[test]
    fn a_write_inside_a_write_through_another_handle_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shodh.db");
        let outer = shared(&path);
        let inner = shared(&path);
        let nested = outer.write(|_| Ok::<_, rusqlite::Error>(put(&inner, "t-nested")));
        assert_misuse(nested.unwrap(), NESTED_WRITE);
        assert_eq!(put(&inner, "t-after").unwrap(), 1);
        assert_eq!(keys_in_order(&inner), ["t-after"]);
    }

    #[test]
    fn a_write_inside_a_write_on_the_same_handle_is_refused_not_deadlocked() {
        let dir = tempfile::tempdir().unwrap();
        let conn = shared(&dir.path().join("shodh.db"));
        let nested = conn.write(|_| Ok::<_, rusqlite::Error>(put(&conn, "t-nested")));
        assert_misuse(nested.unwrap(), NESTED_WRITE);
        assert_eq!(put(&conn, "t-after").unwrap(), 1);
    }

    #[test]
    fn a_read_inside_a_write_is_refused_on_any_handle() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shodh.db");
        let conn = shared(&path);
        let other = shared(&path);
        let (same, another) = conn
            .write(|_| {
                Ok::<_, rusqlite::Error>((conn.read().map(|_| ()), other.read().map(|_| ())))
            })
            .unwrap();
        assert_misuse(same, READ_IN_WRITE);
        assert_misuse(another, READ_IN_WRITE);
    }

    #[test]
    fn a_write_or_second_read_while_holding_a_read_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shodh.db");
        let conn = shared(&path);
        let other = shared(&path);
        {
            let _reading = conn.read().unwrap();
            assert_misuse(put(&other, "t-w"), WRITE_WHILE_READING);
            assert_misuse(put(&conn, "t-w"), WRITE_WHILE_READING);
            assert_misuse(other.read().map(|_| ()), NESTED_READ);
        }
        // Once the guard is gone the same thread reads and writes again.
        assert_eq!(put(&conn, "t-after").unwrap(), 1);
        assert!(other.read().is_ok());
    }

    #[test]
    fn waiting_for_the_audit_writer_inside_a_read_or_write_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shodh.db");
        let log = AuditLog::open(&path, None).unwrap();
        let conn = shared(&path);
        let in_write = conn
            .write(|_| Ok::<_, rusqlite::Error>(log.append(record())))
            .unwrap();
        let in_read = {
            let _reading = conn.read().unwrap();
            log.append(record())
        };
        for refused in [in_write, in_read] {
            match refused {
                Err(AuditError::Sqlite(e)) => assert_misuse(Err::<(), _>(e), AWAIT_IN_DATABASE_USE),
                other => panic!("expected a refusal, got {other:?}"),
            }
        }
        assert_eq!(log.append(record()).unwrap(), 1);
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
            log.submit(record());
            Ok::<_, rusqlite::Error>(())
        })
        .unwrap();
        assert_eq!(log.append(record()).unwrap(), 2);
        assert_eq!(log.unrecorded(), None);
        assert_eq!(keys_in_order(&conn), ["t-w"]);
    }
}
