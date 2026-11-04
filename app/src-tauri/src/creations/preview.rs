//! The live preview of a creation: the `shodh-preview` scheme.
//!
//! An agent-built page is untrusted code. The Artifacts tab shows it in an
//! `<iframe sandbox="allow-scripts">` whose page comes from this scheme, so:
//!
//! - **Its own origin, opaque.** The frame has no `allow-same-origin`, and
//!   every response also carries `Content-Security-Policy: sandbox
//!   allow-scripts`, so the page runs in an opaque origin even if it were
//!   opened some other way. It cannot read the app's DOM, storage or cookies.
//! - **No Tauri IPC.** Tauri's IPC needs the invoke key its scripts put in
//!   the app's main frame only (16 random bytes per launch). On Windows the
//!   IPC goes through a fetch whose `Origin` an opaque frame sends as `null`,
//!   which Tauri refuses, and which `connect-src` blocks anyway. On macOS the
//!   message channel accepts a subframe's messages, so there the key is the
//!   one barrier. The app ACL (capabilities/, permissions/) cannot add a
//!   second one here: Tauri 2.9 resolves a command by window label, webview
//!   label and local or remote origin, the frame shares the label `main`,
//!   and every registered custom scheme, this one included, counts as a
//!   local origin (`Webview::is_local_url`). So the frame's requests resolve
//!   exactly like the app's own; `acl_manifest.rs` pins this. A preview in a
//!   webview of its own label would be refused every command by the ACL
//!   (the same test shows it), but a top-level page loses the frame's
//!   sandbox limits on navigation, so that move needs its own navigation
//!   lock first.
//! - **No files.** It is served only what the creation holds: by an
//!   unguessable token per preview ([`grant`]), a relative path, read from
//!   the open file and never through a link ([`super::handle`]). One preview
//!   cannot fetch another's files: its CSP names only its own token's path.
//! - **No loading or sending over the web unless the user allows it** for
//!   that preview: then `https:` and `wss:` are added to what it may load and
//!   connect to, never `http:`, which would reach `http://ipc.localhost` and
//!   this scheme's other paths on Windows. CSP does not govern every channel:
//!   each page loses the WebRTC interfaces before its own scripts run
//!   ([`PAGE_GUARD`]), WebView2 allows WebRTC no direct UDP, and DNS look-ahead
//!   is off. A browser may still connect ahead to a link the page shows; the
//!   preview's text says so.
//! - **Nowhere else.** The app's own CSP lets frames load only this scheme
//!   (`frame-src`), so the frame cannot navigate itself to another site, and
//!   without `allow-popups` or `allow-top-navigation` it opens no window and
//!   cannot replace the app.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Mutex;

use super::{CreationStore, MAX_FILE_BYTES};

/// The scheme's name, registered in `lib.rs`.
pub const SCHEME: &str = "shodh-preview";

/// Previews kept at once; older tokens stop working.
const MAX_GRANTS: usize = 32;

/// Where the scheme's pages live: Windows (WebView2) serves custom schemes
/// as `http://<scheme>.localhost`, macOS and Linux as `<scheme>://localhost`.
pub fn origin() -> &'static str {
    if cfg!(windows) {
        "http://shodh-preview.localhost"
    } else {
        "shodh-preview://localhost"
    }
}

/// What one preview token serves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    pub creation_id: String,
    /// A saved version, or the folder as it is now (`None`).
    pub version: Option<u32>,
    /// The user allowed this preview to reach the internet.
    pub network: bool,
    /// Shodh's data folder (the versions).
    pub data_dir: PathBuf,
    /// The resolved `Shodh Creations` folder.
    pub root: PathBuf,
}

/// The previews open now, by token.
pub struct Grants {
    open: Mutex<VecDeque<(String, Grant)>>,
}

impl Grants {
    pub const fn new() -> Self {
        Self {
            open: Mutex::new(VecDeque::new()),
        }
    }

    /// A new token for `grant` (the oldest of more than [`MAX_GRANTS`]
    /// lapses), and the URL of its first page.
    pub fn grant(&self, grant: Grant) -> (String, String) {
        let token = uuid::Uuid::new_v4().simple().to_string();
        let mut open = self.open.lock().unwrap_or_else(|e| e.into_inner());
        open.push_back((token.clone(), grant));
        while open.len() > MAX_GRANTS {
            open.pop_front();
        }
        let url = format!("{}/{token}/index.html", origin());
        (token, url)
    }

