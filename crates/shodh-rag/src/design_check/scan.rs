//! The rules' detectors over a prepared [`Source`]. Each returns hits by byte
//! offset; [`super::check_source`] turns them into findings with lines.
//!
//! The detectors of rules ported from pbakaus/impeccable follow its
//! source-text matchers (`crates/detect/src/regex_matchers.rs` at
//! [`super::rules::IMPECCABLE_COMMIT`], Apache-2.0): the same tokens,
//! thresholds and exemptions, re-implemented here over Shodh's own parsing.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::LazyLock;

use regex::Regex;

use super::color::{self, contrast, Rgba};
use super::source::{attr, bound, has_spread, Decl, Source, Tag, Token, DYNAMIC};

/// A rule matched `start..end` of the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub rule: &'static str,
    pub start: usize,
    pub end: usize,
    /// What was matched, or what the rule counted.
    pub evidence: String,
    /// A mechanical fix: `start..end` of the file becomes the text.
    pub fix: Option<(usize, usize, String)>,
}

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("design check pattern")
}

macro_rules! pattern {
    ($name:ident, $re:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| re($re));
    };
}

pattern!(TW_SIDE, r"^border-([lrse])-(\d+)$");
pattern!(TW_TOP_BOTTOM, r"^border-([tb])-(\d+)$");
pattern!(TW_ROUNDED, r"^rounded(?:-[a-z0-9\[\].]+)*$");
pattern!(
    TW_GRAY_TEXT,
    r"^text-(?:gray|slate|zinc|neutral|stone)-(\d+)$"
);
pattern!(
    TW_CHROMATIC_BG,
    r"^bg-(?:red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose)-(\d+)$"
);
pattern!(TW_PURPLE_TEXT, r"^text-(?:purple|violet|indigo)-(\d+)$");
pattern!(TW_FROM_PURPLE, r"^from-(?:purple|violet|indigo)-(\d+)$");
pattern!(
    TW_TO_TELL,
    r"^to-(?:purple|violet|indigo|blue|cyan|pink|fuchsia)-\d+$"
);
pattern!(TW_BIG_TEXT, r"^text-(?:[2-9]xl)$");
pattern!(TW_DISPLAY_TEXT, r"^text-(?:[5-9]xl)$");
pattern!(
    TW_COLORED_SHADOW,
    r"^shadow-(?:red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose)-\d+(?:/\d+)?$"
);
pattern!(TW_ARBITRARY_SIZE, r"^text-\[(\d*\.?\d+)(px|rem)\]$");
pattern!(TW_LEADING, r"^leading-(?:none|tight|\[(\d*\.?\d+)\])$");
pattern!(TW_TRACKING, r"^tracking-\[(-?\d*\.?\d+)em\]$");
pattern!(TW_SMALL_SIZE, r"^(?:w|h|size)-(?:0\.5|1|1\.5|2|2\.5|3)$");
pattern!(
    TW_MARQUEE,
    r"^animate-(?:marquee|ticker|scroll|scroll-x|infinite-scroll)$"
);
pattern!(
    TW_FONT_ARBITRARY,
    r"^font-\[['_]?([A-Za-z][A-Za-z_ ]*?)['_]?(?:,.*)?\]$"
);
pattern!(
    TW_SPACING,
    r"^(?:p|px|py|pt|pb|pl|pr|m|mx|my|mt|mb|ml|mr|gap)-(\d+)$"
);
pattern!(
    CSS_SPACING_PX,
    r"(?i)(?:padding|margin)(?:-(?:top|right|bottom|left))?\s*:\s*(\d+)px"
);
pattern!(
    CSS_SPACING_REM,
    r"(?i)(?:padding|margin)(?:-(?:top|right|bottom|left))?\s*:\s*([0-9.]+)rem"
);
pattern!(CSS_GAP, r"(?i)gap\s*:\s*(\d+)px");
pattern!(LENGTH, r"^(-?\d*\.?\d+)(px|rem|em|%)?$");
pattern!(BORDER_SOLID, r"(?i)^(\d+(?:\.\d+)?)px\s+solid\b(.*)$");
pattern!(
    CUBIC_BEZIER,
    r"cubic-bezier\(\s*(-?[0-9.]+)\s*,\s*(-?[0-9.]+)\s*,\s*(-?[0-9.]+)\s*,\s*(-?[0-9.]+)\s*\)"
);
pattern!(MOTION_WORD, r"(?i)bounce|elastic|wobble|jiggle|spring");
pattern!(MARQUEE_WORD, r"(?i)marquee|ticker");
pattern!(
    GOOGLE_FONTS,
    r#"(?i)fonts\.googleapis\.com/css2?\?[^"'\s)<>]*"#
);
pattern!(
    LOREM,
    r"(?i)\blorem ipsum\b|\bdolor sit amet\b|\bconsectetur adipiscing\b"
);
pattern!(
    FILLER,
    r"(?i)\b(?:your (?:text|title|heading|content|description|tagline) (?:goes )?here|insert (?:text|content|image) here|sample text|placeholder text|text goes here)\b"
);
pattern!(
    NOT_A,
    r"\bNot an? [a-z][^.!?]{1,40}[.!]\s+[A-Z][^.!?]{1,60}[.!]"
);
pattern!(
    SHORT_REBUTTAL,
    r"\b[A-Z][^.!?]{4,80}[.!]\s+(?:No|Just)\s+[a-z][^.!?]{2,60}[.!]"
);
pattern!(
    BODY_SELECTOR,
    r"(?i)(?:^|[\s,>+~(])(?:p|li|body|article|blockquote|td|dd|main)\b|\.prose\b"
);
pattern!(
    HEADING_SELECTOR,
    r"(?i)\bh[1-6]\b|heading|title|display|hero"
);
pattern!(
    INTERACTIVE_SELECTOR,
    r"(?i)(?:^|[\s,>+~(])(?:button|a|input|select|textarea|summary)\b|\[tabindex|\[role"
);

/// The fonts impeccable calls overused (its `font-family` matcher's list).
const OVERUSED_FONTS: [&str; 17] = [
    "Inter",
    "Roboto",
    "Open Sans",
    "Lato",
    "Montserrat",
    "Arial",
    "Helvetica",
    "Fraunces",
    "Geist Sans",
    "Geist Mono",
    "Geist",
    "Mona Sans",
    "Plus Jakarta Sans",
    "Space Grotesk",
    "Recoleta",
    "Instrument Sans",
    "Instrument Serif",
];

