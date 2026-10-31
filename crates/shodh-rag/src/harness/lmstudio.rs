//! Detecting LM Studio on this computer, and saying exactly what is wrong when
//! its models cannot be used.
//!
//! LM Studio's chat models are reached through its local server
//! (`GET <base>/models`, OpenAI-compatible), which is a separate switch in the
//! app (Developer → Start server, or `lms server start`). So "LM Studio isn't
//! running" is often wrong: the app is open with a model loaded and only its
//! server is off. Detection tells these apart:
//!
//! - the server answers with chat models: [`LmStudioState::Running`];
//! - it answers with none (or only embedding models): [`LmStudioState::NoChatModel`];
//! - it refuses without its API token (401/403): [`LmStudioState::NeedsToken`];
//! - it does not answer, but the LM Studio app is open: [`LmStudioState::ServerOff`];
//! - neither: [`LmStudioState::NotRunning`].
//!
//! A server still loading a model can be slow to answer: one retry with a
//! longer timeout is made when the connection was made but the answer did not
//! come in time. A connection that fails (refused, or no connection within
//! [`CONNECT_TIMEOUT`]) is never retried: nothing is listening. On Windows a
//! refused loopback connection is not immediate (the stack tries the SYN again
//! before giving up, about 1–2 s), so the connection has its own timeout, longer
//! than that, and the answer's timeout counts only once it is connected; the
//! `a_closed_port_is_not_running_without_a_retry` test measures it on the CI
//! runners.
//!
//! Only loopback addresses are asked: the list must not leave this computer.

use std::path::Path;
use std::time::Duration;

use serde::Serialize;

use super::catalog_fetch::{
    is_loopback_url, parse_openai_models, CATALOG_USER_AGENT, MAX_LIST_BYTES,
};

/// Time allowed to connect: longer than Windows takes to report a refused loopback
/// connection (about 1–2 s), so "nothing is listening" is told by the refusal itself.
pub const CONNECT_TIMEOUT: Duration = Duration::from_millis(3_000);
/// Time allowed for the answer once connected (LM Studio answers at once when its server is on).
pub const FIRST_TIMEOUT: Duration = Duration::from_millis(1_500);
/// Time allowed for the one retry of a request that timed out (a model loading).
pub const RETRY_TIMEOUT: Duration = Duration::from_millis(4_000);

/// The environment variable the agent runtime reads LM Studio's API token from.
pub const LM_STUDIO_API_KEY_VAR: &str = "LM_STUDIO_API_KEY";

/// Where LM Studio keeps its server settings under the user's home folder (its
/// `port` is the server's port). Not a documented interface: read only when it is
/// there and holds a valid port; any other content is ignored.
pub const SERVER_CONFIG_RELATIVE: &str = ".lmstudio/.internal/http-server-config.json";

/// What detection found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LmStudioState {
    Running,
    NoChatModel,
    NeedsToken,
    ServerOff,
    NotRunning,
}

/// One detection: the state, the chat models (only when running) and the address asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LmStudioDetection {
    pub state: LmStudioState,
    pub models: Vec<String>,
    pub address: String,
}

/// How one request to the server ended.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Probe {
    Models(Vec<String>),
    NeedsToken,
    /// No answer in time.
    TimedOut,
    /// No server (refused, or not a loopback address), or an answer that is not a model list.
    NoServer,
}

/// Whether something accepts a connection at `base`'s host and port within `connect`.
/// A refusal (on Windows, after the stack's own retries) or no answer in time means
/// nothing is listening.
async fn listening(base: &str, connect: Duration) -> bool {
    let Ok(url) = url::Url::parse(base) else {
        return false;
    };
    let (Some(host), Some(port)) = (url.host_str(), url.port_or_known_default()) else {
        return false;
    };
    let host = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    matches!(
        tokio::time::timeout(
            connect,
            tokio::net::TcpStream::connect((host.as_str(), port))
        )
        .await,
        Ok(Ok(_))
    )
}

