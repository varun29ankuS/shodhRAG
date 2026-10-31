//! `run_command`: how a Code session runs a shell command.
//!
//! omp's own shell is off in Code sessions ([`super::code_mode::CODE_TOOLS`]);
//! the model runs commands through this host tool, which the session decides
//! like an edit (approval, allowlist, review list) and then hands to a
//! [`CommandExecutor`]. The production executor, [`ContainedExecutor`], runs
//! each command in a fresh process that Shodh owns:
//! - `bash -c <command>`: Git for Windows' bash on Windows ([`locate_shell`];
//!   refused when it is missing, never another shell), `/bin/bash` on macOS;
//! - in the code folder, stdin closed, with an environment built from scratch
//!   ([`command_env`]): no provider key, token or Shodh secret, and `HOME` and
//!   the temporary folders in a private per-conversation scratch folder
//!   ([`scratch_dir`]);
//! - inside limits ([`CommandLimits`]): a Job Object on Windows, a process
//!   group with a CPU-time limit and a memory watchdog on macOS (the platform
//!   modules), a wall-clock limit and an output cap on both;
//! - on macOS, in macOS's sandbox (Seatbelt, `seatbelt.rs`), rendered from
//!   the command's policy (`policy.rs`): no network, changes only in the code
//!   folder and the scratch folder, nothing of the rest of the home, the
//!   keychains or Shodh's data read. Windows has no sandbox yet, and the
//!   approval prompt and the live panel say so ([`sandbox_text`]);
//! - and when it ends (by itself, at its time limit, on Stop, or when the call
//!   is cancelled), every process it started is stopped.
//!
//! `spawn_contained` in the platform module is the one place a process is
//! created. It fails closed like the rest of this module: on macOS a command
//! whose sandbox cannot be set up is refused, never run without it.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;
use tokio::sync::{mpsc, Notify};

use super::code_mode::CodeSettings;
use super::events::{CommandState, RiskTier};
use super::protocol::{HostToolDefinition, ToolLoadMode};
use super::truncate_chars;

#[cfg(any(target_os = "macos", test))]
mod policy;
#[cfg(target_os = "macos")]
mod seatbelt;

#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "macos")]
use mac as platform;
#[cfg(windows)]
mod win;
#[cfg(windows)]
use win as platform;
#[cfg(not(any(windows, target_os = "macos")))]
mod other;
#[cfg(not(any(windows, target_os = "macos")))]
use other as platform;

#[cfg(test)]
mod tests;

/// The tool's name, as the model calls it.
pub const RUN_COMMAND: &str = "run_command";

/// Step label and tier of a `run_command` call.
pub const LABEL_TEMPLATE: &str = "Running {command}";
pub const TIER: RiskTier = RiskTier::Write;

/// Longest command accepted, in characters: exactly what an approval prompt
/// shows in full ([`super::code_mode::MAX_PREVIEW_CHARS`]), so no part of a
/// command the user approves is ever cut from the prompt.
pub const MAX_COMMAND_CHARS: usize = super::code_mode::MAX_PREVIEW_CHARS;

/// Longest run of blank characters (spaces, tabs, line breaks) a command may
/// have: a longer one could push the rest of the command out of sight in the
/// approval prompt.
pub const MAX_BLANK_RUN: usize = 100;

/// Longest time limit a call may ask for, in seconds, before the workspace's
/// own limit applies ([`CommandLimits::for_call`]).
const MAX_ASKED_SECS: u64 = 24 * 60 * 60;

/// Most processes a command may have running at once (Windows enforces it).
pub const MAX_PROCESSES: u32 = 64;

/// Output kept for the user and the logs, in bytes: the start and the end,
/// with the middle cut.
pub const OUTPUT_CAP_BYTES: usize = 1024 * 1024;

/// Output the model gets back, in characters (the result travels in one omp
/// frame): the start and the end, with the middle cut.
const MODEL_HEAD_CHARS: usize = 6_000;
const MODEL_TAIL_CHARS: usize = 12_000;

/// How often live output is sent to the UI, and the most sent at once.
const LIVE_INTERVAL: Duration = Duration::from_millis(100);
const LIVE_CHUNK_BYTES: usize = 16 * 1024;

/// How long the output pipes are read after every process was stopped.
const DRAIN_GRACE: Duration = Duration::from_secs(2);

/// Environment variable that marks every process of one run (macOS finds
/// processes that left the process tree by it).
pub const RUN_ID_ENV: &str = "SHODH_RUN_COMMAND_ID";

/// What the model is told about the sandbox commands run in on this system.
#[cfg(target_os = "macos")]
const SANDBOX_FOR_MODEL: &str = "Commands run in macOS's sandbox: they have no network \
     access (no downloads, package installs or connections, localhost included), can change \
     only the code folder and the scratch folder, and cannot read the rest of the user's home \
     folder, the keychain or Shodh's data.";
#[cfg(not(target_os = "macos"))]
const SANDBOX_FOR_MODEL: &str = "Network access is not guaranteed.";

/// What the model is told the tool does.
pub fn description() -> String {
    format!(
        "Run one bash command in the code folder and return its exit code and output \
         (stdout and stderr together). Use bash syntax (pipes, &&, quotes) on every system; \
         on Windows this is Git for Windows' bash. Each call is a fresh process in the code \
         folder: nothing carries over between calls (no cd, no exported variables), HOME and \
         the temporary folder are a scratch folder of this conversation, and Shodh's API keys \
         and tokens are not passed to it. {SANDBOX_FOR_MODEL} Do not start \
         background processes, servers, scheduled tasks or services: everything the command \
         starts is stopped when it ends. The command stops at its time limit (the \
         workspace's, at most; ask for less with timeout_secs) and the user can stop it at \
         any time. Output beyond about 1 MB is cut in the middle. Commands are at most \
         {MAX_COMMAND_CHARS} characters, without control or invisible formatting characters \
         and without runs of more than {MAX_BLANK_RUN} blank characters."
    )
}

/// The tool's arguments, as Shodh checks them.
pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "command": {
                "type": "string",
                "minLength": 1,
                "maxLength": MAX_COMMAND_CHARS,
                "description": "The bash command, run in the code folder.",
            },
            "timeout_secs": {
                "type": "integer",
                "minimum": 1,
                "maximum": MAX_ASKED_SECS,
                "description": "Stop the command after this many seconds (at most the workspace's limit).",
            },
        },
        "required": ["command"],
        "additionalProperties": false,
    })
}

