//! SSRF-safe HTTP for agent tools.
//!
//! Every request, and every redirect hop, is checked before a connection is
//! made:
//! 1. the URL must be `http` or `https`, without embedded credentials;
//! 2. the host is resolved here (never by the HTTP stack or a proxy) and
//!    every resolved address must be public ([`super::ssrf::check_ip`]);
//! 3. the connection is pinned to exactly those addresses, so DNS cannot
//!    change between the check and the connect (rebinding);
//! 4. redirects are followed by hand, at most [`MAX_REDIRECTS`], each hop
//!    re-checked from step 1;
//! 5. bodies are read as a stream and cut at the caller's byte cap, whatever
//!    `Content-Length` claims.
//!
//! DNS and HTTP sit behind [`Resolver`] and [`HttpTransport`] so the checks
//! are tested in process, without a network.

use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use url::Url;

use super::ssrf::{check_ip, BlockedAddress};

/// Redirect hops followed per request.
pub const MAX_REDIRECTS: usize = 5;

/// Identifies Shodh to the sites it fetches.
pub const USER_AGENT: &str = concat!(
    "Shodh/",
    env!("CARGO_PKG_VERSION"),
    " (desktop research assistant; fetches pages the user asked about)"
);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WebError {
    #[error("{0} is not a valid web address")]
    InvalidUrl(String),
    #[error("Only http and https addresses can be fetched, not {0}")]
    UnsupportedScheme(String),
    #[error("Addresses with a user name or password are not fetched")]
    Credentials,
    #[error("{host} resolves to a {reason} ({ip}); only public internet addresses are fetched")]
    Blocked {
        host: String,
        ip: IpAddr,
        reason: BlockedAddress,
    },
    #[error("{0} could not be resolved")]
    Unresolvable(String),
    #[error("More than {MAX_REDIRECTS} redirects")]
    TooManyRedirects,
    #[error("A redirect had no valid Location")]
    BadRedirect,
    #[error("The response is larger than {limit} bytes")]
    TooLarge { limit: u64 },
    #[error("The request timed out")]
    Timeout,
    #[error("{0}")]
    Http(String),
    #[error("The server answered {status}")]
    Status { status: u16 },
}

/// Where a host resolves.
#[async_trait]
pub trait Resolver: Send + Sync {
    async fn resolve(&self, host: &str, port: u16) -> std::io::Result<Vec<IpAddr>>;
}

/// The system resolver.
pub struct SystemResolver;