async fn probe_once(
    base: &str,
    token: Option<&str>,
    connect: Duration,
    timeout: Duration,
) -> Probe {
    // Connected first, on its own timeout: the answer's timeout then counts only the
    // server's answer, and a refused connection is never mistaken for a slow server.
    if !listening(base, connect).await {
        return Probe::NoServer;
    }
    let Ok(client) = reqwest::Client::builder()
        .user_agent(CATALOG_USER_AGENT)
        .timeout(timeout)
        .connect_timeout(connect)
        .redirect(reqwest::redirect::Policy::none())
        .build()
    else {
        return Probe::NoServer;
    };
    let mut request = client
        .get(format!("{base}/models"))
        .header(reqwest::header::ACCEPT, "application/json");
    if let Some(token) = token.map(str::trim).filter(|t| !t.is_empty()) {
        request = request.bearer_auth(token);
    }
    let response = match request.send().await {
        Ok(response) => response,
        // Connected, but no answer in time: a model may be loading.
        Err(e) if e.is_timeout() && !e.is_connect() => return Probe::TimedOut,
        // Refused, or no connection in time: nothing is listening.
        Err(_) => return Probe::NoServer,
    };
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Probe::NeedsToken;
    }
    if !status.is_success()
        || response
            .content_length()
            .is_some_and(|n| n > MAX_LIST_BYTES as u64)
    {
        return Probe::NoServer;
    }
    let body = match response.bytes().await {
        Ok(body) if body.len() <= MAX_LIST_BYTES => body,
        Err(e) if e.is_timeout() => return Probe::TimedOut,
        _ => return Probe::NoServer,
    };
    match std::str::from_utf8(&body).ok().map(parse_openai_models) {
        Some(Ok(models)) => Probe::Models(models),
        _ => Probe::NoServer,
    }
}

/// The time limits of one detection.
#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    pub connect: Duration,
    pub first: Duration,
    pub retry: Duration,
}

impl Timeouts {
    pub const APP: Timeouts = Timeouts {
        connect: CONNECT_TIMEOUT,
        first: FIRST_TIMEOUT,
        retry: RETRY_TIMEOUT,
    };
}

/// Ask the server at `base` (e.g. `http://127.0.0.1:1234/v1`) for its models,
/// retrying once with the longer timeout when it connected but did not answer in time.
async fn probe(base: &str, token: Option<&str>, t: Timeouts) -> Probe {
    match probe_once(base, token, t.connect, t.first).await {
        Probe::TimedOut => probe_once(base, token, t.connect, t.retry).await,
        other => other,
    }
}

/// Detect LM Studio at `base`. `app_open` says whether the LM Studio app is
/// running (asked only when its server does not answer).
pub async fn detect_with(
    base: &str,
    token: Option<&str>,
    timeouts: Timeouts,
    app_open: impl FnOnce() -> bool + Send + 'static,
) -> LmStudioDetection {
    let base = base.trim().trim_end_matches('/').to_string();
    let probe = if is_loopback_url(&base) {
        probe(&base, token, timeouts).await
    } else {
        Probe::NoServer
    };
    let (state, models) = match probe {
        Probe::Models(models) if models.is_empty() => (LmStudioState::NoChatModel, models),
        Probe::Models(models) => (LmStudioState::Running, models),
        Probe::NeedsToken => (LmStudioState::NeedsToken, Vec::new()),
        Probe::TimedOut | Probe::NoServer => {
            // The process list is read off the async workers (a snapshot, or `pgrep`).
            let open = tokio::task::spawn_blocking(app_open).await.unwrap_or(false);
            if open {
                (LmStudioState::ServerOff, Vec::new())
            } else {
                (LmStudioState::NotRunning, Vec::new())
            }
        }
    };
    LmStudioDetection {
        state,
        models,
        address: base,
    }
}

/// [`detect_with`] with the app's timeouts and the process list of this computer.
pub async fn detect(base: &str, token: Option<&str>) -> LmStudioDetection {
    detect_with(base, token, Timeouts::APP, app_is_open).await
}

/// The server port from LM Studio's settings file under `home`, when that file
/// is there and names a usable port.
pub fn configured_port(home: &Path) -> Option<u16> {
    let text = std::fs::read_to_string(home.join(SERVER_CONFIG_RELATIVE)).ok()?;
    port_from_config(&text)
}

/// [`configured_port`] under this user's home folder.
pub fn configured_port_for_user() -> Option<u16> {
    dirs::home_dir().and_then(|home| configured_port(&home))
}

/// The `port` of an LM Studio server settings file (1024–65535), if it has one.
pub fn port_from_config(text: &str) -> Option<u16> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let port = value.get("port")?.as_u64()?;
    u16::try_from(port).ok().filter(|p| *p >= 1024)
}

/// Whether a process name is the LM Studio app (its Windows executable is
/// `LM Studio.exe`; on macOS the process is `LM Studio`).
pub fn is_lmstudio_process(name: &str) -> bool {
    let name = name.trim().to_ascii_lowercase();
    let stem = name.strip_suffix(".exe").unwrap_or(&name);
    stem == "lm studio" || stem == "lmstudio"
}

