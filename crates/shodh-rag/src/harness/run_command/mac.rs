//! macOS: `/bin/bash` in a new session and process group, with a CPU-time
//! limit and a memory watchdog.
//!
//! - `setsid` before `exec` makes the command the leader of its own session
//!   and process group; everything it starts stays in that group unless it
//!   leaves it.
//! - `RLIMIT_CPU`, inherited by every process it starts: a process over it
//!   gets `SIGXCPU`, then `SIGKILL`. CPU time counts every thread, so the
//!   limit is the time limit on every core ([`cpu_limit_secs`]): a process
//!   that keeps all cores busy until the wall-clock limit stays under it. It
//!   bounds a process that escaped the end-of-command sweep; the wall clock
//!   ends everything else.
//! - Every process of the run carries the run's marker in its environment
//!   ([`super::RUN_ID_ENV`]), so one that left the group and the tree (a
//!   `setsid` and double fork, reparented to launchd) is still found.
//! - The watchdog sums the physical footprint of the run's processes every
//!   [`WATCH_INTERVAL`]; over the memory limit, the run is killed.
//! - When the command ends, the run's processes (its group, every process of
//!   the user carrying its marker, and all their descendants) are stopped
//!   (`SIGSTOP`, so none can fork meanwhile), then killed, pass after pass
//!   until a pass finds none.
//!
//! A process that left the group and also dropped the marker (re-executed
//! with an empty environment) is not found; step 1b's Seatbelt profile
//! bounds what it can do.

use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use libc::{c_int, pid_t, uid_t};
use tokio::process::Child;

use super::{CommandSpec, Killer, RUN_ID_ENV};

/// The shell commands run in.
const BASH: &str = "/bin/bash";

/// CPU time a process may use beyond its share of the wall-clock limit.
const CPU_MARGIN_SECS: u64 = 5;

/// The `RLIMIT_CPU` of a command with a wall-clock limit of `time_secs` on a
/// machine with `cores` cores: CPU time adds up over threads, so a process
/// may use every core for the whole time limit (a parallel `cargo test` or
/// BLAS) without reaching it.
pub fn cpu_limit_secs(time_secs: u64, cores: u64) -> u64 {
    time_secs
        .saturating_mul(cores.max(1))
        .saturating_add(CPU_MARGIN_SECS)
}

/// How often the watchdog adds up the run's memory.
const WATCH_INTERVAL: Duration = Duration::from_millis(250);

/// Most passes of the final sweep; each one stops and kills every process of
/// the run it finds.
const MAX_SWEEPS: usize = 50;

/// A zombie (`SZOMB`, sys/proc.h): it has ended and only waits to be reaped.
const ZOMBIE: u32 = 5;

/// `/bin/bash`, or why commands cannot run.
pub fn locate_shell() -> Result<PathBuf, String> {
    let bash = PathBuf::from(BASH);
    if bash.is_file() {
        Ok(bash)
    } else {
        Err(format!(
            "Commands run in {BASH}, which was not found on this Mac."
        ))
    }
}

/// Make `dir` readable, writable and enterable by its owner only (0700).
pub fn make_private(dir: &Path) -> Result<(), String> {
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| format!("{} could not be made private: {e}", dir.display()))
}

/// The limit that ended a process killed by `signal`, in words.
pub fn signal_exit(signal: i32) -> Option<&'static str> {
    (signal == libc::SIGXCPU).then_some("the CPU time limit")
}

