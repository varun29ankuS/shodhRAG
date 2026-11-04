//! The live preview of a creation: the `shodh-preview` scheme.
//!
//! An agent-built page is untrusted code. The Artifacts tab shows it in a
//! window of its own ([`super::preview_window`]), never in the app's
//! webview, and that window's pages come only from this scheme:
//!
//! - **No Shodh commands.** The preview window's label (`preview-<id>`) is
//!   named by no capability, so Tauri's ACL refuses it every app command,
//!   every plugin command and every event, on any origin, even though its
//!   page receives the invoke key like any main frame (`acl_manifest.rs`
//!   sends every registered command from it). The app ACL keys capabilities
//!   by webview label, and the preview is a separate top-level window, so it
//!   inherits nothing from `main` (a window-keyed capability would reach
//!   every webview a window hosts).
//! - **Only in its own window.** This scheme serves a token only to the one
//!   preview window it was granted for, and refuses every other webview, the
//!   app's `main` first: the app's webview cannot load or frame agent code
//!   (its `frame-src` names no preview either).
//! - **Its own origin, opaque.** Every response carries `Content-Security-
//!   Policy: sandbox allow-scripts`, so the page runs in an opaque origin:
//!   no storage shared with anything, no forms, no popups. The window is also
//!   private (incognito), so nothing it stores outlives it.
//! - **Nowhere else.** The window may navigate only within its own token's
//!   path, opens no other window and downloads nothing
//!   ([`super::preview_window`]).
//! - **No files.** It is served only what the creation holds: by an
//!   unguessable token per preview ([`Grants::grant`]), a relative path, read
//!   from the open file and never through a link ([`super::handle`]). One
//!   preview cannot fetch another's files: only the token shown now is
//!   served, and its CSP names only its own token's path.
//! - **No loading or sending over the web unless the user allows it** for
//!   that preview: then `https:` and `wss:` are added to what it may load and
//!   connect to, never `http:`, which would reach `http://ipc.localhost` and
//!   this scheme's other paths on Windows. CSP does not govern every channel.
//!   On Windows the preview window has its own browser process whose traffic,
//!   while the switch is off, goes to a dead proxy, and WebRTC may use no
//!   direct UDP ([`super::preview_window`]). On macOS no engine setting
//!   closes WebRTC; there a page script guard ([`GUARD_SCRIPT`], defence in
//!   depth only) is all there is, and the preview's text says a hostile page
//!   may still reach the internet that way. DNS look-ahead is off.
//! - **No devices.** `Permissions-Policy` turns off the camera, microphone,
//!   location, clipboard, payment and every other powerful feature
//!   ([`PERMISSIONS_POLICY`]); on Windows the window also denies every
//!   permission request, and the guard removes media capture (macOS's
//!   webview would grant it without asking).

use std::path::PathBuf;
use std::sync::Mutex;

use super::{CreationStore, MAX_FILE_BYTES};

/// The scheme's name, registered in `lib.rs`.
pub const SCHEME: &str = "shodh-preview";

/// Every preview window's label starts with this; no capability names it.
pub const WEBVIEW_PREFIX: &str = "preview-";

