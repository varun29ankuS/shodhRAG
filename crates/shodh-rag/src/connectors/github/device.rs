//! Sign-in with GitHub's device flow, for the Shodh GitHub App
//! (<https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/generating-a-user-access-token-for-a-github-app#using-the-device-flow-to-generate-a-user-access-token>).
//!
//! 1. [`DeviceFlow::start`] asks `POST /login/device/code` (the client id only) for a
//!    one-time code the user types at <https://github.com/login/device>.
//! 2. [`DeviceFlow::poll`] asks `POST /login/oauth/access_token` (client id, device code,
//!    grant type) no faster than the interval GitHub set. GitHub answers 200 with an
//!    `error` field while it waits: `authorization_pending` (keep waiting), `slow_down`
//!    (wait longer: the new interval comes with it), `expired_token` (start again),
//!    `access_denied` (the user said no), `device_flow_disabled`,
//!    `incorrect_client_credentials`, `incorrect_device_code`, `unsupported_grant_type`.
//! 3. [`DeviceFlow::refresh`] renews an expiring token with its refresh token. GitHub's
//!    docs: the client secret is "required unless the user access token was generated
//!    using the device flow", so no secret is needed. The Shodh app has expiring tokens
//!    off, so GitHub issues no refresh token and this is used only if that changes.
//!
//! **No client secret** exists in Shodh. The client id is public. No `scope` is sent: a
//! GitHub App's user token has no scopes (GitHub returns `scope: ""`); it can do what
//! both the app's permissions and the user allow, on the repositories the app is
//! installed on.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::token::{scrub, RefreshToken, SignIn, UserToken};

/// The Shodh GitHub App's client id (public; App id 5555109, owner varun29ankuS).
pub const CLIENT_ID: &str = "Iv23liJiG7jt0sEpk7yK";

/// The app's public page, where it is installed on accounts and repositories.
pub const APP_INSTALL_URL: &str = "https://github.com/apps/shodh-rag/installations/new";

/// Where a user reviews or revokes the apps they authorised. Signing out of Shodh
/// cannot revoke the token itself: GitHub's revoke endpoint needs the client secret.
pub const AUTHORIZED_APPS_URL: &str = "https://github.com/settings/apps/authorizations";

/// GitHub's sign-in host.
pub const GITHUB_LOGIN: &str = "https://github.com/";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// What `slow_down` adds when GitHub does not say the new interval.
const SLOW_DOWN_STEP: Duration = Duration::from_secs(5);
/// The interval when GitHub does not give one.
const DEFAULT_INTERVAL: Duration = Duration::from_secs(5);

/// A device code to show the user.
#[derive(Clone)]
pub struct DeviceCode {
    /// The code the user types (`WDJB-MJHT`).
    pub user_code: String,
    /// Where they type it.
    pub verification_uri: String,
    pub expires_at: DateTime<Utc>,
    pub interval: Duration,
    /// Shodh's half of the exchange: never shown, logged or sent anywhere but GitHub.
    device_code: String,
}

impl std::fmt::Debug for DeviceCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceCode")
            .field("user_code", &self.user_code)
            .field("verification_uri", &self.verification_uri)
            .field("expires_at", &self.expires_at)
            .field("interval", &self.interval)
            .field("device_code", &"[REDACTED]")
            .finish()
    }
}

/// One poll's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Poll {
    /// The user has not entered the code yet: poll again after the interval.
    Pending,
    /// Poll less often: after this interval from now on.
    SlowDown(Duration),
    Done(SignIn),
}

/// Why sign-in stopped. No variant holds a code or a token.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DeviceError {
    #[error("The code expired before it was entered. Start signing in again.")]
    Expired,
    #[error("Sign-in was cancelled on GitHub.")]
    Denied,
    #[error("GitHub has device sign-in switched off for the Shodh app.")]
    Disabled,
    #[error("GitHub did not recognise the Shodh app ({0}).")]
    App(String),
    #[error("GitHub refused the renewal of the sign-in. Sign in again.")]
    RefreshRefused,
    #[error("GitHub could not be reached: {0}")]
    Network(String),
    #[error("GitHub's answer could not be read: {0}")]
    BadResponse(String),
}

