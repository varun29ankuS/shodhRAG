//! The Socket Mode connection: open, acknowledge, hand over, reconnect.
//!
//! <https://docs.slack.dev/apis/events-api/using-socket-mode>
//!
//! - Every envelope is acknowledged before anything else is done with it; Slack retries
//!   envelopes it sees no acknowledgement for, and a slow answer must never cause a retry.
//! - `disconnect` (`refresh_requested`, `warning`): Slack is about to close the connection;
//!   a new one is opened at once. `link_disabled`: Socket Mode was turned off for the app;
//!   no new connection can work, so the connector stops with an error.
//! - Any other drop: a new connection after an exponential backoff with jitter.
//! - A connection silent for [`Timing::idle`] is pinged; silent again, it is replaced.
//!
//! The transport and how a connection is opened are traits, so this logic is tested over
//! a fake socket, and a different event source (a relay for a distributed app) can sit
//! behind the same [`SocketOpener`].

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::{SinkExt, StreamExt};
use tokio::sync::watch;

use super::events::{ack, parse_frame, DisconnectReason, Envelope, Frame};
use super::tokens::SlackAppToken;
use super::web::{SlackApiError, SlackWeb};

/// What a transport delivered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Incoming {
    Text(String),
    /// A ping or pong: the connection is alive.
    Alive,
}

/// One WebSocket connection.
#[async_trait]
pub trait SocketTransport: Send {
    /// The next frame; `None` once the connection is closed.
    async fn recv(&mut self) -> Option<Result<Incoming, String>>;
    async fn send(&mut self, text: String) -> Result<(), String>;
    async fn ping(&mut self) -> Result<(), String>;
    /// Closes the connection (a close frame, bounded wait).
    async fn close(&mut self);
}

/// Why a connection could not be opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    /// Trying again cannot help (a refused token): the connector stops with this message.
    Fatal(String),
    /// Try again later (`after`: when Slack said, else the backoff).
    Retry {
        message: String,
        after: Option<Duration>,
    },
}

/// Opens connections.
#[async_trait]
pub trait SocketOpener: Send + Sync {
    async fn open(&self) -> Result<Box<dyn SocketTransport>, OpenError>;
}

/// Receives each acknowledged envelope. Must return quickly (it spawns the work).
pub trait EnvelopeSink: Send + Sync {
    fn deliver(&self, envelope: Envelope);
}

/// The connection's state, as the card shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkState {
    Connecting,
    Connected,
    Reconnecting {
        next_try_at: DateTime<Utc>,
        reason: String,
    },
    /// Stopped for a reason the user must fix.
    Error(String),
    Off,
}

/// Where state changes and event times go.
pub trait StatusSink: Send + Sync {
    fn link(&self, state: LinkState);
    fn event_seen(&self, at: DateTime<Utc>);
}

/// How long to wait for what.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    /// Silence after which the connection is pinged (and, silent again, replaced).
    pub idle: Duration,
    pub backoff_start: Duration,
    pub backoff_max: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            idle: Duration::from_secs(90),
            backoff_start: Duration::from_secs(1),
            backoff_max: Duration::from_secs(300),
        }
    }
}

/// How one connection ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionEnd {
    /// Slack asked for a new connection.
    Refresh,
    /// Socket Mode was turned off for the app.
    LinkDisabled,
    /// The connection dropped.
    Dropped(String),
    /// The connector was switched off.
    Stopped,
}

/// Exponential backoff: `start`, doubling, at most `max`, each with ±20 % jitter.
#[derive(Debug, Clone)]
pub struct Backoff {
    start: Duration,
    max: Duration,
    step: u32,
}

impl Backoff {
    pub fn new(start: Duration, max: Duration) -> Self {
        Self {
            start,
            max,
            step: 0,
        }
    }

    pub fn reset(&mut self) {
        self.step = 0;
    }

    /// The next delay before jitter.
    pub fn next_base(&mut self) -> Duration {
        let factor = 2u32.saturating_pow(self.step.min(20));
        self.step = self.step.saturating_add(1);
        self.start.saturating_mul(factor).min(self.max)
    }

    /// The next delay, with jitter `r` in `[0, 1)` spread over ±20 %.
    pub fn next_with(&mut self, r: f64) -> Duration {
        let base = self.next_base();
        base.mul_f64(0.8 + 0.4 * r.clamp(0.0, 1.0))
    }

    pub fn next_delay(&mut self) -> Duration {
        use rand::Rng;
        let r: f64 = rand::thread_rng().gen();
        self.next_with(r)
    }
}