#[async_trait]
impl Resolver for SystemResolver {
    async fn resolve(&self, host: &str, port: u16) -> std::io::Result<Vec<IpAddr>> {
        let addrs = tokio::net::lookup_host((host, port)).await?;
        Ok(addrs.map(|a| a.ip()).collect())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

/// One HTTP exchange, already checked: `connect_to` holds the only
/// addresses the transport may connect to for `url`'s host.
#[derive(Debug, Clone)]
pub struct HttpRequest {
    pub method: Method,
    pub url: Url,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    pub connect_to: Vec<SocketAddr>,
    pub timeout: Duration,
}

pub type BodyStream = Pin<Box<dyn Stream<Item = Result<Bytes, WebError>> + Send>>;

pub struct HttpResponse {
    pub status: u16,
    /// Header names lower-cased.
    pub headers: Vec<(String, String)>,
    pub body: BodyStream,
}

impl HttpResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Sends one request without following redirects.
#[async_trait]
pub trait HttpTransport: Send + Sync {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, WebError>;
}

/// The production transport: reqwest with no proxy (a proxy would resolve
/// DNS itself and defeat the address pinning), no automatic redirects and
/// the connection pinned to the checked addresses.
pub struct ReqwestTransport;

#[async_trait]
impl HttpTransport for ReqwestTransport {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, WebError> {
        let host = request
            .url
            .host_str()
            .ok_or_else(|| WebError::InvalidUrl(request.url.to_string()))?
            .to_string();
        let mut builder = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(request.timeout)
            .connect_timeout(Duration::from_secs(10))
            .user_agent(USER_AGENT);
        if host.parse::<IpAddr>().is_err() {
            builder = builder.resolve_to_addrs(&host, &request.connect_to);
        }
        let client = builder
            .build()
            .map_err(|e| WebError::Http(format!("HTTP client setup failed: {e}")))?;
        let mut call = match request.method {
            Method::Get => client.get(request.url.clone()),
            Method::Post => client.post(request.url.clone()),
        };
        for (name, value) in &request.headers {
            call = call.header(name, value);
        }
        if let Some(body) = request.body {
            call = call.body(body);
        }
        let response = call.send().await.map_err(|e| {
            if e.is_timeout() {
                WebError::Timeout
            } else {
                WebError::Http(format!("Request to {host} failed: {e}"))
            }
        })?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(k, v)| {
                v.to_str()
                    .ok()
                    .map(|v| (k.as_str().to_ascii_lowercase(), v.to_string()))
            })
            .collect();
        let body = response.bytes_stream().map(|chunk| {
            chunk.map_err(|e| {
                if e.is_timeout() {
                    WebError::Timeout
                } else {
                    WebError::Http(format!("Reading the response failed: {e}"))
                }
            })
        });
        Ok(HttpResponse {
            status,
            headers,
            body: Box::pin(body),
        })
    }
}

/// Validate scheme and credentials of a URL.
pub fn check_url(raw: &str) -> Result<Url, WebError> {
    let url = Url::parse(raw.trim()).map_err(|_| WebError::InvalidUrl(raw.to_string()))?;
    match url.scheme() {
        "http" | "https" => {}
        other => return Err(WebError::UnsupportedScheme(other.to_string())),
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(WebError::Credentials);
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err(WebError::InvalidUrl(raw.to_string()));
    }
    Ok(url)
}

/// What a fetch returned after redirects.
pub struct Fetched {
    pub final_url: Url,
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: BodyStream,
}

impl Fetched {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// Read the whole body, failing once it exceeds `limit` bytes.
    pub async fn read_capped(mut self, limit: u64) -> Result<Vec<u8>, WebError> {
        let mut out = Vec::new();
        while let Some(chunk) = self.body.next().await {
            let chunk = chunk?;
            if out.len() as u64 + chunk.len() as u64 > limit {
                return Err(WebError::TooLarge { limit });
            }
            out.extend_from_slice(&chunk);
        }
        Ok(out)
    }
}

/// HTTP client for agent tools. Cheap to clone.
#[derive(Clone)]
pub struct SafeClient {
    resolver: Arc<dyn Resolver>,
    transport: Arc<dyn HttpTransport>,
    /// `host:port` origins the administrator configured (e.g. a self-hosted
    /// SearXNG on the LAN). Their addresses are not classified; nothing the
    /// model supplies can add to this set.
    trusted_origins: Arc<HashSet<String>>,
    timeout: Duration,
}

impl SafeClient {
    pub fn new(resolver: Arc<dyn Resolver>, transport: Arc<dyn HttpTransport>) -> Self {
        Self {
            resolver,
            transport,
            trusted_origins: Arc::new(HashSet::new()),
            timeout: Duration::from_secs(20),
        }
    }