    fn granted(&self, token: &str) -> Option<Grant> {
        self.open
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|(known, _)| known == token)
            .map(|(_, grant)| grant.clone())
    }

    /// Every preview of `creation_id` ends (it was deleted).
    pub fn revoke(&self, creation_id: &str) {
        self.open
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|(_, grant)| grant.creation_id != creation_id);
    }
}

impl Default for Grants {
    fn default() -> Self {
        Self::new()
    }
}

/// The app's previews.
pub static PREVIEWS: Grants = Grants::new();

/// The Content-Security-Policy of every response of the preview `token`.
pub fn csp(token: &str, network: bool) -> String {
    let own = format!("{}/{token}/", origin());
    let web = if network { " https:" } else { "" };
    let sockets = if network { " https: wss:" } else { "" };
    format!(
        "sandbox allow-scripts; default-src 'none'; \
         script-src {own} 'unsafe-inline' 'unsafe-eval' blob:{web}; \
         style-src {own} 'unsafe-inline'{web}; \
         img-src {own} data: blob:{web}; \
         font-src {own} data:{web}; \
         media-src {own} data: blob:{web}; \
         connect-src {own}{sockets}; \
         worker-src {own} blob:; \
         frame-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'"
    )
}

/// The media type of a file, by its extension.
pub fn content_type(path: &str) -> &'static str {
    let extension = path
        .rsplit('/')
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" | "cjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "wasm" => "application/wasm",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "txt" | "md" => "text/plain; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        "xml" => "application/xml",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        _ => "application/octet-stream",
    }
}

/// `segment` with its `%XX` escapes decoded, or `None` when an escape is
/// malformed, the result is not UTF-8, or it holds a separator, a NUL or a
/// `..` (a path must stay a path of plain parts).
fn decode_segment(segment: &str) -> Option<String> {
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let hex = segment.get(at + 1..at + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    let text = String::from_utf8(out).ok()?;
    let bad =
        text.is_empty() || text == "." || text == ".." || text.contains(['/', '\\', '\0', ':']);
    (!bad).then_some(text)
}

/// The token and the file path a request's URI path names
/// (`/<token>/<file>`; a folder serves its `index.html`), or `None`.
pub fn parse_path(path: &str) -> Option<(String, String)> {
    let path = path.split(['?', '#']).next().unwrap_or("");
    let rest = path.strip_prefix('/')?;
    let (token, file) = rest.split_once('/').unwrap_or((rest, ""));
    if token.len() != 32 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut parts: Vec<String> = Vec::new();
    for segment in file.split('/') {
        if segment.is_empty() {
            continue;
        }
        parts.push(decode_segment(segment)?);
    }
    if file.is_empty() || file.ends_with('/') {
        parts.push("index.html".to_string());
    }
    Some((token.to_string(), parts.join("/")))
}

/// A response of the scheme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Served {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
    /// The policy every response carries ([`csp`]), even an error.
    pub csp: String,
}

/// What the scheme answers to a request for `path` from the webview
/// `webview`: only the app's main window shows previews.
pub fn serve(grants: &Grants, webview: &str, method: &str, path: &str) -> Served {
    let refused = |status: u16, text: &str, csp: String| Served {
        status,
        content_type: "text/plain; charset=utf-8",
        body: text.as_bytes().to_vec(),
        csp,
    };
    let locked = csp("00000000000000000000000000000000", false);
    if webview != "main" {
        return refused(403, "Previews show only in Shodh's window.", locked);
    }
    if method != "GET" && method != "HEAD" {
        return refused(405, "Previews only read files.", locked);
    }
    let Some((token, file)) = parse_path(path) else {
        return refused(404, "Not found.", locked);
    };
    let Some(grant) = grants.granted(&token) else {
        return refused(404, "This preview has ended. Open it again.", locked);
    };
    let policy = csp(&token, grant.network);
    let store = CreationStore::in_dir(&grant.data_dir);
    match store.read(
        &grant.root,
        &grant.creation_id,
        grant.version,
        &file,
        MAX_FILE_BYTES,
    ) {
        Ok((body, _)) => Served {
            status: 200,
            content_type: content_type(&file),
            body: if method == "HEAD" {
                Vec::new()
            } else if content_type(&file).starts_with("text/html") {
                with_guard(&body)
            } else {
                body
            },
            csp: policy,
        },
        Err(_) => refused(404, "Not found.", policy),
    }
}

