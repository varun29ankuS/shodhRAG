//! Windows: Git for Windows' bash, in a Job Object.
//!
//! The process is created suspended and without a console window, put in a
//! new Job Object, then resumed, so nothing it runs ever starts outside the
//! job. The job:
//! - kills every process in it when its last handle closes
//!   (`KILL_ON_JOB_CLOSE`: also when Shodh itself exits or crashes);
//! - allows at most [`super::MAX_PROCESSES`] processes at once;
//! - caps the memory of all its processes together (an allocation beyond it
//!   fails; nothing is killed for it);
//! - caps CPU at a share of the whole machine (hard cap);
//! - ends a process on an unhandled exception instead of showing a dialog;
//! - blocks the user-interface actions of `JOB_OBJECT_UILIMIT_*` (desktops,
//!   the clipboard, system and display settings, logging off, global atoms,
//!   other processes' windows);
//! - and allows no breakaway: neither `BREAKAWAY_OK` nor
//!   `SILENT_BREAKAWAY_OK` is set, so `CREATE_BREAKAWAY_FROM_JOB` fails.
//!
//! A completion port tells when a limit refused something. Any step failing
//! ends the suspended process and refuses the command.

use std::ffi::{c_void, OsStr};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use tokio::process::Child;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, LocalFree, BOOL, ERROR_SUCCESS, GENERIC_ALL, HANDLE, HLOCAL, INVALID_HANDLE_VALUE,
};
use windows::Win32::Security::Authorization::{
    SetEntriesInAclW, SetNamedSecurityInfoW, EXPLICIT_ACCESS_W, NO_MULTIPLE_TRUSTEE, SET_ACCESS,
    SE_FILE_OBJECT, TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W,
};
use windows::Win32::Security::{
    GetTokenInformation, TokenUser, ACL, DACL_SECURITY_INFORMATION,
    PROTECTED_DACL_SECURITY_INFORMATION, PSID, SUB_CONTAINERS_AND_OBJECTS_INHERIT, TOKEN_QUERY,
    TOKEN_USER,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectAssociateCompletionPortInformation,
    JobObjectBasicProcessIdList, JobObjectBasicUIRestrictions, JobObjectCpuRateControlInformation,
    JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
    TerminateJobObject, JOBOBJECTINFOCLASS, JOBOBJECT_ASSOCIATE_COMPLETION_PORT,
    JOBOBJECT_BASIC_PROCESS_ID_LIST, JOBOBJECT_BASIC_UI_RESTRICTIONS,
    JOBOBJECT_CPU_RATE_CONTROL_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_CPU_RATE_CONTROL_ENABLE, JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP,
    JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION,
    JOB_OBJECT_LIMIT_JOB_MEMORY, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_UILIMIT_DESKTOP,
    JOB_OBJECT_UILIMIT_DISPLAYSETTINGS, JOB_OBJECT_UILIMIT_EXITWINDOWS,
    JOB_OBJECT_UILIMIT_GLOBALATOMS, JOB_OBJECT_UILIMIT_HANDLES, JOB_OBJECT_UILIMIT_READCLIPBOARD,
    JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS, JOB_OBJECT_UILIMIT_WRITECLIPBOARD,
};
use windows::Win32::System::JobObjects::{
    JOBOBJECT_BASIC_LIMIT_INFORMATION, JOBOBJECT_CPU_RATE_CONTROL_INFORMATION_0,
};
use windows::Win32::System::Registry::{
    RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, REG_ROUTINE_FLAGS, RRF_RT_REG_SZ,
    RRF_SUBKEY_WOW6432KEY, RRF_SUBKEY_WOW6464KEY,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, OpenThread, QueryFullProcessImageNameW,
    ResumeThread, CREATE_NO_WINDOW, CREATE_SUSPENDED, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, THREAD_SUSPEND_RESUME,
};
use windows::Win32::System::IO::{CreateIoCompletionPort, GetQueuedCompletionStatus, OVERLAPPED};

use super::{CommandSpec, Killer, MAX_PROCESSES, NO_GIT_BASH};