    /// The production client: system DNS and reqwest.
    pub fn system() -> Self {
        Self::new(Arc::new(SystemResolver), Arc::new(ReqwestTransport))
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Trust one configured origin's addresses (see `trusted_origins`).
    pub fn trusting(mut self, url: &Url) -> Self {
        if let (Some(host), Some(port)) = (url.host_str(), url.port_or_known_default()) {
            let mut set = (*self.trusted_origins).clone();
            set.insert(format!("{}:{port}", host.to_ascii_lowercase()));
            self.trusted_origins = Arc::new(set);
        }
        self
    }

    fn is_trusted(&self, host: &str, port: u16) -> bool {
        self.trusted_origins
            .contains(&format!("{}:{port}", host.to_ascii_lowercase()))
    }

    /// Resolve and check `url`'s host; returns the addresses to pin.
    async fn checked_addresses(&self, url: &Url) -> Result<Vec<SocketAddr>, WebError> {
        let host = url
            .host_str()
            .ok_or_else(|| WebError::InvalidUrl(url.to_string()))?;
        let port = url
            .port_or_known_default()
            .ok_or_else(|| WebError::InvalidUrl(url.to_string()))?;
        let trusted = self.is_trusted(host, port);
        let ips: Vec<IpAddr> = match url.host() {
            Some(url::Host::Ipv4(ip)) => vec![IpAddr::V4(ip)],
            Some(url::Host::Ipv6(ip)) => vec![IpAddr::V6(ip)],
            Some(url::Host::Domain(domain)) => {
                let domain = domain.trim_end_matches('.');
                let localhost = domain.eq_ignore_ascii_case("localhost")
                    || domain.to_ascii_lowercase().ends_with(".localhost");
                if localhost && !trusted {
                    return Err(WebError::Blocked {
                        host: host.to_string(),
                        ip: IpAddr::from([127, 0, 0, 1]),
                        reason: BlockedAddress::Loopback,
                    });
                }
                self.resolver
                    .resolve(domain, port)
                    .await
                    .map_err(|_| WebError::Unresolvable(host.to_string()))?
            }
            None => return Err(WebError::InvalidUrl(url.to_string())),
        };
        if ips.is_empty() {
            return Err(WebError::Unresolvable(host.to_string()));
        }
        if !trusted {
            for ip in &ips {
                check_ip(*ip).map_err(|reason| WebError::Blocked {
                    host: host.to_string(),
                    ip: *ip,
                    reason,
                })?;
            }
        }
        Ok(ips
            .into_iter()
            .map(|ip| SocketAddr::new(ip, port))
            .collect())
    }

    /// Send a request, following redirects with every hop re-checked.
    /// A POST that is redirected continues as a GET without a body (303
    /// semantics); 307/308 keep the method.
    pub async fn request(
        &self,
        method: Method,
        url: &str,
        headers: Vec<(String, String)>,
        body: Option<Vec<u8>>,
    ) -> Result<Fetched, WebError> {
        let mut url = check_url(url)?;
        let mut method = method;
        let mut body = body;
        for _ in 0..=MAX_REDIRECTS {
            let connect_to = self.checked_addresses(&url).await?;
            let response = self
                .transport
                .send(HttpRequest {
                    method,
                    url: url.clone(),
                    headers: headers.clone(),
                    body: body.clone(),
                    connect_to,
                    timeout: self.timeout,
                })
                .await?;
            if matches!(response.status, 301 | 302 | 303 | 307 | 308) {
                let location = response.header("location").ok_or(WebError::BadRedirect)?;
                let next = url.join(location).map_err(|_| WebError::BadRedirect)?;
                url = check_url(next.as_str())?;
                if !matches!(response.status, 307 | 308) {
                    method = Method::Get;
                    body = None;
                }
                continue;
            }
            return Ok(Fetched {
                final_url: url,
                status: response.status,
                headers: response.headers,
                body: response.body,
            });
        }
        Err(WebError::TooManyRedirects)
    }

    pub async fn get(
        &self,
        url: &str,
        headers: Vec<(String, String)>,
    ) -> Result<Fetched, WebError> {
        self.request(Method::Get, url, headers, None).await
    }

    /// POST JSON and parse a JSON answer of at most `limit` bytes. Non-2xx
    /// answers fail with the status and the start of the body.
    pub async fn post_json(
        &self,
        url: &str,
        headers: Vec<(String, String)>,
        body: &serde_json::Value,
        limit: u64,
    ) -> Result<serde_json::Value, WebError> {
        let mut headers = headers;
        headers.push(("content-type".into(), "application/json".into()));
        let encoded = serde_json::to_vec(body)
            .map_err(|e| WebError::Http(format!("Could not encode the request: {e}")))?;
        let fetched = self
            .request(Method::Post, url, headers, Some(encoded))
            .await?;
        json_body(fetched, limit).await
    }

    /// GET and parse a JSON answer of at most `limit` bytes.
    pub async fn get_json(
        &self,
        url: &str,
        headers: Vec<(String, String)>,
        limit: u64,
    ) -> Result<serde_json::Value, WebError> {
        let fetched = self.get(url, headers).await?;
        json_body(fetched, limit).await
    }
}

async fn json_body(fetched: Fetched, limit: u64) -> Result<serde_json::Value, WebError> {
    let status = fetched.status;
    let bytes = fetched.read_capped(limit).await?;
    if !(200..300).contains(&status) {
        let text = String::from_utf8_lossy(&bytes);
        let start: String = text.chars().take(300).collect();
        return Err(WebError::Http(format!(
            "The server answered {status}: {start}"
        )));
    }
    serde_json::from_slice(&bytes)
        .map_err(|e| WebError::Http(format!("The answer was not valid JSON: {e}")))
}

#[cfg(test)]
pub(crate) mod testing {
    //! In-process DNS and HTTP for tests.

    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct FakeResolver {
        pub hosts: HashMap<String, Vec<IpAddr>>,
    }