/// Start `spec`'s command in `shell`, in its own session and process group
/// with its CPU-time limit.
pub fn spawn_contained(
    shell: &Path,
    spec: &CommandSpec,
    env: &[(String, String)],
    command_id: &str,
) -> Result<(Child, Contained), String> {
    let cores = std::thread::available_parallelism()
        .map(|n| u64::try_from(n.get()).unwrap_or(u64::MAX))
        .unwrap_or(1);
    let cpu_secs = cpu_limit_secs(spec.limits.time.as_secs(), cores);
    // rlim_t is u64 on macOS.
    let soft: libc::rlim_t = cpu_secs;
    let hard = soft.saturating_add(1);
    let mut command = tokio::process::Command::new(shell);
    command
        .arg("-c")
        .arg(&spec.command)
        .current_dir(&spec.folder)
        .env_clear()
        .envs(
            env.iter()
                .map(|(name, value)| (name.as_str(), value.as_str())),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // SAFETY: the closure runs in the child between fork and exec and calls
    // only async-signal-safe functions (setsid, setrlimit).
    unsafe {
        command.pre_exec(move || {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            let limit = libc::rlimit {
                rlim_cur: soft,
                rlim_max: hard,
            };
            if libc::setrlimit(libc::RLIMIT_CPU, &limit) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command
        .spawn()
        .map_err(|e| format!("Shodh could not start {}: {e}", shell.display()))?;
    let Some(pid) = child.id().and_then(|pid| pid_t::try_from(pid).ok()) else {
        return Err("the command's process was not found".to_string());
    };
    Ok((
        child,
        Contained {
            target: Arc::new(Target {
                group: pid,
                marker: format!("{RUN_ID_ENV}={command_id}").into_bytes(),
                // SAFETY: getuid cannot fail.
                uid: unsafe { libc::getuid() },
            }),
            memory_bytes: spec.limits.memory_bytes,
            memory_mb: spec.limits.memory_mb(),
            last_watch: Instant::now(),
            buffer: Vec::new(),
            swept: false,
        },
    ))
}

/// How the processes of one run are found.
#[derive(Debug)]
struct Target {
    /// The run's process group (the command's own pid).
    group: pid_t,
    /// `SHODH_RUN_COMMAND_ID=<id>`, as it appears in their environment.
    marker: Vec<u8>,
    uid: uid_t,
}

/// What `proc_pidinfo` tells about a process.
#[derive(Debug, Clone, Copy)]
struct Proc {
    ppid: pid_t,
    pgid: pid_t,
    uid: uid_t,
    zombie: bool,
}

/// Every process of the system now, by pid.
fn processes() -> HashMap<pid_t, Proc> {
    // SAFETY: a null buffer asks for the number of processes.
    let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    let capacity = usize::try_from(count).unwrap_or(0) + 256;
    let mut pids: Vec<pid_t> = vec![0; capacity];
    let bytes = c_int::try_from(capacity * std::mem::size_of::<pid_t>()).unwrap_or(c_int::MAX);
    // SAFETY: `pids` holds `bytes` bytes.
    let listed = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast::<c_void>(), bytes) };
    pids.truncate(usize::try_from(listed).unwrap_or(0));
    let size = c_int::try_from(std::mem::size_of::<libc::proc_bsdinfo>()).unwrap_or(c_int::MAX);
    pids.into_iter()
        .filter(|&pid| pid > 0)
        .filter_map(|pid| {
            // SAFETY: all zero is a valid proc_bsdinfo.
            let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
            // SAFETY: `info` is `size` bytes, the size PROC_PIDTBSDINFO fills.
            let read = unsafe {
                libc::proc_pidinfo(
                    pid,
                    libc::PROC_PIDTBSDINFO,
                    0,
                    (&mut info as *mut libc::proc_bsdinfo).cast::<c_void>(),
                    size,
                )
            };
            (read == size).then(|| {
                (
                    pid,
                    Proc {
                        ppid: pid_t::try_from(info.pbi_ppid).unwrap_or(0),
                        pgid: pid_t::try_from(info.pbi_pgid).unwrap_or(0),
                        uid: info.pbi_uid,
                        zombie: info.pbi_status == ZOMBIE,
                    },
                )
            })
        })
        .collect()
}

/// Longest argument and environment block of a process (`KERN_ARGMAX`).
fn arg_max() -> usize {
    let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
    let mut value: c_int = 0;
    let mut size = std::mem::size_of::<c_int>();
    // SAFETY: reads one int into `value`.
    let read = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            2,
            (&mut value as *mut c_int).cast::<c_void>(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if read == 0 && value > 0 {
        usize::try_from(value).unwrap_or(1 << 20)
    } else {
        1 << 20
    }
}

/// Whether `pid`'s environment holds `marker` (`KERN_PROCARGS2`: the
/// argument count, the executable path, the arguments, then the
/// environment, each NUL-terminated). `buffer` is reused between calls.
fn carries_marker(pid: pid_t, marker: &[u8], buffer: &mut [u8]) -> bool {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut size = buffer.len();
    // SAFETY: `buffer` holds `size` bytes.
    let read = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buffer.as_mut_ptr().cast::<c_void>(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if read != 0 {
        return false;
    }
    buffer[..size.min(buffer.len())]
        .split(|&byte| byte == 0)
        .any(|entry| entry == marker)
}

/// The live processes of the run: its process group, the user's orphans
/// that carry its marker (with `marked`), and every descendant of those.
/// Never Shodh itself.
fn run_processes(target: &Target, marked: bool, buffer: &mut Vec<u8>) -> HashSet<pid_t> {
    let all = processes();
    // SAFETY: getpid cannot fail.
    let me = unsafe { libc::getpid() };
    let ours = |proc: &Proc| proc.uid == target.uid && !proc.zombie;
    let mut found: HashSet<pid_t> = all
        .iter()
        .filter(|(&pid, proc)| pid != me && ours(proc) && proc.pgid == target.group)
        .map(|(&pid, _)| pid)
        .collect();
    if marked {
        if buffer.is_empty() {
            buffer.resize(arg_max(), 0);
        }
        // A process that left the tree was reparented to launchd (pid 1).
        for (&pid, proc) in &all {
            if pid != me
                && ours(proc)
                && proc.ppid == 1
                && !found.contains(&pid)
                && carries_marker(pid, &target.marker, buffer)
            {
                found.insert(pid);
            }
        }
    }
    loop {
        let before = found.len();
        for (&pid, proc) in &all {
            if pid != me && ours(proc) && found.contains(&proc.ppid) {
                found.insert(pid);
            }
        }
        if found.len() == before {
            break;
        }
    }
    found
}

/// Stop and kill every process of the run, pass after pass until one finds
/// none. Returns how many processes it killed.
fn sweep(target: &Target) -> usize {
    let mut buffer = Vec::new();
    let mut killed: HashSet<pid_t> = HashSet::new();
    for _ in 0..MAX_SWEEPS {
        let found = run_processes(target, true, &mut buffer);
        if found.is_empty() {
            break;
        }
        // SAFETY: signals to the run's own group and processes; an error
        // (the group or process is gone) changes nothing.
        unsafe {
            libc::killpg(target.group, libc::SIGSTOP);
            for &pid in &found {
                libc::kill(pid, libc::SIGSTOP);
            }
            for &pid in &found {
                libc::kill(pid, libc::SIGKILL);
            }
            libc::killpg(target.group, libc::SIGKILL);
        }
        killed.extend(found);
        // Killed processes leave the table once their parent (or launchd)
        // reaps them; zombies are not counted on the next pass.
        std::thread::sleep(Duration::from_millis(5));
    }
    killed.len()
}

/// The physical footprint of `pid`, in bytes.
fn footprint(pid: pid_t) -> u64 {
    // SAFETY: all zero is a valid rusage_info_v2.
    let mut info: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
    // SAFETY: `info` is the structure RUSAGE_INFO_V2 fills.
    let read = unsafe {
        libc::proc_pid_rusage(
            pid,
            libc::RUSAGE_INFO_V2,
            (&mut info as *mut libc::rusage_info_v2).cast::<libc::rusage_info_t>(),
        )
    };
    if read == 0 {
        info.ri_phys_footprint
    } else {
        0
    }
}

/// A running command's process group.
#[derive(Debug)]
pub struct Contained {
    target: Arc<Target>,
    memory_bytes: u64,
    memory_mb: u64,
    last_watch: Instant,
    /// Reused by the watchdog to read environments.
    buffer: Vec<u8>,
    swept: bool,
}

impl Contained {
    /// The memory limit, in words, when the run's processes use more than
    /// it together (checked every [`WATCH_INTERVAL`]).
    pub fn over_limit(&mut self) -> Option<String> {
        if self.last_watch.elapsed() < WATCH_INTERVAL {
            return None;
        }
        self.last_watch = Instant::now();
        let used: u64 = run_processes(&self.target, true, &mut self.buffer)
            .into_iter()
            .map(footprint)
            .sum();
        (used > self.memory_bytes).then(|| {
            format!(
                "the memory limit ({} MB for all its processes)",
                self.memory_mb
            )
        })
    }

    /// Stop every process of the run. Returns how many were still running
    /// (the command's own process, once it has ended, is not among them).
    pub fn terminate(&mut self) -> usize {
        self.swept = true;
        sweep(&self.target)
    }

    /// What acted on the run besides how it ended (nothing on macOS: the
    /// watchdog's kill is the run's end).
    pub fn notes(&mut self) -> Vec<String> {
        Vec::new()
    }

    /// Kills every process of the run at once, from any thread.
    pub fn killer(&self) -> Killer {
        let target = Arc::clone(&self.target);
        Arc::new(move || {
            sweep(&target);
        })
    }
}

impl Drop for Contained {
    fn drop(&mut self) {
        if !self.swept {
            sweep(&self.target);
        }
    }
}