/// The tool as a Code session registers it with omp (`set_host_tools`).
pub fn definition() -> HostToolDefinition {
    HostToolDefinition {
        name: RUN_COMMAND.to_string(),
        label: "Run a command".to_string(),
        description: description(),
        parameters: super::tools::nullable_optionals(schema()),
        load_mode: ToolLoadMode::Essential,
    }
}

/// A `run_command` call's arguments, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandArgs {
    pub command: String,
    /// The time limit the model asked for, in seconds.
    pub timeout_secs: Option<u64>,
}

impl CommandArgs {
    /// The arguments as the approval path and the audit log see them: only
    /// what was given.
    pub fn input(&self) -> Value {
        let mut input = json!({ "command": self.command });
        if let (Some(secs), Some(map)) = (self.timeout_secs, input.as_object_mut()) {
            map.insert("timeout_secs".to_string(), json!(secs));
        }
        input
    }
}

/// A whole number of seconds: an integer, or a number with no fraction
/// (some models write `60.0`).
fn whole_seconds(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| {
        value
            .as_f64()
            .filter(|secs| {
                secs.fract().abs() < f64::EPSILON && *secs >= 0.0 && *secs <= MAX_ASKED_SECS as f64
            })
            .map(|secs| secs as u64)
    })
}

/// The time limit a checked call's `input` ([`CommandArgs::input`]) asks
/// for, in seconds.
pub fn asked_timeout(input: &Value) -> Option<u64> {
    input.get("timeout_secs").and_then(whole_seconds)
}

/// Whether `c` changes how a command looks in the approval prompt without
/// changing what bash runs, or hides text there: control characters other
/// than line breaks and tabs, bidirectional overrides and isolates, and
/// zero-width and other invisible formatting characters.
pub fn hidden_character(c: char) -> bool {
    (c.is_control() && c != '\n' && c != '\t')
        || matches!(
            c,
            '\u{00AD}'
                | '\u{034F}'
                | '\u{061C}'
                | '\u{115F}'
                | '\u{1160}'
                | '\u{17B4}'
                | '\u{17B5}'
                | '\u{180B}'..='\u{180E}'
                | '\u{200B}'..='\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2060}'..='\u{206F}'
                | '\u{3164}'
                | '\u{FE00}'..='\u{FE0F}'
                | '\u{FEFF}'
                | '\u{FFA0}'
                | '\u{FFF9}'..='\u{FFFB}'
                | '\u{E0000}'..='\u{E007F}'
        )
}