/// Completion-port message: the active-process limit refused a process
/// (`JOB_OBJECT_MSG_ACTIVE_PROCESS_LIMIT`, winnt.h).
const MSG_ACTIVE_PROCESS_LIMIT: u32 = 3;
/// Completion-port message: the job memory limit refused an allocation
/// (`JOB_OBJECT_MSG_JOB_MEMORY_LIMIT`, winnt.h).
const MSG_JOB_MEMORY_LIMIT: u32 = 10;

/// Most process ids read from the job at once (the limit, with room for
/// processes still being counted out).
const PID_LIST_CAPACITY: usize = MAX_PROCESSES as usize * 2;

/// A kernel handle Shodh owns, closed when dropped.
#[derive(Debug)]
struct Owned(HANDLE);

// SAFETY: a kernel handle is a process-wide value, not tied to the thread
// that opened it; the objects held here (a job, a completion port, a token,
// a snapshot) may be used from any thread.
unsafe impl Send for Owned {}
// SAFETY: as above; the job and port calls made through a shared handle are
// thread-safe kernel calls.
unsafe impl Sync for Owned {}

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: the handle was opened by Shodh and is closed only here.
        if let Err(e) = unsafe { CloseHandle(self.0) } {
            tracing::debug!(target: "shodh::harness", error = %e, "closing a handle failed");
        }
    }
}

/// `text` as a NUL-terminated UTF-16 string.
fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain(std::iter::once(0)).collect()
}

/// A `REG_SZ` value, read from the given registry view.
fn registry_string(root: HKEY, view: REG_ROUTINE_FLAGS, key: &str, value: &str) -> Option<String> {
    let key = wide(OsStr::new(key));
    let value = wide(OsStr::new(value));
    let flags = RRF_RT_REG_SZ | view;
    let mut size: u32 = 0;
    // SAFETY: the key and value names are NUL-terminated and outlive the
    // call; only the size is asked for.
    let status = unsafe {
        RegGetValueW(
            root,
            PCWSTR(key.as_ptr()),
            PCWSTR(value.as_ptr()),
            flags,
            None,
            None,
            Some(&mut size as *mut u32),
        )
    };
    if status != ERROR_SUCCESS || size == 0 {
        return None;
    }
    let mut buffer = vec![0u16; (size as usize).div_ceil(2)];
    // SAFETY: the buffer holds `size` bytes, as the call reports them.
    let status = unsafe {
        RegGetValueW(
            root,
            PCWSTR(key.as_ptr()),
            PCWSTR(value.as_ptr()),
            flags,
            None,
            Some(buffer.as_mut_ptr().cast::<c_void>()),
            Some(&mut size as *mut u32),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    let text = String::from_utf16_lossy(&buffer[..end]);
    (!text.trim().is_empty()).then(|| text.trim().to_string())
}

/// The folders Git for Windows' installer recorded: for all users (64- and
/// 32-bit registry views), then for the current user.
fn registry_install_paths(roots: &[(HKEY, REG_ROUTINE_FLAGS)]) -> Vec<PathBuf> {
    const KEY: &str = "SOFTWARE\\GitForWindows";
    let mut paths: Vec<PathBuf> = Vec::new();
    for &(root, view) in roots {
        if let Some(path) = registry_string(root, view, KEY, "InstallPath").map(PathBuf::from) {
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
    }
    paths
}

/// Git for Windows' bash ([`super::git_bash`]), or why commands cannot run.
/// An install for all users (recorded under `HKLM`, which only
/// administrators can change, or in Program Files) wins over one recorded
/// for this user under `HKCU`, which any of the user's programs can change.
pub fn locate_shell() -> Result<PathBuf, String> {
    let sources = super::ShellSources {
        install_path: registry_install_paths(&[
            (HKEY_LOCAL_MACHINE, RRF_SUBKEY_WOW6464KEY),
            (HKEY_LOCAL_MACHINE, RRF_SUBKEY_WOW6432KEY),
        ]),
        program_files: std::env::var_os("ProgramW6432")
            .or_else(|| std::env::var_os("ProgramFiles"))
            .map(PathBuf::from),
        user_install_path: registry_install_paths(&[(HKEY_CURRENT_USER, REG_ROUTINE_FLAGS(0))]),
        path: std::env::var_os("PATH"),
    };
    super::git_bash(&sources).ok_or_else(|| NO_GIT_BASH.to_string())
}

/// The current user's token information (`TOKEN_USER`), in a buffer aligned
/// for it.
fn current_user() -> Result<Vec<u64>, String> {
    let mut token = HANDLE::default();
    // SAFETY: the pseudo handle of this process needs no closing; the token
    // handle is owned below.
    let opened = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) };
    opened.map_err(|e| format!("reading the user's identity failed: {e}"))?;
    let token = Owned(token);
    let mut length: u32 = 0;
    // SAFETY: asks only for the size; the call fails with "insufficient
    // buffer" and reports it.
    let _size_only = unsafe { GetTokenInformation(token.0, TokenUser, None, 0, &mut length) };
    if length == 0 {
        return Err("reading the user's identity failed".to_string());
    }
    let mut buffer = vec![0u64; (length as usize).div_ceil(8)];
    // SAFETY: the buffer holds `length` bytes and is aligned for TOKEN_USER.
    let read = unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            Some(buffer.as_mut_ptr().cast::<c_void>()),
            length,
            &mut length,
        )
    };
    read.map_err(|e| format!("reading the user's identity failed: {e}"))?;
    Ok(buffer)
}

