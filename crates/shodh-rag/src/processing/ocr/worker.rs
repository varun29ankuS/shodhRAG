//! OCR in a short-lived child process.
//!
//! The app reads scans in a child process (its own executable started with
//! [`WORKER_ARG`]) instead of in its own: PP-OCRv6's ONNX sessions and the
//! Windows.Data.Pdf / Windows OCR path run there, at below-normal priority.
//! The parent ([`OcrWorker`]) starts the child on the first scan, sends it one
//! file at a time as a JSON line ([`Request`]) and reads its replies, one JSON
//! line each ([`Reply`]): a line per page while it reads, then the text of
//! every page or the error. A child that is idle is stopped
//! ([`crate::lazy_model::IdleUnload`]), so its memory is returned to the
//! system in full; one that stops answering is killed. On Windows the child
//! is in a job object that kills it when the app exits, however it exits.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

use super::priority::BelowNormal;
use super::{OcrPages, PpOcr};

/// The argument that starts an executable as the OCR child (see the app's
/// `main`).
pub const WORKER_ARG: &str = "--ocr-worker";

/// A child that read nothing for this long is stopped (its memory goes back
/// to the system); the next scan starts another.
pub const IDLE: Duration = Duration::from_secs(60);

/// Longest a page may take before the child is taken to be stuck and killed.
pub const PAGE_TIMEOUT: Duration = Duration::from_secs(300);

/// Which engine the child reads with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkerEngine {
    /// PP-OCRv6 from `Request::model_dir`.
    PpOcr,
    /// Windows OCR (Windows.Data.Pdf renders PDF pages).
    System,
}

/// What kind of file the child reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileKind {
    Pdf,
    Image,
}

/// One file to read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    pub kind: FileKind,
    pub path: PathBuf,
    pub engine: WorkerEngine,
    /// The PP-OCRv6 model directory (for [`WorkerEngine::PpOcr`]).
    pub model_dir: Option<PathBuf>,
    /// Resolution PDF pages are rendered at for PP-OCRv6.
    pub dpi: u32,
    /// Resolution PDF pages are rendered at for Windows OCR (`None`: 96 dpi).
    pub system_dpi: Option<u32>,
}

/// One reply line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reply", rename_all = "lowercase")]
pub enum Reply {
    /// Page `page` of `pages` is being read (1-based).
    Page {
        id: u64,
        page: usize,
        pages: usize,
    },
    /// The text of every page, in order (one for an image), and the pages
    /// (1-based) that could not be read, which read as empty.
    Done {
        id: u64,
        pages: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        failed_pages: Vec<usize>,
        /// Why the first failed page could not be read.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        first_error: Option<String>,
    },
    Failed {
        id: u64,
        error: String,
    },
}

// ── The child ──────────────────────────────────────────────────────────────

/// Serves requests from `input` until it closes, writing replies to
/// `output`. The serving thread and the threads ONNX Runtime starts run below
/// normal priority. The PP-OCRv6 model is loaded on its first request and
/// kept while the child runs.
pub fn serve(input: impl BufRead, mut output: impl Write) -> Result<()> {
    let _below_normal = BelowNormal::new();
    let mut model: Option<(PathBuf, PpOcr)> = None;
    for line in input.lines() {
        let line = line.context("reading a request")?;
        if line.trim().is_empty() {
            continue;
        }
        let request: Request = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(e) => {
                write_reply(
                    &mut output,
                    &Reply::Failed {
                        id: 0,
                        error: format!("unreadable request: {e}"),
                    },
                )?;
                continue;
            }
        };
        let id = request.id;
        let result = {
            let out = std::cell::RefCell::new(&mut output);
            let on_page = |page: usize, pages: usize| {
                let mut out = out.borrow_mut();
                // A parent that stopped reading ends the child at its next read.
                let _ = write_reply(&mut **out, &Reply::Page { id, page, pages });
            };
            read(&request, &mut model, &on_page)
        };
        let reply = match result {
            Ok(pages_read) => Reply::Done {
                id,
                pages: pages_read.texts,
                failed_pages: pages_read.failed,
                first_error: pages_read.first_error,
            },
            Err(e) => Reply::Failed {
                id,
                error: format!("{e:#}"),
            },
        };
        write_reply(&mut output, &reply)?;
    }
    Ok(())
}

/// [`serve`] over this process's standard input and output: the OCR child's
/// entry point. Ends the process ([`end_process`]) when its input closes.
pub fn serve_stdio() -> ! {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let code = match serve(stdin.lock(), stdout.lock()) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("OCR worker: {e:#}");
            1
        }
    };
    end_process(code)
}

