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
// `text-[10px]`, `text-[length:0.6rem]`, `text-[0.65em]`, with an optional
// line-height modifier (`/4`, `/[14px]`) and Tailwind 4's trailing `!`.
pattern!(
    TW_ARBITRARY_SIZE,
    r"^text-\[(?:length:)?(\d*\.?\d+)(px|rem|em)\](/[^\s!]+)?(!?)$"
);
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
    r"(?i)\b(?:your (?:text|title|heading|content|description|tagline|name|company) (?:goes )?here|insert (?:text|content|image) here|text goes here)\b"
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
pub fn run(src: &Source, context: &super::Context) -> Vec<Hit> {
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
    if src.kind.has_markup() {
        tailwind_contrast(src, context, &mut out);
    }
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

/// Pixels of one length a font size can be written with: `px`, `rem`, `em`
/// and `%` (both against the 16px root that impeccable assumes for `rem`), a
/// bare number in a JSX style, or a keyword size (`xx-small` 9px, `x-small`
/// 10px, `small` 13px). `None` for what cannot be known from the text
/// (`var()`, `vw`, `smaller`).
fn size_px(token: &str, jsx: bool) -> Option<f64> {
    let t = token.trim().to_ascii_lowercase();
    match t.as_str() {
        "xx-small" => return Some(9.0),
        "x-small" => return Some(10.0),
        "small" => return Some(13.0),
        _ => {}
    }
    let caps = LENGTH.captures(&t)?;
    let n: f64 = caps[1].parse().ok()?;
    match caps.get(2).map(|m| m.as_str()) {
        Some("px") => Some(n),
        Some("rem") | Some("em") => Some(n * 16.0),
        Some("%") => Some(n * 16.0 / 100.0),
        None if jsx => Some(n),
        _ => None,
    }
}

/// The smallest size `value` (a `font-size` value) can render at, in
/// pixels, when the text says: a length or keyword (`!important` dropped),
/// the lower bound of `clamp()`, the smallest known argument of `min()`, the
/// largest of `max()` when every argument is known. `calc()` and anything
/// else unknown give `None` (not flagged).
pub fn font_px(value: &str, jsx: bool) -> Option<f64> {
    let v = value.trim();
    let v = v
        .strip_suffix("!important")
        .or_else(|| v.strip_suffix("! important"))
        .unwrap_or(v)
        .trim();
    let lower = v.to_ascii_lowercase();
    let args = |prefix: &str| -> Option<Vec<Option<f64>>> {
        let inner = lower.strip_prefix(prefix)?.strip_suffix(')')?;
        Some(
            top_level_commas(inner)
                .into_iter()
                .map(|a| size_px(a, false))
                .collect(),
        )
    };
    if let Some(parts) = args("clamp(") {
        return parts.first().copied().flatten();
    }
    if let Some(parts) = args("min(") {
        return parts.into_iter().flatten().reduce(f64::min);
    }
    if let Some(parts) = args("max(") {
        let known: Option<Vec<f64>> = parts.into_iter().collect();
        return known?.into_iter().reduce(f64::max);
    }
    size_px(v, jsx)
}

/// The size of a `font` shorthand (`600 10px/1.2 system-ui`): the first word
/// before the family that is a size, a `/line-height` cut off.
fn shorthand_size(value: &str) -> Option<(f64, String)> {
    for word in value.split_whitespace() {
        let size = word.split('/').next().unwrap_or(word);
        // A weight (`600`) has no unit and is not a size: read on.
        if let Some(px) = size_px(size, false) {
            return Some((px, size.to_string()));
        }
    }
    None
}

/// What a small font size is raised to: the same unit at 12px.
fn raised(size: &str) -> Option<&'static str> {
    let s = size.trim().to_ascii_lowercase();
    if s.ends_with("rem") {
        Some("0.75rem")
    } else if s.ends_with("em") {
        Some("0.75em")
    } else if s.ends_with("px") {
        Some("12px")
    } else if s.ends_with('%') {
        Some("75%")
    } else if matches!(s.as_str(), "xx-small" | "x-small") {
        Some("12px")
    } else {
        None
    }
}