/// Make `dir` the current user's alone: a protected access list (nothing
/// inherited from its parent) granting only the user, inherited by
/// everything inside.
pub fn make_private(dir: &Path) -> Result<(), String> {
    let user = current_user()?;
    // SAFETY: `user` holds a TOKEN_USER written by GetTokenInformation; its
    // SID points into the same buffer, which outlives every use below.
    let sid: PSID = unsafe { (*user.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    let entry = EXPLICIT_ACCESS_W {
        grfAccessPermissions: GENERIC_ALL.0,
        grfAccessMode: SET_ACCESS,
        grfInheritance: SUB_CONTAINERS_AND_OBJECTS_INHERIT,
        Trustee: TRUSTEE_W {
            pMultipleTrustee: std::ptr::null_mut(),
            MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_USER,
            ptstrName: PWSTR(sid.0.cast::<u16>()),
        },
    };
    let mut acl: *mut ACL = std::ptr::null_mut();
    // SAFETY: one entry naming a valid SID; the new list is freed below.
    let status = unsafe { SetEntriesInAclW(Some(&[entry]), None, &mut acl) };
    if status != ERROR_SUCCESS {
        return Err(format!(
            "{} could not be made private (error {})",
            dir.display(),
            status.0
        ));
    }
    let path = wide(dir.as_os_str());
    // SAFETY: the path is NUL-terminated; the list was built above.
    let status = unsafe {
        SetNamedSecurityInfoW(
            PCWSTR(path.as_ptr()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            PSID::default(),
            PSID::default(),
            Some(acl.cast_const()),
            None,
        )
    };
    // SAFETY: SetEntriesInAclW allocated the list with LocalAlloc.
    unsafe { LocalFree(HLOCAL(acl.cast::<c_void>())) };
    if status != ERROR_SUCCESS {
        return Err(format!(
            "{} could not be made private (error {})",
            dir.display(),
            status.0
        ));
    }
    Ok(())
}

/// Keep an existing scratch folder private. It was made private when it was
/// created ([`make_private`]) and lives in the user's own application data,
/// which no other account can change; setting the access list again would
/// walk every file of the folder before each command.
pub fn keep_private(_dir: &Path) -> Result<(), String> {
    Ok(())
}

/// Set one information class of `job`.
fn set_job_info<T>(
    job: &Owned,
    class: JOBOBJECTINFOCLASS,
    info: &T,
    what: &str,
) -> Result<(), String> {
    let size = u32::try_from(std::mem::size_of::<T>()).map_err(|_| format!("{what}: too large"))?;
    // SAFETY: `info` is the structure `class` expects, `size` bytes long.
    let set =
        unsafe { SetInformationJobObject(job.0, class, (info as *const T).cast::<c_void>(), size) };
    set.map_err(|e| format!("setting the {what} failed: {e}"))
}

/// A new job with every limit set, and its completion port.
fn new_job(spec: &CommandSpec) -> Result<(Owned, Owned), String> {
    // SAFETY: an unnamed job with default security.
    let job = Owned(
        unsafe { CreateJobObjectW(None, PCWSTR::null()) }
            .map_err(|e| format!("creating the job failed: {e}"))?,
    );
    let limits = &spec.limits;
    let extended = JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
        BasicLimitInformation: JOBOBJECT_BASIC_LIMIT_INFORMATION {
            LimitFlags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
                | JOB_OBJECT_LIMIT_JOB_MEMORY
                | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION,
            ActiveProcessLimit: limits.max_processes,
            ..Default::default()
        },
        JobMemoryLimit: usize::try_from(limits.memory_bytes).unwrap_or(usize::MAX),
        ..Default::default()
    };
    set_job_info(
        &job,
        JobObjectExtendedLimitInformation,
        &extended,
        "process and memory limits",
    )?;
    let ui = JOBOBJECT_BASIC_UI_RESTRICTIONS {
        UIRestrictionsClass: JOB_OBJECT_UILIMIT_DESKTOP
            | JOB_OBJECT_UILIMIT_DISPLAYSETTINGS
            | JOB_OBJECT_UILIMIT_EXITWINDOWS
            | JOB_OBJECT_UILIMIT_GLOBALATOMS
            | JOB_OBJECT_UILIMIT_HANDLES
            | JOB_OBJECT_UILIMIT_READCLIPBOARD
            | JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS
            | JOB_OBJECT_UILIMIT_WRITECLIPBOARD,
    };
    set_job_info(&job, JobObjectBasicUIRestrictions, &ui, "interface limits")?;
    if limits.cpu_percent < 100 {
        let cpu = JOBOBJECT_CPU_RATE_CONTROL_INFORMATION {
            ControlFlags: JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP,
            // In hundredths of a percent of the whole machine, 1 to 10,000.
            Anonymous: JOBOBJECT_CPU_RATE_CONTROL_INFORMATION_0 {
                CpuRate: limits.cpu_percent.clamp(1, 100) * 100,
            },
        };
        set_job_info(&job, JobObjectCpuRateControlInformation, &cpu, "CPU cap")?;
    }
    // SAFETY: a new port not tied to a file.
    let port = Owned(
        unsafe { CreateIoCompletionPort(INVALID_HANDLE_VALUE, None, 0, 1) }
            .map_err(|e| format!("creating the job's port failed: {e}"))?,
    );
    let association = JOBOBJECT_ASSOCIATE_COMPLETION_PORT {
        CompletionKey: std::ptr::null_mut(),
        CompletionPort: port.0,
    };
    set_job_info(
        &job,
        JobObjectAssociateCompletionPortInformation,
        &association,
        "job's port",
    )?;
    Ok((job, port))
}

/// Resume the one thread of the suspended process `pid`.
fn resume(pid: u32) -> Result<(), String> {
    // SAFETY: a snapshot of every thread, owned below.
    let snapshot = Owned(
        unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) }
            .map_err(|e| format!("listing threads failed: {e}"))?,
    );
    let mut entry = THREADENTRY32 {
        dwSize: u32::try_from(std::mem::size_of::<THREADENTRY32>()).unwrap_or(u32::MAX),
        ..Default::default()
    };
    let mut resumed = 0usize;
    // SAFETY: `entry` has its size set, as the calls require.
    let mut more = unsafe { Thread32First(snapshot.0, &mut entry) }.is_ok();
    while more {
        if entry.th32OwnerProcessID == pid {
            // SAFETY: the thread handle is owned below.
            let opened =
                unsafe { OpenThread(THREAD_SUSPEND_RESUME, BOOL::from(false), entry.th32ThreadID) };
            let thread = opened
                .map(Owned)
                .map_err(|e| format!("opening the command's thread failed: {e}"))?;
            // SAFETY: a thread of the process Shodh created suspended.
            if unsafe { ResumeThread(thread.0) } == u32::MAX {
                return Err("resuming the command failed".to_string());
            }
            resumed += 1;
        }
        // SAFETY: as above.
        more = unsafe { Thread32Next(snapshot.0, &mut entry) }.is_ok();
    }
    if resumed == 0 {
        return Err("the command's thread was not found".to_string());
    }
    Ok(())
}