/// impeccable's buzzword phrases, word for word.
const BUZZWORDS: [&str; 29] = [
    "streamline your",
    "empower your",
    "supercharge your",
    "unleash your",
    "unleash the power",
    "leverage the power",
    "built for the modern",
    "trusted by leading",
    "trusted by the world",
    "best-in-class",
    "industry-leading",
    "world-class",
    "enterprise-grade",
    "next-generation",
    "cutting-edge",
    "transform your business",
    "revolutionize",
    "game-changer",
    "game changing",
    "mission-critical",
    "best of breed",
    "future-proof",
    "future proof",
    "seamless experience",
    "seamlessly integrate",
    "drive engagement",
    "drive growth",
    "drive results",
    "harness the power",
];

/// impeccable's em-dash floor and density (`EM_DASH_FLOOR`, `EM_DASH_CHARS_PER_DASH`).
const EM_DASH_FLOOR: usize = 8;
const EM_DASH_CHARS_PER_DASH: usize = 500;
/// Tracking tighter than this loses letterforms (impeccable's `CRUSHED_TRACKING_EM`).
const CRUSHED_TRACKING_EM: f64 = -0.07;
const CRUSHED_TRACKING_EM_DISPLAY: f64 = -0.09;
const DISPLAY_FONT_SIZE_PX: f64 = 40.0;
/// The user's floor for any text, in pixels.
pub const MIN_TEXT_PX: f64 = 12.0;
/// Distinct radii or type sizes one file may use before it reads as unsystematic.
const MAX_RADII: usize = 4;
const MAX_TYPE_SIZES: usize = 7;

/// Elements a focus outline matters on.
const INTERACTIVE_TAGS: [&str; 6] = ["button", "a", "input", "select", "textarea", "summary"];
/// Elements whose running text the body-text rules read.
const BODY_TAGS: [&str; 8] = [
    "p",
    "li",
    "blockquote",
    "td",
    "dd",
    "article",
    "main",
    "section",
];

struct Out {
    hits: Vec<Hit>,
}

impl Out {
    fn hit(&mut self, rule: &'static str, start: usize, end: usize, evidence: impl Into<String>) {
        self.hits.push(Hit {
            rule,
            start,
            end,
            evidence: evidence.into(),
            fix: None,
        });
    }

    fn fix(
        &mut self,
        rule: &'static str,
        start: usize,
        end: usize,
        evidence: impl Into<String>,
        new: impl Into<String>,
    ) {
        self.hits.push(Hit {
            rule,
            start,
            end,
            evidence: evidence.into(),
            fix: Some((start, end, new.into())),
        });
    }
}

/// Every rule's hits in `src`, in no particular order.
pub fn run(src: &Source) -> Vec<Hit> {
    let mut out = Out { hits: Vec::new() };
    if src.kind.has_markup() {
        images(src, &mut out);
        form_labels(src, &mut out);
        icon_buttons(src, &mut out);
        clickable_divs(src, &mut out);
        headings(src, &mut out);
        element_text(src, &mut out);
        copy_rules(src, &mut out);
        tag_class_rules(src, &mut out);
        centered(src, &mut out);
    }
    focus(src, &mut out);
    declarations(src, &mut out);
    contrast_rule(src, &mut out);
    token_rules(src, &mut out);
    fonts(src, &mut out);
    easing(src, &mut out);
    spacing(src, &mut out);
    scales(src, &mut out);
    placeholder_copy(src, &mut out);
    out.hits
}

fn tag_text<'s>(src: &'s Source, tag: &Tag) -> &'s str {
    &src.text[tag.start..tag.end]
}

fn tokens_of<'s>(src: &'s Source, tag: &Tag) -> Vec<&'s Token> {
    src.tokens_in(tag.start, tag.end).collect()
}

/// The tokens a match at `offset` is read with: its tag's, else its line's.
fn scope_tokens<'s>(src: &'s Source, offset: usize) -> Vec<&'s Token> {
    let (text, start) = src.scope_at(offset);
    src.tokens_in(start, start + text.len()).collect()
}

fn shade(caps: &regex::Captures, i: usize) -> u32 {
    caps.get(i)
        .and_then(|m| m.as_str().parse().ok())
        .unwrap_or(0)
}

fn snippet(text: &str) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > 120 {
        format!("{}…", flat.chars().take(119).collect::<String>())
    } else {
        flat
    }
}

/// A length in pixels: `px`, `rem` (16px), or a bare number in a JSX style.
fn px_of(value: &str, jsx: bool) -> Option<f64> {
    let caps = LENGTH.captures(value.trim())?;
    let n: f64 = caps[1].parse().ok()?;
    match caps.get(2).map(|m| m.as_str()) {
        Some("px") => Some(n),
        Some("rem") => Some(n * 16.0),
        None if jsx => Some(n),
        _ => None,
    }
}

fn is_rounded(tokens: &[&Token]) -> bool {
    tokens
        .iter()
        .any(|t| TW_ROUNDED.is_match(&t.base) && t.base != "rounded-none")
}

fn scope_px(src: &Source, offset: usize, prop: &str) -> Option<f64> {
    let (text, start) = src.scope_at(offset);
    src.decls_in(start, start + text.len())
        .filter(|d| d.prop == prop)
        .filter_map(|d| px_of(&d.value, d.jsx))
        .last()
}

// ─── Markup rules ────────────────────────────────────────────────────────────

const PLACEHOLDER_SOURCES: [&str; 8] = [
    "placehold.co",
    "placeholder.com",
    "placehold.it",
    "placekitten.com",
    "picsum.photos",
    "dummyimage.com",
    "fakeimg.pl",
    "placeholder.",
];

fn images(src: &Source, out: &mut Out) {
    for tag in src.tags.iter().filter(|t| !t.closing) {
        let text = tag_text(src, tag);
        if has_spread(text) {
            continue;
        }
        if (tag.name == "img" || tag.name == "Image") && bound(text, "alt").is_none() {
            out.hit("img-missing-alt", tag.start, tag.end, snippet(text));
        }
        if tag.name != "img" {
            continue;
        }
        let dynamic = bound(text, "srcset").is_some()
            || attr(text, ":src").is_some()
            || attr(text, "v-bind:src").is_some()
            || text.contains("{src}");
        match attr(text, "src") {
            None if !dynamic => out.hit("broken-image", tag.start, tag.end, snippet(text)),
            Some(value) if value != DYNAMIC => {
                let value = value.trim().to_ascii_lowercase();
                if value.is_empty()
                    || value == "#"
                    || PLACEHOLDER_SOURCES.iter().any(|p| value.contains(p))
                {
                    out.hit("broken-image", tag.start, tag.end, snippet(text));
                }
            }
            _ => {}
        }
    }
}