/// Whether a CSS selector names the `sup` or `sub` element (as a whole
/// element name, never a class such as `.subtitle`).
fn names_sup_or_sub(selector: &str) -> bool {
    selector
        .split([' ', ',', '>', '+', '~', '\n', '\t'])
        .filter(|compound| !compound.is_empty())
        .any(|compound| {
            let name: String = compound
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                .collect();
            name.eq_ignore_ascii_case("sup") || name.eq_ignore_ascii_case("sub")
        })
}

/// Elements whose side border is a convention, never a card's accent
/// (impeccable's `SAFE_ELEMENT_RE`).
const SIDE_TAB_SAFE_TAGS: [&str; 7] = ["blockquote", "nav", "pre", "code", "a", "input", "span"];
/// A corner rounder than this reads as a rounded card (impeccable's
/// `SIDE_ACCENT_MIN_RADIUS_PX`).
const SIDE_ACCENT_MIN_RADIUS_PX: f64 = 4.0;
/// The box width a percentage radius is read against (impeccable's
/// `NOMINAL_CARD_WIDTH_PX`).
const NOMINAL_CARD_WIDTH_PX: f64 = 1000.0;

/// A box's corner radii, `[top-left, top-right, bottom-right, bottom-left]`;
/// `None` where the text does not say (a `var()`, a theme token).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Corners([Option<f64>; 4]);

impl Corners {
    fn square() -> Self {
        Corners([Some(0.0); 4])
    }

    fn unknown() -> Self {
        Corners([None; 4])
    }

    fn set(&mut self, corners: &[usize], value: Option<f64>, raise: bool) {
        for &i in corners {
            self.0[i] = match (raise, self.0[i], value) {
                (true, Some(a), Some(b)) => Some(a.max(b)),
                (true, _, _) => None,
                (false, _, v) => v,
            };
        }
    }

    /// Whether both corners away from `side` (0 top, 1 right, 2 bottom,
    /// 3 left) round the box. An unknown corner keeps the finding, as in
    /// impeccable.
    fn rounded_away_from(&self, side: usize) -> bool {
        let away = match side {
            0 => [2, 3],
            1 => [0, 3],
            2 => [0, 1],
            _ => [1, 2],
        };
        away.iter()
            .all(|&i| self.0[i].is_none_or(|r| r >= SIDE_ACCENT_MIN_RADIUS_PX))
    }
}

/// Whether the tag's classes are all written out (a literal `class` or
/// `className`), so a radius missing from them is truly missing.
fn class_is_literal(tag: &str) -> bool {
    match bound(tag, "classname").or_else(|| bound(tag, "class")) {
        Some(DYNAMIC) => false,
        Some(_) => !has_spread(tag),
        None => false,
    }
}

/// The corners Tailwind's `rounded-*` classes give (a class behind a
/// variant, `md:rounded-lg`, can round the box, so it only raises them).
fn tailwind_corners(tokens: &[&Token]) -> Corners {
    let mut corners = Corners::square();
    for t in tokens {
        let Some(rest) = t.base.strip_prefix("rounded") else {
            continue;
        };
        if !(rest.is_empty() || rest.starts_with('-')) {
            continue;
        }
        let mut parts = rest.trim_start_matches('-').splitn(2, '-');
        let first = parts.next().unwrap_or("");
        let (which, size): (&[usize], &str) = match first {
            "t" => (&[0, 1], parts.next().unwrap_or("")),
            "r" | "e" => (&[1, 2], parts.next().unwrap_or("")),
            "b" => (&[2, 3], parts.next().unwrap_or("")),
            "l" | "s" => (&[0, 3], parts.next().unwrap_or("")),
            "tl" | "ss" => (&[0], parts.next().unwrap_or("")),
            "tr" | "se" => (&[1], parts.next().unwrap_or("")),
            "br" | "ee" => (&[2], parts.next().unwrap_or("")),
            "bl" | "es" => (&[3], parts.next().unwrap_or("")),
            _ => (&[0, 1, 2, 3], rest.trim_start_matches('-')),
        };
        let px = match size {
            "" => Some(4.0),
            "none" => Some(0.0),
            "xs" | "sm" => Some(2.0),
            "md" => Some(6.0),
            "lg" => Some(8.0),
            "xl" => Some(12.0),
            "2xl" => Some(16.0),
            "3xl" => Some(24.0),
            "4xl" => Some(32.0),
            "full" => Some(9999.0),
            other => other
                .strip_prefix('[')
                .and_then(|s| s.strip_suffix(']'))
                .and_then(radius_px),
        };
        corners.set(which, px, !t.variants.is_empty());
    }
    corners
}