/// Ends this process with `code` without running the libraries' unload
/// code (Windows: `TerminateProcess` on itself rather than `ExitProcess`).
///
/// A process that ran ONNX Runtime and then rendered with Windows.Data.Pdf
/// did not finish exiting (measured on an Intel GPU): after `main` returned,
/// one thread spun forever inside the GPU driver's user-mode Direct3D 11 DLL
/// (`igd10um64xe.dll`), and the exiting process could no longer be killed.
/// The process had also loaded Direct3D 12 and DirectML; the reverse order
/// (Windows.Data.Pdf first) exited. Ending the process without unload
/// notifications skips the code that spun.
/// Output is flushed first; nothing else of the process needs its unload code.
pub fn end_process(code: i32) -> ! {
    use std::io::Write as _;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetCurrentProcess() -> isize;
            fn TerminateProcess(process: isize, exit_code: u32) -> i32;
        }
        // SAFETY: terminates this process; the pseudo handle is always valid.
        unsafe {
            TerminateProcess(GetCurrentProcess(), code as u32);
        }
    }
    std::process::exit(code)
}

fn write_reply(output: &mut impl Write, reply: &Reply) -> Result<()> {
    let mut line = serde_json::to_vec(reply)?;
    line.push(b'\n');
    output.write_all(&line)?;
    output.flush()?;
    Ok(())
}

fn read(
    request: &Request,
    model: &mut Option<(PathBuf, PpOcr)>,
    on_page: &dyn Fn(usize, usize),
) -> Result<OcrPages> {
    match request.engine {
        WorkerEngine::PpOcr => {
            let dir = request
                .model_dir
                .as_ref()
                .context("no OCR model directory was given")?;
            if model.as_ref().is_none_or(|(loaded, _)| loaded != dir) {
                *model = None;
                *model = Some((dir.clone(), PpOcr::load(dir)?));
            }
            let (_, ocr) = model.as_ref().context("the OCR model is not loaded")?;
            match request.kind {
                FileKind::Pdf => {
                    let bytes = std::fs::read(&request.path)
                        .with_context(|| format!("reading {}", request.path.display()))?;
                    super::read_pdf(ocr, &bytes, request.dpi, &request.path, on_page)
                }
                FileKind::Image => Ok(OcrPages {
                    texts: vec![super::read_image(ocr, &request.path)?],
                    ..OcrPages::default()
                }),
            }
        }
        WorkerEngine::System => read_system(request, on_page),
    }
}

#[cfg(windows)]
fn read_system(request: &Request, on_page: &dyn Fn(usize, usize)) -> Result<OcrPages> {
    use crate::processing::windows_ocr;
    match request.kind {
        FileKind::Pdf => windows_ocr::ocr_pdf_pages(&request.path, request.system_dpi, on_page),
        FileKind::Image => Ok(OcrPages {
            texts: vec![windows_ocr::ocr_image(&request.path)?],
            ..OcrPages::default()
        }),
    }
}

#[cfg(not(windows))]
fn read_system(_request: &Request, _on_page: &dyn Fn(usize, usize)) -> Result<OcrPages> {
    bail!("the system OCR exists only on Windows")
}

// ── The parent ─────────────────────────────────────────────────────────────

/// How to start the child: an executable and its arguments (the app passes
/// its own executable and [`WORKER_ARG`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerCommand {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    /// Environment variables set for the child (beyond this process's).
    pub env: Vec<(OsString, OsString)>,
}

impl WorkerCommand {
    /// This executable started as the OCR child.
    pub fn current_exe() -> Result<Self> {
        Ok(Self {
            program: std::env::current_exe().context("locating this executable")?,
            args: vec![WORKER_ARG.into()],
            env: Vec::new(),
        })
    }
}

/// A running child: its pipes, the reader of its replies and (Windows) the
/// job that kills it with this process.
struct Running {
    child: Child,
    stdin: Option<ChildStdin>,
    replies: Receiver<std::io::Result<String>>,
    #[cfg(windows)]
    _job: job::KillOnClose,
}