/// The longest run of blank characters in `command`.
fn longest_blank_run(command: &str) -> usize {
    let mut longest = 0;
    let mut run = 0;
    for c in command.chars() {
        if c.is_whitespace() {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    longest
}

/// Check a call's arguments against [`schema`]: exactly `command` (non-empty
/// text, at most [`MAX_COMMAND_CHARS`], nothing that hides part of it in the
/// approval prompt: [`hidden_character`], [`MAX_BLANK_RUN`]) and an optional
/// `timeout_secs` (a whole number of seconds). omp's intent argument `i` is
/// dropped; `null` means not given. Anything else is refused.
pub fn parse_args(args: &Value) -> Result<CommandArgs, String> {
    let Some(map) = args.as_object() else {
        return Err("The arguments must be an object with a command.".to_string());
    };
    let mut command = None;
    let mut timeout_secs = None;
    for (key, value) in map {
        match (key.as_str(), value) {
            ("i", _) | (_, Value::Null) => {}
            ("command", Value::String(text)) => command = Some(text.clone()),
            ("command", _) => return Err("command must be text.".to_string()),
            ("timeout_secs", value) => {
                let secs = whole_seconds(value)
                    .filter(|secs| (1..=MAX_ASKED_SECS).contains(secs))
                    .ok_or_else(|| {
                        format!(
                            "timeout_secs must be a whole number of seconds from 1 to \
                             {MAX_ASKED_SECS}."
                        )
                    })?;
                timeout_secs = Some(secs);
            }
            (other, _) => {
                return Err(format!(
                    "run_command takes only command and timeout_secs, not {other}."
                ))
            }
        }
    }
    let command = command.ok_or_else(|| "run_command needs a command.".to_string())?;
    if command.trim().is_empty() {
        return Err("run_command needs a command.".to_string());
    }
    if command.chars().count() > MAX_COMMAND_CHARS {
        return Err(format!(
            "The command is longer than {MAX_COMMAND_CHARS} characters; write a script file \
             and run that."
        ));
    }
    if let Some(c) = command.chars().find(|&c| hidden_character(c)) {
        return Err(format!(
            "The command contains the character U+{:04X}, a control or invisible formatting \
             character; write it without one.",
            u32::from(c)
        ));
    }
    if longest_blank_run(&command) > MAX_BLANK_RUN {
        return Err(format!(
            "The command has more than {MAX_BLANK_RUN} blank characters in a row; write it \
             without long runs of spaces or empty lines."
        ));
    }
    Ok(CommandArgs {
        command,
        timeout_secs,
    })
}

/// What a command may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandLimits {
    /// Wall-clock limit.
    pub time: Duration,
    /// Whether the call asked for a shorter limit than the workspace's.
    pub time_asked: bool,
    /// Memory all of its processes may use together.
    pub memory_bytes: u64,
    /// Share of the whole machine's CPU, in percent.
    pub cpu_percent: u32,
    /// Most processes at once.
    pub max_processes: u32,
}

impl CommandLimits {
    /// The limits of one call under the workspace's `settings`: the time
    /// limit the call asked for, never more than the workspace's.
    pub fn for_call(settings: &CodeSettings, asked_secs: Option<u64>) -> Self {
        let workspace = settings.command_timeout_secs;
        let secs = asked_secs.map_or(workspace, |asked| asked.min(workspace));
        CommandLimits {
            time: Duration::from_secs(secs),
            time_asked: asked_secs.is_some_and(|asked| asked < workspace),
            memory_bytes: settings.memory_limit_mb.saturating_mul(1024 * 1024),
            cpu_percent: settings.cpu_percent,
            max_processes: MAX_PROCESSES,
        }
    }

    pub fn memory_mb(&self) -> u64 {
        self.memory_bytes / (1024 * 1024)
    }

    /// The time limit in words ("10 minutes", "90 seconds").
    pub fn time_text(&self) -> String {
        duration_words(self.time)
    }

    /// What an approval prompt says about the limits and the sandbox: only
    /// what this system enforces.
    pub fn prompt_rows(&self) -> [(&'static str, String); 3] {
        [
            (
                "time_limit",
                if self.time_asked {
                    format!("{} (asked by the assistant)", self.time_text())
                } else {
                    format!("{} (the workspace's limit)", self.time_text())
                },
            ),
            ("limits", self.enforced_text()),
            ("sandbox", sandbox_text().to_string()),
        ]
    }

    /// The limits this system enforces, in words.
    pub fn enforced_text(&self) -> String {
        #[cfg(windows)]
        let enforced = format!(
            "Memory {} for all its processes (an allocation beyond it fails), CPU {}%, at \
             most {} processes at once.",
            memory_words(self.memory_mb()),
            self.cpu_percent,
            self.max_processes
        );
        #[cfg(target_os = "macos")]
        let enforced = format!(
            "Memory {} for all its processes and {} processes at once (the command is \
             stopped beyond either), lower CPU priority (macOS has no CPU cap).",
            memory_words(self.memory_mb()),
            self.max_processes
        );
        #[cfg(not(any(windows, target_os = "macos")))]
        let enforced = "None: commands do not run on this system.".to_string();
        format!("{enforced} {STOPPED_AT_END}")
    }
}

/// What the approval prompt and the live panel of a macOS command say about
/// its sandbox: what `seatbelt.rs` enforces.
pub const SANDBOX_MAC: &str = "Sandboxed: no network, only this project folder (and this \
     conversation's scratch folder) can change. It cannot read the rest of your home folder, \
     your keychain or Shodh's data.";

/// What they say on Windows, which has no sandbox yet.
pub const SANDBOX_WINDOWS: &str = "Not sandboxed on Windows yet — runs with your account's \
     access, after your approval, with time/memory limits and without your API keys.";

/// What they say where commands do not run.
pub const SANDBOX_NONE: &str = "Not sandboxed: commands do not run on this system.";

/// What the approval prompt and the live panel say about the sandbox
/// commands run in on this system.
pub fn sandbox_text() -> &'static str {
    if cfg!(target_os = "macos") {
        SANDBOX_MAC
    } else if cfg!(windows) {
        SANDBOX_WINDOWS
    } else {
        SANDBOX_NONE
    }
}

/// Why a failed command may have failed because of macOS's sandbox, in
/// plain words, from what it printed: no network, or a file outside the
/// folders it may use. `None` when nothing in the output points at it.
#[cfg(any(target_os = "macos", test))]
pub fn failure_hint(output: &str) -> Option<&'static str> {
    const NETWORK: &[&str] = &[
        "could not resolve host",
        "could not resolve hostname",
        "temporary failure in name resolution",
        "name or service not known",
        "nodename nor servname",
        "getaddrinfo",
        "network is unreachable",
        "no route to host",
        "failed to connect to",
        "couldn't connect to server",
        "unable to connect to",
        "connect: operation not permitted",
        "socket: operation not permitted",
        "failed to lookup address",
        "dns error",
    ];
    const FILES: &[&str] = &["operation not permitted", "permission denied"];
    let lowered = output.to_lowercase();
    if NETWORK.iter().any(|needle| lowered.contains(needle)) {
        return Some(
            "Network is blocked for commands: downloads, package installs and connections to \
             servers (on this computer too) fail in Shodh's sandbox.",
        );
    }
    if FILES.iter().any(|needle| lowered.contains(needle)) {
        return Some(
            "Commands can change only the project folder and this conversation's scratch \
             folder, and cannot read the rest of your home folder, your keychain or Shodh's \
             data.",
        );
    }
    None
}

/// What every command prompt says about processes left at the end.
pub const STOPPED_AT_END: &str = "Every process the command starts is stopped when it ends, \
     except programs it has the system start for it (scheduled tasks, services, launch agents, \
     apps).";

fn duration_words(time: Duration) -> String {
    let secs = time.as_secs();
    if secs >= 60 && secs.is_multiple_of(60) {
        let minutes = secs / 60;
        format!("{minutes} minute{}", if minutes == 1 { "" } else { "s" })
    } else {
        format!("{secs} second{}", if secs == 1 { "" } else { "s" })
    }
}

fn memory_words(mb: u64) -> String {
    if mb >= 1024 && mb.is_multiple_of(1024) {
        format!("{} GB", mb / 1024)
    } else {
        format!("{mb} MB")
    }
}

/// One command to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub command: String,
    /// The code folder: the command's working folder.
    pub folder: PathBuf,
    /// The conversation's scratch folder ([`scratch_dir`]).
    pub scratch: PathBuf,
    /// Shodh's data folder, which holds the scratch folder: commands never
    /// read the rest of it.
    pub data_dir: PathBuf,
    pub limits: CommandLimits,
}

/// How a command ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutcome {
    pub state: CommandState,
    pub exit_code: Option<i32>,
    /// What acted on it, in words (the limit that killed it, why it was
    /// refused, what a limit refused while it ran, processes stopped at its
    /// end).
    pub note: Option<String>,
    /// stdout and stderr interleaved, at most [`OUTPUT_CAP_BYTES`] (the
    /// middle cut with a marker).
    pub output: String,
    pub duration: Duration,
    /// Some of its processes had left its process tree and group (found by
    /// their run marker and stopped). Whatever left that way may have taken
    /// others with it that Shodh cannot find, so the session treats what the
    /// command changed as unknown.
    pub escaped: bool,
}

impl CommandOutcome {
    /// A command that never started.
    pub fn refused(reason: impl Into<String>) -> Self {
        CommandOutcome {
            state: CommandState::Refused,
            exit_code: None,
            note: Some(reason.into()),
            output: String::new(),
            duration: Duration::ZERO,
            escaped: false,
        }
    }