    impl FakeResolver {
        pub fn with(mut self, host: &str, ips: &[&str]) -> Self {
            self.hosts.insert(
                host.to_string(),
                ips.iter().map(|ip| ip.parse().unwrap()).collect(),
            );
            self
        }
    }

    #[async_trait]
    impl Resolver for FakeResolver {
        async fn resolve(&self, host: &str, _port: u16) -> std::io::Result<Vec<IpAddr>> {
            self.hosts
                .get(host)
                .cloned()
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no such host"))
        }
    }

    /// A canned response for a URL.
    #[derive(Clone)]
    pub struct Canned {
        pub status: u16,
        pub headers: Vec<(String, String)>,
        pub body: Vec<u8>,
    }

    impl Canned {
        pub fn ok(content_type: &str, body: impl Into<Vec<u8>>) -> Self {
            Self {
                status: 200,
                headers: vec![("content-type".into(), content_type.into())],
                body: body.into(),
            }
        }
        pub fn redirect(location: &str) -> Self {
            Self {
                status: 302,
                headers: vec![("location".into(), location.into())],
                body: Vec::new(),
            }
        }
    }

    #[derive(Default)]
    pub struct FakeTransport {
        pub routes: HashMap<String, Canned>,
        pub sent: Mutex<Vec<HttpRequest>>,
    }

    impl FakeTransport {
        pub fn route(mut self, url: &str, canned: Canned) -> Self {
            self.routes.insert(url.to_string(), canned);
            self
        }
    }

    #[async_trait]
    impl HttpTransport for FakeTransport {
        async fn send(&self, request: HttpRequest) -> Result<HttpResponse, WebError> {
            let canned = self
                .routes
                .get(request.url.as_str())
                .cloned()
                .ok_or(WebError::Status { status: 404 })?;
            self.sent
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(request);
            // Deliver the body in small chunks, as a network would.
            let chunks: Vec<Result<Bytes, WebError>> = canned
                .body
                .chunks(1024)
                .map(|c| Ok(Bytes::copy_from_slice(c)))
                .collect();
            Ok(HttpResponse {
                status: canned.status,
                headers: canned.headers,
                body: Box::pin(futures::stream::iter(chunks)),
            })
        }
    }