fn form_labels(src: &Source, out: &mut Out) {
    static FOR_RE: LazyLock<Regex> =
        LazyLock::new(|| re(r#"(?i)\s(?:for|htmlfor)\s*=\s*["']([^"']+)["']"#));
    let labelled: HashSet<String> = FOR_RE
        .captures_iter(&src.text)
        .map(|c| c[1].to_string())
        .collect();
    let lower = src.text.to_ascii_lowercase();
    let dynamic_for =
        lower.contains("htmlfor={") || lower.contains(":for=") || lower.contains(" for={");
    let mut in_label = 0usize;
    for tag in &src.tags {
        if tag.name == "label" {
            if tag.closing {
                in_label = in_label.saturating_sub(1);
            } else if !tag.self_closing {
                in_label += 1;
            }
            continue;
        }
        if tag.closing || !matches!(tag.name.as_str(), "input" | "select" | "textarea") {
            continue;
        }
        let text = tag_text(src, tag);
        if has_spread(text) || in_label > 0 {
            continue;
        }
        let kind = bound(text, "type").unwrap_or("text").to_ascii_lowercase();
        if matches!(
            kind.as_str(),
            "hidden" | "submit" | "button" | "reset" | "image"
        ) {
            continue;
        }
        let named = ["aria-label", "aria-labelledby", "title"]
            .iter()
            .any(|name| bound(text, name).is_some_and(|v| !v.trim().is_empty()));
        let by_label = match bound(text, "id") {
            Some(DYNAMIC) => dynamic_for,
            Some(id) => labelled.contains(id),
            None => false,
        };
        if !named && !by_label {
            out.hit("control-missing-label", tag.start, tag.end, snippet(text));
        }
    }
}

/// Whether `c` is an emoji (pictographs, symbols, dingbats, flags), not a
/// typographic mark such as a check or an arrow.
pub fn is_emoji(c: char) -> bool {
    let n = c as u32;
    matches!(n, 0x1F300..=0x1FAFF | 0x1F1E6..=0x1F1FF | 0x2600..=0x26FF | 0x2700..=0x27BF)
        && !matches!(n, 0x2713 | 0x2714 | 0x2717 | 0x2718 | 0x2756..=0x2767)
}

fn emoji_free(text: &str) -> String {
    text.chars()
        .filter(|c| !is_emoji(*c) && !matches!(*c as u32, 0xFE0F | 0x200D))
        .collect()
}

fn icon_buttons(src: &Source, out: &mut Out) {
    for (i, tag) in src.tags.iter().enumerate() {
        if tag.closing || tag.self_closing || tag.name != "button" {
            continue;
        }
        let text = tag_text(src, tag);
        if has_spread(text)
            || ["aria-label", "aria-labelledby", "title"]
                .iter()
                .any(|name| bound(text, name).is_some())
        {
            continue;
        }
        let Some(close) = src.tags[i + 1..]
            .iter()
            .find(|t| t.closing && t.name == "button")
        else {
            continue;
        };
        let inner: Vec<&Tag> = src.tags[i + 1..]
            .iter()
            .take_while(|t| t.start < close.start)
            .collect();
        // The words between the inner tags, and an image's alt text, name the button.
        let mut words = String::new();
        let mut at = tag.end;
        let mut named_by_image = false;
        for t in &inner {
            words.push_str(&src.text[at..t.start]);
            at = t.end;
            let inner_text = tag_text(src, t);
            if (t.name == "img" || t.name == "Image")
                && bound(inner_text, "alt").is_some_and(|a| !a.trim().is_empty())
            {
                named_by_image = true;
            }
        }
        words.push_str(&src.text[at..close.start]);
        if words.contains('{') || named_by_image {
            continue;
        }
        let has_icon = !inner.is_empty() || words.chars().any(is_emoji);
        if has_icon && emoji_free(&words).trim().is_empty() {
            out.hit(
                "icon-button-missing-label",
                tag.start,
                tag.end,
                snippet(text),
            );
        }
    }
}

fn clickable_divs(src: &Source, out: &mut Out) {
    for tag in src.tags.iter().filter(|t| !t.closing) {
        if tag.name != "div" && tag.name != "span" {
            continue;
        }
        let text = tag_text(src, tag);
        let clicks = ["onclick", "@click", "v-on:click", "on:click"]
            .iter()
            .any(|name| attr(text, name).is_some());
        if !clicks || has_spread(text) {
            continue;
        }
        if attr(text, "role").is_none() || bound(text, "tabindex").is_none() {
            out.hit("non-semantic-button", tag.start, tag.end, snippet(text));
        }
    }
}

fn headings(src: &Source, out: &mut Out) {
    let mut previous: Option<u32> = None;
    for tag in src.tags.iter().filter(|t| !t.closing) {
        let level = match tag.name.as_str() {
            "h1" => 1,
            "h2" => 2,
            "h3" => 3,
            "h4" => 4,
            "h5" => 5,
            "h6" => 6,
            _ => continue,
        };
        if let Some(prev) = previous {
            if level > prev + 1 {
                out.hit(
                    "skipped-heading",
                    tag.start,
                    tag.end,
                    format!("h{prev} then h{level}"),
                );
            }
        }
        previous = Some(level);
    }
}

/// Emoji used as icons, and filler copy, in element text.
fn element_text(src: &Source, out: &mut Out) {
    for run in &src.runs {
        let lead = run.text.len() - run.text.trim_start().len();
        let text = run.text.trim();
        let start = run.start + lead;
        let mut chars = text.chars();
        if let Some(first) = chars.next() {
            if is_emoji(first) {
                let rest: String = chars
                    .skip_while(|c| is_emoji(*c) || matches!(*c as u32, 0xFE0F | 0x200D))
                    .collect();
                let labels = rest.starts_with(char::is_whitespace)
                    && rest.trim_start().starts_with(char::is_alphabetic);
                if rest.trim().is_empty() || labels {
                    out.hit("emoji-icon", start, start + first.len_utf8(), snippet(text));
                }
            }
        }
        for m in FILLER.find_iter(&run.text) {
            out.hit(
                "placeholder-text",
                run.start + m.start(),
                run.start + m.end(),
                m.as_str(),
            );
        }
    }
}

/// Lorem ipsum anywhere in the file (strings and props too), once per line.
fn placeholder_copy(src: &Source, out: &mut Out) {
    let mut lines = BTreeSet::new();
    for m in LOREM.find_iter(&src.text) {
        if lines.insert(src.line_of(m.start())) {
            out.hit("placeholder-text", m.start(), m.end(), m.as_str());
        }
    }
}

/// The element text of the file joined into one string, and where each run
/// starts in it (with its offset in the file).
fn joined_text(src: &Source) -> (String, Vec<(usize, usize)>) {
    let mut text = String::new();
    let mut starts = Vec::new();
    for run in &src.runs {
        starts.push((text.len(), run.start));
        let flat = run.text.replace("&mdash;", "\u{2014}");
        text.push_str(&flat);
        text.push(' ');
    }
    (text, starts)
}

/// The file offset of byte `at` of the joined text.
fn file_offset(starts: &[(usize, usize)], at: usize) -> usize {
    let i = starts.partition_point(|(joined, _)| *joined <= at);
    match i.checked_sub(1).and_then(|i| starts.get(i)) {
        Some((joined, file)) => file + (at - joined),
        None => 0,
    }
}

fn copy_rules(src: &Source, out: &mut Out) {
    let (text, starts) = joined_text(src);
    if text.trim().is_empty() {
        return;
    }
    let lower = text.to_ascii_lowercase();
    let mut buzz: Vec<(usize, &str)> = Vec::new();
    for phrase in BUZZWORDS {
        buzz.extend(lower.match_indices(phrase));
    }
    buzz.sort_unstable();
    if let Some((at, phrase)) = buzz.first() {
        let start = file_offset(&starts, *at);
        out.hit(
            "marketing-buzzword",
            start,
            start + phrase.len(),
            format!(
                "{} buzzword phrase{}: \"{phrase}\"",
                buzz.len(),
                if buzz.len() == 1 { "" } else { "s" }
            ),
        );
    }

    let mut cadence: Vec<regex::Match> = NOT_A.find_iter(&text).collect();
    cadence.extend(SHORT_REBUTTAL.find_iter(&text));
    cadence.sort_by_key(|m| m.start());
    if cadence.len() >= 3 {
        let first = cadence[0];
        let start = file_offset(&starts, first.start());
        out.hit(
            "aphoristic-cadence",
            start,
            start + first.len(),
            format!(
                "{} aphoristic constructions: \"{}\"",
                cadence.len(),
                snippet(first.as_str())
            ),
        );
    }

    // A run of more than two hyphens is a rule, not a dash.
    let mut dashes: Vec<usize> = text.match_indices('\u{2014}').map(|(at, _)| at).collect();
    let bytes = text.as_bytes();
    for (at, _) in text.match_indices("--") {
        let before = at.checked_sub(1).map(|i| bytes[i]);
        let after = bytes.get(at + 2).copied();
        if before != Some(b'-')
            && after != Some(b'-')
            && before == Some(b' ')
            && after == Some(b' ')
        {
            dashes.push(at);
        }
    }
    dashes.sort_unstable();
    let chars = text.chars().count();
    if dashes.len() >= EM_DASH_FLOOR && chars <= dashes.len() * EM_DASH_CHARS_PER_DASH {
        let start = file_offset(&starts, dashes[0]);
        out.hit(
            "em-dash-overuse",
            start,
            start + 1,
            format!("{} em-dashes in body text", dashes.len()),
        );
    }
}

/// Rules read from one element's classes and its own text.
fn tag_class_rules(src: &Source, out: &mut Out) {
    for tag in src.tags.iter().filter(|t| !t.closing) {
        let tokens = tokens_of(src, tag);
        if tokens.is_empty() {
            continue;
        }
        let plain = |t: &&&Token| t.variants.is_empty();
        let own_text = src
            .text_after(tag.end)
            .map(|r| r.text.trim().chars().count())
            .unwrap_or(0);
        let heading = matches!(tag.name.as_str(), "h1" | "h2" | "h3" | "h4" | "h5" | "h6");
        let big = tokens.iter().any(|t| TW_BIG_TEXT.is_match(&t.base));

        // Gray text on a coloured fill (shade 700 and darker is near-black ink).
        let fill = tokens.iter().filter(plain).find(|t| {
            TW_CHROMATIC_BG
                .captures(&t.base)
                .is_some_and(|c| shade(&c, 1) >= 400)
        });
        if let Some(fill) = fill {
            for gray in tokens.iter().filter(plain) {
                if TW_GRAY_TEXT
                    .captures(&gray.base)
                    .is_some_and(|c| shade(&c, 1) < 700)
                {
                    out.hit(
                        "gray-on-color",
                        gray.start,
                        gray.end,
                        format!("{} on {}", gray.base, fill.base),
                    );
                }
            }
        }

        let upper = tokens.iter().any(|t| t.base == "uppercase");
        if upper
            && own_text >= 80
            && !matches!(tag.name.as_str(), "button" | "a" | "nav" | "label" | "th")
        {
            let t = tokens
                .iter()
                .find(|t| t.base == "uppercase")
                .expect("found");
            out.hit(
                "all-caps-body",
                t.start,
                t.end,
                format!("uppercase on {own_text} characters of text"),
            );
        }

        for t in tokens.iter().filter(plain) {
            let wide = t.base == "tracking-widest"
                || TW_TRACKING
                    .captures(&t.base)
                    .and_then(|c| c[1].parse::<f64>().ok())
                    .is_some_and(|em| em > 0.05);
            if wide && !upper && own_text >= 40 {
                out.hit("wide-tracking", t.start, t.end, t.base.clone());
            }
            if let Some(c) = TW_LEADING.captures(&t.base) {
                let tight = c
                    .get(1)
                    .map(|m| m.as_str().parse::<f64>().unwrap_or(1.0) < 1.3)
                    .unwrap_or(true);
                let body = BODY_TAGS.contains(&tag.name.as_str()) || own_text >= 120;
                if tight && body && !heading && !big {
                    out.fix(
                        "tight-leading",
                        t.start,
                        t.end,
                        t.base.clone(),
                        "leading-normal",
                    );
                }
            }
        }

        // A small round dot that pulses.
        let pulses = tokens
            .iter()
            .any(|t| t.base == "animate-ping" || t.base == "animate-pulse");
        if pulses
            && tokens.iter().any(|t| t.base == "rounded-full")
            && tokens.iter().any(|t| TW_SMALL_SIZE.is_match(&t.base))
        {
            let t = tokens
                .iter()
                .find(|t| t.base == "animate-ping" || t.base == "animate-pulse")
                .expect("found");
            out.hit("pulsing-dot", t.start, t.end, t.base.clone());
        }
    }
    for tag in src
        .tags
        .iter()
        .filter(|t| !t.closing && t.name == "marquee")
    {
        out.hit("marquee", tag.start, tag.end, "<marquee>");
    }
}

fn centered(src: &Source, out: &mut Out) {
    let mut first: Option<usize> = None;
    let mut center = 0usize;
    let mut left = 0usize;
    for t in &src.tokens {
        match t.base.as_str() {
            "text-center" => {
                center += 1;
                first.get_or_insert(t.start);
            }
            "text-left" | "text-start" | "text-justify" => left += 1,
            _ => {}
        }
    }
    for d in src.decls.iter().filter(|d| d.prop == "text-align") {
        match d.value.to_ascii_lowercase().as_str() {
            "center" => {
                center += 1;
                first.get_or_insert(d.start);
            }
            "left" | "start" | "justify" => left += 1,
            _ => {}
        }
    }
    if let Some(at) = first {
        if center >= 6 && left == 0 && src.runs.len() >= 6 {
            out.hit(
                "centered-everything",
                at,
                at + 1,
                format!("{center} centred blocks, none aligned left"),
            );
        }
    }
}

// ─── Rules on CSS declarations ───────────────────────────────────────────────

fn focus(src: &Source, out: &mut Out) {
    // Tailwind: `outline-none` on a control with no focus style of its own.
    for t in src
        .tokens
        .iter()
        .filter(|t| t.base == "outline-none" || t.base == "outline-0")
    {
        let Some(tag) = src.tag_at(t.start) else {
            continue;
        };
        let text = tag_text(src, tag);
        let control = INTERACTIVE_TAGS.contains(&tag.name.as_str())
            || bound(text, "tabindex").is_some()
            || attr(text, "role").is_some();
        if !control {
            continue;
        }
        let replaced = tokens_of(src, tag).iter().any(|o| {
            o.variants.contains("focus-visible:")
                || o.variants.contains("focus-within:")
                || (o.variants.contains("focus:")
                    && ["ring", "border", "shadow", "bg-", "underline", "outline-"]
                        .iter()
                        .any(|p| o.base.starts_with(p))
                    && !o.base.starts_with("outline-none")
                    && o.base != "outline-0")
        });
        if !replaced {
            out.fix(
                "missing-focus-style",
                t.start,
                t.end,
                format!("{}{}", t.variants, t.base),
                format!(
                    "{} focus-visible:ring-2 focus-visible:ring-offset-2",
                    t.base
                ),
            );
        }
    }
    // CSS: `outline: none` with no :focus-visible style anywhere in the file.
    if src.text.contains(":focus-visible") {
        return;
    }
    for d in &src.decls {
        let off = match d.prop.as_str() {
            "outline" | "outline-style" => matches!(d.value.as_str(), "none" | "0" | "0px"),
            "outline-width" => matches!(d.value.as_str(), "0" | "0px"),
            _ => false,
        };
        if !off {
            continue;
        }
        let selector = src.selector_at(d.start);
        let control = match src.tag_at(d.start) {
            Some(tag) => INTERACTIVE_TAGS.contains(&tag.name.as_str()),
            None => selector.contains(":focus") || INTERACTIVE_SELECTOR.is_match(selector),
        };
        let (scope, start) = src.scope_at(d.start);
        let replaced = src
            .decls_in(start, start + scope.len())
            .any(|o| o.prop == "box-shadow" && o.value != "none");
        if control && !replaced {
            out.hit(
                "missing-focus-style",
                d.start,
                d.value_end,
                format!("{}: {}", d.prop, d.value),
            );
        }
    }
}

fn number_em(value: &str) -> Option<f64> {
    value.trim().strip_suffix("em")?.parse().ok()
}

fn declarations(src: &Source, out: &mut Out) {
    for d in &src.decls {
        let selector = src.selector_at(d.start);
        let tag = src.tag_at(d.start);
        let in_tag_named = |names: &[&str]| tag.is_some_and(|t| names.contains(&t.name.as_str()));
        let body = BODY_SELECTOR.is_match(selector) || in_tag_named(&BODY_TAGS[..]);
        let heading = HEADING_SELECTOR.is_match(selector)
            || in_tag_named(&["h1", "h2", "h3", "h4", "h5", "h6"][..]);
        let value = d.value.as_str();
        let lower = value.to_ascii_lowercase();
        match d.prop.as_str() {
            "font-size" => {
                let Some(px) = px_of(value, d.jsx) else {
                    continue;
                };
                let small_print = selector.contains("sup")
                    || selector.contains("sub")
                    || in_tag_named(&["sup", "sub"][..]);
                if px > 0.0 && px < MIN_TEXT_PX && !small_print {
                    let new = if lower.ends_with("rem") {
                        "0.75rem"
                    } else if lower.ends_with("px") {
                        "12px"
                    } else {
                        "12"
                    };
                    out.fix(
                        "tiny-text",
                        d.value_start,
                        d.value_end,
                        format!("font-size: {value}"),
                        new,
                    );
                }
            }
            "text-align" if lower == "justify" => {
                let (scope, _) = src.scope_at(d.start);
                if !scope.contains("hyphens") {
                    out.fix(
                        "justified-text",
                        d.value_start,
                        d.value_end,
                        format!("text-align: {value}"),
                        "left",
                    );
                }
            }
            "line-height" => {
                let ratio = if let Some(pct) = lower.strip_suffix('%') {
                    pct.parse::<f64>().ok().map(|p| p / 100.0)
                } else {
                    lower.parse::<f64>().ok()
                };
                let large = scope_px(src, d.start, "font-size").is_some_and(|px| px >= 24.0);
                if ratio.is_some_and(|r| r > 0.0 && r < 1.3) && body && !heading && !large {
                    out.fix(
                        "tight-leading",
                        d.value_start,
                        d.value_end,
                        format!("line-height: {value}"),
                        "1.5",
                    );
                }
            }
            "letter-spacing" => {
                let Some(em) = number_em(&lower) else {
                    continue;
                };
                let size = scope_px(src, d.start, "font-size").unwrap_or(16.0);
                let limit = if size >= DISPLAY_FONT_SIZE_PX {
                    CRUSHED_TRACKING_EM_DISPLAY
                } else {
                    CRUSHED_TRACKING_EM
                };
                if em < limit {
                    out.fix(
                        "extreme-negative-tracking",
                        d.value_start,
                        d.value_end,
                        format!("letter-spacing: {value}"),
                        "-0.02em",
                    );
                } else if em > 0.05 && body {
                    let (scope, _) = src.scope_at(d.start);
                    if !scope.to_ascii_lowercase().contains("uppercase") {
                        out.hit(
                            "wide-tracking",
                            d.start,
                            d.value_end,
                            format!("letter-spacing: {value}"),
                        );
                    }
                }
            }
            "background-clip" | "-webkit-background-clip" if lower == "text" => {
                let (scope, _) = src.scope_at(d.start);
                if scope.to_ascii_lowercase().contains("gradient") {
                    out.hit(
                        "gradient-text",
                        d.start,
                        d.value_end,
                        "background-clip: text + gradient",
                    );
                }
            }
            "background" | "background-image" => background(d, &lower, out),
            "box-shadow" | "text-shadow" => glow(d, out),
            "animation" | "animation-name" => {
                if MARQUEE_WORD.is_match(value)
                    && (lower.contains("infinite") || d.prop == "animation-name")
                {
                    out.hit(
                        "marquee",
                        d.start,
                        d.value_end,
                        format!("{}: {value}", d.prop),
                    );
                }
            }
            prop if prop.starts_with("border-") => borders(src, d, out),
            _ => {}
        }
    }
}

fn background(d: &Decl, lower: &str, out: &mut Out) {
    if lower.contains("repeating-linear-gradient(")
        || lower.contains("repeating-radial-gradient(")
        || lower.contains("repeating-conic-gradient(")
    {
        out.hit(
            "repeating-stripes-gradient",
            d.start,
            d.value_end,
            snippet(&d.value),
        );
    }
    if !lower.contains("gradient(") {
        return;
    }
    let colors = color::colors_in(&d.value);
    if colors.iter().any(Rgba::is_purple) {
        out.hit(
            "ai-color-palette",
            d.start,
            d.value_end,
            format!("{}: {}", d.prop, snippet(&d.value)),
        );
    }
    if lower.contains("radial-gradient(")
        && colors.iter().any(Rgba::is_chromatic)
        && colors.iter().any(|c| c.a < 0.1)
    {
        out.hit("radial-halo", d.start, d.value_end, snippet(&d.value));
    }
}

/// Split at commas outside parentheses.
fn top_level_commas(value: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut from = 0;
    for (i, c) in value.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(&value[from..i]);
                from = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&value[from..]);
    parts
}

/// A zero-offset, blurred, chromatic shadow: a glow.
fn glow(d: &Decl, out: &mut Out) {
    for shadow in top_level_commas(&d.value) {
        let lengths: Vec<f64> = shadow
            .split_whitespace()
            .filter(|w| !w.contains('('))
            .filter_map(|w| px_of(w, false).or_else(|| (w == "0").then_some(0.0)))
            .collect();
        let colors = color::colors_in(shadow);
        if lengths.len() >= 3
            && lengths[0] == 0.0
            && lengths[1] == 0.0
            && lengths[2] >= 8.0
            && colors.iter().any(Rgba::is_chromatic)
        {
            out.hit(
                "dark-glow",
                d.start,
                d.value_end,
                format!("{}: {}", d.prop, snippet(shadow)),
            );
            return;
        }
    }
}

fn borders(src: &Source, d: &Decl, out: &mut Out) {
    let lower = d.value.to_ascii_lowercase();
    let selector = src.selector_at(d.start);
    let safe_element = src.tag_at(d.start).is_some_and(|t| {
        matches!(
            t.name.as_str(),
            "blockquote" | "nav" | "pre" | "code" | "a" | "input" | "span"
        )
    }) || ["blockquote", "nav", "pre", "code"].iter().any(|e| {
        selector
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|w| w == *e)
    });
    let (scope, start) = src.scope_at(d.start);
    let radius = src
        .decls_in(start, start + scope.len())
        .any(|o| o.prop == "border-radius" && !matches!(o.value.as_str(), "0" | "0px" | "none"));
    let width = |v: &str| -> Option<(f64, String)> {
        let caps = BORDER_SOLID.captures(v)?;
        Some((caps[1].parse().ok()?, caps[2].to_string()))
    };
    match d.prop.as_str() {
        "border-left" | "border-right" | "border-inline-start" | "border-inline-end" => {
            let Some((n, rest)) = width(&lower) else {
                return;
            };
            let neutral = color::colors_in(&rest)
                .first()
                .is_some_and(Rgba::is_neutral);
            let threshold = if radius { 2.0 } else { 3.0 };
            if !safe_element && !neutral && n >= threshold {
                out.hit(
                    "side-tab",
                    d.start,
                    d.value_end,
                    format!("{}: {}", d.prop, d.value),
                );
            }
        }
        "border-left-width"
        | "border-right-width"
        | "border-inline-start-width"
        | "border-inline-end-width" => {
            if let Some(px) = px_of(&lower, false) {
                if !safe_element && px >= 3.0 {
                    out.hit(
                        "side-tab",
                        d.start,
                        d.value_end,
                        format!("{}: {}", d.prop, d.value),
                    );
                }
            }
        }
        "border-top" | "border-bottom" => {
            if let Some((n, _)) = width(&lower) {
                if n >= 3.0 && radius {
                    out.hit(
                        "border-accent-on-rounded",
                        d.start,
                        d.value_end,
                        format!("{}: {}", d.prop, d.value),
                    );
                }
            }
        }
        _ => {}
    }
}