/// One radius value in pixels (`%` against [`NOMINAL_CARD_WIDTH_PX`]).
fn radius_px(value: &str) -> Option<f64> {
    let v = value.trim();
    if v == "0" {
        return Some(0.0);
    }
    if let Some(pct) = v.strip_suffix('%') {
        return pct
            .parse::<f64>()
            .ok()
            .map(|p| p / 100.0 * NOMINAL_CARD_WIDTH_PX);
    }
    px_of(v, false)
}

/// The corners a CSS block or a tag's style declares: `None` overall when it
/// declares no radius (the initial square box).
fn declared_corners(src: &Source, start: usize, end: usize) -> Option<Corners> {
    let mut corners = Corners::square();
    apply_corners(src, start, end, &mut corners).then_some(corners)
}

/// The corners of the CSS rule around `offset` with every rule of the file
/// that names one of its selectors merged in, in source order (a later
/// declaration wins, as in the cascade): `.card { border-left: … }` and a
/// later `.card { border-radius: … }` are one box. Rules of other files are
/// not seen. `None` when the offset is in no rule (an inline style), or when
/// none of those rules declares a radius.
fn merged_rule_corners(src: &Source, offset: usize) -> Option<Option<Corners>> {
    let rule = src
        .css_rules
        .iter()
        .filter(|r| r.start <= offset && offset < r.end)
        .min_by_key(|r| r.end - r.start)?;
    let mut corners = Corners::square();
    let mut declared = false;
    for other in src
        .css_rules
        .iter()
        .filter(|o| o.selectors.iter().any(|s| rule.selectors.contains(s)))
    {
        declared |= apply_corners(src, other.start, other.end, &mut corners);
    }
    Some(declared.then_some(corners))
}