/// The device-flow client.
#[derive(Clone)]
pub struct DeviceFlow {
    client: reqwest::Client,
    base: String,
    client_id: String,
}

impl std::fmt::Debug for DeviceFlow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceFlow")
            .field("base", &self.base)
            .finish()
    }
}

impl DeviceFlow {
    /// The flow against github.com for the Shodh app.
    pub fn new() -> Result<Self, DeviceError> {
        Self::with_base(GITHUB_LOGIN, CLIENT_ID)
    }

    /// The flow against `base` (tests use a local server).
    pub fn with_base(base: &str, client_id: &str) -> Result<Self, DeviceError> {
        let mut base = base.to_string();
        if !base.ends_with('/') {
            base.push('/');
        }
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .user_agent(concat!("Shodh/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| DeviceError::Network(e.without_url().to_string()))?;
        Ok(Self {
            client,
            base,
            client_id: client_id.to_string(),
        })
    }

    async fn post(
        &self,
        path: &str,
        form: &[(&str, &str)],
        secret: &str,
    ) -> Result<Value, DeviceError> {
        let response = self
            .client
            .post(format!("{}{path}", self.base))
            .header(reqwest::header::ACCEPT, "application/json")
            .form(form)
            .send()
            .await
            .map_err(|e| DeviceError::Network(scrub(&e.without_url().to_string(), secret)))?;
        let status = response.status().as_u16();
        let text = response
            .text()
            .await
            .map_err(|e| DeviceError::Network(scrub(&e.without_url().to_string(), secret)))?;
        match serde_json::from_str::<Value>(&text) {
            Ok(value) => Ok(value),
            Err(_) if status >= 500 => Err(DeviceError::Network(format!(
                "GitHub answered HTTP {status}"
            ))),
            Err(_) => Err(DeviceError::BadResponse(format!("HTTP {status}, not JSON"))),
        }
    }

    /// Asks for a device code.
    pub async fn start(&self) -> Result<DeviceCode, DeviceError> {
        let value = self
            .post(
                "login/device/code",
                &[("client_id", self.client_id.as_str())],
                "",
            )
            .await?;
        if let Some(error) = value.get("error").and_then(Value::as_str) {
            return Err(flow_error(error));
        }
        let text = |key: &str| {
            value
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_string)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| DeviceError::BadResponse(format!("no {key}")))
        };
        let device_code = text("device_code")?;
        let user_code = text("user_code")?;
        let verification_uri = text("verification_uri")?;
        if !verification_uri.starts_with("https://github.com/") && self.base == GITHUB_LOGIN {
            return Err(DeviceError::BadResponse(
                "an unexpected sign-in page".into(),
            ));
        }
        let expires_in = value
            .get("expires_in")
            .and_then(Value::as_u64)
            .unwrap_or(900);
        let interval = value
            .get("interval")
            .and_then(Value::as_u64)
            .map(Duration::from_secs)
            .unwrap_or(DEFAULT_INTERVAL);
        Ok(DeviceCode {
            user_code,
            verification_uri,
            expires_at: Utc::now()
                + chrono::Duration::seconds(i64::try_from(expires_in).unwrap_or(900)),
            interval,
            device_code,
        })
    }

    /// Asks once whether the user entered the code.
    pub async fn poll(&self, code: &DeviceCode) -> Result<Poll, DeviceError> {
        let value = self
            .post(
                "login/oauth/access_token",
                &[
                    ("client_id", self.client_id.as_str()),
                    ("device_code", code.device_code.as_str()),
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ],
                &code.device_code,
            )
            .await?;
        match value.get("error").and_then(Value::as_str) {
            Some("authorization_pending") => Ok(Poll::Pending),
            Some("slow_down") => Ok(Poll::SlowDown(
                value
                    .get("interval")
                    .and_then(Value::as_u64)
                    .map(Duration::from_secs)
                    .unwrap_or(code.interval + SLOW_DOWN_STEP),
            )),
            Some(other) => Err(flow_error(other)),
            None => sign_in_of(&value, Utc::now()).map(Poll::Done),
        }
    }