fn stopped(stop: &watch::Receiver<bool>) -> bool {
    *stop.borrow()
}

/// Runs one connection until it ends: acknowledges each envelope, then hands it to `sink`.
/// Returns how it ended and whether Slack's `hello` was seen.
pub async fn run_connection(
    transport: &mut dyn SocketTransport,
    sink: &dyn EnvelopeSink,
    status: &dyn StatusSink,
    stop: &mut watch::Receiver<bool>,
    idle: Duration,
) -> (ConnectionEnd, bool) {
    let mut hello = false;
    let mut pinged = false;
    loop {
        if stopped(stop) {
            transport.close().await;
            return (ConnectionEnd::Stopped, hello);
        }
        let next = tokio::select! {
            changed = stop.changed() => {
                if changed.is_err() || stopped(stop) {
                    transport.close().await;
                    return (ConnectionEnd::Stopped, hello);
                }
                continue;
            }
            next = tokio::time::timeout(idle, transport.recv()) => next,
        };
        let incoming = match next {
            Err(_) if pinged => {
                transport.close().await;
                return (
                    ConnectionEnd::Dropped("Slack stopped answering".into()),
                    hello,
                );
            }
            Err(_) => {
                pinged = true;
                if let Err(e) = transport.ping().await {
                    return (ConnectionEnd::Dropped(e), hello);
                }
                continue;
            }
            Ok(None) => {
                return (
                    ConnectionEnd::Dropped("Slack closed the connection".into()),
                    hello,
                )
            }
            Ok(Some(Err(e))) => return (ConnectionEnd::Dropped(e), hello),
            Ok(Some(Ok(incoming))) => incoming,
        };
        pinged = false;
        let Incoming::Text(text) = incoming else {
            continue;
        };
        match parse_frame(&text) {
            Ok(Frame::Hello) => {
                hello = true;
                status.link(LinkState::Connected);
            }
            Ok(Frame::Disconnect(DisconnectReason::LinkDisabled)) => {
                transport.close().await;
                return (ConnectionEnd::LinkDisabled, hello);
            }
            Ok(Frame::Disconnect(reason)) => {
                tracing::debug!(target: "shodh::slack", ?reason, "Slack asked for a new connection");
                transport.close().await;
                return (ConnectionEnd::Refresh, hello);
            }
            Ok(Frame::Envelope(envelope)) => {
                // Acknowledge first, always: nothing below may delay it.
                if let Err(e) = transport.send(ack(&envelope.envelope_id)).await {
                    return (ConnectionEnd::Dropped(e), hello);
                }
                status.event_seen(Utc::now());
                sink.deliver(envelope);
            }
            Ok(Frame::Other(kind)) => {
                tracing::debug!(target: "shodh::slack", %kind, "Socket Mode frame not used");
            }
            Err(e) => {
                tracing::warn!(target: "shodh::slack", error = %e, "unreadable Socket Mode frame");
            }
        }
    }
}

/// Waits `delay`, or less when the connector is switched off. True when switched off.
async fn wait_or_stop(delay: Duration, stop: &mut watch::Receiver<bool>) -> bool {
    if stopped(stop) {
        return true;
    }
    tokio::select! {
        _ = tokio::time::sleep(delay) => stopped(stop),
        changed = stop.changed() => changed.is_err() || stopped(stop),
    }
}

/// The message shown when Socket Mode is off for the app.
pub const LINK_DISABLED: &str = "Socket Mode was turned off for the Slack app. Turn it on again under Socket Mode in the app's settings on api.slack.com, then switch the connector on.";