impl Running {
    fn start(command: &WorkerCommand) -> Result<Self> {
        let mut cmd = Command::new(&command.program);
        cmd.args(&command.args)
            .envs(command.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
            cmd.creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS);
        }
        let mut child = cmd
            .spawn()
            .with_context(|| format!("starting the OCR process {}", command.program.display()))?;
        #[cfg(windows)]
        let job = match job::KillOnClose::new().and_then(|job| job.assign(&child).map(|()| job)) {
            Ok(job) => job,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e.context("putting the OCR process in a job"));
            }
        };
        let stdout = child
            .stdout
            .take()
            .context("the OCR process has no output")?;
        let stdin = child.stdin.take();
        let (tx, replies) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("shodh-ocr-replies".to_string())
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    if tx.send(line).is_err() {
                        return;
                    }
                }
            })
            .context("starting the OCR reply reader")?;
        tracing::info!(pid = child.id(), "OCR process started");
        Ok(Self {
            child,
            stdin,
            replies,
            #[cfg(windows)]
            _job: job,
        })
    }

    fn send(&mut self, request: &Request) -> Result<()> {
        let stdin = self.stdin.as_mut().context("the OCR process was stopped")?;
        let mut line = serde_json::to_vec(request)?;
        line.push(b'\n');
        stdin.write_all(&line)?;
        stdin.flush()?;
        Ok(())
    }

    /// Ends the child: its input is closed, which ends it; one still running
    /// after a moment is killed.
    fn stop(mut self) {
        drop(self.stdin.take());
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                _ => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    break;
                }
            }
        }
        tracing::info!("OCR process stopped");
    }

    fn kill(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        tracing::warn!("OCR process killed");
    }
}

struct State {
    running: Option<Running>,
    last_used: Instant,
    next_id: u64,
}

/// The parent's handle on the OCR child: started on first use, one file at a
/// time, stopped when idle.
pub struct OcrWorker {
    command: WorkerCommand,
    model_dir: Mutex<Option<PathBuf>>,
    state: Mutex<State>,
    page_timeout: Duration,
}

impl std::fmt::Debug for OcrWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OcrWorker")
            .field("command", &self.command)
            .finish_non_exhaustive()
    }
}

impl OcrWorker {
    pub fn new(command: WorkerCommand) -> Self {
        Self::with_page_timeout(command, PAGE_TIMEOUT)
    }

    pub fn with_page_timeout(command: WorkerCommand, page_timeout: Duration) -> Self {
        Self {
            command,
            model_dir: Mutex::new(None),
            state: Mutex::new(State {
                running: None,
                last_used: Instant::now(),
                next_id: 1,
            }),
            page_timeout,
        }
    }