    /// Renews an expiring sign-in with its refresh token (no client secret: the token
    /// came from the device flow).
    pub async fn refresh(&self, refresh: &RefreshToken) -> Result<SignIn, DeviceError> {
        let value = self
            .post(
                "login/oauth/access_token",
                &[
                    ("client_id", self.client_id.as_str()),
                    ("grant_type", "refresh_token"),
                    ("refresh_token", refresh.expose()),
                ],
                refresh.expose(),
            )
            .await?;
        match value.get("error").and_then(Value::as_str) {
            Some("bad_refresh_token") | Some("unauthorized") => Err(DeviceError::RefreshRefused),
            Some(other) => Err(flow_error(other)),
            None => sign_in_of(&value, Utc::now()),
        }
    }
}

fn flow_error(error: &str) -> DeviceError {
    match error {
        "expired_token" | "token_expired" => DeviceError::Expired,
        "access_denied" => DeviceError::Denied,
        "device_flow_disabled" => DeviceError::Disabled,
        "incorrect_client_credentials" | "unsupported_grant_type" | "incorrect_device_code" => {
            DeviceError::App(error.to_string())
        }
        "bad_refresh_token" => DeviceError::RefreshRefused,
        other => DeviceError::BadResponse(other.chars().take(80).collect()),
    }
}

