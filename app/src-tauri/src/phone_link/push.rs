//! Notifications to a paired phone through UnifiedPush, when the user chose
//! it on the phone: the phone registers its endpoint over the link
//! (`phone_push_register`), and this computer posts a content-free wake-up to
//! it when a step waits for approval, when an answer the phone asked for is
//! ready, and when a reminder rings (`shodh_link::notify`: the body is one
//! fixed word). The details come over the link when the user opens the app.
//!
//! The endpoint is a secret (anyone holding it can wake the phone): it is kept
//! in the OS credential store, one entry per phone, removed with the phone,
//! and never logged. reqwest's errors name the URL, so only `without_url()`
//! is ever printed. Local-only mode posts nothing: a push leaves the computer.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::Duration;

use serde_json::Value;
use shodh_link::notify::{self, Endpoint, Kind};
use tauri::{AppHandle, Listener, Manager};

use crate::api_key_store;

/// How long one post may take.
const POST_TIMEOUT: Duration = Duration::from_secs(15);

/// The credential-store user of phone `node_id`'s push endpoint. Not a provider id.
pub fn push_user(node_id: &str) -> String {
    format!("phone-link-push-{node_id}")
}

/// Keep (or, with `None`, forget) the push endpoint of phone `node_id`.
pub fn register(node_id: &str, raw: Option<&str>) -> Result<(), String> {
    match raw.map(str::trim).filter(|r| !r.is_empty()) {
        None => api_key_store::remove_secret(&push_user(node_id)),
        Some(raw) => {
            let endpoint = Endpoint::parse(raw).map_err(str::to_string)?;
            api_key_store::store_secret(&push_user(node_id), endpoint.expose())
        }
    }
}

fn endpoint_of(node_id: &str) -> Option<Endpoint> {
    let kept = api_key_store::load_secret(&push_user(node_id))
        .ok()
        .flatten()?;
    Endpoint::parse(&kept).ok()
}

/// Post `kind` to `endpoint`. The error is in words, without the URL.
pub async fn post(client: &reqwest::Client, endpoint: &Endpoint, kind: Kind) -> Result<(), String> {
    let answer = client
        .post(endpoint.expose())
        .header("Content-Type", "text/plain")
        // An old wake-up is no use: the phone reads the details when it opens.
        .header("TTL", "3600")
        .body(notify::payload(kind))
        .send()
        .await
        .map_err(|e| e.without_url().to_string())?;
    if !answer.status().is_success() {
        return Err(format!(
            "the push server answered {}",
            answer.status().as_u16()
        ));
    }
    Ok(())
}

/// Sessions phones asked questions in, so only that phone hears its answer is ready.
#[derive(Default)]
pub struct Askers {
    sessions: Mutex<HashMap<String, HashSet<String>>>,
}

impl Askers {
    /// Phone `node_id` asked in session `session_id`.
    pub fn asked(&self, session_id: &str, node_id: &str) {
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions
            .entry(session_id.to_string())
            .or_default()
            .insert(node_id.to_string());
    }

    fn of(&self, session_id: &str) -> Vec<String> {
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions
            .get(session_id)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default()
    }
}

/// Which phones hear of `kind`: every paired one for an approval or a reminder;
/// for an answer, the phones that asked in its session.
pub fn recipients(kind: Kind, paired: &[String], askers: &[String]) -> Vec<String> {
    match kind {
        Kind::Answer => paired
            .iter()
            .filter(|p| askers.contains(p))
            .cloned()
            .collect(),
        Kind::Approval | Kind::Reminder | Kind::Brief => paired.to_vec(),
    }
}

async fn send(app: &AppHandle, kind: Kind, session: Option<String>) {
    if super::local_only(app) {
        return;
    }
    let Some(state) = app.try_state::<super::PhoneLinkState>() else {
        return;
    };
    let Ok(devices) = state.devices().await else {
        return;
    };
    let paired: Vec<String> = devices.list().into_iter().map(|d| d.node_id).collect();
    let askers = session
        .map(|s| app.state::<Askers>().of(&s))
        .unwrap_or_default();
    let client = match reqwest::Client::builder().timeout(POST_TIMEOUT).build() {
        Ok(client) => client,
        Err(e) => {
            tracing::warn!(target: "shodh::link", error = %e.without_url(), "the push client could not start");
            return;
        }
    };
    for node in recipients(kind, &paired, &askers) {
        let Some(endpoint) = endpoint_of(&node) else {
            continue;
        };
        match post(&client, &endpoint, kind).await {
            Ok(()) => {
                tracing::debug!(target: "shodh::link", kind = kind.word(), "a phone was woken")
            }
            Err(e) => {
                tracing::info!(target: "shodh::link", kind = kind.word(), error = %e, "a phone could not be woken")
            }
        }
    }
}