/// Text and background colours declared together, below WCAG AA.
fn contrast_rule(src: &Source, out: &mut Out) {
    let mut scopes: BTreeMap<usize, Vec<&Decl>> = BTreeMap::new();
    for d in &src.decls {
        if matches!(
            d.prop.as_str(),
            "color" | "background" | "background-color" | "font-size" | "font-weight"
        ) {
            scopes.entry(src.scope_at(d.start).1).or_default().push(d);
        }
    }
    let single = |value: &str| -> Option<Rgba> {
        let lower = value.to_ascii_lowercase();
        if lower.contains("var(") || lower.contains("gradient") || lower.contains("url(") {
            return None;
        }
        let colors = color::colors_in(value);
        (colors.len() == 1).then(|| colors[0])
    };
    for decls in scopes.values() {
        let fg = decls.iter().rev().find(|d| d.prop == "color");
        let bg = decls
            .iter()
            .rev()
            .find(|d| d.prop == "background" || d.prop == "background-color");
        let (Some(fg), Some(bg)) = (fg, bg) else {
            continue;
        };
        let (Some(text), Some(ground)) = (single(&fg.value), single(&bg.value)) else {
            continue;
        };
        if ground.a < 0.99 || text.a < 0.1 {
            continue;
        }
        let size = decls
            .iter()
            .rev()
            .filter(|d| d.prop == "font-size")
            .find_map(|d| px_of(&d.value, d.jsx))
            .unwrap_or(16.0);
        let bold = decls.iter().any(|d| {
            d.prop == "font-weight"
                && (d.value == "bold" || d.value.parse::<u32>().is_ok_and(|w| w >= 700))
        });
        let large = size >= 24.0 || (size >= 18.66 && bold);
        let needed = if large { 3.0 } else { 4.5 };
        let ratio = contrast(&text.over(&ground), &ground);
        if ratio < needed {
            out.hit(
                "low-contrast",
                fg.start,
                fg.value_end,
                format!(
                    "{} on {} is {:.1}:1 (needs {needed}:1)",
                    fg.value, bg.value, ratio
                ),
            );
        }
    }
}