    /// Whether the command ran and exited with 0.
    pub fn succeeded(&self) -> bool {
        self.state == CommandState::Done && self.exit_code == Some(0)
    }

    /// One line for the step in the UI ("Exit 0 · 2 lines").
    pub fn summary(&self, limits: &CommandLimits) -> String {
        let status = self.status_words(limits);
        let lines = self.output.lines().filter(|l| !l.trim().is_empty()).count();
        if lines == 0 || self.state == CommandState::Refused {
            status
        } else {
            format!(
                "{status} · {lines} line{} of output",
                if lines == 1 { "" } else { "s" }
            )
        }
    }

    fn status_words(&self, limits: &CommandLimits) -> String {
        match self.state {
            CommandState::Done => match self.exit_code {
                Some(code) => format!("Exit {code}"),
                None => "Ended".to_string(),
            },
            CommandState::TimedOut => format!("Timed out after {}", limits.time_text()),
            CommandState::Stopped => "Stopped by you".to_string(),
            CommandState::Killed => format!(
                "Killed: {}",
                self.note.as_deref().unwrap_or("a limit was reached")
            ),
            CommandState::Refused => format!(
                "Refused: {}",
                self.note.as_deref().unwrap_or("it could not be started")
            ),
        }
    }

    /// What the model gets back: how it ended, then the output's start and
    /// end.
    pub fn text_for_model(&self, limits: &CommandLimits) -> String {
        let secs = self.duration.as_secs_f64();
        let mut text = match self.state {
            CommandState::Done => match self.exit_code {
                Some(code) => format!("Exit code {code} after {secs:.1} s."),
                None => format!("The command ended without an exit code after {secs:.1} s."),
            },
            CommandState::TimedOut => format!(
                "The command reached its time limit ({}) and was stopped, with every \
                 process it started.",
                limits.time_text()
            ),
            CommandState::Stopped => format!(
                "The user stopped this command after {secs:.1} s; every process it started \
                 was stopped. Do not run it again unless the user asks."
            ),
            CommandState::Killed => format!(
                "The command was killed after {secs:.1} s: {}. Every process it started was \
                 stopped.",
                self.note.as_deref().unwrap_or("a limit was reached")
            ),
            CommandState::Refused => format!(
                "The command was not run: {}",
                self.note.as_deref().unwrap_or("it could not be started.")
            ),
        };
        if let (Some(note), false) = (
            &self.note,
            matches!(self.state, CommandState::Killed | CommandState::Refused),
        ) {
            text.push(' ');
            text.push_str(note);
        }
        if self.state != CommandState::Refused {
            if self.output.trim().is_empty() {
                text.push_str("\nNo output.");
            } else {
                text.push_str("\nOutput (stdout and stderr):\n");
                text.push_str(&head_and_tail(
                    &self.output,
                    MODEL_HEAD_CHARS,
                    MODEL_TAIL_CHARS,
                ));
            }
        }
        text
    }
}

/// `text` with at most `head` characters from its start and `tail` from its
/// end, and a marker saying how much was cut between them.
pub fn head_and_tail(text: &str, head: usize, tail: usize) -> String {
    let total = text.chars().count();
    if total <= head + tail {
        return text.to_string();
    }
    let start: String = text.chars().take(head).collect();
    let end: String = text.chars().skip(total - tail).collect();
    format!(
        "{start}\n[… {} characters cut …]\n{end}",
        total - head - tail
    )
}

/// Output kept within a byte cap: the first half and the last half, with how
/// much was dropped between them.
#[derive(Debug)]
pub struct CappedOutput {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    half: usize,
    dropped: u64,
}

impl CappedOutput {
    pub fn new(cap: usize) -> Self {
        CappedOutput {
            head: Vec::new(),
            tail: VecDeque::new(),
            half: (cap / 2).max(1),
            dropped: 0,
        }
    }

    pub fn push(&mut self, mut bytes: &[u8]) {
        if self.head.len() < self.half {
            let take = (self.half - self.head.len()).min(bytes.len());
            self.head.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
        }
        if bytes.len() >= self.half {
            self.dropped += (self.tail.len() + bytes.len() - self.half) as u64;
            self.tail.clear();
            self.tail.extend(&bytes[bytes.len() - self.half..]);
            return;
        }
        let over = (self.tail.len() + bytes.len()).saturating_sub(self.half);
        if over > 0 {
            self.tail.drain(..over);
            self.dropped += over as u64;
        }
        self.tail.extend(bytes);
    }

    /// Bytes dropped from the middle.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// The kept output as text (invalid UTF-8 replaced), with a marker where
    /// the middle was cut.
    pub fn text(&self) -> String {
        let mut text = String::from_utf8_lossy(&self.head).into_owned();
        if self.dropped > 0 {
            text.push_str(&format!("\n[… {} bytes cut …]\n", self.dropped));
        }
        let (front, back) = self.tail.as_slices();
        let mut tail = Vec::with_capacity(front.len() + back.len());
        tail.extend_from_slice(front);
        tail.extend_from_slice(back);
        text.push_str(&String::from_utf8_lossy(&tail));
        text
    }
}

/// Output waiting to be shown live: sent every [`LIVE_INTERVAL`], at most
/// [`LIVE_CHUNK_BYTES`] at a time (the newest kept), never splitting a UTF-8
/// character.
#[derive(Debug, Default)]
pub struct LiveOutput {
    pending: Vec<u8>,
    skipped: u64,
}

impl LiveOutput {
    pub fn push(&mut self, bytes: &[u8]) {
        self.pending.extend_from_slice(bytes);
        if self.pending.len() > LIVE_CHUNK_BYTES {
            let over = self.pending.len() - LIVE_CHUNK_BYTES;
            self.pending.drain(..over);
            self.skipped += over as u64;
        }
    }

    /// The text to send now, if any. An incomplete character at the end
    /// waits for the rest; at the `last` flush it is sent as it is.
    pub fn take(&mut self, last: bool) -> Option<String> {
        let ready = if last {
            self.pending.len()
        } else {
            match std::str::from_utf8(&self.pending) {
                Ok(_) => self.pending.len(),
                Err(e) if e.error_len().is_none() => e.valid_up_to(),
                Err(_) => self.pending.len(),
            }
        };
        if ready == 0 && self.skipped == 0 {
            return None;
        }
        let bytes: Vec<u8> = self.pending.drain(..ready).collect();
        let mut text = String::new();
        if self.skipped > 0 {
            text.push_str(&format!("[… {} bytes not shown …]\n", self.skipped));
            self.skipped = 0;
        }
        text.push_str(&String::from_utf8_lossy(&bytes));
        Some(text)
    }
}