/// Apply the radius declarations of `start..end` to `corners`; whether any
/// was found.
fn apply_corners(src: &Source, start: usize, end: usize, corners: &mut Corners) -> bool {
    let mut declared = false;
    for d in src.decls_in(start, end) {
        let one = |v: &str| {
            if d.jsx {
                px_of(v, true)
            } else {
                radius_px(v)
            }
        };
        match d.prop.as_str() {
            "border-radius" => {
                declared = true;
                let shape = d.value.split('/').next().unwrap_or("");
                let values: Vec<Option<f64>> = shape.split_whitespace().map(one).collect();
                let at = |i: usize| values.get(i).copied().flatten();
                let all = match values.len() {
                    1 => [at(0), at(0), at(0), at(0)],
                    2 => [at(0), at(1), at(0), at(1)],
                    3 => [at(0), at(1), at(2), at(1)],
                    4 => [at(0), at(1), at(2), at(3)],
                    _ => [None; 4],
                };
                *corners = Corners(all);
            }
            "border-top-left-radius" => {
                declared = true;
                corners.set(&[0], one(&d.value), false);
            }
            "border-top-right-radius" => {
                declared = true;
                corners.set(&[1], one(&d.value), false);
            }
            "border-bottom-right-radius" => {
                declared = true;
                corners.set(&[2], one(&d.value), false);
            }
            "border-bottom-left-radius" => {
                declared = true;
                corners.set(&[3], one(&d.value), false);
            }
            _ => {}
        }
    }
    declared
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
        .filter_map(|d| font_px(&d.value, d.jsx))
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
        // A component named `Image` is an image only with a source (next/image);
        // without one it is an icon (lucide-react's `<Image />`).
        let image = tag.name == "img"
            || (tag.name == "Image"
                && (bound(text, "src").is_some() || bound(text, "srcset").is_some()));
        if image && bound(text, "alt").is_none() {
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

/// Whether `c` is drawn as an emoji by default (Unicode's
/// `Emoji_Presentation` property): pictographs, flags, ⭐ ⏰ ⏳ ✅ ❌ 🆕 and the
/// like. Typographic glyphs used deliberately as icons (✕ ★ ☆ ☰ ❯ ♥, checks,
/// the dingbat arrows) are text by default and are not emoji, unless an
/// emoji variation selector follows them ([`is_emoji_at`]).
pub fn is_emoji(c: char) -> bool {
    matches!(
        c as u32,
        0x231A..=0x231B
            | 0x23E9..=0x23EC
            | 0x23F0
            | 0x23F3
            | 0x25FD..=0x25FE
            | 0x2614..=0x2615
            | 0x2648..=0x2653
            | 0x267F
            | 0x2693
            | 0x26A1
            | 0x26AA..=0x26AB
            | 0x26BD..=0x26BE
            | 0x26C4..=0x26C5
            | 0x26CE
            | 0x26D4
            | 0x26EA
            | 0x26F2..=0x26F3
            | 0x26F5
            | 0x26FA
            | 0x26FD
            | 0x2705
            | 0x270A..=0x270B
            | 0x2728
            | 0x274C
            | 0x274E
            | 0x2753..=0x2755
            | 0x2757
            | 0x2795..=0x2797
            | 0x27B0
            | 0x27BF
            | 0x2B1B..=0x2B1C
            | 0x2B50
            | 0x2B55
            | 0x1F004
            | 0x1F0CF
            | 0x1F18E
            | 0x1F191..=0x1F19A
            | 0x1F1E6..=0x1F1FF
            | 0x1F201
            | 0x1F21A
            | 0x1F22F
            | 0x1F232..=0x1F23A
            | 0x1F250..=0x1F251
            | 0x1F300..=0x1F64F
            | 0x1F680..=0x1F6FF
            | 0x1F7E0..=0x1F7EB
            | 0x1F90C..=0x1F9FF
            | 0x1FA70..=0x1FAFF
    )
}

/// Whether the character `c` followed by `next` shows as an emoji: emoji by
/// default, or any symbol asked to show as one (U+FE0F after it: ▶️ ⬆️ ❤️).
pub fn is_emoji_at(c: char, next: Option<char>) -> bool {
    is_emoji(c) || (next == Some('\u{FE0F}') && !c.is_ascii_alphanumeric())
}

/// Joiners, variation selectors, skin tones and keycap marks inside an emoji.
fn is_emoji_part(c: char) -> bool {
    matches!(c as u32, 0xFE0F | 0x200D | 0x20E3 | 0x1F3FB..=0x1F3FF | 0xE0020..=0xE007F)
}

fn has_emoji(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    (0..chars.len()).any(|i| is_emoji_at(chars[i], chars.get(i + 1).copied()))
}

fn emoji_free(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    (0..chars.len())
        .filter(|&i| !is_emoji_at(chars[i], chars.get(i + 1).copied()) && !is_emoji_part(chars[i]))
        .map(|i| chars[i])
        .collect()
}

/// Elements that are never children: they open no scope.
const VOID_TAGS: [&str; 14] = [
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track",
    "wbr", "param",
];

/// For each text run (by index), the tags open around it, outermost first
/// (indexes into `src.tags`).
fn run_ancestors(src: &Source) -> Vec<Vec<usize>> {
    let mut stack: Vec<usize> = Vec::new();
    let mut out = Vec::with_capacity(src.runs.len());
    let mut next_tag = 0;
    for run in &src.runs {
        while next_tag < src.tags.len() && src.tags[next_tag].start < run.start {
            let tag = &src.tags[next_tag];
            if tag.closing {
                if let Some(at) = stack.iter().rposition(|i| src.tags[*i].name == tag.name) {
                    stack.truncate(at);
                }
            } else if !tag.self_closing && !VOID_TAGS.contains(&tag.name.as_str()) {
                stack.push(next_tag);
            }
            next_tag += 1;
        }
        out.push(stack.clone());
    }
    out
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
        let has_icon = !inner.is_empty() || has_emoji(&words);
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
        let handler = ["onclick", "@click", "v-on:click", "on:click"]
            .iter()
            .find_map(|name| attr_expression(text, name));
        let Some(handler) = handler else {
            continue;
        };
        if has_spread(text) || only_stops_the_event(handler) {
            continue;
        }
        // Hidden from assistive technology, or presentational: not a control.
        let hidden = bound(text, "aria-hidden").is_some_and(|v| v.trim() == "true" || v == DYNAMIC);
        let role = attr(text, "role").map(|r| r.trim().to_ascii_lowercase());
        if hidden || matches!(role.as_deref(), Some("presentation" | "none")) {
            continue;
        }
        // A backdrop: an overlay laid over the page that closes what is on it.
        let tokens = tokens_of(src, tag);
        let backdrop = tokens.iter().any(|t| t.base == "inset-0")
            && tokens
                .iter()
                .any(|t| t.base == "fixed" || t.base == "absolute");
        if backdrop {
            continue;
        }
        if role.is_none() || bound(text, "tabindex").is_none() {
            out.hit("non-semantic-button", tag.start, tag.end, snippet(text));
        }
    }
}

/// The text of attribute `name`'s value as written: a `{…}` expression's
/// inside, or a quoted value. `None` when the tag has no such attribute.
fn attr_expression<'t>(tag: &'t str, name: &str) -> Option<&'t str> {
    attr(tag, name)?;
    let lower = tag.to_ascii_lowercase();
    let at = lower.match_indices(name).map(|(at, _)| at).find(|at| {
        *at > 0
            && tag.as_bytes()[at - 1].is_ascii_whitespace()
            && lower[at + name.len()..].trim_start().starts_with('=')
    })?;
    let rest = tag[at + name.len()..]
        .trim_start()
        .strip_prefix('=')?
        .trim_start();
    let bytes = rest.as_bytes();
    match bytes.first()? {
        b'{' => {
            let mut depth = 0usize;
            for (i, b) in bytes.iter().enumerate() {
                match b {
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(&rest[1..i]);
                        }
                    }
                    _ => {}
                }
            }
            Some(&rest[1..])
        }
        q @ (b'"' | b'\'') => {
            let body = &rest[1..];
            Some(&body[..body.find(*q as char).unwrap_or(body.len())])
        }
        _ => Some(rest.split_whitespace().next().unwrap_or("")),
    }
}