/// What every preview page gets first, before its own scripts: no DNS look-
/// ahead, and no WebRTC (peer connections reach STUN and TURN servers outside
/// what CSP governs). The interfaces are made undefined and unchangeable on
/// the page's window; the frame cannot open another window or frame to get
/// fresh ones (no `allow-popups`, `frame-src 'none'`), and workers have none.
pub const PAGE_GUARD: &str = "<meta http-equiv=\"x-dns-prefetch-control\" content=\"off\">\
<script>(function(){var w=window;['RTCPeerConnection','webkitRTCPeerConnection',\
'RTCDataChannel','RTCIceCandidate','RTCSessionDescription','RTCRtpSender','RTCRtpReceiver',\
'RTCRtpTransceiver','RTCDtlsTransport','RTCIceTransport','RTCSctpTransport',\
'RTCCertificate'].forEach(function(n){try{Object.defineProperty(w,n,{value:undefined,\
writable:false,configurable:false});}catch(e){}});})();</script>";

/// `page` with [`PAGE_GUARD`] first: right after its doctype (so the page keeps
/// its standards mode), else at the very start.
pub fn with_guard(page: &[u8]) -> Vec<u8> {
    let mut start = 0;
    if page.starts_with(&[0xEF, 0xBB, 0xBF]) {
        start = 3;
    }
    while start < page.len() && page[start].is_ascii_whitespace() {
        start += 1;
    }
    let doctype = b"<!doctype";
    let at = if page.len() >= start + doctype.len()
        && page[start..start + doctype.len()].eq_ignore_ascii_case(doctype)
    {
        page[start..]
            .iter()
            .position(|&b| b == b'>')
            .map_or(start, |end| start + end + 1)
    } else {
        start
    };
    let mut out = Vec::with_capacity(page.len() + PAGE_GUARD.len());
    out.extend_from_slice(&page[..at]);
    out.extend_from_slice(PAGE_GUARD.as_bytes());
    out.extend_from_slice(&page[at..]);
    out
}