/// The sign-in of a successful token answer.
fn sign_in_of(value: &Value, now: DateTime<Utc>) -> Result<SignIn, DeviceError> {
    let access = value
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| DeviceError::BadResponse("no access token".into()))?;
    let access = UserToken::parse(access).map_err(|e| DeviceError::BadResponse(e.to_string()))?;
    let after = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_i64)
            .filter(|s| *s > 0)
            .map(|s| now + chrono::Duration::seconds(s))
    };
    let refresh = value
        .get("refresh_token")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(RefreshToken::parse)
        .transpose()
        .map_err(|e| DeviceError::BadResponse(e.to_string()))?;
    Ok(SignIn {
        access,
        expires_at: after("expires_in"),
        refresh,
        refresh_expires_at: after("refresh_token_expires_in"),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::fake::{serve, Reply};
    use super::super::token::tests::{REFRESH, TOKEN};
    use super::*;

    async fn client_of(replies: Vec<Reply>) -> (DeviceFlow, super::super::fake::Log) {
        let (base, log) = serve(replies).await;
        (DeviceFlow::with_base(&base, CLIENT_ID).unwrap(), log)
    }

    fn code_reply() -> Reply {
        Reply::json(
            200,
            json!({
                "device_code": "3584d83530557fdd1f46af8289938c8ef79f9dc5",
                "user_code": "WDJB-MJHT",
                "verification_uri": "https://github.com/login/device",
                "expires_in": 900,
                "interval": 5
            }),
        )
    }

    #[tokio::test]
    async fn sign_in_waits_slows_down_and_finishes() {
        let (flow, log) = client_of(vec![
            code_reply(),
            Reply::json(200, json!({"error": "authorization_pending"})),
            Reply::json(200, json!({"error": "slow_down", "interval": 10})),
            Reply::json(
                200,
                json!({"access_token": TOKEN, "token_type": "bearer", "scope": ""}),
            ),
        ])
        .await;
        let code = flow.start().await.unwrap();
        assert_eq!(code.user_code, "WDJB-MJHT");
        assert_eq!(code.interval, Duration::from_secs(5));
        assert_eq!(flow.poll(&code).await.unwrap(), Poll::Pending);
        assert_eq!(
            flow.poll(&code).await.unwrap(),
            Poll::SlowDown(Duration::from_secs(10))
        );
        let Poll::Done(sign_in) = flow.poll(&code).await.unwrap() else {
            panic!("signed in");
        };
        assert_eq!(sign_in.access.expose(), TOKEN);
        assert_eq!(
            sign_in.expires_at, None,
            "expiry is off: the token does not expire"
        );
        assert_eq!(sign_in.refresh, None);

        let seen = log.lock().unwrap();
        // The client id only: no secret, no scope.
        assert_eq!(seen[0].path, "/login/device/code");
        assert_eq!(seen[0].body, format!("client_id={CLIENT_ID}"));
        assert!(seen[0]
            .head
            .to_ascii_lowercase()
            .contains("accept: application/json"));
        assert_eq!(seen[1].path, "/login/oauth/access_token");
        assert!(seen[1]
            .body
            .contains("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code"));
        assert!(!seen.iter().any(|s| s.body.contains("client_secret")));
        assert!(!seen.iter().any(|s| s.body.contains("scope")));
        // The device code is never printed.
        let debug = format!("{code:?}");
        assert!(!debug.contains("3584d835"), "{debug}");
    }

    #[tokio::test]
    async fn slow_down_without_an_interval_adds_five_seconds() {
        let (flow, _) = client_of(vec![
            code_reply(),
            Reply::json(200, json!({"error": "slow_down"})),
        ])
        .await;
        let code = flow.start().await.unwrap();
        assert_eq!(
            flow.poll(&code).await.unwrap(),
            Poll::SlowDown(Duration::from_secs(10))
        );
    }

    #[tokio::test]
    async fn expired_denied_and_disabled_end_the_sign_in() {
        for (error, expected) in [
            ("expired_token", DeviceError::Expired),
            ("access_denied", DeviceError::Denied),
            ("device_flow_disabled", DeviceError::Disabled),
            (
                "incorrect_client_credentials",
                DeviceError::App("incorrect_client_credentials".into()),
            ),
        ] {
            let (flow, _) = client_of(vec![
                code_reply(),
                Reply::json(200, json!({ "error": error })),
            ])
            .await;
            let code = flow.start().await.unwrap();
            assert_eq!(flow.poll(&code).await.unwrap_err(), expected, "{error}");
        }
        let (flow, _) = client_of(vec![Reply::json(
            200,
            json!({"error": "device_flow_disabled"}),
        )])
        .await;
        assert_eq!(flow.start().await.unwrap_err(), DeviceError::Disabled);
    }

    #[tokio::test]
    async fn an_expiring_token_is_renewed_without_a_secret() {
        let (flow, log) = client_of(vec![
            Reply::json(
                200,
                json!({
                    "access_token": TOKEN,
                    "expires_in": 28800,
                    "refresh_token": REFRESH,
                    "refresh_token_expires_in": 15897600,
                    "scope": "",
                    "token_type": "bearer"
                }),
            ),
            Reply::json(200, json!({"error": "bad_refresh_token"})),
        ])
        .await;
        let refresh = RefreshToken::parse(REFRESH).unwrap();
        let renewed = flow.refresh(&refresh).await.unwrap();
        assert_eq!(renewed.access.expose(), TOKEN);
        assert!(renewed.expires_at.is_some());
        assert!(renewed.refresh.is_some());
        assert_eq!(
            flow.refresh(&refresh).await.unwrap_err(),
            DeviceError::RefreshRefused
        );
        let seen = log.lock().unwrap();
        assert!(seen[0].body.contains("grant_type=refresh_token"));
        assert!(!seen[0].body.contains("client_secret"));
    }

    #[tokio::test]
    async fn errors_never_carry_a_secret() {
        // Nothing listens: reqwest's error would name the URL; the form is never in it.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let flow = DeviceFlow::with_base(&format!("http://127.0.0.1:{port}/"), CLIENT_ID).unwrap();
        let error = flow
            .refresh(&RefreshToken::parse(REFRESH).unwrap())
            .await
            .unwrap_err();
        assert!(matches!(error, DeviceError::Network(_)));
        let text = format!("{error} {error:?}");
        assert!(!text.contains("ghr_"), "{text}");
    }
}