// ─── Rules on class tokens ───────────────────────────────────────────────────

fn token_rules(src: &Source, out: &mut Out) {
    for t in &src.tokens {
        let base = t.base.as_str();
        if let Some(c) = TW_SIDE.captures(base) {
            let n = shade(&c, 2);
            let rounded = is_rounded(&scope_tokens(src, t.start));
            if n >= if rounded { 2 } else { 4 } {
                out.hit("side-tab", t.start, t.end, base);
            }
            continue;
        }
        if let Some(c) = TW_TOP_BOTTOM.captures(base) {
            let scope = scope_tokens(src, t.start);
            if shade(&c, 2) >= 1
                && is_rounded(&scope)
                && !scope.iter().any(|o| o.base == "animate-spin")
            {
                out.hit("border-accent-on-rounded", t.start, t.end, base);
            }
            continue;
        }
        match base {
            "bg-clip-text" => {
                let gradient = scope_tokens(src, t.start).iter().any(|o| {
                    ["bg-gradient-to-", "bg-linear-", "bg-radial", "bg-conic"]
                        .iter()
                        .any(|p| o.base.starts_with(p))
                });
                if gradient {
                    out.hit(
                        "gradient-text",
                        t.start,
                        t.end,
                        "bg-clip-text + bg-gradient",
                    );
                }
            }
            "text-justify" if t.variants.is_empty() => {
                if !scope_tokens(src, t.start)
                    .iter()
                    .any(|o| o.base == "hyphens-auto")
                {
                    out.fix("justified-text", t.start, t.end, base, "text-left");
                }
            }
            "animate-bounce" => {
                out.hit("bounce-easing", t.start, t.end, "animate-bounce (Tailwind)")
            }
            _ => {}
        }
        if TW_PURPLE_TEXT.is_match(base) {
            let heading = src
                .tag_at(t.start)
                .is_some_and(|tag| matches!(tag.name.as_str(), "h1" | "h2" | "h3"));
            if heading
                || scope_tokens(src, t.start)
                    .iter()
                    .any(|o| TW_BIG_TEXT.is_match(&o.base))
            {
                out.hit(
                    "ai-color-palette",
                    t.start,
                    t.end,
                    format!("{base} on heading"),
                );
            }
        }
        if TW_FROM_PURPLE.is_match(base)
            && scope_tokens(src, t.start)
                .iter()
                .any(|o| TW_TO_TELL.is_match(&o.base))
        {
            out.hit(
                "ai-color-palette",
                t.start,
                t.end,
                format!("{base} gradient"),
            );
        }
        if TW_COLORED_SHADOW.is_match(base) {
            out.hit("dark-glow", t.start, t.end, base);
        }
        if TW_MARQUEE.is_match(base) {
            out.hit("marquee", t.start, t.end, base);
        }
        if let Some(c) = TW_ARBITRARY_SIZE.captures(base) {
            let n: f64 = c[1].parse().unwrap_or(16.0);
            let px = if &c[2] == "rem" { n * 16.0 } else { n };
            if px > 0.0 && px < MIN_TEXT_PX {
                out.fix("tiny-text", t.start, t.end, base, "text-xs");
            }
        }
        if let Some(c) = TW_TRACKING.captures(base) {
            let em: f64 = c[1].parse().unwrap_or(0.0);
            let display = scope_tokens(src, t.start)
                .iter()
                .any(|o| TW_DISPLAY_TEXT.is_match(&o.base));
            let limit = if display {
                CRUSHED_TRACKING_EM_DISPLAY
            } else {
                CRUSHED_TRACKING_EM
            };
            if em < limit {
                out.fix(
                    "extreme-negative-tracking",
                    t.start,
                    t.end,
                    base,
                    "tracking-tight",
                );
            }
        }
    }
}