/// Keeps a connection open while the connector is on: connects, reconnects (at once when
/// Slack asks, with backoff after a drop) and stops on a fatal error or when `stop` turns
/// true.
pub async fn supervise(
    opener: Arc<dyn SocketOpener>,
    sink: Arc<dyn EnvelopeSink>,
    status: Arc<dyn StatusSink>,
    mut stop: watch::Receiver<bool>,
    timing: Timing,
) {
    let mut backoff = Backoff::new(timing.backoff_start, timing.backoff_max);
    status.link(LinkState::Connecting);
    loop {
        if stopped(&stop) {
            status.link(LinkState::Off);
            return;
        }
        let opened = tokio::select! {
            opened = opener.open() => opened,
            changed = stop.changed() => {
                if changed.is_err() || stopped(&stop) {
                    status.link(LinkState::Off);
                    return;
                }
                continue;
            }
        };
        let (delay, reason) = match opened {
            Ok(mut transport) => {
                let (end, hello) = run_connection(
                    transport.as_mut(),
                    sink.as_ref(),
                    status.as_ref(),
                    &mut stop,
                    timing.idle,
                )
                .await;
                if hello {
                    backoff.reset();
                }
                match end {
                    ConnectionEnd::Stopped => {
                        status.link(LinkState::Off);
                        return;
                    }
                    ConnectionEnd::LinkDisabled => {
                        status.link(LinkState::Error(LINK_DISABLED.into()));
                        return;
                    }
                    ConnectionEnd::Refresh => {
                        status.link(LinkState::Connecting);
                        continue;
                    }
                    ConnectionEnd::Dropped(reason) => (backoff.next_delay(), reason),
                }
            }
            Err(OpenError::Fatal(message)) => {
                status.link(LinkState::Error(message));
                return;
            }
            Err(OpenError::Retry { message, after }) => {
                let delay = after.unwrap_or_else(|| backoff.next_delay());
                (delay, message)
            }
        };
        tracing::info!(target: "shodh::slack", delay_ms = delay.as_millis() as u64, %reason, "Slack connection lost; reconnecting");
        let next_try_at = Utc::now() + chrono::Duration::from_std(delay).unwrap_or_default();
        status.link(LinkState::Reconnecting {
            next_try_at,
            reason: reason.clone(),
        });
        if wait_or_stop(delay, &mut stop).await {
            status.link(LinkState::Off);
            return;
        }
        status.link(LinkState::Connecting);
    }
}

/// What a refused `apps.connections.open` means for the connector.
pub fn open_error(error: SlackApiError) -> OpenError {
    match &error {
        SlackApiError::RateLimited { retry_after } => OpenError::Retry {
            message: "Slack is rate limiting new connections".into(),
            after: *retry_after,
        },
        e if e.is_auth() => OpenError::Fatal(format!(
            "Slack refused the app-level token ({}). Create a new app-level token with the \
             connections:write scope and connect again.",
            e.code().unwrap_or("refused")
        )),
        SlackApiError::Api { code, .. } => OpenError::Retry {
            message: format!("Slack refused the connection ({code})"),
            after: None,
        },
        other => OpenError::Retry {
            message: other.to_string(),
            after: None,
        },
    }
}

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// A WebSocket connection to Slack (TLS from the system: SChannel, Security.framework or
/// OpenSSL, the same stack as the app's other HTTPS calls).
pub struct WsTransport(WsStream);

/// The longest wait for a close handshake.
const CLOSE_WAIT: Duration = Duration::from_secs(2);

#[async_trait]
impl SocketTransport for WsTransport {
    async fn recv(&mut self) -> Option<Result<Incoming, String>> {
        use tokio_tungstenite::tungstenite::Message;
        loop {
            return match self.0.next().await? {
                Err(e) => Some(Err(e.to_string())),
                Ok(Message::Text(text)) => Some(Ok(Incoming::Text(text.as_str().to_owned()))),
                // Pongs to Slack's pings are sent by the WebSocket library.
                Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => Some(Ok(Incoming::Alive)),
                Ok(Message::Close(_)) => None,
                Ok(Message::Binary(_)) | Ok(Message::Frame(_)) => continue,
            };
        }
    }

    async fn send(&mut self, text: String) -> Result<(), String> {
        use tokio_tungstenite::tungstenite::Message;
        self.0
            .send(Message::text(text))
            .await
            .map_err(|e| e.to_string())
    }

    async fn ping(&mut self) -> Result<(), String> {
        use tokio_tungstenite::tungstenite::Message;
        self.0
            .send(Message::Ping(bytes::Bytes::new()))
            .await
            .map_err(|e| e.to_string())
    }

    async fn close(&mut self) {
        let _ = tokio::time::timeout(CLOSE_WAIT, self.0.close(None)).await;
    }
}

/// Opens Socket Mode connections with an installation's app-level token.
pub struct SlackSocketOpener {
    pub web: SlackWeb,
    pub app_token: SlackAppToken,
}

#[async_trait]
impl SocketOpener for SlackSocketOpener {
    async fn open(&self) -> Result<Box<dyn SocketTransport>, OpenError> {
        let url = self
            .web
            .open_connection(&self.app_token)
            .await
            .map_err(open_error)?;
        // The URL holds a one-time ticket; it is never logged.
        match tokio_tungstenite::connect_async(url.as_str()).await {
            Ok((stream, _)) => Ok(Box::new(WsTransport(stream))),
            Err(e) => Err(OpenError::Retry {
                message: format!("the WebSocket connection failed: {}", ws_error(&e)),
                after: None,
            }),
        }
    }
}

