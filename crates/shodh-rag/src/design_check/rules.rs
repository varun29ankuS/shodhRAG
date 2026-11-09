//! The design check's rules: what each looks for, how bad it is, and what to
//! do about it, in plain words.
//!
//! Rules marked [`RuleOrigin::Impeccable`] are ported from the source-text
//! detector of pbakaus/impeccable (<https://github.com/pbakaus/impeccable>,
//! commit [`IMPECCABLE_COMMIT`], Apache License 2.0; see `NOTICE`). Their ids,
//! names and descriptions are impeccable's (`crates/foundation/src/registry.rs`
//! there), kept word for word. Changed by Shodh: the detection is a new Rust
//! implementation over source text only (impeccable's rendered-page checks need
//! a browser and are not ported), several rules are narrowed to what source text
//! can show, and every rule gains a suggested fix. Where Shodh departs from
//! impeccable on purpose: the taste rules (gradient-text, ai-color-palette,
//! overused-font) are advisory here, never warnings, because a user may ask
//! for exactly that font, colour or gradient; side-tab's Tailwind form also
//! skips the elements impeccable exempts only in CSS (blockquote, nav, pre,
//! code, a, input, span); and side-tab reads the radius of the element's own
//! classes or block, not of other rules for the same element.
//! Rules marked [`RuleOrigin::Shodh`] are Shodh's own (accessibility and
//! consistency checks the user asked for; impeccable covers them only on a
//! rendered page).

use serde::Serialize;

/// The impeccable commit the ported rules were read from.
pub const IMPECCABLE_COMMIT: &str = "a90602e9f6fed600d6d9b18cf78275d66cac7a6c";
pub const IMPECCABLE_URL: &str = "https://github.com/pbakaus/impeccable";

/// How bad a finding is, worst first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DesignSeverity {
    /// People cannot use or read it (a missing label, an image with no text).
    Error,
    /// A visible flaw or a generated-UI tell.
    Warning,
    /// Worth a look; often a deliberate choice.
    Advisory,
}

impl DesignSeverity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Advisory => "advisory",
        }
    }
}

/// Where a rule comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RuleOrigin {
    Impeccable,
    Shodh,
}

/// One rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Rule {
    pub id: &'static str,
    pub name: &'static str,
    /// `slop` (a generated-UI tell), `quality` or `accessibility`.
    pub category: &'static str,
    pub severity: DesignSeverity,
    pub origin: RuleOrigin,
    /// What is wrong and why, in plain words.
    pub description: &'static str,
    /// What to do instead.
    pub fix: &'static str,
}

const fn imp(
    id: &'static str,
    name: &'static str,
    category: &'static str,
    severity: DesignSeverity,
    description: &'static str,
    fix: &'static str,
) -> Rule {
    Rule {
        id,
        name,
        category,
        severity,
        origin: RuleOrigin::Impeccable,
        description,
        fix,
    }
}

const fn own(
    id: &'static str,
    name: &'static str,
    category: &'static str,
    severity: DesignSeverity,
    description: &'static str,
    fix: &'static str,
) -> Rule {
    Rule {
        id,
        name,
        category,
        severity,
        origin: RuleOrigin::Shodh,
        description,
        fix,
    }
}

use DesignSeverity::{Advisory, Error, Warning};