fn fonts(src: &Source, out: &mut Out) {
    let overused = |family: &str| -> Option<&'static str> {
        let name = family.trim().trim_matches(|c| c == '"' || c == '\'').trim();
        OVERUSED_FONTS
            .iter()
            .find(|f| f.eq_ignore_ascii_case(name))
            .copied()
    };
    for d in src.decls.iter().filter(|d| d.prop == "font-family") {
        let first = d.value.split(',').next().unwrap_or("");
        if let Some(font) = overused(first) {
            out.hit(
                "overused-font",
                d.start,
                d.value_end,
                format!("font-family: {font}"),
            );
        }
    }
    for t in &src.tokens {
        if let Some(c) = TW_FONT_ARBITRARY.captures(&t.base) {
            if let Some(font) = overused(&c[1].replace('_', " ")) {
                out.hit(
                    "overused-font",
                    t.start,
                    t.end,
                    format!("{}: {font}", t.base),
                );
            }
        }
    }
    for m in GOOGLE_FONTS.find_iter(&src.text) {
        let url = m.as_str();
        let found = url
            .split(['?', '&'])
            .filter_map(|p| p.strip_prefix("family="))
            .find_map(|f| {
                let name = f.split(':').next().unwrap_or("").replace('+', " ");
                overused(&name)
            });
        if let Some(font) = found {
            out.hit(
                "overused-font",
                m.start(),
                m.end(),
                format!("Google Fonts: {font}"),
            );
        }
    }
}