/// Runs a command for a Code session. [`ContainedExecutor`] is the one
/// Shodh uses; the session's tests stand in their own.
#[async_trait]
pub trait CommandExecutor: Send + Sync + std::fmt::Debug {
    /// Run `spec`, sending live output through `control`, and report how it
    /// ended. Dropping the future stops every process it started.
    async fn run(&self, spec: CommandSpec, control: RunControl) -> CommandOutcome;
}

/// What a running command reports to and takes from the session.
pub struct RunControl {
    /// The run's id: [`stop`] takes it.
    pub command_id: String,
    /// The run registered under `command_id` by whoever starts it, before
    /// its process exists: [`stop`] reaches it from then on. The executor
    /// ends the run when a stop is asked for.
    pub registration: RunRegistration,
    /// Called once the command's process started and [`stop`] can reach it;
    /// never for a command that was refused.
    pub on_started: Box<dyn Fn() + Send + Sync>,
    /// Receives live output (at most about ten times a second).
    pub on_output: Box<dyn Fn(String) + Send + Sync>,
}

/// Runs each command in its own contained process ([`run_contained`]).
#[derive(Debug, Default, Clone, Copy)]
pub struct ContainedExecutor;

#[async_trait]
impl CommandExecutor for ContainedExecutor {
    async fn run(&self, spec: CommandSpec, control: RunControl) -> CommandOutcome {
        run_contained(spec, locate_shell(), control).await
    }
}

// ── Stop ────────────────────────────────────────────────────────────────────

/// Ends every process of one run at once, from any thread (the platform
/// module's `Contained::killer`).
pub type Killer = Arc<dyn Fn() + Send + Sync>;

/// A run's stop request, and how to end its processes at once.
#[derive(Default)]
struct StopSignal {
    requested: AtomicBool,
    notify: Notify,
    killer: Mutex<Option<Killer>>,
}

impl StopSignal {
    fn request(&self) {
        self.requested.store(true, Ordering::SeqCst);
        self.notify.notify_one();
    }

    /// End the run's processes now (the app is exiting: the run's own task
    /// may never be polled again).
    fn kill_now(&self) {
        let killer = self
            .killer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(killer) = killer {
            killer();
        }
    }
}

fn runs() -> &'static Mutex<HashMap<String, Arc<StopSignal>>> {
    static RUNS: OnceLock<Mutex<HashMap<String, Arc<StopSignal>>>> = OnceLock::new();
    RUNS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock_runs() -> std::sync::MutexGuard<'static, HashMap<String, Arc<StopSignal>>> {
    runs().lock().unwrap_or_else(|e| e.into_inner())
}

/// A run registered for [`stop`] while it runs; removed when dropped. Every
/// [`CommandExecutor`] registers its runs, so the user's Stop and the
/// session's cancel reach them.
pub struct RunRegistration {
    id: String,
    signal: Arc<StopSignal>,
}

impl RunRegistration {
    pub fn new(id: &str) -> Self {
        let signal = Arc::new(StopSignal::default());
        lock_runs().insert(id.to_string(), Arc::clone(&signal));
        RunRegistration {
            id: id.to_string(),
            signal,
        }
    }

    /// Whether a stop was asked for.
    pub fn stop_requested(&self) -> bool {
        self.signal.requested.load(Ordering::SeqCst)
    }

    /// Resolves once a stop is asked for (at once if it already was).
    pub async fn stopped(&self) {
        while !self.stop_requested() {
            self.signal.notify.notified().await;
        }
    }

    /// How [`stop_all`] ends the run's processes at once.
    fn set_killer(&self, killer: Killer) {
        *self.signal.killer.lock().unwrap_or_else(|e| e.into_inner()) = Some(killer);
    }
}

impl Drop for RunRegistration {
    fn drop(&mut self) {
        lock_runs().remove(&self.id);
    }
}

/// Stop the running command `command_id` and every process it started.
/// Returns whether such a command was running.
pub fn stop(command_id: &str) -> bool {
    let signal = lock_runs().get(command_id).cloned();
    match signal {
        Some(signal) => {
            signal.request();
            true
        }
        None => false,
    }
}

/// Stop every running command and end its processes before returning (the
/// app is exiting). Returns how many were running.
pub fn stop_all() -> usize {
    let signals: Vec<Arc<StopSignal>> = lock_runs().values().cloned().collect();
    for signal in &signals {
        signal.request();
        signal.kill_now();
    }
    signals.len()
}

// ── Shell ───────────────────────────────────────────────────────────────────

/// Why a command cannot run on Windows without Git for Windows.
pub const NO_GIT_BASH: &str = "Commands run in Git for Windows' bash, which was not found on \
     this computer. Install Git for Windows (https://git-scm.com/download/win), then ask again.";

/// Where Git for Windows may be: the folders its installer recorded for all
/// users (`HKLM`, writable only by administrators), the Program Files
/// folder, the folders recorded for this user (`HKCU`), and `PATH`.
#[derive(Debug, Default, Clone)]
pub struct ShellSources {
    pub install_path: Vec<PathBuf>,
    pub program_files: Option<PathBuf>,
    pub user_install_path: Vec<PathBuf>,
    pub path: Option<std::ffi::OsString>,
}

/// Git for Windows' `bin\bash.exe` from `sources`: an install for all users
/// (recorded, then in Program Files) first, then one for this user only,
/// then a `bash.exe` on `PATH` that sits in a Git for Windows tree. What any
/// program of the user can change (its own registry keys, `PATH`) is
/// consulted only when no install for all users exists. Never the WSL
/// launcher in System32 or an app alias. `None` when there is none.
pub fn git_bash(sources: &ShellSources) -> Option<PathBuf> {
    let installed = sources
        .install_path
        .iter()
        .cloned()
        .chain(sources.program_files.as_ref().map(|dir| dir.join("Git")))
        .chain(sources.user_install_path.iter().cloned())
        .map(|git| git.join("bin").join("bash.exe"))
        .collect::<Vec<_>>();
    if let Some(found) = installed.into_iter().find(|bash| bash.is_file()) {
        return Some(found);
    }
    let path = sources.path.as_ref()?;
    std::env::split_paths(path).find_map(|dir| git_tree_bash(&dir))
}