pattern!(
    STOP_ONLY,
    r"^\s*(?:\(?\s*\w*\s*(?::\s*[\w.<>]+)?\s*\)?\s*=>\s*)?\{?\s*(?:\w+\.(?:stopPropagation|preventDefault)\(\)\s*;?\s*)+\}?\s*$"
);

/// A handler that only stops the event from reaching what is behind
/// (`e => e.stopPropagation()`): the inside of a modal, not a control.
fn only_stops_the_event(handler: &str) -> bool {
    STOP_ONLY.is_match(handler) || handler.trim() == "stopPropagation"
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

/// Elements whose text is a control's label: an emoji there is an icon.
const LABEL_TAGS: [&str; 6] = ["button", "a", "label", "summary", "Button", "Link"];

/// Emoji used as icons, and filler copy, in element text.
///
/// An emoji is an icon when it is the label of a button, link or label
/// element: alone, or in front of a label of one to three words
/// (`<button>🗑️ Delete</button>`). Emoji in a sentence, a heading, a table
/// cell or a list of countries are content and are not reported.
fn element_text(src: &Source, out: &mut Out) {
    let ancestors = run_ancestors(src);
    for (run, open) in src.runs.iter().zip(&ancestors) {
        for m in FILLER.find_iter(&run.text) {
            out.hit(
                "placeholder-text",
                run.file_offset(m.start()),
                run.file_offset(m.end()),
                m.as_str(),
            );
        }
        let Some(&inner) = open.last() else {
            continue;
        };
        if !LABEL_TAGS.contains(&src.tags[inner].name.as_str()) {
            continue;
        }
        let lead = run.text.len() - run.text.trim_start().len();
        let text = run.text.trim();
        let chars: Vec<char> = text.chars().collect();
        let Some(&first) = chars.first() else {
            continue;
        };
        if !is_emoji_at(first, chars.get(1).copied()) {
            continue;
        }
        let rest: String = chars
            .iter()
            .skip(1)
            .skip_while(|c| is_emoji(**c) || is_emoji_part(**c))
            .collect();
        let words = rest.split_whitespace().count();
        let label = rest.starts_with(char::is_whitespace)
            && rest.trim_start().starts_with(char::is_alphabetic)
            && words <= 3;
        if rest.trim().is_empty() || label {
            let start = run.file_offset(lead);
            out.hit("emoji-icon", start, start + first.len_utf8(), snippet(text));
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
/// starts in it (with the run's index).
fn joined_text(src: &Source) -> (String, Vec<(usize, usize)>) {
    let mut text = String::new();
    let mut starts = Vec::new();
    for (i, run) in src.runs.iter().enumerate() {
        starts.push((text.len(), i));
        text.push_str(&run.text);
        text.push(' ');
    }
    (text, starts)
}

/// The file offset of byte `at` of the joined text.
fn file_offset(src: &Source, starts: &[(usize, usize)], at: usize) -> usize {
    let i = starts.partition_point(|(joined, _)| *joined <= at);
    match i.checked_sub(1).and_then(|i| starts.get(i)) {
        Some((joined, run)) => {
            let run = &src.runs[*run];
            run.file_offset((at - joined).min(run.text.len()))
        }
        None => 0,
    }
}

pattern!(EM_DASH_ENTITY, r"(?i)&mdash;|&#0*8212;|&#x0*2014;");

fn copy_rules(src: &Source, out: &mut Out) {
    let (text, starts) = joined_text(src);
    let file_offset = |at: usize| file_offset(src, &starts, at);
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
        let start = file_offset(*at);
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
        let start = file_offset(first.start());
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
    // An entity (`&mdash;`) is one dash, and one character of the text.
    let mut entity_chars = 0;
    for m in EM_DASH_ENTITY.find_iter(&text) {
        dashes.push(m.start());
        entity_chars += m.len() - 1;
    }
    dashes.sort_unstable();
    let chars = text.chars().count() - entity_chars;
    if dashes.len() >= EM_DASH_FLOOR && chars <= dashes.len() * EM_DASH_CHARS_PER_DASH {
        let start = file_offset(dashes[0]);
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
                    .is_some_and(|c| (300..700).contains(&shade(&c, 1)))
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

/// Body text blocks that centring is judged on (headings, buttons, links and
/// table headers are centred by convention and are not counted).
const PROSE_TAGS: [&str; 6] = ["p", "li", "dd", "blockquote", "figcaption", "td"];
/// Body blocks a file needs before it reads as a layout, and the share of them
/// that must be centred.
const MIN_PROSE_BLOCKS: usize = 4;
const CENTRED_SHARE: f64 = 0.8;

/// How an element aligns its text, from its classes and inline style:
/// `Some(true)` centred, `Some(false)` aligned to a side, `None` inherited.
fn own_alignment(src: &Source, tag: &Tag) -> Option<bool> {
    let mut found = None;
    for t in tokens_of(src, tag).iter().filter(|t| t.variants.is_empty()) {
        match t.base.as_str() {
            "text-center" => found = Some(true),
            "text-left" | "text-start" | "text-right" | "text-end" | "text-justify" => {
                found = Some(false)
            }
            _ => {}
        }
    }
    for d in src.decls_in(tag.start, tag.end) {
        if d.prop == "text-align" {
            found = Some(d.value.trim().eq_ignore_ascii_case("center"));
        }
    }
    found
}

/// Centring dominates the body text: of the paragraphs, list items, table
/// cells and quotes with text, at least [`CENTRED_SHARE`] are centred (by
/// their own classes or an ancestor's), and there are at least
/// [`MIN_PROSE_BLOCKS`] of them. Left alignment is the default, so a block
/// with no alignment written counts as left.
fn centered(src: &Source, out: &mut Out) {
    let ancestors = run_ancestors(src);
    let mut prose = 0usize;
    let mut centred = 0usize;
    let mut first: Option<usize> = None;
    for (run, open) in src.runs.iter().zip(&ancestors) {
        let Some(&inner) = open.last() else {
            continue;
        };
        if !PROSE_TAGS.contains(&src.tags[inner].name.as_str()) || run.text.trim().len() < 3 {
            continue;
        }
        prose += 1;
        let aligned = open
            .iter()
            .rev()
            .find_map(|i| own_alignment(src, &src.tags[*i]))
            .unwrap_or(false);
        if aligned {
            centred += 1;
            first.get_or_insert(src.tags[inner].start);
        }
    }
    if let Some(at) = first {
        if prose >= MIN_PROSE_BLOCKS && centred as f64 >= prose as f64 * CENTRED_SHARE {
            out.hit(
                "centered-everything",
                at,
                at + 1,
                format!("{centred} of {prose} text blocks are centred"),
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
            "font-size" | "font" => {
                let small_print = names_sup_or_sub(selector) || in_tag_named(&["sup", "sub"][..]);
                if small_print {
                    continue;
                }
                // The size and, for a simple value, the exact text to raise.
                let (px, written) = if d.prop == "font" {
                    match shorthand_size(value) {
                        Some((px, size)) => (px, Some(size)),
                        None => continue,
                    }
                } else {
                    let Some(px) = font_px(value, d.jsx) else {
                        continue;
                    };
                    let bare = value.trim_end_matches("!important").trim().to_string();
                    let simple = size_px(&bare, d.jsx).is_some();
                    (px, simple.then_some(bare))
                };
                if px > 0.0 && px < MIN_TEXT_PX {
                    let evidence = format!("{}: {value}", d.prop);
                    let new = written.as_deref().and_then(|size| {
                        let to = if d.jsx && size.parse::<f64>().is_ok() {
                            "12"
                        } else {
                            raised(size)?
                        };
                        let at = value.find(size)?;
                        Some(format!("{}{to}{}", &value[..at], &value[at + size.len()..]))
                    });
                    match new {
                        Some(new) => {
                            out.fix("tiny-text", d.value_start, d.value_end, evidence, new)
                        }
                        None => out.hit("tiny-text", d.value_start, d.value_end, evidence),
                    }
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
    // A stylesheet rule reads every rule of the file with its selector; an
    // inline style or a JSX style object reads its own declarations.
    let corners = match src.tag_at(d.start) {
        Some(_) => declared_corners(src, start, start + scope.len()),
        None => merged_rule_corners(src, d.start)
            .unwrap_or_else(|| declared_corners(src, start, start + scope.len())),
    };
    let radius = corners.is_some_and(|c| c != Corners::square());
    // impeccable's gate: a side stripe is reported only on a box rounded away
    // from it; a block that declares no radius is the initial square box.
    let side = if d.prop.contains("left") || d.prop.contains("start") {
        3
    } else {
        1
    };
    let rounded_away = corners.is_some_and(|c| c.rounded_away_from(side));
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
            if !safe_element && !neutral && n >= threshold && rounded_away {
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
                if !safe_element && px >= 3.0 && rounded_away {
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

/// Tailwind sizes in pixels and weights that make text "large" for WCAG.
fn tailwind_large(tokens: &[&Token]) -> bool {
    let size = tokens
        .iter()
        .rev()
        .filter(|t| t.variants.is_empty())
        .find_map(|t| tw_text_size(&t.base))
        .unwrap_or(16);
    let bold = tokens.iter().any(|t| {
        t.variants.is_empty()
            && matches!(
                t.base.as_str(),
                "font-bold" | "font-extrabold" | "font-black"
            )
    });
    size >= 24 || (size >= 19 && bold)
}

/// Tailwind text and background colours on one element, below WCAG AA
/// (4.5:1, 3:1 for large text), from Tailwind's default palette and the
/// project's config colours. A colour that cannot be resolved, a
/// see-through background (what is behind it is unknown) or a class behind
/// a variant (`hover:`, `dark:`) is not judged.
fn tailwind_contrast(src: &Source, context: &super::Context, out: &mut Out) {
    for tag in src.tags.iter().filter(|t| !t.closing) {
        let tokens = tokens_of(src, tag);
        // The last class of each kind wins, as in Tailwind's own order.
        let resolved = |prefix: &str| {
            tokens
                .iter()
                .rev()
                .filter(|t| t.variants.is_empty())
                .find_map(|t| {
                    let part = t.base.strip_prefix(prefix)?;
                    super::tailwind::resolve(part, &context.colors).map(|c| (*t, c))
                })
        };
        let (Some((text_token, text)), Some((bg_token, ground))) =
            (resolved("text-"), resolved("bg-"))
        else {
            continue;
        };
        if ground.a < 0.99 || text.a < 0.05 {
            continue;
        }
        let needed = if tailwind_large(&tokens) { 3.0 } else { 4.5 };
        let ratio = contrast(&text.over(&ground), &ground);
        if ratio < needed {
            out.hit(
                "low-contrast",
                text_token.start,
                text_token.end,
                format!(
                    "{} on {} is {:.1}:1 (needs {needed}:1)",
                    text_token.base, bg_token.base, ratio
                ),
            );
        }
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
            .find_map(|d| font_px(&d.value, d.jsx))
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
            let scope = scope_tokens(src, t.start);
            let rounded = is_rounded(&scope);
            // impeccable's gate: only a card rounded away from its stripe. A
            // square box (a quote, a nav item, a gutter) is an old convention.
            // Shodh also skips the elements impeccable's CSS path exempts.
            let side = if matches!(&c[1], "l" | "s") { 3 } else { 1 };
            let tag = src.tag_at(t.start);
            let safe = tag.is_some_and(|tag| SIDE_TAB_SAFE_TAGS.contains(&tag.name.as_str()));
            let corners = tag
                .filter(|tag| class_is_literal(tag_text(src, tag)))
                .map(|_| tailwind_corners(&scope))
                .unwrap_or_else(Corners::unknown);
            if !safe && n >= if rounded { 2 } else { 4 } && corners.rounded_away_from(side) {
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
            let px = if &c[2] == "px" { n } else { n * 16.0 };
            if px > 0.0 && px < MIN_TEXT_PX {
                // The line-height modifier and the `!` are kept.
                let rest = format!(
                    "{}{}",
                    c.get(3).map_or("", |m| m.as_str()),
                    c.get(4).map_or("", |m| m.as_str())
                );
                out.fix("tiny-text", t.start, t.end, base, format!("text-xs{rest}"));
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
            (if &c[2] == "px" { n } else { n * 16.0 }).round() as i64
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
                if let Some(px) = font_px(&d.value, d.jsx) {
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