/// `env` with Git for Windows' tool folders first on `PATH`, so `ls`, `grep`
/// and the rest are found without the user's shell setup.
fn with_git_path(shell: &Path, env: &[(String, String)]) -> Vec<(String, String)> {
    let Some(git) = shell.parent().and_then(Path::parent) else {
        return env.to_vec();
    };
    let tools = [git.join("usr").join("bin"), git.join("mingw64").join("bin")];
    let mut out: Vec<(String, String)> = env.to_vec();
    let path = out
        .iter()
        .position(|(name, _)| name.eq_ignore_ascii_case("PATH"));
    let prefix = tools
        .iter()
        .map(|dir| dir.display().to_string())
        .collect::<Vec<_>>()
        .join(";");
    match path {
        Some(i) => {
            let rest = std::mem::take(&mut out[i].1);
            out[i] = ("PATH".to_string(), format!("{prefix};{rest}"));
        }
        None => out.push(("PATH".to_string(), prefix)),
    }
    out
}

/// Commands' sandbox on this system: none yet, so the approval prompt says
/// commands are not sandboxed ([`super::sandbox_text`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Boundary;

impl Boundary {
    /// The system's sandbox (none).
    pub fn system() -> Self {
        Boundary
    }
}

/// Start `spec`'s command in `shell`, inside a new job ([`new_job`]).
pub fn spawn_contained(
    shell: &Path,
    spec: &CommandSpec,
    env: &[(String, String)],
    _command_id: &str,
    _boundary: &Boundary,
) -> Result<(Child, Contained), String> {
    let (job, port) = new_job(spec).map_err(|e| {
        format!("Shodh could not set up the command's limits ({e}), so it was not run.")
    })?;
    let mut command = tokio::process::Command::new(shell);
    command
        .arg("-c")
        .raw_arg(super::msys_quoted(&spec.command))
        .current_dir(&spec.folder)
        .env_clear()
        .envs(with_git_path(shell, env))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags((CREATE_SUSPENDED | CREATE_NO_WINDOW).0)
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|e| format!("Shodh could not start bash ({}): {e}", shell.display()))?;
    let placed = match (child.raw_handle(), child.id()) {
        (Some(process), Some(pid)) => {
            // SAFETY: the handle is the suspended child's, open while `child` lives.
            let assigned = unsafe { AssignProcessToJobObject(job.0, HANDLE(process)) };
            assigned
                .map_err(|e| format!("putting the command in its job failed: {e}"))
                .and_then(|()| resume(pid))
        }
        _ => Err("the command's process was not found".to_string()),
    };
    if let Err(e) = placed {
        if let Err(kill) = child.start_kill() {
            tracing::warn!(target: "shodh::harness", error = %kill, "could not end a command that failed to start");
        }
        // SAFETY: the job holds at most the process that failed to start.
        let _ended = unsafe { TerminateJobObject(job.0, 1) };
        return Err(format!(
            "Shodh could not contain the command ({e}), so it was not run."
        ));
    }
    let main_pid = child.id().unwrap_or(0);
    Ok((
        child,
        Contained {
            job: Arc::new(job),
            port,
            main_pid,
            memory_mb: spec.limits.memory_mb(),
            max_processes: spec.limits.max_processes,
            notes: Vec::new(),
        },
    ))
}