/// `dir`'s `bash.exe` when `dir` is the `bin` or `usr\bin` folder of a Git
/// for Windows tree: that tree's `bin\bash.exe`.
fn git_tree_bash(dir: &Path) -> Option<PathBuf> {
    let lowered = dir.to_string_lossy().to_lowercase().replace('/', "\\");
    if lowered.contains("\\system32") || lowered.contains("windowsapps") {
        return None;
    }
    if !dir.join("bash.exe").is_file() {
        return None;
    }
    let named = |path: &Path, name: &str| {
        path.file_name()
            .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(name))
    };
    if !named(dir, "bin") {
        return None;
    }
    let parent = dir.parent()?;
    // <git>\bin\bash.exe, with <git>\usr\bin\bash.exe beside it.
    if parent.join("usr").join("bin").join("bash.exe").is_file() {
        return Some(dir.join("bash.exe"));
    }
    // <git>\usr\bin\bash.exe: the launcher in <git>\bin sets up the tools.
    if named(parent, "usr") {
        let launcher = parent.parent()?.join("bin").join("bash.exe");
        if launcher.is_file() {
            return Some(launcher);
        }
    }
    None
}

/// The shell commands run in on this computer, or why there is none.
pub fn locate_shell() -> Result<PathBuf, String> {
    platform::locate_shell()
}

/// `command` as one argument of a Windows command line that the MSYS2
/// runtime in Git for Windows' bash reads back exactly: always in double
/// quotes (so it is neither split nor globbed), backslashes and quotes
/// escaped with a backslash. A command starting like a drive (`C:`) gets a
/// leading space, which bash ignores, because MSYS2 reads quoted text that
/// starts with a drive as a Windows path and keeps its backslashes.
pub fn msys_quoted(command: &str) -> String {
    let mut chars = command.chars();
    let drive_like = matches!(
        (chars.next(), chars.next()),
        (Some(letter), Some(':')) if letter.is_ascii_alphabetic()
    );
    let mut out = String::with_capacity(command.len() + 8);
    out.push('"');
    if drive_like {
        out.push(' ');
    }
    for c in command.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ── Environment and scratch folder ──────────────────────────────────────────

/// Folder of the per-conversation scratch folders, in Shodh's data folder.
pub const SCRATCH_DIR: &str = "code-scratch";

/// The scratch folder of `conversation_id`'s commands: its `HOME` and
/// temporary folder. Named by a hash of the id, so any id gives one plain
/// folder name.
pub fn scratch_dir(data_dir: &Path, conversation_id: &str) -> PathBuf {
    use sha2::{Digest, Sha256};
    let digest = hex::encode(Sha256::digest(conversation_id.as_bytes()));
    data_dir.join(SCRATCH_DIR).join(&digest[..32])
}

/// Remove `conversation_id`'s scratch folder (the conversation was
/// deleted). Nothing to remove is fine.
pub fn remove_scratch(data_dir: &Path, conversation_id: &str) -> Result<(), String> {
    let dir = scratch_dir(data_dir, conversation_id);
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("{} could not be removed: {e}", dir.display())),
    }
}

/// Create the scratch folder if needed, the user's alone. A folder that
/// cannot be made private is removed again. An existing one must be a real
/// folder, not a link or junction to somewhere else, and is kept private
/// ([`platform::keep_private`]).
pub fn prepare_scratch(dir: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(dir) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
            return Err(format!(
                "{} is not a plain folder (a link, junction or file is in its place), so \
                 Shodh does not use it",
                dir.display()
            ))
        }
        Ok(_) => return platform::keep_private(dir),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("{} could not be read: {e}", dir.display())),
    }
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("{} could not be created: {e}", dir.display()))?;
    platform::make_private(dir).inspect_err(|_| {
        if let Err(e) = std::fs::remove_dir_all(dir) {
            tracing::warn!(target: "shodh::harness", error = %e, "a scratch folder that could not be made private was not removed");
        }
    })
}

/// Toolchain folders a command needs although `HOME` is the scratch folder:
/// rustup's proxies (`cargo`, `rustc` on `PATH`) look for their toolchains
/// there. Each is the user's own setting, else the default folder in the
/// real home when it exists.
pub const TOOLCHAIN_HOMES: [(&str, &str); 2] =
    [("CARGO_HOME", ".cargo"), ("RUSTUP_HOME", ".rustup")];

/// Settings every command runs with, so the user's accounts are not used
/// without a prompt that says so:
/// - cargo: the crates.io token comes only from the environment, and this
///   one is not a token (`credentials.toml` in the real `CARGO_HOME` would
///   otherwise publish, yank or change owners silently);
/// - git: no credential helper (Git for Windows' system settings name the
///   Credential Manager, macOS's the keychain), and no prompt for one.
pub const ACCOUNT_GUARDS: [(&str, &str); 7] = [
    (
        "CARGO_REGISTRY_TOKEN",
        "shodh-commands-do-not-use-your-crates-io-token",
    ),
    ("CARGO_REGISTRY_GLOBAL_CREDENTIAL_PROVIDERS", "cargo:token"),
    ("GIT_CONFIG_COUNT", "1"),
    ("GIT_CONFIG_KEY_0", "credential.helper"),
    ("GIT_CONFIG_VALUE_0", ""),
    ("GIT_TERMINAL_PROMPT", "0"),
    ("GCM_INTERACTIVE", "never"),
];