/// Whether `label` is a preview window's.
pub fn is_preview_webview(label: &str) -> bool {
    label
        .strip_prefix(WEBVIEW_PREFIX)
        .is_some_and(|id| id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// The scheme and host of the scheme's pages: Windows (WebView2) serves
/// custom schemes as `http://<scheme>.localhost`, macOS and Linux as
/// `<scheme>://localhost`.
pub fn origin_parts() -> (&'static str, &'static str) {
    if cfg!(windows) {
        ("http", "shodh-preview.localhost")
    } else {
        (SCHEME, "localhost")
    }
}

/// Where the scheme's pages live ([`origin_parts`]).
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
    /// The only webview this token is served to: its preview window.
    pub webview: String,
}

/// The preview shown now, by token. One at a time: a new preview ends the
/// one before, so a token works only while its window shows it.
pub struct Grants {
    shown: Mutex<Option<(String, Grant)>>,
}

impl Grants {
    pub const fn new() -> Self {
        Self {
            shown: Mutex::new(None),
        }
    }

    /// A new token for `grant`, which replaces the preview shown before, and
    /// the URL of its first page.
    pub fn grant(&self, grant: Grant) -> (String, String) {
        let token = uuid::Uuid::new_v4().simple().to_string();
        *self.shown.lock().unwrap_or_else(|e| e.into_inner()) = Some((token.clone(), grant));
        let url = format!("{}/{token}/index.html", origin());
        (token, url)
    }

    fn granted(&self, token: &str) -> Option<Grant> {
        self.shown
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .filter(|(known, _)| known == token)
            .map(|(_, grant)| grant.clone())
    }

    /// The preview of `creation_id` ends (it was deleted). Returns the
    /// webview that showed it.
    pub fn revoke(&self, creation_id: &str) -> Option<String> {
        let mut shown = self.shown.lock().unwrap_or_else(|e| e.into_inner());
        if shown
            .as_ref()
            .is_some_and(|(_, grant)| grant.creation_id == creation_id)
        {
            return shown.take().map(|(_, grant)| grant.webview);
        }
        None
    }

    /// The preview of `webview` ends (its window closed).
    pub fn end(&self, webview: &str) {
        let mut shown = self.shown.lock().unwrap_or_else(|e| e.into_inner());
        if shown
            .as_ref()
            .is_some_and(|(_, grant)| grant.webview == webview)
        {
            *shown = None;
        }
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
/// `webview`: a token is served only to the preview window it was granted
/// for, never to the app's own webview or any other.
pub fn serve(grants: &Grants, webview: &str, method: &str, path: &str) -> Served {
    let refused = |status: u16, text: &str, csp: String| Served {
        status,
        content_type: "text/plain; charset=utf-8",
        body: text.as_bytes().to_vec(),
        csp,
    };
    let locked = csp("00000000000000000000000000000000", false);
    if !is_preview_webview(webview) {
        return refused(
            403,
            "Previews show only in their own preview window.",
            locked,
        );
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
    if grant.webview != webview {
        return refused(403, "This preview shows in another window.", locked);
    }
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

/// The script of [`PAGE_GUARD`], as one literal for both uses. It holds no
/// `<` (it is inlined in a `<script>` tag) and no `"#`.
macro_rules! guard_script {
    () => {
        r#"(function(){
var R=['RTCPeerConnection','webkitRTCPeerConnection','RTCDataChannel','RTCIceCandidate',
'RTCSessionDescription','RTCRtpSender','RTCRtpReceiver','RTCRtpTransceiver','RTCDtlsTransport',
'RTCIceTransport','RTCSctpTransport','RTCCertificate','RTCIceGatherer','RTCRtpScriptTransform',
'RTCEncodedAudioFrame','RTCEncodedVideoFrame'];
var M=['mediaDevices','getUserMedia','webkitGetUserMedia','mozGetUserMedia','getDisplayMedia'];
var D=Object.defineProperty,G=Object.getOwnPropertyDescriptor;
function off(o,n){try{D(o,n,{value:undefined,writable:false,configurable:false});}catch(e){}}
function getter(p,n,after){try{var d=G(p,n);if(!d||!d.get)return;var g=d.get;
D(p,n,{configurable:false,enumerable:d.enumerable,get:function(){var v=g.call(this);after(v);return v;}});}catch(e){}}
function setter(p,n,after){try{var d=G(p,n);if(!d||!d.set)return;var s=d.set;
D(p,n,{configurable:false,enumerable:d.enumerable,get:d.get,set:function(v){try{s.call(this,v);}finally{after();}}});}catch(e){}}
function method(p,n,after){try{var f=p[n];if(typeof f!=='function')return;
D(p,n,{configurable:false,writable:false,value:function(){try{return f.apply(this,arguments);}finally{after();}}});}catch(e){}}
function seal(w){
try{if(!w||w.__shodhGuard===true)return;D(w,'__shodhGuard',{value:true});}catch(e){return;}
R.forEach(function(n){off(w,n);});
try{M.forEach(function(n){off(w.navigator,n);});}catch(e){}
try{var NP=w.Navigator&&w.Navigator.prototype;if(NP)M.forEach(function(n){off(NP,n);});}catch(e){}
try{var MP=w.MediaDevices&&w.MediaDevices.prototype;if(MP){off(MP,'getUserMedia');off(MP,'getDisplayMedia');}}catch(e){}
function sweep(){try{var f=w.frames;for(var i=0;i!==f.length;i++){seal(f[i]);}}catch(e){}}
function child(v){try{if(v)seal(v.defaultView||v);}catch(e){}sweep();}
['HTMLIFrameElement','HTMLFrameElement','HTMLObjectElement','HTMLEmbedElement'].forEach(function(c){
var P=w[c]&&w[c].prototype;if(P){getter(P,'contentWindow',child);getter(P,'contentDocument',child);}});
var N=w.Node&&w.Node.prototype,E=w.Element&&w.Element.prototype,DP=w.Document&&w.Document.prototype,
RP=w.Range&&w.Range.prototype,SP=w.ShadowRoot&&w.ShadowRoot.prototype;
[[N,['appendChild','insertBefore','replaceChild']],
[E,['append','prepend','after','before','replaceWith','replaceChildren','insertAdjacentElement','insertAdjacentHTML','setHTMLUnsafe']],
[DP,['write','writeln','append','prepend','replaceChildren']],[RP,['insertNode','surroundContents']]].forEach(function(x){
if(x[0])x[1].forEach(function(n){method(x[0],n,sweep);});});
[E,SP].forEach(function(p){if(p){setter(p,'innerHTML',sweep);setter(p,'outerHTML',sweep);}});
try{new w.MutationObserver(sweep).observe(w.document,{childList:true,subtree:true});}catch(e){}
sweep();}
seal(window);})();"#
    };
}

/// A guard against WebRTC and media capture, the channels CSP and the
/// webview do not close by themselves. Peer connections reach STUN and TURN
/// servers outside what CSP governs, and on macOS the webview grants every
/// camera and microphone request without asking. The script makes the
/// WebRTC interfaces and `navigator.mediaDevices` / `getUserMedia`
/// undefined and unchangeable on the page's window, and on every frame
/// window the page reaches: a frame's window is guarded when it is read
/// (`contentWindow`, `contentDocument`), and after every DOM insertion and
/// on every DOM change all of `window.frames` is. The preview window also
/// runs it in every frame that loads a document
/// ([`super::preview_window`]).
///
/// This is defence in depth, not a seal: page scripts have broken JS guards
/// before. On Windows the preview window's own browser arguments are the
/// real block while the internet switch is off (its traffic goes to a dead
/// proxy and WebRTC may use no direct UDP); on macOS nothing below this
/// script closes WebRTC, and the preview says so.
pub const GUARD_SCRIPT: &str = guard_script!();

/// What every preview HTML page gets first, before its own scripts: no DNS
/// look-ahead, and [`GUARD_SCRIPT`] again, so the guard holds even where a
/// webview would skip its start-up scripts.
pub const PAGE_GUARD: &str = concat!(
    "<meta http-equiv=\"x-dns-prefetch-control\" content=\"off\"><script>",
    guard_script!(),
    "</script>"
);

/// Every powerful feature, off for every preview page. The window is
/// top-level, so it has no frame `allow` list to rely on; this header is
/// the list. Chromium honours it; WebKit only in part, which is why the
/// guard also removes media capture.
pub const PERMISSIONS_POLICY: &str = "accelerometer=(), bluetooth=(), browsing-topics=(), \
camera=(), clipboard-read=(), clipboard-write=(), compute-pressure=(), display-capture=(), \
encrypted-media=(), fullscreen=(), gamepad=(), geolocation=(), gyroscope=(), hid=(), \
identity-credentials-get=(), idle-detection=(), keyboard-map=(), local-fonts=(), \
magnetometer=(), microphone=(), midi=(), otp-credentials=(), payment=(), \
picture-in-picture=(), publickey-credentials-create=(), publickey-credentials-get=(), \
screen-wake-lock=(), serial=(), storage-access=(), usb=(), web-share=(), \
window-management=(), xr-spatial-tracking=()";

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
        .header("Permissions-Policy", PERMISSIONS_POLICY)
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

    const WINDOW: &str = "preview-0123456789abcdef0123456789abcdef";

    fn grant_for(creation_id: &str, data: &std::path::Path, root: &std::path::Path) -> Grant {
        Grant {
            creation_id: creation_id.to_string(),
            version: None,
            network: false,
            data_dir: data.to_path_buf(),
            root: root.to_path_buf(),
            webview: WINDOW.to_string(),
        }
    }

    #[test]
    fn preview_windows_have_their_own_labels() {
        assert!(is_preview_webview(WINDOW));
        for label in [
            "main",
            "print-0123456789abcdef0123456789abcdef",
            "preview-",
            "preview-short",
            "preview-0123456789abcdef0123456789abcdeg",
            "preview-0123456789abcdef0123456789abcdef0",
            "Preview-0123456789abcdef0123456789abcdef",
        ] {
            assert!(!is_preview_webview(label), "{label}");
        }
        let (scheme, host) = origin_parts();
        assert_eq!(origin(), format!("{scheme}://{host}"));
    }

    #[test]
    fn a_preview_is_served_only_to_its_own_window() {
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
        let (token, url) = grants.grant(grant_for(&creation.id, &data, &root));
        assert_eq!(url, format!("{}/{token}/index.html", origin()));
        let get = |webview: &str, method: &str, path: &str| serve(&grants, webview, method, path);
        let page = get(WINDOW, "GET", &format!("/{token}/index.html"));
        assert_eq!(page.status, 200);
        // A page gets the guard (no WebRTC, no DNS look-ahead) before its own content.
        assert_eq!(page.body, with_guard(b"<p>hi</p>"));
        assert!(page.body.starts_with(PAGE_GUARD.as_bytes()));
        assert_eq!(page.content_type, "text/html; charset=utf-8");
        assert_eq!(page.csp, csp(&token, false));
        // The app's own webview, a print window and another preview window
        // are refused, even with the right token: agent code never runs in
        // a webview that holds grants, nor in another preview's window.
        for webview in [
            "main",
            "print-1",
            "preview-ffffffffffffffffffffffffffffffff",
            "creation-preview",
        ] {
            let answer = get(webview, "GET", &format!("/{token}/index.html"));
            assert_eq!(answer.status, 403, "{webview}");
            assert_eq!(
                answer.content_type, "text/plain; charset=utf-8",
                "{webview}"
            );
            assert_ne!(answer.body, page.body, "{webview}");
            assert!(
                answer.csp.starts_with("sandbox allow-scripts;"),
                "{webview}"
            );
        }
        // Another method, an unknown token, a missing file.
        assert_eq!(
            get(WINDOW, "POST", &format!("/{token}/index.html")).status,
            405
        );
        let unknown = "f".repeat(32);
        assert_eq!(
            get(WINDOW, "GET", &format!("/{unknown}/index.html")).status,
            404
        );
        let missing = get(WINDOW, "GET", &format!("/{token}/nope.js"));
        assert_eq!(missing.status, 404);
        // Even an error keeps the page sandboxed.
        assert!(missing.csp.starts_with("sandbox allow-scripts;"));
        // A deleted creation's preview ends, and the caller learns its window.
        assert_eq!(grants.revoke("cr_other"), None);
        assert_eq!(grants.revoke(&creation.id), Some(WINDOW.to_string()));
        assert_eq!(
            get(WINDOW, "GET", &format!("/{token}/index.html")).status,
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
            assert!(GUARD_SCRIPT.contains(&format!("'{name}'")), "{name}");
        }
        assert!(PAGE_GUARD.contains("configurable:false"));
        // Frames the page makes are guarded too, and media capture is gone
        // (app/tests/artifacts.test.ts runs the script against such frames).
        for part in [
            "getter(P,'contentWindow',child)",
            "getter(P,'contentDocument',child)",
            "'appendChild'",
            "'innerHTML'",
            "MutationObserver",
            "'mediaDevices'",
            "'getUserMedia'",
            "'getDisplayMedia'",
            "seal(window)",
        ] {
            assert!(GUARD_SCRIPT.contains(part), "{part}");
        }
        // The page's guard is the window's start-up script, in a script tag.
        assert!(PAGE_GUARD.contains(&format!("<script>{GUARD_SCRIPT}</script>")));
        assert!(!GUARD_SCRIPT.contains('<'));
    }

    #[test]
    fn every_response_turns_dns_look_ahead_and_devices_off_and_stays_sandboxed() {
        for webview in [WINDOW, "main"] {
            let request = tauri::http::Request::builder()
                .method("GET")
                .uri(format!("{}/{}/index.html", origin(), "f".repeat(32)))
                .body(Vec::new())
                .unwrap();
            let answer = response(webview, &request);
            let headers = answer.headers();
            assert_eq!(headers["X-DNS-Prefetch-Control"], "off");
            assert_eq!(headers["X-Content-Type-Options"], "nosniff");
            assert_eq!(headers["Permissions-Policy"], PERMISSIONS_POLICY);
            assert!(headers["Content-Security-Policy"]
                .to_str()
                .unwrap()
                .starts_with("sandbox allow-scripts;"));
        }
        // Every feature the old frame's `allow` list turned off, and full screen.
        let features: Vec<&str> = PERMISSIONS_POLICY.split(", ").collect();
        for feature in [
            "camera",
            "microphone",
            "geolocation",
            "clipboard-read",
            "clipboard-write",
            "display-capture",
            "usb",
            "serial",
            "bluetooth",
            "payment",
            "fullscreen",
            "publickey-credentials-create",
            "local-fonts",
            "screen-wake-lock",
            "web-share",
            "storage-access",
            "picture-in-picture",
            "gamepad",
            "accelerometer",
            "gyroscope",
            "magnetometer",
            "encrypted-media",
            "otp-credentials",
            "identity-credentials-get",
            "compute-pressure",
            "browsing-topics",
            "keyboard-map",
            "xr-spatial-tracking",
        ] {
            let off = format!("{feature}=()");
            assert!(features.contains(&off.as_str()), "{feature}");
        }
        assert!(
            features.iter().all(|f| f.ends_with("=()")),
            "{PERMISSIONS_POLICY}"
        );
    }

    #[test]
    fn a_new_preview_ends_the_one_before() {
        let data = PathBuf::from("data");
        let root = PathBuf::from("root");
        let base = grant_for("cr_0123456789abcdef0123456789abcdef", &data, &root);
        let grants = Grants::new();
        let (first, _) = grants.grant(base.clone());
        assert_eq!(grants.granted(&first), Some(base.clone()));
        let next = Grant {
            webview: "preview-ffffffffffffffffffffffffffffffff".into(),
            ..base
        };
        let (second, _) = grants.grant(next.clone());
        assert_eq!(grants.granted(&first), None);
        assert_eq!(grants.granted(&second), Some(next));
        // Closing the earlier window does not end the later one.
        grants.end(WINDOW);
        assert!(grants.granted(&second).is_some());
        grants.end("preview-ffffffffffffffffffffffffffffffff");
        assert_eq!(grants.granted(&second), None);
    }

    #[test]
    fn files_get_their_media_type() {
        assert_eq!(content_type("index.html"), "text/html; charset=utf-8");
        assert_eq!(content_type("js/app.MJS"), "text/javascript; charset=utf-8");
        assert_eq!(content_type("a/b.svg"), "image/svg+xml");
        assert_eq!(content_type("Makefile"), "application/octet-stream");
        assert_eq!(content_type("x.exe"), "application/octet-stream");
    }

    /// The app's own webview can frame no preview: agent code shows only in
    /// a preview window, which holds no grants. (It may still frame blobs,
    /// for the PDF viewer.)
    #[test]
    fn the_app_frames_no_preview() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
        for key in ["csp", "devCsp"] {
            let policy = &config["app"]["security"][key];
            let frame_src = policy["frame-src"]
                .as_str()
                .unwrap_or_else(|| panic!("{key} frame-src"));
            let sources: Vec<&str> = frame_src.split_whitespace().collect();
            assert_eq!(sources, ["blob:"], "{key}");
            let object = policy.as_object().expect("a policy object");
            for (directive, value) in object {
                let value = value.as_str().unwrap_or_default();
                assert!(
                    !value.contains("shodh-preview"),
                    "{key} {directive}: {value}"
                );
            }
        }
    }
}