    /// The PP-OCRv6 model directory, when the model is installed.
    pub fn model_dir(&self) -> Option<PathBuf> {
        self.model_dir
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// The model is installed in `dir` (or, with `None`, not installed).
    pub fn set_model_dir(&self, dir: Option<PathBuf>) {
        *self.model_dir.lock().unwrap_or_else(|e| e.into_inner()) = dir;
    }

    /// Whether the child is running.
    pub fn is_running(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .running
            .is_some()
    }

    /// Reads the file at `path` in the child with `engine`; the text of each
    /// page in order (one for an image). `on_page` is called before each
    /// page of a PDF. A page that could not be read reads as empty
    /// ([`Self::read_pages`] says which).
    pub fn read(
        &self,
        kind: FileKind,
        path: &Path,
        engine: WorkerEngine,
        dpi: u32,
        system_dpi: Option<u32>,
        on_page: &dyn Fn(usize, usize),
    ) -> Result<Vec<String>> {
        self.read_pages(kind, path, engine, dpi, system_dpi, on_page)
            .map(|pages_read| pages_read.texts)
    }

    /// [`Self::read`], with the pages that could not be read.
    pub fn read_pages(
        &self,
        kind: FileKind,
        path: &Path,
        engine: WorkerEngine,
        dpi: u32,
        system_dpi: Option<u32>,
        on_page: &dyn Fn(usize, usize),
    ) -> Result<OcrPages> {
        let model_dir = match engine {
            WorkerEngine::PpOcr => {
                Some(self.model_dir().context("the OCR model is not installed")?)
            }
            WorkerEngine::System => None,
        };
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.last_used = Instant::now();
        let id = state.next_id;
        state.next_id += 1;
        let request = Request {
            id,
            kind,
            path: path.to_path_buf(),
            engine,
            model_dir,
            dpi,
            system_dpi,
        };
        // A child that exited (or whose input broke) is started again once.
        let mut sent = false;
        for _ in 0..2 {
            if state.running.is_none() {
                state.running = Some(Running::start(&self.command)?);
            }
            let running = state
                .running
                .as_mut()
                .context("the OCR process is not running")?;
            match running.send(&request) {
                Ok(()) => {
                    sent = true;
                    break;
                }
                Err(e) => {
                    tracing::warn!(error = %format!("{e:#}"), "OCR process did not take a request; restarting it");
                    if let Some(running) = state.running.take() {
                        running.kill();
                    }
                }
            }
        }
        if !sent {
            bail!("the OCR process could not be started");
        }
        let result = self.await_reply(&mut state, id, on_page);
        state.last_used = Instant::now();
        result
    }

    fn await_reply(
        &self,
        state: &mut State,
        id: u64,
        on_page: &dyn Fn(usize, usize),
    ) -> Result<OcrPages> {
        loop {
            let running = state
                .running
                .as_mut()
                .context("the OCR process is not running")?;
            let line = match running.replies.recv_timeout(self.page_timeout) {
                Ok(Ok(line)) => line,
                Ok(Err(e)) => {
                    if let Some(running) = state.running.take() {
                        running.kill();
                    }
                    return Err(anyhow!("reading from the OCR process: {e}"));
                }
                Err(RecvTimeoutError::Timeout) => {
                    if let Some(running) = state.running.take() {
                        running.kill();
                    }
                    bail!(
                        "the OCR process did not answer for {} s and was stopped",
                        self.page_timeout.as_secs()
                    );
                }
                Err(RecvTimeoutError::Disconnected) => {
                    if let Some(running) = state.running.take() {
                        running.kill();
                    }
                    bail!("the OCR process ended while reading");
                }
            };
            match serde_json::from_str::<Reply>(&line) {
                Ok(Reply::Page {
                    id: reply,
                    page,
                    pages,
                }) if reply == id => on_page(page, pages),
                Ok(Reply::Done {
                    id: reply,
                    pages,
                    failed_pages,
                    first_error,
                }) if reply == id => {
                    return Ok(OcrPages {
                        texts: pages,
                        failed: failed_pages,
                        first_error,
                    })
                }
                Ok(Reply::Failed { id: reply, error }) if reply == id => {
                    return Err(anyhow!(error))
                }
                // A reply to an earlier request that timed out is skipped.
                Ok(_) => {}
                Err(e) => tracing::debug!(error = %e, "unreadable OCR process line skipped"),
            }
        }
    }

    /// Stops the child now (it is started again on the next scan).
    pub fn stop(&self) {
        let running = self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .running
            .take();
        if let Some(running) = running {
            running.stop();
        }
    }
}

impl crate::lazy_model::IdleUnload for OcrWorker {
    /// Stops a child that read nothing for `idle`. A child that is reading
    /// holds the lock, so it is never stopped mid-file.
    fn unload_if_idle(&self, idle: Duration) -> bool {
        let Ok(mut state) = self.state.try_lock() else {
            return false;
        };
        if state.running.is_none() || state.last_used.elapsed() < idle {
            return false;
        }
        let running = state.running.take();
        drop(state);
        if let Some(running) = running {
            running.stop();
        }
        true
    }
}

impl Drop for OcrWorker {
    fn drop(&mut self) {
        let running = self
            .state
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .running
            .take();
        if let Some(running) = running {
            running.stop();
        }
    }
}

#[cfg(windows)]
mod job {
    //! A job object that kills its processes when its handle is closed: when
    //! the app exits, crashes or is killed, the OCR child goes with it.

    use anyhow::{bail, Result};
    use std::os::windows::io::AsRawHandle;

    type Handle = isize;

    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS: i32 = 9;

    #[repr(C)]
    #[derive(Default)]
    struct BasicLimitInformation {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct IoCounters {
        read_operation_count: u64,
        write_operation_count: u64,
        other_operation_count: u64,
        read_transfer_count: u64,
        write_transfer_count: u64,
        other_transfer_count: u64,
    }

    #[repr(C)]
    #[derive(Default)]
    struct ExtendedLimitInformation {
        basic: BasicLimitInformation,
        io: IoCounters,
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateJobObjectW(attributes: *const core::ffi::c_void, name: *const u16) -> Handle;
        fn SetInformationJobObject(
            job: Handle,
            class: i32,
            info: *const core::ffi::c_void,
            length: u32,
        ) -> i32;
        fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
    }

    pub struct KillOnClose(Handle);

    // SAFETY: a job handle may be used and closed from any thread.
    unsafe impl Send for KillOnClose {}

