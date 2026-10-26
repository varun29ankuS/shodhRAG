# Sandbox step 1a: Code mode commands run in a Shodh-owned process

Status: implementation plan for PR 1a of the sandbox proposal (step 1). Step 1b
(AppContainer on Windows, Seatbelt on macOS) builds on it.

## Why

Code mode's only code execution was omp's `bash` tool, an embedded shell inside
omp's own process. That process holds the LLM provider's API key in its
environment, runs with the user's full rights and has no limits, and anything a
command started in the background outlived it. A prompt injection from a cloned
repository could read the key, leave a process running, or exhaust the machine.

## Decisions

1. **omp's `bash` is off.** `CODE_TOOLS` is `read, grep, glob, ast_grep, edit,
   write`; the launch flags, the pinned settings (`bash.enabled: false`) and the
   guard agree. A session's tool inventory must be exactly those plus the host
   tools Shodh registered, must contain `run_command`, and must not contain any
   of omp's execution tools (`bash`, `eval`, `run_code`, `debug`, `ida`, `task`,
   `computer`, `browser`, `vibe_spawn`, `vibe_send`): a future omp that adds one
   fails the session closed. The guard blocks those names outright and never
   treats them as host tools, even when `SHODH_HOST_TOOLS` names them.

2. **`run_command { command, timeout_secs? }` is a host tool the Code session
   owns.** It is not in the tool registry (Research sessions never see it); the
   session adds its definition to `set_host_tools` and to the guard's host tool
   list, and intercepts its calls in `dispatch_host_tool`. Because it bypasses the
   registry, the session re-applies the registry's protections itself: schema
   validation (exactly `command` and `timeout_secs`), the per-answer tool budget,
   a `tool_call` audit event, the step catalog entry, and no "allow for this
   conversation" grant. A registry or MCP tool named `run_command` stops the
   session at start.

3. **Approval is the guarded path, moved, not the generic host-tool gate.** The
   call goes through `decide_guarded` exactly as omp's `bash` did: branch before
   the first change (`plan_change`), `destructive_reason` warnings, the allowlist
   with the Step 0 rule (review token, `command_eligible`, the review list, cut
   previews, before/after tree snapshots, uncompared-call records), Inbox
   approvals that run the command but review nothing, and the time limit shown in
   the prompt. `GuardRequest.ui_id` becomes optional (a host call has no omp
   dialog). Code that only served omp's bash is deleted: `background_arguments`
   (`async`/`name`/`ready`), the `cwd` and `pty` handling, the "no time limit"
   wording. A `timeout_secs` the model sets still makes a command ineligible to
   run unasked, as any argument did.

4. **The process.** One fresh process per call, in the code folder, stdin null,
   the command passed as one argument to `bash -c`:
   - Windows: Git for Windows' `bash.exe` (registry `HKLM\SOFTWARE\GitForWindows
     InstallPath`, then `%ProgramFiles%\Git\bin\bash.exe`, then a `bash.exe` on
     `PATH` that sits in a Git for Windows tree; never `System32` or
     `WindowsApps`). Not found: the command is refused with a message to install
     Git for Windows; there is no PowerShell or cmd fallback. The argument is
     written with `raw_arg`, always double-quoted, backslashes and quotes escaped,
     so the MSYS2 runtime neither splits nor globs it.
   - macOS: `/bin/bash -c`.
   - Environment built from scratch: the sidecar's `PASSTHROUGH_VARS` without the
     proxy variables, and `HOME`, `USERPROFILE`, `TEMP`, `TMP`, `TMPDIR` pointing
     at a per-conversation scratch folder under Shodh's app data
     (`code-scratch/<sha256 of the conversation id>`, user-only: mode 0700, or a
     protected DACL granting only the user on Windows, set when the folder is
     made; removed when the conversation is deleted). No provider keys, no
     `SHODH_*` secrets, no tokens. `CARGO_HOME` and `RUSTUP_HOME` are passed (the
     user's own values, else `~/.cargo` and `~/.rustup` when they exist): with
     `HOME` in the scratch folder, rustup's `cargo` proxy would otherwise find no
     toolchain and the default allowlist's `cargo test` would fail. On Windows,
     Git for Windows' `usr/bin` and `mingw64/bin` folders come first on `PATH`.

5. **Limits.**
   - Windows: the process is created suspended, assigned to a new Job Object, then
     resumed (its one thread found through a Toolhelp snapshot). The job has
     `KILL_ON_JOB_CLOSE`, an active-process limit of 64, a job memory limit
     (setting, default 4 GB), a hard CPU-rate cap (setting, default 80%),
     `DIE_ON_UNHANDLED_EXCEPTION`, every `UILIMIT_*`, and neither `BREAKAWAY_OK`
     nor `SILENT_BREAKAWAY_OK`. A completion port reports when the memory or
     process limit refused something; a job memory limit makes allocations fail,
     it kills nothing, and the result says exactly that. Any step failing
     terminates the suspended process and refuses the command (fail closed).
   - macOS: a new session and process group (`setsid` in `pre_exec`),
     `RLIMIT_CPU` (the time limit on every core plus a margin: CPU time adds up
     over threads, so a parallel `cargo test` must not reach it before the wall
     clock; it bounds a process that escaped the sweep), and a watchdog that sums `ri_phys_footprint` over the run's
     processes every 200 ms and kills them all over the memory limit. The run is
     marked with a `SHODH_RUN_COMMAND_ID` variable; the sweep finds the process
     group, every descendant (from the `PROC_PIDTBSDINFO` table, not
     `proc_listchildpids`), and every process of the user that still carries the
     marker (looked for among its orphans, the processes launchd adopted), stops
     them all, kills them, and repeats until a pass finds none.
     A process that escaped the tree and also dropped the marker (re-exec with an
     empty environment) is not found; 1b's Seatbelt profile bounds what it can do.
   - Both: a wall-clock limit (setting, default 10 minutes; the model may ask for
     less, never more) and output capped at 1 MB (head and tail kept, the middle
     cut with a marker). The model gets a smaller head and tail (the tool result
     travels in one omp frame); the UI and logs get the 1 MB.
   - The end condition is the main process exiting, the time limit, or Stop. Then
     the job is terminated or the group swept, and only then are the pipes drained
     (bounded), so a background child holding stdout cannot hang the call. Every
     descendant is gone when the command ends: normally, on timeout, on Stop, on
     cancel, when the session closes (the run's handle kills on drop) and on app
     exit (`RunEvent::Exit` stops every registered run; on Windows closing the job
     handle does it too).

6. **Background processes.** `starts_background` still makes a command ineligible
   to run unasked and still notes it on the prompt, now honestly: "Shodh stops
   every process this command started when it ends." Since nothing outlives the
   command, the per-conversation `background` list is no longer added to (old
   entries are still honoured: edits keep asking).

7. **Edits while a command runs.** omp schedules host tools like its old bash
   (shared, edits exclusive), so an edit never executes during a command of the
   same batch. As defence in depth, edits are applied without asking only while
   no command of the conversation is running.

8. **Live status and Stop.** New `AgentEvent`s per command: `command_started`
   (command id, command, time limit), `command_output` (stdout and stderr
   interleaved, at most about ten events a second, each bounded), and
   `command_finished` (state `done` / `timed_out` / `stopped` / `killed` /
   `refused`, exit code, a note such as which limit acted, duration). The
   Code-mode transcript shows a panel per command: the command, elapsed time,
   streaming output (tail kept), the state, and a Stop button calling
   `run_command_stop(commandId)`, which terminates the job or process group at
   once, also while the model waits on the tool; the tool result tells the model
   the user stopped it. Finished runs collapse to one line with expandable
   output. `run_command_stop` is excluded from agent coverage: it is the user's
   control over the agent's own command.

9. **Settings** (workspace Code section): time limit (10 s to 60 min, default
   10 min), memory limit (256 MB to 64 GB, default 4 GB), CPU cap (10% to 100%,
   default 80%). Saving rejects values out of range; a hand-edited file is
   clamped on load.

10. **Output** is plain text: terminal colour and cursor sequences are removed
    and a carriage return (a redrawn progress line) becomes a line break, in the
    runner, so the model, the panel and the logs see the same text.

## Where 1b plugs in

`run_command/mod.rs` resolves the shell, builds the environment and the scratch
folder, and calls `spawn_contained` in the platform module, the one place a
process is created. 1b wraps that call: on Windows the process is created with an
AppContainer token (`PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES`) before it is
put in the same job; on macOS `/bin/bash` is started through `sandbox-exec -p`
with a deny-default profile. Both keep the fail-closed rule: if the sandbox cannot
be set up, the command is refused. 1b must also grant read and execute access to
the toolchain folders this step passes (`CARGO_HOME`, `RUSTUP_HOME`, Python's
folders) to the AppContainer SID, or `cargo test` and `python` stop working.

## Tests (CI, Windows and macOS)

A test-only crate `crates/sandbox-probe` (no dependencies but `libc` on Unix)
builds the `sandbox-probe <case>` binary the tests launch through the runner,
with no model and no network. CI builds it before the library tests; the tests
fail when it is missing. Each negative case is paired with a control run of the
same probe outside the runner.

| Case | Expected |
|---|---|
| Key in environment (`OPENROUTER_API_KEY` and every provider key name, a `SHODH_*` variable, an arbitrary one, all random canaries) | absent in the command; present in the control |
| Persistence: a detached child (breakaway attempted on Windows, `setsid` on macOS) writing a heartbeat every 100 ms | heartbeat stops within 1 s of the end; no process left; the control's keeps beating |
| Fork loop | Windows refuses past 64 processes; the run ends at its time limit; nothing left |
| Busy loop; 8 GB allocation | timed out at the wall limit; the allocation fails in the job (Windows) or the watchdog kills it (macOS); the same allocation passes in the control |
| Stop | a long command with children is stopped through the API: state Stopped, every descendant gone |
| Usability | `ls \| grep x && python -c 'import sys'` and `git status` in a temp repository succeed; a hostile command line (quotes, backslashes, `$`, `!`, `%`, newlines, non-ASCII, `ls*`) reaches bash byte for byte |
| Git Bash missing | refused with the message; nothing runs |
| Inventory | a Code session's tools contain `run_command` and none of `bash`, `eval`, `run_code` |

The #96 approval tests are kept and moved to `run_command`, with an executor that
the test releases, in place of omp's end frame.

## Security review decisions (PR #105)

- **What the prompt shows is what runs.**
  - A command is at most as long as the prompt shows in full (2,000 characters).
  - A command is refused when it holds control characters (other than line breaks and tabs), bidirectional overrides, zero-width or other invisible characters, or a run of more than 100 blank characters.
- **The prompt promises only what this system enforces.**
  - Windows: job memory, a CPU hard cap and 64 processes.
  - macOS: the memory watchdog and 64 processes, through `RLIMIT_NPROC` set to the user's process count at the start plus 64 (macOS counts processes per user). macOS has no CPU cap, so commands there run at nice 10 and the prompt and settings say so.
- **Programs the system starts are not stopped.** That covers scheduled tasks, services, launch agents and jobs, and apps opened through the desktop (`schtasks`, `sc`, `wmic`, `Invoke-CimMethod`, `Register-ScheduledTask`, `explorer`, `launchctl`, `open`, `osascript`, `at`, `crontab`, `systemd-run` ...).
  - Such commands ask, and their prompt says so.
  - Once approved, they are recorded as running on, so edits ask from then on, as before this step.
  - On macOS, a final sweep that finds processes which left the run's process group marks what the command changed as unknown.
- **Accounts.**
  - Commands that publish or use the user's registry or GitHub account are flagged and always ask: cargo `publish`/`yank`/`owner`/`login`, npm-family `publish`/`unpublish`/`deprecate`/`token`/`owner`/`login`, `twine`/`poetry`/`uv`/`gem` publish, `docker push`/`login`, and `gh` subcommands that act on the account.
  - Every command also gets a `CARGO_REGISTRY_TOKEN` that is not a token, with `cargo:token` as the only credential provider. It gets no git credential helper (`credential.helper` reset through `GIT_CONFIG_*`), with `GIT_TERMINAL_PROMPT=0` and `GCM_INTERACTIVE=never`.
- **`CARGO_HOME` stays the user's own.** A scratch `CARGO_HOME` per conversation would download the registry index and every crate again for each conversation, and once step 1b cuts the network it would fail `cargo test` outright. The token is the only secret in it, and the environment above overrides it. Until 1b, a command can read `credentials.toml` like any other file the user can read.
- **"Not inherited" is what step 1a promises.** Same-user secrets stay reachable until 1b. Step 1b must test that, inside the sandbox:
  - the Credential Manager (`CredRead`, `PasswordVault`) and the keychain (`security find-generic-password -s com.shodh.rag-app`) fail;
  - other processes' memory and environments (`ReadProcessMemory`, `ps eww`) cannot be read;
  - Shodh's data folder (the scratch folder's parent) is denied.
- **Monitoring.** The macOS watchdog runs on its own thread over the group and its descendants. The final sweep, which also looks for marked orphans, runs on a blocking thread.
- **Cancel.** A cancelled command, or one still running when the answer ends, is stopped rather than dropped. What it changed is compared only after its processes are gone.
- **Handles (Windows).** Rust's standard library serializes every process creation behind one lock (`CREATE_PROCESS_LOCK`), so concurrent spawns do not leak pipe ends to each other, and Shodh creates no process any other way. The command still inherits any handle that native code inside Shodh made inheritable. Restricting that needs `PROC_THREAD_ATTRIBUTE_HANDLE_LIST`, which stable Rust's `Command` cannot pass (`raw_attribute` is unstable). It lands with 1b's own `CreateProcessW`, whose AppContainer attribute uses the same attribute list.
- **Git for Windows is looked for in this order:** installs for all users (`HKLM`, Program Files), then the user's own (`HKCU`), then `PATH`.
- **Scratch folder.** It must be a real folder, not a link or junction. On macOS its mode is set again before every command. On Windows it gets a protected user-only DACL when it is created. It lives inside the user's own application data, which no other account can change, so setting the DACL again would only rewalk the folder before each command.