/// Whether the LM Studio app is running on this computer.
#[cfg(windows)]
pub fn app_is_open() -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    // SAFETY: a snapshot of the process list, closed below.
    let Ok(snapshot) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else {
        return false;
    };
    let mut entry = PROCESSENTRY32W {
        dwSize: u32::try_from(std::mem::size_of::<PROCESSENTRY32W>()).unwrap_or(u32::MAX),
        ..Default::default()
    };
    let mut found = false;
    // SAFETY: `entry` has its size set, as the calls require.
    let mut more = unsafe { Process32FirstW(snapshot, &mut entry) }.is_ok();
    while more {
        let len = entry
            .szExeFile
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(entry.szExeFile.len());
        if is_lmstudio_process(&String::from_utf16_lossy(&entry.szExeFile[..len])) {
            found = true;
            break;
        }
        // SAFETY: as above.
        more = unsafe { Process32NextW(snapshot, &mut entry) }.is_ok();
    }
    // SAFETY: the snapshot handle is owned here and closed once.
    unsafe {
        let _ = CloseHandle(snapshot);
    }
    found
}

/// Whether the LM Studio app is running on this computer.
#[cfg(target_os = "macos")]
pub fn app_is_open() -> bool {
    std::process::Command::new("pgrep")
        .args(["-x", "LM Studio"])
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// Whether the LM Studio app is running on this computer (not detected here).
#[cfg(not(any(windows, target_os = "macos")))]
pub fn app_is_open() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// A one-endpoint HTTP server on a free loopback port: every request gets
    /// `status` and `body`, after `delays[n]` for the n-th request (the last delay
    /// repeats). Returns its `/v1` base URL.
    async fn fake_server(
        status: &'static str,
        body: &'static str,
        delays: Vec<Duration>,
    ) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        // Requests are counted, not connections: detection connects once without
        // sending anything to see whether something listens.
        let requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let requests = requests.clone();
                let delays = delays.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 2048];
                    let read = socket.read(&mut buf).await.unwrap_or(0);
                    if read == 0 {
                        return;
                    }
                    let n = requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let delay = delays.get(n).or(delays.last()).copied().unwrap_or_default();
                    tokio::time::sleep(delay).await;
                    let reply = format!(
                        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(reply.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        format!("http://127.0.0.1:{port}/v1")
    }

    /// A loopback port with nothing listening (bound, then released).
    async fn closed_port() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        format!("http://127.0.0.1:{port}/v1")
    }

    const FAST: Timeouts = Timeouts {
        connect: CONNECT_TIMEOUT,
        first: Duration::from_millis(300),
        retry: Duration::from_millis(3_000),
    };

    async fn detect_test(base: &str, token: Option<&str>, app_open: bool) -> LmStudioDetection {
        detect_with(base, token, FAST, move || app_open).await
    }

    /// Measured, not assumed: a closed loopback port (where Windows takes a while to
    /// report the refusal) is reported as not running, without the retry, well within
    /// the connection's own timeout.
    #[tokio::test]
    async fn a_closed_port_is_not_running_without_a_retry() {
        let base = closed_port().await;
        let started = std::time::Instant::now();
        let found = detect_with(&base, None, Timeouts::APP, || false).await;
        let took = started.elapsed();
        eprintln!("a refused loopback connection was reported after {took:?}");
        assert_eq!(found.state, LmStudioState::NotRunning);
        // A retry would add the retry timeout; a connection timeout would take the
        // whole connect timeout. The refusal arrives before either.
        assert!(
            took < CONNECT_TIMEOUT,
            "detection of a closed port took {took:?}, as long as the connect timeout"
        );
    }

    #[tokio::test]
    async fn a_server_with_a_chat_model_is_running() {
        let base = fake_server(
            "200 OK",
            r#"{"data":[{"id":"qwen3-8b"},{"id":"text-embedding-nomic-embed-text-v1.5"}]}"#,
            vec![Duration::ZERO],
        )
        .await;
        let found = detect_test(&base, None, false).await;
        assert_eq!(found.state, LmStudioState::Running);
        assert_eq!(found.models, vec!["qwen3-8b".to_string()]);
        assert_eq!(found.address, base);
    }

    #[tokio::test]
    async fn only_embedding_models_or_none_is_no_chat_model() {
        let embeddings = fake_server(
            "200 OK",
            r#"{"data":[{"id":"text-embedding-nomic-embed-text-v1.5"}]}"#,
            vec![Duration::ZERO],
        )
        .await;
        assert_eq!(
            detect_test(&embeddings, None, true).await.state,
            LmStudioState::NoChatModel
        );
        let empty = fake_server("200 OK", r#"{"data":[]}"#, vec![Duration::ZERO]).await;
        assert_eq!(
            detect_test(&empty, None, true).await.state,
            LmStudioState::NoChatModel
        );
    }

    #[tokio::test]
    async fn a_server_that_wants_its_token_says_so() {
        let base = fake_server(
            "401 Unauthorized",
            r#"{"error":"unauthorized"}"#,
            vec![Duration::ZERO],
        )
        .await;
        assert_eq!(
            detect_test(&base, None, true).await.state,
            LmStudioState::NeedsToken
        );
        let forbidden = fake_server("403 Forbidden", "{}", vec![Duration::ZERO]).await;
        assert_eq!(
            detect_test(&forbidden, Some("wrong"), true).await.state,
            LmStudioState::NeedsToken
        );
    }

    #[tokio::test]
    async fn the_token_is_sent_as_a_bearer_header() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = tokio::spawn(async move {
            // The first connection may only check that something listens: wait for a request.
            let (mut socket, buf, n) = loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 4096];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                if n > 0 {
                    break (socket, buf, n);
                }
            };
            let body = r#"{"data":[{"id":"qwen3-8b"}]}"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(reply.as_bytes()).await.unwrap();
            String::from_utf8_lossy(&buf[..n]).to_ascii_lowercase()
        });
        let base = format!("http://127.0.0.1:{port}/v1");
        let found = detect_test(&base, Some("lm-token-1"), false).await;
        assert_eq!(found.state, LmStudioState::Running);
        assert!(seen
            .await
            .unwrap()
            .contains("authorization: bearer lm-token-1"));
    }

    #[tokio::test]
    async fn the_server_off_while_the_app_is_open_is_told_apart_from_not_running() {
        let base = closed_port().await;
        assert_eq!(
            detect_test(&base, None, true).await.state,
            LmStudioState::ServerOff
        );
        assert_eq!(
            detect_test(&base, None, false).await.state,
            LmStudioState::NotRunning
        );
    }

    #[tokio::test]
    async fn a_slow_server_gets_one_retry_with_a_longer_timeout() {
        // The first answer comes after the first timeout; the retry is answered at once.
        let base = fake_server(
            "200 OK",
            r#"{"data":[{"id":"qwen3-8b"}]}"#,
            vec![Duration::from_millis(1_500), Duration::ZERO],
        )
        .await;
        assert_eq!(
            detect_test(&base, None, false).await.state,
            LmStudioState::Running
        );

        // A server that never answers in time is given up on after the retry.
        let hung = fake_server("200 OK", r#"{"data":[]}"#, vec![Duration::from_secs(30)]).await;
        let started = std::time::Instant::now();
        assert_eq!(
            detect_test(&hung, None, true).await.state,
            LmStudioState::ServerOff
        );
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[tokio::test]
    async fn only_loopback_addresses_are_asked() {
        let found = detect_test("http://192.168.1.20:1234/v1", None, false).await;
        assert_eq!(found.state, LmStudioState::NotRunning);
    }

    #[test]
    fn the_port_comes_from_lm_studios_settings_file_when_it_names_one() {
        assert_eq!(
            port_from_config(r#"{"port": 4321, "cors": false}"#),
            Some(4321)
        );
        assert_eq!(port_from_config(r#"{"port": 80}"#), None);
        assert_eq!(port_from_config(r#"{"port": 70000}"#), None);
        assert_eq!(port_from_config(r#"{"port": "1234"}"#), None);
        assert_eq!(port_from_config("not json"), None);

        let home = tempfile::tempdir().unwrap();
        assert_eq!(configured_port(home.path()), None);
        let file = home.path().join(SERVER_CONFIG_RELATIVE);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, r#"{"autoStartOnLaunch": false, "port": 5678}"#).unwrap();
        assert_eq!(configured_port(home.path()), Some(5678));
    }

    #[tokio::test]
    async fn a_server_on_another_port_is_found_at_that_port() {
        let base = fake_server(
            "200 OK",
            r#"{"data":[{"id":"gemma-3"}]}"#,
            vec![Duration::ZERO],
        )
        .await;
        // The default port has nothing; the port from the settings file has the server.
        assert_eq!(
            detect_test(&closed_port().await, None, false).await.state,
            LmStudioState::NotRunning
        );
        assert_eq!(
            detect_test(&base, None, false).await.state,
            LmStudioState::Running
        );
    }

    #[test]
    fn lm_studio_process_names() {
        assert!(is_lmstudio_process("LM Studio.exe"));
        assert!(is_lmstudio_process("LM Studio"));
        assert!(is_lmstudio_process("lmstudio.exe"));
        assert!(!is_lmstudio_process("lms.exe"));
        assert!(!is_lmstudio_process("studio.exe"));
    }
}