    impl KillOnClose {
        pub fn new() -> Result<Self> {
            // SAFETY: no security attributes and no name: an anonymous job.
            let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if handle == 0 {
                bail!(
                    "CreateJobObject failed: {}",
                    std::io::Error::last_os_error()
                );
            }
            let job = Self(handle);
            let info = ExtendedLimitInformation {
                basic: BasicLimitInformation {
                    limit_flags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                    ..Default::default()
                },
                ..Default::default()
            };
            // SAFETY: `info` is a JOBOBJECT_EXTENDED_LIMIT_INFORMATION of the
            // length passed, alive for the call.
            let ok = unsafe {
                SetInformationJobObject(
                    job.0,
                    JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS,
                    (&info as *const ExtendedLimitInformation).cast(),
                    std::mem::size_of::<ExtendedLimitInformation>() as u32,
                )
            };
            if ok == 0 {
                bail!(
                    "SetInformationJobObject failed: {}",
                    std::io::Error::last_os_error()
                );
            }
            Ok(job)
        }

        pub fn assign(&self, child: &std::process::Child) -> Result<()> {
            // SAFETY: both handles are valid for the call.
            let ok = unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle() as Handle) };
            if ok == 0 {
                bail!(
                    "AssignProcessToJobObject failed: {}",
                    std::io::Error::last_os_error()
                );
            }
            Ok(())
        }
    }