fn easing(src: &Source, out: &mut Out) {
    for d in &src.decls {
        if matches!(
            d.prop.as_str(),
            "animation"
                | "animation-name"
                | "animation-timing-function"
                | "transition-timing-function"
        ) {
            if let Some(word) = MOTION_WORD.find(&d.value) {
                out.hit(
                    "bounce-easing",
                    d.start,
                    d.value_end,
                    format!("{}: {}", d.prop, word.as_str()),
                );
            }
        }
    }
    for c in CUBIC_BEZIER.captures_iter(&src.text) {
        let y1: f64 = c[2].parse().unwrap_or(0.0);
        let y2: f64 = c[4].parse().unwrap_or(0.0);
        if !(-0.1..=1.1).contains(&y1) || !(-0.1..=1.1).contains(&y2) {
            let whole = c.get(0).expect("match");
            out.fix(
                "bounce-easing",
                whole.start(),
                whole.end(),
                whole.as_str(),
                "cubic-bezier(0.22, 1, 0.36, 1)",
            );
        }
    }
}

/// impeccable's monotonous-spacing analyzer: at least ten spacing values,
/// more than 60% of them one value (rounded to 4px), three or fewer values
/// in all, and the common one not zero.
fn spacing(src: &Source, out: &mut Out) {
    let mut values: Vec<(f64, usize)> = Vec::new();
    for c in CSS_SPACING_PX.captures_iter(&src.text) {
        let v: f64 = c[1].parse().unwrap_or(0.0);
        if v > 0.0 && v < 200.0 {
            values.push((v, c.get(0).expect("match").start()));
        }
    }
    for c in CSS_SPACING_REM.captures_iter(&src.text) {
        let v = (c[1].parse::<f64>().unwrap_or(0.0) * 16.0).round();
        if v > 0.0 && v < 200.0 {
            values.push((v, c.get(0).expect("match").start()));
        }
    }
    for c in CSS_GAP.captures_iter(&src.text) {
        values.push((
            c[1].parse().unwrap_or(0.0),
            c.get(0).expect("match").start(),
        ));
    }
    for t in &src.tokens {
        if let Some(c) = TW_SPACING.captures(&t.base) {
            values.push((c[1].parse::<f64>().unwrap_or(0.0) * 4.0, t.start));
        }
    }
    if values.len() < 10 {
        return;
    }
    let rounded: Vec<(i64, usize)> = values
        .iter()
        .map(|(v, at)| (((v / 4.0).round() * 4.0) as i64, *at))
        .collect();
    let mut counts: BTreeMap<i64, usize> = BTreeMap::new();
    for (v, _) in &rounded {
        *counts.entry(*v).or_default() += 1;
    }
    // Most common first; among equals, the smallest value.
    let (dominant, max) = counts
        .iter()
        .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
        .map(|(v, n)| (*v, *n))
        .expect("values");
    let distinct = counts.keys().filter(|v| **v > 0).count();
    let share = max as f64 / rounded.len() as f64;
    if share <= 0.6 || distinct > 3 || dominant == 0 {
        return;
    }
    let at = rounded
        .iter()
        .filter(|(v, _)| *v == dominant)
        .map(|(_, at)| *at)
        .min()
        .unwrap_or(0);
    out.hit(
        "monotonous-spacing",
        at,
        at + 1,
        format!(
            "~{dominant}px used {max}/{} times ({}%)",
            rounded.len(),
            (share * 100.0).round()
        ),
    );
}