    pub fn client(
        resolver: FakeResolver,
        transport: FakeTransport,
    ) -> (SafeClient, Arc<FakeTransport>) {
        let transport = Arc::new(transport);
        (
            SafeClient::new(Arc::new(resolver), transport.clone()),
            transport,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    #[test]
    fn urls_must_be_http_without_credentials() {
        assert!(check_url("https://example.com/a").is_ok());
        assert!(matches!(
            check_url("file:///etc/passwd"),
            Err(WebError::UnsupportedScheme(_))
        ));
        assert!(matches!(
            check_url("ftp://example.com"),
            Err(WebError::UnsupportedScheme(_))
        ));
        assert_eq!(
            check_url("https://user:pw@example.com").unwrap_err(),
            WebError::Credentials
        );
        assert!(check_url("not a url").is_err());
    }

    #[tokio::test]
    async fn private_hosts_and_numeric_tricks_are_refused_before_connecting() {
        let (client, transport) = client(
            FakeResolver::default()
                .with("internal.example", &["10.0.0.5"])
                .with("mixed.example", &["93.184.216.34", "127.0.0.1"]),
            FakeTransport::default(),
        );
        for url in [
            "http://127.0.0.1/",
            "http://2130706433/",
            "http://0x7f.1/",
            "http://[::1]/",
            "http://[::ffff:127.0.0.1]/",
            "http://169.254.169.254/latest/meta-data",
            "http://localhost:8080/",
            "http://internal.example/",
            "http://mixed.example/",
        ] {
            let err = client
                .get(url, vec![])
                .await
                .err()
                .unwrap_or_else(|| panic!("{url}"));
            assert!(matches!(err, WebError::Blocked { .. }), "{url}: {err}");
        }
        assert!(transport.sent.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn connections_are_pinned_to_the_checked_addresses() {
        let (client, transport) = client(
            FakeResolver::default().with("example.com", &["93.184.216.34"]),
            FakeTransport::default()
                .route("https://example.com/", Canned::ok("text/html", "<p>hi</p>")),
        );
        let fetched = client.get("https://example.com/", vec![]).await.unwrap();
        assert_eq!(fetched.status, 200);
        let sent = transport.sent.lock().unwrap();
        assert_eq!(
            sent[0].connect_to,
            vec!["93.184.216.34:443".parse::<SocketAddr>().unwrap()]
        );
    }

    #[tokio::test]
    async fn redirects_are_rechecked_and_capped() {
        let (client, transport) = client(
            FakeResolver::default()
                .with("public.example", &["93.184.216.34"])
                .with("evil.example", &["93.184.216.35"])
                .with("rebind.example", &["192.168.1.10"]),
            FakeTransport::default()
                .route(
                    "https://public.example/start",
                    Canned::redirect("https://rebind.example/admin"),
                )
                .route(
                    "https://public.example/meta",
                    Canned::redirect("http://169.254.169.254/"),
                )
                .route("https://public.example/rel", Canned::redirect("/final"))
                .route(
                    "https://public.example/final",
                    Canned::ok("text/plain", "done"),
                )
                .route(
                    "https://evil.example/loop",
                    Canned::redirect("https://evil.example/loop"),
                ),
        );
        let err = client
            .get("https://public.example/start", vec![])
            .await
            .err()
            .unwrap();
        assert!(matches!(err, WebError::Blocked { ref host, .. } if host == "rebind.example"));
        let err = client
            .get("https://public.example/meta", vec![])
            .await
            .err()
            .unwrap();
        assert!(matches!(err, WebError::Blocked { .. }));
        let ok = client
            .get("https://public.example/rel", vec![])
            .await
            .unwrap();
        assert_eq!(ok.final_url.as_str(), "https://public.example/final");
        let err = client
            .get("https://evil.example/loop", vec![])
            .await
            .err()
            .unwrap();
        assert_eq!(err, WebError::TooManyRedirects);
        let hops = transport
            .sent
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.url.as_str() == "https://evil.example/loop")
            .count();
        assert_eq!(hops, MAX_REDIRECTS + 1);
    }

    #[tokio::test]
    async fn bodies_are_capped_while_streaming() {
        let (client, _) = client(
            FakeResolver::default().with("big.example", &["93.184.216.34"]),
            FakeTransport::default().route(
                "https://big.example/",
                Canned::ok("text/plain", vec![b'x'; 10_000]),
            ),
        );
        let fetched = client.get("https://big.example/", vec![]).await.unwrap();
        assert_eq!(
            fetched.read_capped(4_096).await.unwrap_err(),
            WebError::TooLarge { limit: 4_096 }
        );
        let fetched = client.get("https://big.example/", vec![]).await.unwrap();
        assert_eq!(fetched.read_capped(10_000).await.unwrap().len(), 10_000);
    }

    #[tokio::test]
    async fn configured_origins_may_be_private() {
        let searx = Url::parse("http://searx.lan:8888/search").unwrap();
        let (client, _) = client(
            FakeResolver::default().with("searx.lan", &["192.168.1.20"]),
            FakeTransport::default().route(
                "http://searx.lan:8888/search?q=x",
                Canned::ok("application/json", "{}"),
            ),
        );
        assert!(client
            .get("http://searx.lan:8888/search?q=x", vec![])
            .await
            .is_err());
        let trusted = client.trusting(&searx);
        assert!(trusted
            .get("http://searx.lan:8888/search?q=x", vec![])
            .await
            .is_ok());
        assert!(trusted.get("http://searx.lan:9999/", vec![]).await.is_err());
    }
}