/// Every rule, in the order findings of equal severity are listed.
pub const RULES: [Rule; 34] = [
    // Accessibility (Shodh's own).
    own(
        "img-missing-alt",
        "Image without alt text",
        "accessibility",
        Error,
        "An <img> has no alt attribute, so a screen reader announces the file name or nothing. Every image needs alt text; a purely decorative one takes alt=\"\".",
        "Add alt=\"…\" describing what the image shows, or alt=\"\" when it is only decoration.",
    ),
    own(
        "control-missing-label",
        "Form field without a label",
        "accessibility",
        Error,
        "An input, select or text area has no label, aria-label or aria-labelledby. A placeholder is not a label: it disappears as soon as someone types, and many screen readers skip it.",
        "Wrap the field in a <label>, point a <label for=\"…\"> at its id, or give it aria-label.",
    ),
    own(
        "icon-button-missing-label",
        "Button with no name",
        "accessibility",
        Error,
        "A button shows only an icon and has no text, aria-label or title, so a screen reader announces just \"button\".",
        "Add a short visible label next to the icon (icon on the left), or aria-label=\"…\" when space is tight.",
    ),
    own(
        "non-semantic-button",
        "Clickable div or span",
        "accessibility",
        Warning,
        "A div or span reacts to clicks but has no role and no tabindex, so keyboard users cannot reach it and screen readers do not announce it as a control. (Backdrops, presentational and hidden elements, and handlers that only stop the event are not reported.)",
        "Use a <button type=\"button\"> (or an <a> for navigation); it gets focus, Enter and Space for free.",
    ),
    own(
        "missing-focus-style",
        "Focus outline removed",
        "accessibility",
        Error,
        "The focus outline is turned off with no visible focus style in its place, so keyboard users cannot see where they are.",
        "Keep the outline, or replace it with a visible :focus-visible style (a ring or outline of at least 2px).",
    ),
    imp(
        "low-contrast",
        "Low contrast text",
        "quality",
        Warning,
        "Text does not meet WCAG AA contrast requirements (4.5:1 for body, 3:1 for large text). Increase the contrast between text and background.",
        "Darken the text or lighten the background until the ratio is at least 4.5:1 (3:1 for large text).",
    ),
    imp(
        "tiny-text",
        "Tiny body text",
        "quality",
        Warning,
        "Body text below 12px is hard to read, especially on high-DPI screens. Use at least 14px for body content, 16px is ideal.",
        "Use 12px at the very least (Tailwind text-xs); 14px to 16px for body text.",
    ),
    imp(
        "skipped-heading",
        "Skipped heading level",
        "quality",
        Warning,
        "Heading levels should not skip (e.g. h1 then h3 with no h2). Screen readers use heading hierarchy for navigation. Skipping levels breaks the document outline.",
        "Use the next heading level down, and style it to the size you want.",
    ),
    imp(
        "broken-image",
        "Broken or placeholder image",
        "quality",
        Warning,
        "<img> tags with empty src, missing src, or placeholder values ship as broken-image boxes. Use real images, generated assets, or remove the tag.",
        "Point src at a real image of the project, or remove the image.",
    ),
    own(
        "placeholder-text",
        "Placeholder text left in",
        "quality",
        Warning,
        "Lorem ipsum or filler copy (\"Your text here\", \"Insert content here\") is still on the page. It reads as unfinished and hides how the layout behaves with real words.",
        "Replace it with the real words, or with short honest copy that says what goes there.",
    ),
    own(
        "emoji-icon",
        "Emoji used as an icon",
        "quality",
        Warning,
        "An emoji is the label of a button, link or label element: alone, or before a label of one to three words. Emoji render differently on every system, cannot be coloured with the theme, and are read aloud by name. Emoji in sentences, headings and tables are content and are not reported; typographic glyphs used as icons (✕ ★ ☰ ❯ arrows) are not emoji.",
        "Use an icon from the project's icon set (aria-hidden, left of the label), or drop it.",
    ),
    imp(
        "gray-on-color",
        "Gray text on colored background",
        "quality",
        Warning,
        "Gray text looks washed out on colored backgrounds. Use a darker shade of the background color instead, or white/near-white for contrast.",
        "Use white or a near-white for the text, or a darker shade of the background's own colour.",
    ),
    imp(
        "justified-text",
        "Justified text",
        "quality",
        Warning,
        "In a column this narrow, justifying without hyphenation stretches the word spaces until vertical rivers of white run down the block. Use text-align: left, widen the measure, or enable hyphens: auto if you must justify. Scripts that justify on a character grid or by elongating glyphs, such as CJK, Thai and Arabic, are not affected and are not reported.",
        "Align the text left (text-align: left, Tailwind text-left).",
    ),
    imp(
        "tight-leading",
        "Tight line height",
        "quality",
        Warning,
        "Line height below 1.3x the font size makes multi-line text hard to read. Use 1.5 to 1.7 for body text so lines have room to breathe.",
        "Use a line height of 1.5 (Tailwind leading-normal) for body text.",
    ),
    imp(
        "extreme-negative-tracking",
        "Crushed letter spacing",
        "slop",
        Warning,
        "Letter-spacing pulled tighter than the point where characters keep their own shapes costs legibility. Tighten display type optically, not destructively.",
        "Use at most -0.02em (Tailwind tracking-tight).",
    ),
    imp(
        "wide-tracking",
        "Wide letter spacing on body text",
        "quality",
        Warning,
        "Letter spacing above 0.05em on body text disrupts natural character groupings and slows reading. Reserve wide tracking for short uppercase labels only.",
        "Remove the letter spacing from running text; keep it for short uppercase labels.",
    ),
    imp(
        "all-caps-body",
        "All-caps body text",
        "quality",
        Warning,
        "Long passages in uppercase are hard to read. We recognize words by shape (ascenders and descenders), which all-caps removes. Reserve uppercase for short labels and headings. Only a run that reads as a sentence counts: 80 characters or more of an element's own text. Buttons, nav items, kickers and eyebrows set in caps are a convention and stay silent.",
        "Set the passage in sentence case; keep uppercase for short labels.",
    ),
    imp(
        "side-tab",
        "Side-tab accent border",
        "slop",
        Warning,
        "Thick colored border on one side of a card — the most recognizable tell of AI-generated UIs. Use a subtler accent or remove it entirely.",
        "Remove the side border, or use a 1px border all round with a quieter colour.",
    ),
    imp(
        "border-accent-on-rounded",
        "Border accent on rounded element",
        "slop",
        Warning,
        "Thick accent border on a rounded card — the border clashes with the rounded corners. Remove the border or the border-radius.",
        "Remove the top or bottom accent border, or the rounding.",
    ),
    imp(
        "gradient-text",
        "Gradient text",
        "slop",
        Advisory,
        "Gradient text is decorative rather than meaningful — a common AI tell, especially on headings and metrics. Use solid colors for text.",
        "Use one solid text colour from the palette.",
    ),
    imp(
        "ai-color-palette",
        "AI color palette",
        "slop",
        Advisory,
        "Purple/violet gradients and cyan-on-dark are the most recognizable tells of AI-generated UIs. A gradient in one of those hues is the tell on its own; flat neon ink on a dark ground is charged once a second tell hue joins it. Choose a distinctive, intentional palette.",
        "Pick a palette for this product (one accent colour, used sparingly) instead of the purple-to-blue default.",
    ),
    imp(
        "overused-font",
        "Overused font",
        "slop",
        Advisory,
        "Inter, Roboto, Fraunces, Geist, Plus Jakarta Sans, and Space Grotesk are used on so many sites they no longer feel distinctive. Each new wave of AI-generated UIs converges on the same handful of faces. Choose a face that gives your interface personality.",
        "Choose a face that suits the product, or the project's own font stack.",
    ),
    imp(
        "dark-glow",
        "Glowing shadow accents",
        "slop",
        Warning,
        "Colored glow shadows — a zero-offset chromatic halo (box- or text-shadow) on any background, or any colored blurred shadow on a dark background — are the default \"cool\" look of AI-generated UIs. Use neutral elevation shadows and subtle, purposeful lighting instead.",
        "Use a neutral shadow with a small offset (for example 0 1px 3px rgba(0,0,0,0.12)), or none.",
    ),
    imp(
        "marquee",
        "Auto-scrolling marquee",
        "slop",
        Warning,
        "Continuously auto-scrolling content demands attention it has not earned and hides half its content at any moment. Reserve motion for content that changes; let readers move at their own pace.",
        "Show the items still, in a row or a grid that wraps.",
    ),
    imp(
        "pulsing-dot",
        "Pulsing status dot",
        "slop",
        Warning,
        "Small pulsing status dots simulate liveness decoratively. Reserve pulse animation for indicators tied to genuinely live, changing data; a static indicator with clear labeling is honest and calmer.",
        "Show a still dot with a word next to it (\"Live\", \"Online\").",
    ),
    imp(
        "marketing-buzzword",
        "Marketing buzzword",
        "slop",
        Warning,
        "Generic SaaS phrases (streamline / empower / supercharge / world-class / enterprise-grade / next-generation / cutting-edge / etc) are instant AI tells. Pick a specific verb and noun that says what the product literally does.",
        "Say what the product does, with a plain verb and noun.",
    ),
    imp(
        "aphoristic-cadence",
        "Aphoristic-cadence copy",
        "slop",
        Warning,
        "Three or more sections landing on a short rebuttal sentence (\"X. No Y.\" / \"X. Just Y.\") or a manufactured-contrast aphorism (\"Not a feature. A platform.\") reads as AI cadence, not voice. Once is fine; the pattern is the tell.",
        "Rewrite the lines as plain statements of what the thing does.",
    ),
    imp(
        "monotonous-spacing",
        "Monotonous spacing",
        "slop",
        Warning,
        "The same spacing value used everywhere — no rhythm, no variation. Use tight groupings for related items and generous separations between sections.",
        "Use a small step inside groups and a larger one between sections, from one spacing scale.",
    ),
    imp(
        "bounce-easing",
        "Bounce or elastic easing",
        "slop",
        Advisory,
        "Bounce and elastic easing feel dated and tacky. Real objects decelerate smoothly — use exponential easing (ease-out-quart/quint/expo) instead.",
        "Use an ease-out curve such as cubic-bezier(0.22, 1, 0.36, 1).",
    ),
    imp(
        "em-dash-overuse",
        "Em-dash overuse",
        "slop",
        Advisory,
        "Em-dash saturation in body copy is an AI cadence tell. Advisory only: humans use em-dashes legitimately, so this fires only on saturation — at least 8 em-dashes (— or --) at a density near one per 500 characters of body text — never on a long article that uses a few. Prefer commas, colons, periods, or parentheses.",
        "Replace most dashes with commas, colons or full stops.",
    ),
    imp(
        "radial-halo",
        "Radial-gradient background halo",
        "slop",
        Advisory,
        "A chromatic radial-gradient wash — saturated at the center, fading to transparent — used as a decorative background glow on a dark page. Same tell as glowing shadows, drawn with a gradient instead of a shadow. Ground the surface with a solid or subtly shifted background instead.",
        "Use a solid background, or a very subtle shift between two close shades.",
    ),
    imp(
        "repeating-stripes-gradient",
        "Repeating-gradient stripes",
        "slop",
        Advisory,
        "Repeating-gradient stripes used as surface decoration are a recurring generated-UI signature. Reach for a deliberate texture or leave the surface plain.",
        "Leave the surface plain, or use a texture made for this product.",
    ),
    own(
        "centered-everything",
        "Everything centred",
        "quality",
        Advisory,
        "Almost every block of text in the file is centred and nothing is aligned left. Centred paragraphs are slow to read because each line starts somewhere new.",
        "Centre only short headings and calls to action; align paragraphs and lists left.",
    ),
    own(
        "inconsistent-scale",
        "Too many radius or type sizes",
        "quality",
        Advisory,
        "The file uses many different corner radii or font sizes. A design reads as one system when it repeats a few values from a scale.",
        "Pick three or four radii and five or six type sizes, and use only those.",
    ),
];

/// The rule with id `id`.
pub fn rule(id: &str) -> Option<&'static Rule> {
    RULES.iter().find(|r| r.id == id)
}

/// Where `id` sits in [`RULES`] (for a stable order).
pub fn rank(id: &str) -> usize {
    RULES.iter().position(|r| r.id == id).unwrap_or(RULES.len())
}