/// Post wake-ups for what calls for one: approvals and finished answers (agent
/// events) and reminders. Listens from startup; posts only to phones that
/// registered an endpoint.
pub fn start(app: &AppHandle) {
    app.manage(Askers::default());
    let handle = app.clone();
    app.listen_any(crate::agent_session_commands::AGENT_EVENT, move |event| {
        let Ok(payload) = serde_json::from_str::<Value>(event.payload()) else {
            return;
        };
        let Some(kind) = notify::kind_of(shodh_link::bridge::AGENT_EVENT, &payload) else {
            return;
        };
        let session = payload
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_string);
        let app = handle.clone();
        tauri::async_runtime::spawn(async move { send(&app, kind, session).await });
    });
    let handle = app.clone();
    app.listen_any(crate::reminders::REMINDER_FIRED_EVENT, move |_| {
        let app = handle.clone();
        tauri::async_runtime::spawn(async move { send(&app, Kind::Reminder, None).await });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::{Arc, Mutex as StdMutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Everything the logs say, kept for the test to read.
    #[derive(Clone, Default)]
    struct Captured(Arc<StdMutex<Vec<u8>>>);

    impl Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
        type Writer = Captured;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    #[test]
    fn only_the_phone_that_asked_hears_its_answer_is_ready() {
        let paired = vec!["a".to_string(), "b".to_string()];
        assert_eq!(recipients(Kind::Answer, &paired, &["b".to_string()]), ["b"]);
        assert_eq!(recipients(Kind::Answer, &paired, &[]), Vec::<String>::new());
        assert_eq!(recipients(Kind::Approval, &paired, &[]), ["a", "b"]);
        assert_eq!(recipients(Kind::Reminder, &paired, &[]), ["a", "b"]);
        let askers = Askers::default();
        askers.asked("s1", "b");
        assert_eq!(askers.of("s1"), ["b"]);
        assert!(askers.of("s2").is_empty());
    }

    #[tokio::test]
    async fn the_endpoint_url_never_appears_in_logs() {
        let logs = Captured::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(logs.clone())
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);

        // A push server on this computer that refuses (500), and one that is not there.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![0u8; 4096];
            let read = socket.read(&mut request).await.unwrap();
            socket
                .write_all(b"HTTP/1.1 500 Internal Server Error\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                .await
                .unwrap();
            String::from_utf8_lossy(&request[..read]).into_owned()
        });
        // Secret-shaped values are made at run time.
        let token = format!("{}{}{}", "up", "Zk7", "Qm9Xw2");
        let client = reqwest::Client::builder()
            .timeout(POST_TIMEOUT)
            .build()
            .unwrap();
        // An http endpoint is never kept; the test posts to the local server directly.
        let local = serde_json::from_value::<Endpoint>(serde_json::json!(format!(
            "http://127.0.0.1:{port}/{token}"
        )))
        .unwrap();
        let refused = post(&client, &local, Kind::Approval).await.unwrap_err();
        let request = server.await.unwrap();
        // What was sent is the kind's word, nothing else.
        assert!(request.ends_with("approval"), "{request}");
        tracing::info!(target: "shodh::link", error = %refused, "a phone could not be woken");

        let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let gone = closed.local_addr().unwrap().port();
        drop(closed);
        let missing = serde_json::from_value::<Endpoint>(serde_json::json!(format!(
            "http://127.0.0.1:{gone}/{token}"
        )))
        .unwrap();
        let unreachable = post(&client, &missing, Kind::Answer).await.unwrap_err();
        tracing::info!(target: "shodh::link", error = %unreachable, "a phone could not be woken");
        tracing::debug!(target: "shodh::link", endpoint = ?missing, "kept");

        let written = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
        assert!(
            written.contains("a phone could not be woken"),
            "the logs were captured: {written}"
        );
        assert!(
            !written.contains(&token),
            "the endpoint leaked into the logs: {written}"
        );
        assert!(!refused.contains(&token) && !unreachable.contains(&token));
        assert!(written.contains("[REDACTED]"));
    }

    #[test]
    fn only_an_https_endpoint_is_kept() {
        assert!(register("phone", Some("http://ntfy.example/x")).is_err());
        assert!(register("phone", Some("javascript:alert(1)")).is_err());
        assert_eq!(push_user("ab12"), "phone-link-push-ab12");
    }
}