/// [`serve`] as an HTTP response, with the headers every preview response
/// carries.
pub fn response(
    webview: &str,
    request: &tauri::http::Request<Vec<u8>>,
) -> tauri::http::Response<Vec<u8>> {
    let served = serve(
        &PREVIEWS,
        webview,
        request.method().as_str(),
        request.uri().path(),
    );
    tauri::http::Response::builder()
        .status(served.status)
        .header("Content-Type", served.content_type)
        .header("Content-Security-Policy", served.csp)
        .header("X-Content-Type-Options", "nosniff")
        // No look-ahead DNS for the host names a page shows (a channel CSP does not govern).
        .header("X-DNS-Prefetch-Control", "off")
        .header("Referrer-Policy", "no-referrer")
        .header("Cache-Control", "no-store")
        // The page's own requests come from its opaque origin.
        .header("Access-Control-Allow-Origin", "*")
        .body(served.body)
        .unwrap_or_else(|_| {
            tauri::http::Response::builder()
                .status(500)
                .body(Vec::new())
                .unwrap_or_default()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_a_token_and_plain_parts() {
        let token = "0123456789abcdef0123456789abcdef";
        assert_eq!(
            parse_path(&format!("/{token}/")),
            Some((token.to_string(), "index.html".to_string()))
        );
        assert_eq!(
            parse_path(&format!("/{token}")),
            Some((token.to_string(), "index.html".to_string()))
        );
        assert_eq!(
            parse_path(&format!("/{token}/js/app.js?v=2")),
            Some((token.to_string(), "js/app.js".to_string()))
        );
        assert_eq!(
            parse_path(&format!("/{token}/my%20page.html")),
            Some((token.to_string(), "my page.html".to_string()))
        );
        assert_eq!(
            parse_path(&format!("/{token}/docs/")),
            Some((token.to_string(), "docs/index.html".to_string()))
        );
        for bad in [
            format!("/{token}/../secret.txt"),
            format!("/{token}/%2e%2e/secret.txt"),
            format!("/{token}/a%2Fb"),
            format!("/{token}/a%5Cb"),
            format!("/{token}/C:%5Cx"),
            format!("/{token}/x%00"),
            format!("/{token}/%zz"),
            format!("/{token}/./x"),
            "/short/index.html".to_string(),
            format!("/{}/x", "g".repeat(32)),
            "index.html".to_string(),
        ] {
            assert_eq!(parse_path(&bad), None, "{bad}");
        }
    }

    #[test]
    fn the_policy_keeps_the_page_sandboxed_and_offline_unless_allowed() {
        let token = "0123456789abcdef0123456789abcdef";
        let own = format!("{}/{token}/", origin());
        // The sources a policy names, one per word.
        let sources = |policy: &str| -> Vec<String> {
            policy
                .split([' ', ';'])
                .filter(|word| !word.is_empty())
                .map(str::to_string)
                .collect()
        };
        let offline = csp(token, false);
        assert!(offline.starts_with("sandbox allow-scripts;"));
        assert!(!offline.contains("allow-same-origin"));
        assert!(offline.contains("default-src 'none'"));
        assert!(offline.contains(&format!("connect-src {own};")));
        let named = sources(&offline);
        for scheme in ["https:", "http:", "wss:", "ws:", "ipc:", "*", "'self'"] {
            assert!(!named.iter().any(|s| s == scheme), "{scheme} in {offline}");
        }
        // Every host source is this preview's own path.
        for source in named.iter().filter(|s| s.contains("://")) {
            assert_eq!(source, &own, "{offline}");
        }
        let online = csp(token, true);
        assert!(online.starts_with("sandbox allow-scripts;"));
        assert!(online.contains(&format!("connect-src {own} https: wss:;")));
        // Never plain http (http://ipc.localhost, this scheme on Windows), never ipc.
        let named = sources(&online);
        for scheme in ["http:", "ipc:", "ws:", "*", "'self'"] {
            assert!(!named.iter().any(|s| s == scheme), "{scheme} in {online}");
        }
        for source in named.iter().filter(|s| s.contains("://")) {
            assert_eq!(source, &own, "{online}");
        }
        assert!(online.contains("frame-src 'none'"));
        assert!(online.contains("form-action 'none'"));
    }

    #[test]
    fn only_granted_previews_in_the_main_window_are_served() {
        let dir = tempfile::tempdir().unwrap();
        let documents = dir.path().join("Documents");
        std::fs::create_dir_all(&documents).unwrap();
        let root = super::super::creations_root(&documents).unwrap();
        let data = dir.path().join("data");
        let store = CreationStore::in_dir(&data);
        let creation = store
            .prepare(&root, "conv", "a page", "2026-11-03T10:00:00Z")
            .unwrap();
        std::fs::write(
            std::path::Path::new(&creation.folder).join("index.html"),
            "<p>hi</p>",
        )
        .unwrap();
        let grants = Grants::new();
        let (token, url) = grants.grant(Grant {
            creation_id: creation.id.clone(),
            version: None,
            network: false,
            data_dir: data.clone(),
            root: root.clone(),
        });
        assert_eq!(url, format!("{}/{token}/index.html", origin()));
        let get = |webview: &str, method: &str, path: &str| serve(&grants, webview, method, path);
        let page = get("main", "GET", &format!("/{token}/index.html"));
        assert_eq!(page.status, 200);
        // A page gets the guard (no WebRTC, no DNS look-ahead) before its own content.
        assert_eq!(page.body, with_guard(b"<p>hi</p>"));
        assert!(page.body.starts_with(PAGE_GUARD.as_bytes()));
        assert_eq!(page.content_type, "text/html; charset=utf-8");
        assert_eq!(page.csp, csp(&token, false));
        // Another window, another method, an unknown token, a missing file.
        assert_eq!(
            get("print-1", "GET", &format!("/{token}/index.html")).status,
            403
        );
        assert_eq!(
            get("main", "POST", &format!("/{token}/index.html")).status,
            405
        );
        let unknown = "f".repeat(32);
        assert_eq!(
            get("main", "GET", &format!("/{unknown}/index.html")).status,
            404
        );
        let missing = get("main", "GET", &format!("/{token}/nope.js"));
        assert_eq!(missing.status, 404);
        // Even an error keeps the page sandboxed.
        assert!(missing.csp.starts_with("sandbox allow-scripts;"));
        // A deleted creation's previews end.
        grants.revoke(&creation.id);
        assert_eq!(
            get("main", "GET", &format!("/{token}/index.html")).status,
            404
        );
    }

    #[test]
    fn the_guard_comes_first_and_keeps_the_doctype_first() {
        let page = b"\xEF\xBB\xBF  <!DOCTYPE html><html><script>x()</script></html>";
        let guarded = String::from_utf8(with_guard(page)).unwrap();
        let guard = guarded
            .find("<meta http-equiv=\"x-dns-prefetch-control\"")
            .unwrap();
        assert!(guarded.find("<!DOCTYPE html>").unwrap() < guard);
        assert!(guard < guarded.find("x()").unwrap());
        let bare = String::from_utf8(with_guard(b"<script>x()</script>")).unwrap();
        assert!(bare.starts_with(PAGE_GUARD));
        for name in [
            "RTCPeerConnection",
            "webkitRTCPeerConnection",
            "RTCDataChannel",
        ] {
            assert!(PAGE_GUARD.contains(&format!("'{name}'")), "{name}");
        }
        assert!(PAGE_GUARD.contains("configurable:false"));
    }

    #[test]
    fn every_response_turns_dns_look_ahead_off_and_stays_sandboxed() {
        let request = tauri::http::Request::builder()
            .method("GET")
            .uri(format!("{}/{}/index.html", origin(), "f".repeat(32)))
            .body(Vec::new())
            .unwrap();
        let answer = response("main", &request);
        let headers = answer.headers();
        assert_eq!(headers["X-DNS-Prefetch-Control"], "off");
        assert_eq!(headers["X-Content-Type-Options"], "nosniff");
        assert!(headers["Content-Security-Policy"]
            .to_str()
            .unwrap()
            .starts_with("sandbox allow-scripts;"));
    }

    #[test]
    fn old_previews_lapse() {
        let base = Grant {
            creation_id: "cr_0123456789abcdef0123456789abcdef".into(),
            version: Some(1),
            network: false,
            data_dir: PathBuf::from("data"),
            root: PathBuf::from("root"),
        };
        let grants = Grants::new();
        let (first, _) = grants.grant(base.clone());
        let (second, _) = grants.grant(base.clone());
        for _ in 0..MAX_GRANTS - 1 {
            grants.grant(base.clone());
        }
        assert_eq!(grants.granted(&first), None);
        assert_eq!(grants.granted(&second), Some(base));
    }

    #[test]
    fn files_get_their_media_type() {
        assert_eq!(content_type("index.html"), "text/html; charset=utf-8");
        assert_eq!(content_type("js/app.MJS"), "text/javascript; charset=utf-8");
        assert_eq!(content_type("a/b.svg"), "image/svg+xml");
        assert_eq!(content_type("Makefile"), "application/octet-stream");
        assert_eq!(content_type("x.exe"), "application/octet-stream");
    }

    /// The app's own policy lets frames load only blobs and this scheme, so a
    /// preview cannot navigate itself to another site.
    #[test]
    fn the_app_lets_frames_load_only_previews() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
        for key in ["csp", "devCsp"] {
            let frame_src = config["app"]["security"][key]["frame-src"]
                .as_str()
                .unwrap_or_else(|| panic!("{key} frame-src"));
            let mut sources: Vec<&str> = frame_src.split_whitespace().collect();
            sources.sort_unstable();
            assert_eq!(
                sources,
                ["blob:", "http://shodh-preview.localhost", "shodh-preview:"],
                "{key}"
            );
            let connect = config["app"]["security"][key]["connect-src"]
                .as_str()
                .unwrap();
            assert!(!connect.contains("shodh-preview"), "{key}: {connect}");
        }
    }
}