/// A WebSocket error without the URL (which carries the connection ticket).
fn ws_error(e: &tokio_tungstenite::tungstenite::Error) -> String {
    use tokio_tungstenite::tungstenite::Error;
    match e {
        Error::Http(response) => format!("HTTP {}", response.status()),
        Error::Url(_) => "the connection URL was not usable".into(),
        Error::Io(io) => io.kind().to_string(),
        Error::Tls(_) => "TLS failed".into(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use serde_json::json;

    use super::*;

    /// What happened, in order, across the fake socket and the sink.
    type Log = Arc<Mutex<Vec<String>>>;

    struct FakeTransport {
        frames: VecDeque<Option<Result<Incoming, String>>>,
        log: Log,
        /// After the frames: wait forever (`true`) or report closed.
        hang: bool,
    }

    #[async_trait]
    impl SocketTransport for FakeTransport {
        async fn recv(&mut self) -> Option<Result<Incoming, String>> {
            match self.frames.pop_front() {
                Some(frame) => frame,
                None if self.hang => std::future::pending().await,
                None => None,
            }
        }
        async fn send(&mut self, text: String) -> Result<(), String> {
            self.log.lock().unwrap().push(format!("send {text}"));
            Ok(())
        }
        async fn ping(&mut self) -> Result<(), String> {
            self.log.lock().unwrap().push("ping".into());
            Ok(())
        }
        async fn close(&mut self) {
            self.log.lock().unwrap().push("close".into());
        }
    }

    struct LogSink(Log);
    impl EnvelopeSink for LogSink {
        fn deliver(&self, envelope: Envelope) {
            self.0
                .lock()
                .unwrap()
                .push(format!("deliver {}", envelope.envelope_id));
        }
    }

    #[derive(Default)]
    struct LogStatus(Mutex<Vec<LinkState>>, Mutex<usize>);
    impl StatusSink for LogStatus {
        fn link(&self, state: LinkState) {
            self.0.lock().unwrap().push(state);
        }
        fn event_seen(&self, _at: DateTime<Utc>) {
            *self.1.lock().unwrap() += 1;
        }
    }

    fn text(value: serde_json::Value) -> Option<Result<Incoming, String>> {
        Some(Ok(Incoming::Text(value.to_string())))
    }

    fn envelope(id: &str) -> Option<Result<Incoming, String>> {
        text(
            json!({"envelope_id": id, "type": "events_api", "payload": {"type": "event_callback"}}),
        )
    }

    #[tokio::test]
    async fn every_envelope_is_acknowledged_before_it_is_handled() {
        let log: Log = Default::default();
        let mut transport = FakeTransport {
            frames: VecDeque::from(vec![
                text(json!({"type": "hello"})),
                envelope("E1"),
                Some(Ok(Incoming::Alive)),
                envelope("E2"),
                text(json!({"type": "disconnect", "reason": "refresh_requested"})),
            ]),
            log: log.clone(),
            hang: false,
        };
        let status = LogStatus::default();
        let (_tx, mut stop) = watch::channel(false);
        let (end, hello) = run_connection(
            &mut transport,
            &LogSink(log.clone()),
            &status,
            &mut stop,
            Duration::from_secs(60),
        )
        .await;
        assert_eq!(end, ConnectionEnd::Refresh);
        assert!(hello);
        assert_eq!(
            *log.lock().unwrap(),
            vec![
                r#"send {"envelope_id":"E1"}"#.to_string(),
                "deliver E1".into(),
                r#"send {"envelope_id":"E2"}"#.into(),
                "deliver E2".into(),
                "close".into(),
            ]
        );
        assert_eq!(*status.0.lock().unwrap(), vec![LinkState::Connected]);
        assert_eq!(*status.1.lock().unwrap(), 2);
    }

    #[tokio::test]
    async fn link_disabled_and_drops_end_the_connection() {
        let log: Log = Default::default();
        let (_tx, mut stop) = watch::channel(false);
        let status = LogStatus::default();
        let mut disabled = FakeTransport {
            frames: VecDeque::from(vec![text(
                json!({"type": "disconnect", "reason": "link_disabled"}),
            )]),
            log: log.clone(),
            hang: false,
        };
        let sink = LogSink(log.clone());
        let (end, _) = run_connection(
            &mut disabled,
            &sink,
            &status,
            &mut stop,
            Duration::from_secs(60),
        )
        .await;
        assert_eq!(end, ConnectionEnd::LinkDisabled);
        let mut dropped = FakeTransport {
            frames: VecDeque::from(vec![Some(Err("reset by peer".into()))]),
            log: log.clone(),
            hang: false,
        };
        let (end, hello) = run_connection(
            &mut dropped,
            &sink,
            &status,
            &mut stop,
            Duration::from_secs(60),
        )
        .await;
        assert_eq!(end, ConnectionEnd::Dropped("reset by peer".into()));
        assert!(!hello);
        // Unreadable frames are skipped, not fatal.
        let mut garbage = FakeTransport {
            frames: VecDeque::from(vec![
                text(json!({"no": "type"})),
                Some(Ok(Incoming::Text("nope".into()))),
            ]),
            log: log.clone(),
            hang: false,
        };
        let (end, _) = run_connection(
            &mut garbage,
            &sink,
            &status,
            &mut stop,
            Duration::from_secs(60),
        )
        .await;
        assert_eq!(
            end,
            ConnectionEnd::Dropped("Slack closed the connection".into())
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_silent_connection_is_pinged_then_replaced() {
        let log: Log = Default::default();
        let (_tx, mut stop) = watch::channel(false);
        let mut silent = FakeTransport {
            frames: VecDeque::new(),
            log: log.clone(),
            hang: true,
        };
        let (end, _) = run_connection(
            &mut silent,
            &LogSink(log.clone()),
            &LogStatus::default(),
            &mut stop,
            Duration::from_secs(90),
        )
        .await;
        assert_eq!(
            end,
            ConnectionEnd::Dropped("Slack stopped answering".into())
        );
        assert_eq!(
            *log.lock().unwrap(),
            vec!["ping".to_string(), "close".into()]
        );
    }

    #[tokio::test]
    async fn switching_off_closes_the_connection() {
        let log: Log = Default::default();
        let (tx, mut stop) = watch::channel(false);
        let mut hanging = FakeTransport {
            frames: VecDeque::from(vec![text(json!({"type": "hello"}))]),
            log: log.clone(),
            hang: true,
        };
        let sink = LogSink(log.clone());
        let status = LogStatus::default();
        let run = run_connection(
            &mut hanging,
            &sink,
            &status,
            &mut stop,
            Duration::from_secs(600),
        );
        let (result, _) = tokio::join!(run, async {
            tokio::task::yield_now().await;
            tx.send(true).unwrap();
        });
        assert_eq!(result.0, ConnectionEnd::Stopped);
        assert_eq!(*log.lock().unwrap(), vec!["close".to_string()]);
    }

    /// Opens scripted connections; counts the opens.
    struct ScriptedOpener {
        script: Mutex<VecDeque<Result<Vec<Option<Result<Incoming, String>>>, OpenError>>>,
        log: Log,
        opens: Mutex<usize>,
    }

    #[async_trait]
    impl SocketOpener for ScriptedOpener {
        async fn open(&self) -> Result<Box<dyn SocketTransport>, OpenError> {
            *self.opens.lock().unwrap() += 1;
            let next = self.script.lock().unwrap().pop_front();
            match next {
                Some(Ok(frames)) => Ok(Box::new(FakeTransport {
                    frames: frames.into(),
                    log: self.log.clone(),
                    hang: false,
                })),
                Some(Err(e)) => Err(e),
                None => Err(OpenError::Fatal("script finished".into())),
            }
        }
    }

    fn timing() -> Timing {
        Timing {
            idle: Duration::from_secs(90),
            backoff_start: Duration::from_millis(10),
            backoff_max: Duration::from_millis(40),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn refreshes_reconnect_at_once_and_drops_back_off() {
        let log: Log = Default::default();
        let opener = Arc::new(ScriptedOpener {
            script: Mutex::new(VecDeque::from(vec![
                Ok(vec![
                    text(json!({"type": "hello"})),
                    text(json!({"type": "disconnect", "reason": "warning"})),
                ]),
                Ok(vec![
                    text(json!({"type": "hello"})),
                    Some(Err("reset".into())),
                ]),
                Err(OpenError::Retry {
                    message: "offline".into(),
                    after: None,
                }),
                Ok(vec![text(
                    json!({"type": "disconnect", "reason": "link_disabled"}),
                )]),
            ])),
            log: log.clone(),
            opens: Mutex::new(0),
        });
        let status = Arc::new(LogStatus::default());
        let (_tx, stop) = watch::channel(false);
        supervise(
            opener.clone(),
            Arc::new(LogSink(log.clone())),
            status.clone(),
            stop,
            timing(),
        )
        .await;
        assert_eq!(*opener.opens.lock().unwrap(), 4);
        let states = status.0.lock().unwrap().clone();
        let names: Vec<&str> = states
            .iter()
            .map(|s| match s {
                LinkState::Connecting => "connecting",
                LinkState::Connected => "connected",
                LinkState::Reconnecting { .. } => "reconnecting",
                LinkState::Error(_) => "error",
                LinkState::Off => "off",
            })
            .collect();
        assert_eq!(
            names,
            vec![
                "connecting",
                "connected", // first connection
                "connecting",
                "connected", // refreshed at once, no backoff
                "reconnecting",
                "connecting", // dropped: backoff
                "reconnecting",
                "connecting", // open failed: backoff
                "error",      // Socket Mode turned off
            ]
        );
        let LinkState::Reconnecting { reason, .. } = &states[4] else {
            unreachable!()
        };
        assert_eq!(reason, "reset");
        assert_eq!(states.last(), Some(&LinkState::Error(LINK_DISABLED.into())));
    }

    #[tokio::test]
    async fn a_refused_token_stops_without_retrying() {
        let opener = Arc::new(ScriptedOpener {
            script: Mutex::new(VecDeque::from(vec![Err(open_error(SlackApiError::Api {
                code: "invalid_auth".into(),
                needed: None,
            }))])),
            log: Default::default(),
            opens: Mutex::new(0),
        });
        let status = Arc::new(LogStatus::default());
        let (_tx, stop) = watch::channel(false);
        supervise(
            opener.clone(),
            Arc::new(LogSink(Default::default())),
            status.clone(),
            stop,
            timing(),
        )
        .await;
        assert_eq!(*opener.opens.lock().unwrap(), 1);
        let last = status.0.lock().unwrap().last().cloned().unwrap();
        let LinkState::Error(message) = last else {
            panic!("an error")
        };
        assert!(message.contains("invalid_auth"));
    }

    #[tokio::test(start_paused = true)]
    async fn switching_off_during_a_backoff_stops_at_once() {
        let opener = Arc::new(ScriptedOpener {
            script: Mutex::new(VecDeque::from(vec![Err(OpenError::Retry {
                message: "offline".into(),
                after: Some(Duration::from_secs(3_600)),
            })])),
            log: Default::default(),
            opens: Mutex::new(0),
        });
        let status = Arc::new(LogStatus::default());
        let (tx, stop) = watch::channel(false);
        let run = supervise(
            opener.clone(),
            Arc::new(LogSink(Default::default())),
            status.clone(),
            stop,
            timing(),
        );
        tokio::join!(run, async {
            tokio::time::sleep(Duration::from_secs(5)).await;
            tx.send(true).unwrap();
        });
        assert_eq!(status.0.lock().unwrap().last(), Some(&LinkState::Off));
        assert_eq!(*opener.opens.lock().unwrap(), 1);
    }

    #[test]
    fn backoff_doubles_to_its_cap_with_bounded_jitter() {
        let mut b = Backoff::new(Duration::from_secs(1), Duration::from_secs(300));
        let bases: Vec<u64> = (0..11).map(|_| b.next_base().as_secs()).collect();
        assert_eq!(bases, vec![1, 2, 4, 8, 16, 32, 64, 128, 256, 300, 300]);
        b.reset();
        assert_eq!(b.next_with(0.0), Duration::from_millis(800));
        assert_eq!(b.next_with(1.0), Duration::from_millis(2_400));
        for _ in 0..50 {
            let d = b.next_delay();
            assert!(d <= Duration::from_secs(360), "{d:?}");
        }
    }

    #[test]
    fn open_errors_are_fatal_only_for_refused_tokens() {
        assert!(matches!(
            open_error(SlackApiError::Api {
                code: "token_revoked".into(),
                needed: None
            }),
            OpenError::Fatal(_)
        ));
        assert_eq!(
            open_error(SlackApiError::RateLimited {
                retry_after: Some(Duration::from_secs(9))
            }),
            OpenError::Retry {
                message: "Slack is rate limiting new connections".into(),
                after: Some(Duration::from_secs(9))
            }
        );
        assert!(matches!(
            open_error(SlackApiError::Network("down".into())),
            OpenError::Retry { after: None, .. }
        ));
        assert!(matches!(
            open_error(SlackApiError::Status(503)),
            OpenError::Retry { .. }
        ));
    }
}