/// Tailwind's radius scale in pixels.
fn tw_radius(base: &str) -> Option<i64> {
    let rest = base.strip_prefix("rounded")?;
    if !(rest.is_empty() || rest.starts_with('-')) {
        return None;
    }
    let size = rest
        .rsplit_once('-')
        .map(|(_, s)| s)
        .filter(|s| {
            !matches!(
                *s,
                "t" | "b"
                    | "l"
                    | "r"
                    | "s"
                    | "e"
                    | "tl"
                    | "tr"
                    | "bl"
                    | "br"
                    | "ss"
                    | "se"
                    | "es"
                    | "ee"
            )
        })
        .unwrap_or("");
    match size {
        "" => Some(4),
        "xs" => Some(2),
        "sm" => Some(2),
        "md" => Some(6),
        "lg" => Some(8),
        "xl" => Some(12),
        "2xl" => Some(16),
        "3xl" => Some(24),
        "4xl" => Some(32),
        "none" | "full" => None,
        other => other
            .strip_prefix('[')
            .and_then(|s| s.strip_suffix("px]"))
            .and_then(|s| s.parse::<f64>().ok())
            .map(|v| v.round() as i64),
    }
}

/// Tailwind's type scale in pixels.
fn tw_text_size(base: &str) -> Option<i64> {
    Some(match base {
        "text-xs" => 12,
        "text-sm" => 14,
        "text-base" => 16,
        "text-lg" => 18,
        "text-xl" => 20,
        "text-2xl" => 24,
        "text-3xl" => 30,
        "text-4xl" => 36,
        "text-5xl" => 48,
        "text-6xl" => 60,
        "text-7xl" => 72,
        "text-8xl" => 96,
        "text-9xl" => 128,
        _ => {
            let c = TW_ARBITRARY_SIZE.captures(base)?;
            let n: f64 = c[1].parse().ok()?;
            (if &c[2] == "rem" { n * 16.0 } else { n }).round() as i64
        }
    })
}

/// Too many distinct radii or type sizes in one file.
fn scales(src: &Source, out: &mut Out) {
    let mut radii: Vec<(i64, usize)> = Vec::new();
    let mut sizes: Vec<(i64, usize)> = Vec::new();
    for d in &src.decls {
        match d.prop.as_str() {
            "border-radius" => {
                if let Some(px) = px_of(&d.value, d.jsx) {
                    if px > 0.0 && px < 999.0 {
                        radii.push((px.round() as i64, d.start));
                    }
                }
            }
            "font-size" => {
                if let Some(px) = px_of(&d.value, d.jsx) {
                    sizes.push((px.round() as i64, d.start));
                }
            }
            _ => {}
        }
    }
    for t in &src.tokens {
        if let Some(r) = tw_radius(&t.base) {
            radii.push((r, t.start));
        }
        if let Some(s) = tw_text_size(&t.base) {
            sizes.push((s, t.start));
        }
    }
    for (list, limit, what) in [
        (&mut radii, MAX_RADII, "corner radii"),
        (&mut sizes, MAX_TYPE_SIZES, "font sizes"),
    ] {
        list.sort_by_key(|(_, at)| *at);
        let mut seen: Vec<i64> = Vec::new();
        let mut over: Option<usize> = None;
        for (value, at) in list.iter() {
            if !seen.contains(value) {
                seen.push(*value);
                if seen.len() == limit + 1 {
                    over = Some(*at);
                }
            }
        }
        if let Some(at) = over {
            seen.sort_unstable();
            let shown: Vec<String> = seen.iter().map(|v| format!("{v}px")).collect();
            out.hit(
                "inconsistent-scale",
                at,
                at + 1,
                format!("{} {what}: {}", seen.len(), shown.join(", ")),
            );
        }
    }
}
