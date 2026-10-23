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