/// A command's whole environment: the system variables programs need
/// ([`super::sidecar::PASSTHROUGH_VARS`] without the proxy settings), the
/// toolchain folders ([`TOOLCHAIN_HOMES`], found from the user's real
/// `home`), the account guards ([`ACCOUNT_GUARDS`]), the home and temporary
/// folders in `scratch`, and the run's marker ([`RUN_ID_ENV`]). Nothing
/// else of Shodh's environment is inherited: no provider key, token or Shodh
/// setting. (Inherited is all this promises: on Windows, which has no sandbox
/// yet, a command can still read what any program of the user can.)
pub fn command_env(
    scratch: &Path,
    command_id: &str,
    parent: impl Fn(&str) -> Option<String>,
    home: Option<&Path>,
) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = super::sidecar::PASSTHROUGH_VARS
        .iter()
        .filter(|name| !name.to_ascii_lowercase().contains("proxy"))
        .filter_map(|name| parent(name).map(|value| (name.to_string(), value)))
        .collect();
    for (name, default) in TOOLCHAIN_HOMES {
        let value = parent(name)
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                home.map(|home| home.join(default))
                    .filter(|dir| dir.is_dir())
                    .map(|dir| dir.display().to_string())
            });
        if let Some(value) = value {
            env.push((name.to_string(), value));
        }
    }
    for (name, value) in ACCOUNT_GUARDS {
        env.push((name.to_string(), value.to_string()));
    }
    let scratch = scratch.display().to_string();
    for name in ["HOME", "USERPROFILE", "TEMP", "TMP", "TMPDIR"] {
        env.push((name.to_string(), scratch.clone()));
    }
    env.push((RUN_ID_ENV.to_string(), command_id.to_string()));
    env
}

// ── Output ──────────────────────────────────────────────────────────────────

/// Where [`OutputFilter`] is inside an escape sequence.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Escape {
    #[default]
    None,
    /// After ESC.
    Start,
    /// A control sequence (`ESC [`), until its final byte.
    Csi,
    /// An operating-system command (`ESC ]`), until BEL, `ESC \`, a line
    /// break or [`MAX_OSC_BYTES`].
    Osc,
    /// ESC inside an operating-system command.
    OscEnd,
    /// The intermediate bytes of a two-part escape (`ESC ( B` chooses a
    /// character set), until its final byte.
    Intermediate,
}

/// Longest operating-system command (a window title, a link) removed from
/// output; a longer one is not one, and the text after it is kept.
const MAX_OSC_BYTES: usize = 4 * 1024;

/// Terminal control removed from command output, which is shown as plain
/// text: colour and cursor sequences are dropped, and a carriage return (a
/// redrawn progress line) becomes a line break. The state is kept across
/// chunks, so a sequence split between two reads is still removed.
#[derive(Debug, Default)]
pub struct OutputFilter {
    escape: Escape,
    carriage_return: bool,
    /// Bytes of the operating-system command being removed.
    osc_bytes: usize,
}

impl OutputFilter {
    /// `bytes` without terminal control.
    pub fn clean(&mut self, bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(bytes.len());
        for &byte in bytes {
            match self.escape {
                Escape::Start => {
                    self.escape = match byte {
                        b'[' => Escape::Csi,
                        b']' => {
                            self.osc_bytes = 0;
                            Escape::Osc
                        }
                        0x20..=0x2f => Escape::Intermediate,
                        _ => Escape::None,
                    };
                    continue;
                }
                Escape::Intermediate => {
                    if !(0x20..=0x2f).contains(&byte) {
                        self.escape = Escape::None;
                    }
                    continue;
                }
                Escape::Csi => match byte {
                    0x40..=0x7e => {
                        self.escape = Escape::None;
                        continue;
                    }
                    0x20..=0x3f => continue,
                    // Not a control sequence after all: the byte is text.
                    _ => self.escape = Escape::None,
                },
                Escape::Osc | Escape::OscEnd => {
                    self.osc_bytes += 1;
                    let after_escape = self.escape == Escape::OscEnd;
                    match byte {
                        0x07 => {
                            self.escape = Escape::None;
                            continue;
                        }
                        b'\\' if after_escape => {
                            self.escape = Escape::None;
                            continue;
                        }
                        0x1b => {
                            self.escape = Escape::OscEnd;
                            continue;
                        }
                        // Unterminated: the line, or too long a sequence,
                        // ends it, and the text goes on.
                        b'\n' => self.escape = Escape::None,
                        _ if self.osc_bytes > MAX_OSC_BYTES => self.escape = Escape::None,
                        _ => {
                            self.escape = Escape::Osc;
                            continue;
                        }
                    }
                }
                Escape::None => {}
            }
            if self.carriage_return {
                self.carriage_return = false;
                out.push(b'\n');
                if byte == b'\n' {
                    continue;
                }
            }
            match byte {
                0x1b => self.escape = Escape::Start,
                b'\r' => self.carriage_return = true,
                _ => out.push(byte),
            }
        }
        out
    }

    /// What was held back at the end of the output.
    pub fn finish(&mut self) -> Vec<u8> {
        self.escape = Escape::None;
        if std::mem::take(&mut self.carriage_return) {
            vec![b'\n']
        } else {
            Vec::new()
        }
    }
}

// ── Running ─────────────────────────────────────────────────────────────────

/// Why the main loop of a run ended.
enum Ended {
    Exited(Option<std::process::ExitStatus>),
    TimedOut,
    Stopped,
    Killed(String),
}

/// Copy one output pipe into `tx` until it closes.
async fn pump<R: tokio::io::AsyncRead + Unpin>(mut reader: R, tx: mpsc::Sender<Vec<u8>>) {
    let mut buf = vec![0u8; 8 * 1024];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if tx.send(buf[..n].to_vec()).await.is_err() {
                    break;
                }
            }
        }
    }
}

/// Run `spec` in `shell` (or refuse with the reason there is none) in this
/// system's sandbox: [`run_bounded`] with the system's own boundary.
pub async fn run_contained(
    spec: CommandSpec,
    shell: Result<PathBuf, String>,
    control: RunControl,
) -> CommandOutcome {
    run_bounded(spec, shell, platform::Boundary::system(), control).await
}