    impl Drop for KillOnClose {
        fn drop(&mut self) {
            // SAFETY: the handle is owned and closed once; closing it kills
            // the processes still in the job.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn the_job_limit_structure_has_the_windows_layout() {
            // JOBOBJECT_EXTENDED_LIMIT_INFORMATION is 144 bytes on 64-bit Windows.
            #[cfg(target_pointer_width = "64")]
            assert_eq!(std::mem::size_of::<super::ExtendedLimitInformation>(), 144);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_and_replies_are_json_lines() {
        let request = Request {
            id: 7,
            kind: FileKind::Pdf,
            path: PathBuf::from("C:/docs/scan.pdf"),
            engine: WorkerEngine::PpOcr,
            model_dir: Some(PathBuf::from("C:/models/pp-ocrv6")),
            dpi: 300,
            system_dpi: None,
        };
        let line = serde_json::to_string(&request).unwrap();
        assert!(!line.contains('\n'));
        assert!(line.contains("\"engine\":\"ppocr\""), "{line}");
        assert_eq!(serde_json::from_str::<Request>(&line).unwrap(), request);
        let page = serde_json::to_string(&Reply::Page {
            id: 7,
            page: 2,
            pages: 9,
        })
        .unwrap();
        assert_eq!(page, r#"{"reply":"page","id":7,"page":2,"pages":9}"#);
        // Failed pages cross to the parent; a reply without them reads as none.
        let done = Reply::Done {
            id: 7,
            pages: vec!["one".to_string(), String::new()],
            failed_pages: vec![2],
            first_error: Some("page 2 could not be rendered".to_string()),
        };
        let line = serde_json::to_string(&done).unwrap();
        assert_eq!(serde_json::from_str::<Reply>(&line).unwrap(), done);
        assert_eq!(
            serde_json::from_str::<Reply>(r#"{"reply":"done","id":7,"pages":["one"]}"#).unwrap(),
            Reply::Done {
                id: 7,
                pages: vec!["one".to_string()],
                failed_pages: Vec::new(),
                first_error: None,
            }
        );
    }

    /// The child's loop: progress and the result of each request, an error
    /// for a request it cannot read, and the end when its input closes.
    #[test]
    fn the_ocr_worker_answers_each_request_and_ends_with_its_input() {
        let missing = Request {
            id: 1,
            kind: FileKind::Image,
            path: PathBuf::from("missing.png"),
            engine: WorkerEngine::PpOcr,
            model_dir: None,
            dpi: 300,
            system_dpi: None,
        };
        let input = format!("{}\nnot json\n\n", serde_json::to_string(&missing).unwrap());
        let mut output = Vec::new();
        serve(input.as_bytes(), &mut output).unwrap();
        let replies: Vec<Reply> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(replies.len(), 2, "{replies:?}");
        assert!(
            matches!(&replies[0], Reply::Failed { id: 1, error } if error.contains("model directory")),
            "{replies:?}"
        );
        assert!(
            matches!(&replies[1], Reply::Failed { id: 0, error } if error.contains("unreadable request")),
            "{replies:?}"
        );
    }

    /// Set in the environment of the child the tests start: this test
    /// binary, running only [`ocr_worker_child_entry`].
    const CHILD_ENV: &str = "SHODH_TEST_OCR_WORKER_CHILD";

    /// The child's entry point when this test binary is started as the OCR
    /// child; an ordinary run passes through.
    #[test]
    fn ocr_worker_child_entry() {
        if std::env::var_os(CHILD_ENV).is_some() {
            // End libtest's "test ... " line, so replies start on their own.
            let _ = std::io::stdout().write_all(b"\n");
            serve_stdio();
        }
    }

    /// The test binary as the OCR child (libtest's own output lines are not
    /// replies and are skipped by the parent).
    fn test_child() -> OcrWorker {
        OcrWorker::with_page_timeout(
            WorkerCommand {
                program: std::env::current_exe().unwrap(),
                args: [
                    "--exact",
                    "processing::ocr::worker::tests::ocr_worker_child_entry",
                    "--nocapture",
                    "--test-threads=1",
                ]
                .iter()
                .map(OsString::from)
                .collect(),
                env: vec![(CHILD_ENV.into(), "1".into())],
            },
            Duration::from_secs(120),
        )
    }

    /// Files go to a child process, which is stopped when idle and started
    /// again for the next file.
    #[test]
    fn the_ocr_worker_reads_in_a_child_that_stops_when_idle() {
        let worker = test_child();
        let missing = Path::new("no-such-scan.png");
        let err = worker
            .read(
                FileKind::Image,
                missing,
                WorkerEngine::System,
                300,
                None,
                &|_, _| {},
            )
            .unwrap_err();
        // The child answered (it did not die or hang).
        let message = format!("{err:#}");
        assert!(!message.contains("OCR process"), "{message}");
        assert!(worker.is_running());
        use crate::lazy_model::IdleUnload;
        assert!(
            !worker.unload_if_idle(Duration::from_secs(3600)),
            "recently used"
        );
        assert!(worker.unload_if_idle(Duration::ZERO));
        assert!(!worker.is_running());
        let again = worker
            .read(
                FileKind::Image,
                missing,
                WorkerEngine::System,
                300,
                None,
                &|_, _| {},
            )
            .unwrap_err();
        assert_eq!(format!("{again:#}"), message);
        assert!(worker.is_running());
        worker.stop();
        assert!(!worker.is_running());
    }

    /// A scanned PDF read by PP-OCRv6 in the child, page by page.
    #[test]
    #[cfg_attr(
        not(shodh_test_models),
        ignore = "requires SHODH_TEST_MODELS containing pp-ocrv6/"
    )]
    fn the_ocr_worker_reads_a_scan_with_ppocr_in_the_child() {
        let root = std::env::var_os("SHODH_TEST_MODELS").expect("SHODH_TEST_MODELS");
        let worker = test_child();
        worker.set_model_dir(Some(super::super::model_dir(Path::new(&root))));
        let scan = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../shodh-eval/data/scanned/corpus/invoices/inv-2025-0164-scan.pdf");
        let seen = std::sync::Mutex::new(Vec::new());
        let pages = worker
            .read(
                FileKind::Pdf,
                &scan,
                WorkerEngine::PpOcr,
                300,
                None,
                &|page, pages| seen.lock().unwrap().push((page, pages)),
            )
            .unwrap();
        assert_eq!(seen.into_inner().unwrap(), [(1, 2), (2, 2)]);
        assert!(pages[0].contains("Invoice No: INV-2025-0164"), "{pages:?}");
        assert!(
            pages[1].contains("Total amount due: INR 4,63,740.00"),
            "{pages:?}"
        );
    }

    /// A child that does not start is an error, not a hang.
    #[test]
    fn an_ocr_worker_that_cannot_start_fails_the_read() {
        let worker = OcrWorker::new(WorkerCommand {
            program: PathBuf::from("this-ocr-worker-does-not-exist"),
            args: Vec::new(),
            env: Vec::new(),
        });
        let err = worker
            .read(
                FileKind::Image,
                Path::new("a.png"),
                WorkerEngine::System,
                300,
                None,
                &|_, _| {},
            )
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("starting the OCR process"),
            "{err:#}"
        );
        assert!(!worker.is_running());
        let err = worker
            .read(
                FileKind::Image,
                Path::new("a.png"),
                WorkerEngine::PpOcr,
                300,
                None,
                &|_, _| {},
            )
            .unwrap_err();
        assert!(format!("{err:#}").contains("not installed"), "{err:#}");
    }
}
