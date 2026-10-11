//! Turning fetched pages into text the model can read.

use std::sync::LazyLock;

use regex::Regex;

/// Line width for rendered text; long enough that prose is not re-wrapped
/// mid-sentence in ways that hurt quoting.
const RENDER_WIDTH: usize = 400;

static TITLE: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"(?is)<title[^>]*>(.*?)</title>").ok());

static META_CHARSET: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r#"(?i)<meta[^>]+charset\s*=\s*["']?([A-Za-z0-9_\-]+)"#).ok());

/// Decode a body using the charset from `content_type`, else from a
/// `<meta charset>` in the first bytes, else UTF-8 (invalid bytes replaced).
pub fn decode_body(bytes: &[u8], content_type: Option<&str>) -> String {
    let from_header = content_type.and_then(|ct| {
        ct.split(';')
            .map(str::trim)
            .find_map(|p| p.strip_prefix("charset="))
            .map(|c| c.trim_matches('"').to_string())
    });
    let from_meta = || {
        let head = String::from_utf8_lossy(&bytes[..bytes.len().min(2048)]).into_owned();
        META_CHARSET
            .as_ref()
            .and_then(|re| re.captures(&head))
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
    };
    let label = from_header.or_else(from_meta);
    let encoding = label
        .as_deref()
        .and_then(|l| encoding_rs::Encoding::for_label(l.as_bytes()))
        .unwrap_or(encoding_rs::UTF_8);
    let (text, _, _) = encoding.decode(bytes);
    text.into_owned()
}

/// Collapse runs of blank lines and trailing spaces.
fn tidy(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank = 0;
    for line in text.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim().to_string()
}

/// The page title, if the page has one.
pub fn html_title(html: &str) -> Option<String> {
    let raw = TITLE.as_ref()?.captures(html)?.get(1)?.as_str();
    let text = html2text::from_read(raw.as_bytes(), RENDER_WIDTH).ok()?;
    let title = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (!title.is_empty()).then_some(title)
}

/// Readable text of an HTML page (scripts, styles and markup removed;
/// links kept as footnotes by the renderer).
pub fn html_to_text(html: &str) -> String {
    match html2text::from_read(html.as_bytes(), RENDER_WIDTH) {
        Ok(text) => tidy(&text),
        Err(e) => {
            tracing::debug!(target: "shodh::harness", error = %e, "HTML rendering failed; using raw text");
            tidy(html)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_and_styles_do_not_reach_the_text() {
        let html = r#"<html><head><title> Notice &amp; periods </title>
            <style>body { color: red }</style>
            <script>window.steal = document.cookie;</script></head>
            <body><h1>Leases</h1><p>The notice period is <b>60 days</b>.</p>


            <p>Second paragraph.</p></body></html>"#;
        assert_eq!(html_title(html).as_deref(), Some("Notice & periods"));
        let text = html_to_text(html);
        assert!(text.contains("The notice period is"), "{text}");
        assert!(text.contains("60 days"));
        assert!(!text.contains("document.cookie"));
        assert!(!text.contains("color: red"));
        assert!(!text.contains("\n\n\n"));
    }

    #[test]
    fn bodies_decode_with_the_declared_charset() {
        let latin1 = b"caf\xe9";
        assert_eq!(
            decode_body(latin1, Some("text/html; charset=ISO-8859-1")),
            "café"
        );
        let meta = b"<meta charset=\"windows-1252\"><p>\x93quoted\x94</p>";
        assert!(decode_body(meta, Some("text/html")).contains("\u{201c}quoted\u{201d}"));
        assert_eq!(decode_body("héllo".as_bytes(), None), "héllo");
        assert_eq!(html_title("<p>no title</p>"), None);
    }
}