/// Run `spec` in `shell` (or refuse with the reason there is none) in the
/// sandbox of `boundary`: a fresh contained process
/// ([`platform::spawn_contained`], which refuses it when the sandbox cannot
/// be set up) whose output is kept
/// within [`OUTPUT_CAP_BYTES`] and sent live through `control`. The main
/// process exiting, the time limit, a memory limit or [`stop`] ends the run;
/// then every process it started is stopped before the output pipes are
/// read to their end, so a background process holding them cannot keep the
/// call open.
pub async fn run_bounded(
    spec: CommandSpec,
    shell: Result<PathBuf, String>,
    boundary: platform::Boundary,
    control: RunControl,
) -> CommandOutcome {
    let started = Instant::now();
    let shell = match shell {
        Ok(shell) => shell,
        Err(reason) => return CommandOutcome::refused(reason),
    };
    if let Err(e) = prepare_scratch(&spec.scratch) {
        return CommandOutcome::refused(format!(
            "Shodh could not prepare the command's scratch folder: {e}"
        ));
    }
    let registration = &control.registration;
    let home = dirs::home_dir();
    let env = command_env(
        &spec.scratch,
        &control.command_id,
        |name| std::env::var(name).ok(),
        home.as_deref(),
    );
    let (mut child, mut contained) =
        match platform::spawn_contained(&shell, &spec, &env, &control.command_id, &boundary) {
            Ok(spawned) => spawned,
            Err(reason) => return CommandOutcome::refused(reason),
        };
    registration.set_killer(contained.killer());
    (control.on_started)();
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(64);
    if let Some(stdout) = child.stdout.take() {
        tokio::spawn(pump(stdout, tx.clone()));
    }
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(pump(stderr, tx.clone()));
    }
    drop(tx);

    let mut output = CappedOutput::new(OUTPUT_CAP_BYTES);
    let mut live = LiveOutput::default();
    let mut filter = OutputFilter::default();
    let deadline = tokio::time::Instant::now() + spec.limits.time;
    let mut tick = tokio::time::interval(LIVE_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut pipes_open = true;
    let ended = loop {
        if registration.stop_requested() {
            break Ended::Stopped;
        }
        tokio::select! {
            status = child.wait() => break Ended::Exited(status.ok()),
            _ = tokio::time::sleep_until(deadline) => break Ended::TimedOut,
            _ = registration.stopped() => break Ended::Stopped,
            chunk = rx.recv(), if pipes_open => match chunk {
                Some(bytes) => {
                    let bytes = filter.clean(&bytes);
                    output.push(&bytes);
                    live.push(&bytes);
                }
                None => pipes_open = false,
            },
            _ = tick.tick() => {
                if let Some(text) = live.take(false) {
                    (control.on_output)(text);
                }
                if let Some(limit) = contained.over_limit() {
                    break Ended::Killed(limit);
                }
            }
        }
    };
    // Every process still running goes, then the pipes are read to their end.
    // The sweep lists processes and waits between passes: off the async
    // workers. Dropping this future meanwhile does not stop it.
    let swept = tokio::task::spawn_blocking(move || {
        let left = contained.terminate();
        (left, contained.notes(), contained.escaped())
    })
    .await;
    let (left, mut notes, escaped) = swept.unwrap_or_else(|e| {
        tracing::warn!(target: "shodh::harness", error = %e, "the end-of-command sweep did not finish");
        (
            0,
            vec!["Shodh could not confirm that every process it started was stopped.".to_string()],
            true,
        )
    });
    let status = match ended {
        Ended::Exited(status) => status,
        _ => {
            if let Err(e) = child.start_kill() {
                tracing::debug!(target: "shodh::harness", error = %e, "the command's process had already ended");
            }
            tokio::time::timeout(DRAIN_GRACE, child.wait())
                .await
                .ok()
                .and_then(Result::ok)
        }
    };
    let drain_until = tokio::time::Instant::now() + DRAIN_GRACE;
    while pipes_open {
        match tokio::time::timeout_at(drain_until, rx.recv()).await {
            Ok(Some(bytes)) => {
                let bytes = filter.clean(&bytes);
                output.push(&bytes);
                live.push(&bytes);
            }
            Ok(None) | Err(_) => pipes_open = false,
        }
    }
    let rest = filter.finish();
    output.push(&rest);
    live.push(&rest);
    if let Some(text) = live.take(true) {
        (control.on_output)(text);
    }
    let (state, exit_code) = match ended {
        Ended::Exited(_) => match exit_of(status.as_ref()) {
            Exit::Code(code) => (CommandState::Done, Some(code)),
            Exit::Limit(limit) => {
                notes.insert(0, limit);
                (CommandState::Killed, None)
            }
            Exit::Signal(signal) => {
                notes.push(format!("It was ended by signal {signal}."));
                (CommandState::Done, None)
            }
            Exit::Unknown => (CommandState::Done, None),
        },
        Ended::TimedOut => (CommandState::TimedOut, None),
        Ended::Stopped => (CommandState::Stopped, None),
        Ended::Killed(limit) => {
            notes.insert(0, limit);
            (CommandState::Killed, None)
        }
    };
    if state == CommandState::Done && left > 0 {
        notes.push(format!(
            "{left} process{} it started {} still running when it ended and {} stopped.",
            if left == 1 { "" } else { "es" },
            if left == 1 { "was" } else { "were" },
            if left == 1 { "was" } else { "were" },
        ));
    }
    let output = output.text();
    // A failure the sandbox may explain is explained in words.
    #[cfg(target_os = "macos")]
    if state == CommandState::Done && exit_code != Some(0) {
        if let Some(hint) = failure_hint(&output) {
            notes.push(hint.to_string());
        }
    }
    CommandOutcome {
        state,
        exit_code,
        note: (!notes.is_empty()).then(|| notes.join(" ")),
        output,
        duration: started.elapsed(),
        escaped,
    }
}

/// How a process's exit reads.
enum Exit {
    Code(i32),
    /// A limit the system enforced ended it (in words).
    Limit(String),
    Signal(i32),
    Unknown,
}

fn exit_of(status: Option<&std::process::ExitStatus>) -> Exit {
    let Some(status) = status else {
        return Exit::Unknown;
    };
    if let Some(code) = status.code() {
        return Exit::Code(code);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return platform::signal_exit(signal)
                .map_or(Exit::Signal(signal), |limit| Exit::Limit(limit.to_string()));
        }
    }
    Exit::Unknown
}

/// A command's text shortened for labels and logs.
pub fn short_command(command: &str) -> String {
    truncate_chars(
        &command.split_whitespace().collect::<Vec<_>>().join(" "),
        80,
    )
}
