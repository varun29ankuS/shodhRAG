//! `sandbox-probe <case>`: one thing a command might do, for the tests of
//! Code mode's command runner (`crates/shodh-rag/src/harness/run_command/tests.rs`).
//! The tests run each case through the runner and, as a control, directly,
//! so a case that is stopped inside the runner is shown to work outside it.
//!
//! Cases:
//! - `env`: print every environment variable, `NAME=value` per line.
//! - `heartbeat FILE`: append a line to FILE every 100 ms, forever.
//! - `detach FILE`: start `heartbeat FILE` so that it outlives this process
//!   (Windows: breakaway from the job is tried first, and the result
//!   printed; elsewhere: a new session, which leaves the process group),
//!   print its pid (also written to FILE.pid) and exit.
//! - `hold`: sleep forever.
//! - `busy`: spin on the CPU forever.
//! - `alloc MB [SECS]`: allocate and touch MB megabytes, printing how far it
//!   got (exit 3 when an allocation is refused), then hold for SECS seconds.
//! - `children N`: start N `hold` children, print their pids, then hold.
//! - `fork-loop N`: start up to N `hold` children, stop at the first refusal,
//!   print how many started and their pids, and exit.

use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// Bytes allocated at a time by `alloc`.
const CHUNK_BYTES: usize = 16 * 1024 * 1024;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("env") => env(),
        Some("heartbeat") => heartbeat(text(&args, 1)),
        Some("detach") => detach(text(&args, 1)),
        Some("hold") => hold(),
        Some("busy") => busy(),
        Some("alloc") => alloc(
            number(&args, 1),
            args.get(2).map_or(0, |_| number(&args, 2)),
        ),
        Some("children") => children(number(&args, 1)),
        Some("fork-loop") => fork_loop(number(&args, 1)),
        _ => {
            eprintln!(
                "usage: sandbox-probe env | heartbeat FILE | detach FILE | hold | busy | \
                 alloc MB [SECS] | children N | fork-loop N"
            );
            2
        }
    };
    let _ = std::io::stdout().flush();
    std::process::exit(code);
}

fn text(args: &[String], index: usize) -> &str {
    match args.get(index) {
        Some(value) => value,
        None => {
            eprintln!("missing argument {index}");
            std::process::exit(2);
        }
    }
}

fn number(args: &[String], index: usize) -> u64 {
    match text(args, index).parse() {
        Ok(value) => value,
        Err(e) => {
            eprintln!("argument {index} is not a number: {e}");
            std::process::exit(2);
        }
    }
}

fn env() -> i32 {
    let mut out = std::io::stdout().lock();
    for (name, value) in std::env::vars_os() {
        let _ = writeln!(
            out,
            "{}={}",
            name.to_string_lossy(),
            value.to_string_lossy()
        );
    }
    0
}

fn heartbeat(file: &str) -> i32 {
    loop {
        let beat = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(file)
            .and_then(|mut f| f.write_all(b"beat\n"));
        if let Err(e) = beat {
            eprintln!("heartbeat: {e}");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// This program, to start again with other arguments.
fn this() -> std::path::PathBuf {
    match std::env::current_exe() {
        Ok(path) => path,
        Err(e) => {
            eprintln!("current_exe: {e}");
            std::process::exit(2);
        }
    }
}

fn quiet(command: &mut Command) -> &mut Command {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
}

#[cfg(windows)]
fn spawn_detached(file: &str) -> std::io::Result<Child> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    let spawn = |flags: u32| {
        let mut command = Command::new(this());
        quiet(command.args(["heartbeat", file]))
            .creation_flags(flags)
            .spawn()
    };
    match spawn(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB) {
        Ok(child) => {
            println!("breakaway allowed");
            Ok(child)
        }
        Err(e) => {
            println!("breakaway refused: {e}");
            spawn(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        }
    }
}

#[cfg(unix)]
fn spawn_detached(file: &str) -> std::io::Result<Child> {
    use std::os::unix::process::CommandExt;
    let mut command = Command::new(this());
    quiet(command.args(["heartbeat", file]));
    // SAFETY: setsid is async-signal-safe.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command.spawn()
}

fn detach(file: &str) -> i32 {
    match spawn_detached(file) {
        Ok(child) => {
            println!("pid {}", child.id());
            // Also in FILE.pid, for a caller that cannot read this output to
            // its end (on Windows the child inherits the output pipe).
            if let Err(e) = std::fs::write(format!("{file}.pid"), child.id().to_string()) {
                println!("pid file not written: {e}");
                return 1;
            }
            0
        }
        Err(e) => {
            println!("detach failed: {e}");
            1
        }
    }
}

fn hold() -> i32 {
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn busy() -> i32 {
    let mut n: u64 = 0;
    loop {
        n = std::hint::black_box(n.wrapping_mul(6364136223846793005).wrapping_add(1));
    }
}

fn alloc(megabytes: u64, hold_secs: u64) -> i32 {
    let mut blocks: Vec<Vec<u8>> = Vec::new();
    let mut done: u64 = 0;
    while done < megabytes {
        let mut block: Vec<u8> = Vec::new();
        if block.try_reserve_exact(CHUNK_BYTES).is_err() {
            println!("allocation failed at {done} MB");
            return 3;
        }
        // Written, so every page is really used.
        block.resize(CHUNK_BYTES, 1);
        blocks.push(block);
        done += (CHUNK_BYTES / (1024 * 1024)) as u64;
    }
    println!("allocated {done} MB");
    let _ = std::io::stdout().flush();
    std::thread::sleep(Duration::from_secs(hold_secs));
    std::hint::black_box(&blocks);
    0
}

fn spawn_hold() -> std::io::Result<Child> {
    let mut command = Command::new(this());
    quiet(command.arg("hold")).spawn()
}

fn children(count: u64) -> i32 {
    for _ in 0..count {
        match spawn_hold() {
            Ok(child) => println!("pid {}", child.id()),
            Err(e) => {
                println!("spawn failed: {e}");
                return 1;
            }
        }
    }
    let _ = std::io::stdout().flush();
    hold()
}

fn fork_loop(limit: u64) -> i32 {
    let mut pids = Vec::new();
    for _ in 0..limit {
        match spawn_hold() {
            Ok(child) => pids.push(child.id()),
            Err(e) => {
                println!("refused after {}: {e}", pids.len());
                break;
            }
        }
    }
    println!("spawned {}", pids.len());
    let pids: Vec<String> = pids.iter().map(u32::to_string).collect();
    println!("pids {}", pids.join(" "));
    0
}