/// A running command's job.
#[derive(Debug)]
pub struct Contained {
    job: Arc<Owned>,
    port: Owned,
    main_pid: u32,
    memory_mb: u64,
    max_processes: u32,
    notes: Vec<String>,
}

/// End every process in `job`.
fn terminate_job(job: &Owned) {
    // SAFETY: the job handle is open while `job` lives.
    if let Err(e) = unsafe { TerminateJobObject(job.0, 1) } {
        tracing::debug!(target: "shodh::harness", error = %e, "the command's job had already ended");
    }
}

/// Whether process `pid` runs `conhost.exe`: the console host Windows starts
/// for a console program, which leaves on its own when the program ends.
fn is_console_host(pid: u32) -> bool {
    // SAFETY: the process handle is owned below.
    let opened = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, BOOL::from(false), pid) };
    let Ok(process) = opened.map(Owned) else {
        return false;
    };
    let mut buffer = vec![0u16; 1024];
    let mut size = u32::try_from(buffer.len()).unwrap_or(u32::MAX);
    // SAFETY: `buffer` holds `size` characters.
    let named = unsafe {
        QueryFullProcessImageNameW(
            process.0,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut size,
        )
    };
    if named.is_err() {
        return false;
    }
    let name = String::from_utf16_lossy(&buffer[..size as usize]);
    Path::new(&name)
        .file_name()
        .is_some_and(|file| file.to_string_lossy().eq_ignore_ascii_case("conhost.exe"))
}

impl Contained {
    /// Read what the job's limits refused since the last look.
    fn drain_port(&mut self) {
        loop {
            let mut message: u32 = 0;
            let mut key: usize = 0;
            let mut overlapped: *mut OVERLAPPED = std::ptr::null_mut();
            // SAFETY: a zero wait on the job's own port.
            let got = unsafe {
                GetQueuedCompletionStatus(self.port.0, &mut message, &mut key, &mut overlapped, 0)
            };
            if got.is_err() {
                break;
            }
            let note = match message {
                MSG_JOB_MEMORY_LIMIT => format!(
                    "The memory limit ({} MB for all its processes) refused an allocation.",
                    self.memory_mb
                ),
                MSG_ACTIVE_PROCESS_LIMIT => format!(
                    "The process limit ({} at once) refused to start a process.",
                    self.max_processes
                ),
                _ => continue,
            };
            if !self.notes.contains(&note) {
                self.notes.push(note);
            }
        }
    }

    /// A limit that ended the command, in words. On Windows the job refuses
    /// what is over a limit instead of ending the command, so never.
    pub fn over_limit(&mut self) -> Option<String> {
        self.drain_port();
        None
    }

    /// The processes still in the job besides the command's own and the
    /// console host.
    fn others_running(&self) -> usize {
        #[repr(C)]
        struct List {
            header: JOBOBJECT_BASIC_PROCESS_ID_LIST,
            more: [usize; PID_LIST_CAPACITY],
        }
        // SAFETY: plain integers; all zero is a valid empty list.
        let mut list: List = unsafe { std::mem::zeroed() };
        let size = u32::try_from(std::mem::size_of::<List>()).unwrap_or(u32::MAX);
        // SAFETY: `list` is a process id list with room for
        // PID_LIST_CAPACITY + 1 ids, `size` bytes long.
        let read = unsafe {
            QueryInformationJobObject(
                self.job.0,
                JobObjectBasicProcessIdList,
                (&mut list as *mut List).cast::<c_void>(),
                size,
                None,
            )
        };
        if read.is_err() {
            return 0;
        }
        let count = (list.header.NumberOfProcessIdsInList as usize).min(PID_LIST_CAPACITY + 1);
        let base = std::ptr::addr_of!(list.header.ProcessIdList).cast::<usize>();
        (0..count)
            // SAFETY: `count` ids follow one another from `ProcessIdList`
            // into `more`, inside `list`.
            .map(|i| unsafe { *base.add(i) })
            .filter_map(|pid| u32::try_from(pid).ok())
            .filter(|&pid| pid != self.main_pid && !is_console_host(pid))
            .count()
    }

    /// End every process of the command. Returns how many were still running
    /// besides the command's own.
    pub fn terminate(&mut self) -> usize {
        self.drain_port();
        let left = self.others_running();
        terminate_job(&self.job);
        left
    }

    /// What the limits refused while the command ran, in words.
    pub fn notes(&mut self) -> Vec<String> {
        self.drain_port();
        self.notes.clone()
    }

    /// Whether processes left the run: never on Windows, where the job
    /// allows no breakaway.
    pub fn escaped(&self) -> bool {
        false
    }

    /// Ends every process of the command at once, from any thread.
    pub fn killer(&self) -> Killer {
        let job = Arc::clone(&self.job);
        Arc::new(move || terminate_job(&job))
    }
}

impl Drop for Contained {
    fn drop(&mut self) {
        terminate_job(&self.job);
    }
}
