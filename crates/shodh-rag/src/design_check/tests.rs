//! Every rule has a fixture that triggers it and one that does not; the
//! check is deterministic; mechanical fixes are exact one-line replacements.

use super::*;

fn ids(path: &str, text: &str) -> Vec<&'static str> {
    check_source(path, text).0.iter().map(|f| f.rule).collect()
}

/// `rule` fires on `bad` and stays silent on `good`.
#[track_caller]
fn fires(rule: &str, path: &str, bad: &str, good: &str) {
    assert!(rules::rule(rule).is_some(), "{rule} is not a rule");
    let found = ids(path, bad);
    assert!(
        found.contains(&rule),
        "{rule} did not fire on {bad:?}: {found:?}"
    );
    let clean = ids(path, good);
    assert!(
        !clean.contains(&rule),
        "{rule} fired on {good:?}: {clean:?}"
    );
}

/// The mechanical fix `rule` proposes on `bad`.
#[track_caller]
fn fix_of(rule: &str, path: &str, bad: &str) -> DesignReplacement {
    check_source(path, bad)
        .0
        .into_iter()
        .find(|f| f.rule == rule)
        .unwrap_or_else(|| panic!("{rule} did not fire"))
        .replace
        .unwrap_or_else(|| panic!("{rule} proposed no mechanical fix"))
}

#[test]
fn img_missing_alt() {
    fires(
        "img-missing-alt",
        "a.html",
        r#"<img src="/team.jpg">"#,
        r#"<img src="/team.jpg" alt="The team"><img src="/line.svg" alt="">"#,
    );
    fires(
        "img-missing-alt",
        "a.tsx",
        r#"const A = () => <img src={hero} />;"#,
        r#"const A = () => <img src={hero} alt={label} />;"#,
    );
    // Vue binds it with a colon.
    assert!(
        !ids("a.vue", r#"<template><img :src="s" :alt="t"></template>"#)
            .contains(&"img-missing-alt")
    );
}

#[test]
fn broken_image() {
    fires(
        "broken-image",
        "a.html",
        r#"<img src="https://placehold.co/600x400" alt="">"#,
        r#"<img src="/hero.jpg" alt="Hero">"#,
    );
    fires(
        "broken-image",
        "a.html",
        r#"<img src="" alt="x">"#,
        r#"<img src="/a.png" alt="x">"#,
    );
    fires(
        "broken-image",
        "a.tsx",
        r#"<img alt="Logo" />"#,
        r#"<img src={logo} alt="Logo" />"#,
    );
}

#[test]
fn control_missing_label() {
    fires(
        "control-missing-label",
        "a.html",
        r#"<input type="email" placeholder="Email">"#,
        r#"<label for="email">Email</label><input id="email" type="email">"#,
    );
    fires(
        "control-missing-label",
        "a.html",
        r#"<select name="size"><option>S</option></select>"#,
        r#"<label>Size <select name="size"><option>S</option></select></label>"#,
    );
    fires(
        "control-missing-label",
        "a.tsx",
        r#"<textarea value={v} onChange={set} />"#,
        r#"<textarea aria-label="Notes" value={v} onChange={set} />"#,
    );
    assert!(!ids(
        "a.html",
        r#"<input type="hidden" name="t"><input type="submit" value="Go">"#
    )
    .contains(&"control-missing-label"));
}

#[test]
fn icon_button_missing_label() {
    fires(
        "icon-button-missing-label",
        "a.html",
        r#"<button><svg viewBox="0 0 24 24"><path d="M0 0h24"/></svg></button>"#,
        r#"<button><svg viewBox="0 0 24 24"><path d="M0 0h24"/></svg> Save</button>"#,
    );
    fires(
        "icon-button-missing-label",
        "a.tsx",
        r#"<button onClick={() => close()}><X className="w-4 h-4" /></button>"#,
        r#"<button aria-label="Close" onClick={() => close()}><X className="w-4 h-4" /></button>"#,
    );
    // Text set at run time may name it.
    assert!(
        !ids("a.tsx", r#"<button onClick={go}><Icon />{label}</button>"#)
            .contains(&"icon-button-missing-label")
    );
}

#[test]
fn non_semantic_button() {
    fires(
        "non-semantic-button",
        "a.tsx",
        r#"<div onClick={open} className="card">Open</div>"#,
        r#"<div role="button" tabIndex={0} onClick={open} className="card">Open</div>"#,
    );
    fires(
        "non-semantic-button",
        "a.vue",
        r#"<template><span @click="go">Go</span></template>"#,
        r#"<template><button type="button" @click="go">Go</button></template>"#,
    );
}

#[test]
fn missing_focus_style() {
    fires(
        "missing-focus-style",
        "a.tsx",
        r#"<button className="outline-none px-4">Go</button>"#,
        r#"<button className="outline-none focus-visible:ring-2 px-4">Go</button>"#,
    );
    fires(
        "missing-focus-style",
        "a.css",
        "button:focus { outline: none; }\n",
        "button:focus { outline: none; }\nbutton:focus-visible { outline: 2px solid #333; }\n",
    );
    // A non-interactive element may drop its outline.
    assert!(
        !ids("a.tsx", r#"<div className="outline-none">x</div>"#).contains(&"missing-focus-style")
    );
    let fix = fix_of(
        "missing-focus-style",
        "a.tsx",
        "      <button className=\"outline-none px-4\">Go</button>\n",
    );
    assert_eq!(
        fix.new,
        "      <button className=\"outline-none focus-visible:ring-2 focus-visible:ring-offset-2 px-4\">Go</button>"
    );
}

#[test]
fn low_contrast() {
    fires(
        "low-contrast",
        "a.css",
        ".note { color: #999; background: #fff; }",
        ".note { color: #333; background: #fff; }",
    );
    fires(
        "low-contrast",
        "a.html",
        r#"<p style="color: #aaa; background-color: #ffffff">Hi</p>"#,
        r#"<p style="color: #222; background-color: #ffffff">Hi</p>"#,
    );
    // Large text needs only 3:1 (#888 on white is 3.5:1).
    assert!(!ids(
        "a.css",
        ".h { color: #888; background: #fff; font-size: 32px; }"
    )
    .contains(&"low-contrast"));
    assert!(ids(
        "a.css",
        ".h { color: #888; background: #fff; font-size: 14px; }"
    )
    .contains(&"low-contrast"));
}

#[test]
fn tiny_text_below_twelve_pixels() {
    fires(
        "tiny-text",
        "a.css",
        ".small { font-size: 10px; }",
        ".small { font-size: 12px; }",
    );
    fires(
        "tiny-text",
        "a.tsx",
        r#"<span className="text-[10px] uppercase">New</span>"#,
        r#"<span className="text-xs uppercase">New</span>"#,
    );
    fires(
        "tiny-text",
        "a.tsx",
        r#"<p style={{ fontSize: 11 }}>Hi</p>"#,
        r#"<p style={{ fontSize: 12 }}>Hi</p>"#,
    );
    fires(
        "tiny-text",
        "a.scss",
        ".x { font-size: 0.6rem; }",
        ".x { font-size: 0.75rem; }",
    );
    let fix = fix_of("tiny-text", "a.css", ".small {\n  font-size: 10px;\n}\n");
    assert_eq!(fix.old, "  font-size: 10px;");
    assert_eq!(fix.new, "  font-size: 12px;");
    let fix = fix_of(
        "tiny-text",
        "a.tsx",
        "<span className=\"md:text-[9px] font-medium\">x</span>\n",
    );
    assert_eq!(
        fix.new,
        "<span className=\"md:text-xs font-medium\">x</span>"
    );
}

#[test]
fn skipped_heading() {
    fires(
        "skipped-heading",
        "a.html",
        "<h1>Title</h1>\n<h3>Part</h3>",
        "<h1>Title</h1>\n<h2>Part</h2>\n<h3>Detail</h3>\n<h2>Next</h2>",
    );
}

#[test]
fn placeholder_text() {
    fires(
        "placeholder-text",
        "a.html",
        "<p>Lorem ipsum dolor sit amet, consectetur adipiscing elit.</p>",
        "<p>Shodh reads your papers and answers with citations.</p>",
    );
    fires(
        "placeholder-text",
        "a.tsx",
        "<h2>Your title here</h2>",
        "<h2>Reading list</h2>",
    );
    // One finding per line, not one per filler phrase.
    let found = ids("a.html", "<p>Lorem ipsum dolor sit amet</p>");
    assert_eq!(
        found.iter().filter(|r| **r == "placeholder-text").count(),
        1
    );
}

#[test]
fn emoji_icon() {
    // An emoji as the label of a button, link or label element.
    fires(
        "emoji-icon",
        "a.html",
        "<button>🗑️ Delete</button>",
        "<button>Delete</button>",
    );
    fires(
        "emoji-icon",
        "a.tsx",
        "<a href=\"/new\">🆕</a>",
        "<a href=\"/new\">New</a>",
    );
    fires(
        "emoji-icon",
        "a.html",
        "<button>⭐ Star</button>",
        "<button>★ Star</button>",
    );
    fires(
        "emoji-icon",
        "a.html",
        "<label>⏰ Remind me</label>",
        "<label>Remind me</label>",
    );
    fires(
        "emoji-icon",
        "a.html",
        "<button>▶️ Play</button>",
        "<button>▶ Play</button>",
    );
    // Typographic glyphs used as icons are not emoji.
    for glyph in ["✕", "✖", "★", "☆", "☰", "❯", "➜", "♥", "✓"] {
        let found = ids(
            "a.html",
            &format!("<button aria-label=\"x\">{glyph}</button>"),
        );
        assert!(!found.contains(&"emoji-icon"), "{glyph}: {found:?}");
    }
    // Emoji as content: a sentence, a heading, a status cell, a country list.
    for content in [
        "<p>🎉 You're in! Your account is ready.</p>",
        "<h1>🪔 Happy Diwali</h1>",
        "<table><tr><td>✅</td></tr></table>",
        "<select><option>🇮🇳 India</option></select>",
        "<p>We shipped it 🎉</p>",
        "<button>🎉 Thanks for joining us all the way</button>",
    ] {
        assert!(!ids("a.html", content).contains(&"emoji-icon"), "{content}");
    }
}

#[test]
fn gray_on_color() {
    fires(
        "gray-on-color",
        "a.tsx",
        r#"<button className="text-slate-300 bg-red-500">Log out</button>"#,
        r#"<button className="text-slate-300 bg-red-500/10">Log out</button>"#,
    );
    assert!(!ids(
        "a.tsx",
        r#"<p className="text-gray-600 bg-amber-100">Note</p>"#
    )
    .contains(&"gray-on-color"));
    assert!(!ids(
        "a.tsx",
        r#"<div className="text-gray-800 bg-yellow-400">Card</div>"#
    )
    .contains(&"gray-on-color"));
}

#[test]
fn justified_text() {
    fires(
        "justified-text",
        "a.css",
        "p { text-align: justify; }",
        "p { text-align: justify; hyphens: auto; }",
    );
    let fix = fix_of(
        "justified-text",
        "a.tsx",
        "<p className=\"text-justify\">x</p>\n",
    );
    assert_eq!(fix.new, "<p className=\"text-left\">x</p>");
}

#[test]
fn tight_leading() {
    fires(
        "tight-leading",
        "a.css",
        "p { line-height: 1.1; }",
        "h1 { line-height: 1.1; }",
    );
    fires(
        "tight-leading",
        "a.tsx",
        r#"<p className="leading-tight">Body copy</p>"#,
        r#"<h1 className="leading-tight">Title</h1>"#,
    );
    assert_eq!(
        fix_of("tight-leading", "a.css", "p { line-height: 1.1; }").new,
        "p { line-height: 1.5; }"
    );
}

#[test]
fn extreme_negative_tracking() {
    fires(
        "extreme-negative-tracking",
        "a.css",
        "h2 { letter-spacing: -0.08em; }",
        "h2 { letter-spacing: -0.02em; }",
    );
    // Display type may go further before letterforms touch.
    assert!(
        !ids("a.css", "h1 { font-size: 48px; letter-spacing: -0.08em; }")
            .contains(&"extreme-negative-tracking")
    );
    fires(
        "extreme-negative-tracking",
        "a.tsx",
        r#"<h2 className="tracking-[-0.1em]">x</h2>"#,
        r#"<h2 className="tracking-tight">x</h2>"#,
    );
}

#[test]
fn wide_tracking() {
    fires(
        "wide-tracking",
        "a.css",
        "p { letter-spacing: 0.1em; }",
        "p { letter-spacing: 0.1em; text-transform: uppercase; }",
    );
    fires(
        "wide-tracking",
        "a.html",
        r#"<p class="tracking-widest">This paragraph is long enough to read as running body text.</p>"#,
        r#"<p class="tracking-widest uppercase">Short label</p>"#,
    );
}

#[test]
fn all_caps_body() {
    let long = "This whole paragraph is set in capitals, which makes it slow to read because word shapes go away.";
    fires(
        "all-caps-body",
        "a.html",
        &format!(r#"<p class="uppercase">{long}</p>"#),
        r#"<p class="uppercase">New arrivals</p>"#,
    );
}

#[test]
fn side_tab() {
    // impeccable's gate: a stripe on a card rounded away from it.
    fires(
        "side-tab",
        "a.tsx",
        r#"<div className="rounded-lg border-l-4 border-blue-500 p-4">Note</div>"#,
        r#"<div className="border-l-4 border-blue-500 p-4">Note</div>"#,
    );
    assert!(ids(
        "a.tsx",
        r#"<div className="rounded-lg border-l-2 p-4">Note</div>"#
    )
    .contains(&"side-tab"));
    // Rounded only on the stripe's own side: square away from it.
    assert!(!ids(
        "a.tsx",
        r#"<div className="rounded-l-lg border-l-4 border-blue-500">x</div>"#
    )
    .contains(&"side-tab"));
    // A quote or an active nav item keeps its convention.
    for safe in [
        r#"<blockquote className="rounded-lg border-l-4 border-gray-300 pl-4 italic">Quote</blockquote>"#,
        r#"<nav><a className="rounded-md border-l-4 border-blue-600" href="/">Home</a></nav>"#,
    ] {
        assert!(!ids("a.tsx", safe).contains(&"side-tab"), "{safe}");
    }
    // Classes built at run time: the radius is unknown, so it stays reported.
    assert!(ids(
        "a.tsx",
        r#"<div className={cn("border-l-4 border-blue-500", r)}>x</div>"#
    )
    .contains(&"side-tab"));
    fires(
        "side-tab",
        "a.css",
        ".card { border-left: 4px solid #3b82f6; border-radius: 12px; }",
        ".card { border-left: 4px solid #3b82f6; }",
    );
    assert!(!ids(
        "a.css",
        ".c { border-left: 4px solid #3b82f6; border-radius: 12px 0 0 12px; }"
    )
    .contains(&"side-tab"));
    assert!(!ids(
        "a.css",
        "blockquote { border-left: 4px solid #cccccc; border-radius: 8px; }"
    )
    .contains(&"side-tab"));
    assert!(ids(
        "a.css",
        ".c { border-left: 4px solid #3b82f6; border-radius: var(--r); }"
    )
    .contains(&"side-tab"));
}

#[test]
fn border_accent_on_rounded() {
    fires(
        "border-accent-on-rounded",
        "a.tsx",
        r#"<div className="rounded-xl border-t-4 border-indigo-500">x</div>"#,
        r#"<div className="border-t-4 border-indigo-500">x</div>"#,
    );
    fires(
        "border-accent-on-rounded",
        "a.css",
        ".c { border-radius: 12px; border-top: 4px solid #6366f1; }",
        ".c { border-radius: 0; border-top: 4px solid #6366f1; }",
    );
}

#[test]
fn gradient_text() {
    fires(
        "gradient-text",
        "a.css",
        ".h { background: linear-gradient(90deg, #f97316, #facc15); -webkit-background-clip: text; }",
        ".h { background: #f97316; -webkit-background-clip: text; }",
    );
    fires(
        "gradient-text",
        "a.tsx",
        r#"<h1 className="bg-gradient-to-r from-pink-500 to-orange-500 bg-clip-text text-transparent">Hi</h1>"#,
        r#"<h1 className="bg-clip-text text-orange-600">Hi</h1>"#,
    );
}

#[test]
fn ai_color_palette() {
    fires(
        "ai-color-palette",
        "a.tsx",
        r#"<div className="bg-gradient-to-r from-purple-600 to-blue-600">x</div>"#,
        r#"<div className="bg-gradient-to-r from-orange-500 to-amber-400">x</div>"#,
    );
    fires(
        "ai-color-palette",
        "a.tsx",
        r#"<h1 className="text-violet-600">Hi</h1>"#,
        r#"<p className="text-violet-600">Hi</p>"#,
    );
    fires(
        "ai-color-palette",
        "a.css",
        ".hero { background: linear-gradient(135deg, #667eea 0%, #764ba2 100%); }",
        ".hero { background: linear-gradient(135deg, #f97316, #facc15); }",
    );
}

#[test]
fn overused_font() {
    fires(
        "overused-font",
        "a.css",
        "body { font-family: Inter, sans-serif; }",
        "body { font-family: \"Source Serif 4\", serif; }",
    );
    fires(
        "overused-font",
        "a.html",
        r#"<link href="https://fonts.googleapis.com/css2?family=Inter:wght@400;700&display=swap" rel="stylesheet">"#,
        r#"<link href="https://fonts.googleapis.com/css2?family=Fraunces+Display&display=swap" rel="stylesheet">"#,
    );
}

#[test]
fn dark_glow() {
    fires(
        "dark-glow",
        "a.css",
        ".btn { box-shadow: 0 0 20px rgba(168, 85, 247, 0.6); }",
        ".btn { box-shadow: 0 1px 3px rgba(0, 0, 0, 0.12); }",
    );
    fires(
        "dark-glow",
        "a.tsx",
        r#"<div className="shadow-lg shadow-purple-500/50">x</div>"#,
        r#"<div className="shadow-lg">x</div>"#,
    );
}

#[test]
fn marquee() {
    fires(
        "marquee",
        "a.html",
        "<marquee>News</marquee>",
        "<p>News</p>",
    );
    fires(
        "marquee",
        "a.css",
        ".track { animation: marquee 20s linear infinite; }",
        ".track { animation: fade 1s ease-out; }",
    );
}

#[test]
fn pulsing_dot() {
    fires(
        "pulsing-dot",
        "a.tsx",
        r#"<span className="h-2 w-2 rounded-full bg-green-500 animate-ping"></span>"#,
        r#"<div className="h-24 w-full rounded animate-pulse"></div>"#,
    );
}

#[test]
fn marketing_buzzword() {
    fires(
        "marketing-buzzword",
        "a.html",
        "<p>Supercharge your workflow with world-class tools.</p>",
        "<p>Shodh finds the passage and cites it.</p>",
    );
}

#[test]
fn aphoristic_cadence() {
    fires(
        "aphoristic-cadence",
        "a.html",
        "<p>Not a tool. A partner.</p><p>Fast setup. No waiting.</p><p>Built for you. Just results.</p>",
        "<p>Not a tool. A partner.</p><p>Setup takes two minutes.</p>",
    );
}

#[test]
fn em_dash_overuse() {
    let dashes = "<p>One — two — three — four — five — six — seven — eight — nine.</p>";
    fires(
        "em-dash-overuse",
        "a.html",
        dashes,
        "<p>One — two — three.</p>",
    );
}

#[test]
fn monotonous_spacing() {
    let same = ".a { padding: 16px; }\n".repeat(10);
    let varied = [4, 8, 16, 24, 32, 48, 4, 8, 16, 24]
        .iter()
        .map(|px| format!(".a {{ padding: {px}px; }}\n"))
        .collect::<String>();
    fires("monotonous-spacing", "a.css", &same, &varied);
}

#[test]
fn bounce_easing() {
    fires(
        "bounce-easing",
        "a.css",
        ".a { transition-timing-function: cubic-bezier(0.68, -0.55, 0.27, 1.55); }",
        ".a { transition-timing-function: cubic-bezier(0.22, 1, 0.36, 1); }",
    );
    fires(
        "bounce-easing",
        "a.tsx",
        r#"<span className="animate-bounce">↓</span>"#,
        r#"<span className="animate-spin">↻</span>"#,
    );
    let fix = fix_of(
        "bounce-easing",
        "a.css",
        ".a { transition: transform 0.3s cubic-bezier(0.68, -0.55, 0.27, 1.55); }",
    );
    assert_eq!(
        fix.new,
        ".a { transition: transform 0.3s cubic-bezier(0.22, 1, 0.36, 1); }"
    );
}

#[test]
fn radial_halo() {
    fires(
        "radial-halo",
        "a.css",
        ".bg { background: radial-gradient(circle, rgba(14, 165, 233, 0.5), transparent 70%); }",
        ".bg { background: radial-gradient(circle, #ffffff, #eeeeee); }",
    );
}

#[test]
fn repeating_stripes_gradient() {
    fires(
        "repeating-stripes-gradient",
        "a.css",
        ".s { background-image: repeating-linear-gradient(45deg, #000 0 2px, transparent 2px 8px); }",
        ".s { background-image: linear-gradient(#fff, #f5f5f5); }",
    );
}

#[test]
fn centered_everything() {
    let p = |class: &str| format!("<p class=\"{class}\">A short line of copy.</p>\n");
    let all: String = (0..6).map(|_| p("text-center")).collect();
    // Six centred stat tiles beside paragraphs left at the default alignment.
    let tiles: String = (0..6)
        .map(|_| "<div class=\"text-center\"><h3>42</h3><span>Users</span></div>\n".to_string())
        .chain((0..8).map(|_| "<p>Body copy that reads left aligned.</p>\n".to_string()))
        .collect();
    fires("centered-everything", "a.html", &all, &tiles);
    // Centring inherited from a wrapper counts; one centred paragraph among many does not.
    let wrapped = format!(
        "<main class=\"text-center\">{}</main>",
        (0..5).map(|_| p("")).collect::<String>()
    );
    assert!(ids("a.html", &wrapped).contains(&"centered-everything"));
    let one: String = [p("text-center")]
        .into_iter()
        .chain((0..5).map(|_| p("")))
        .collect();
    assert!(!ids("a.html", &one).contains(&"centered-everything"));
}

#[test]
fn inconsistent_scale() {
    let radii = ".a{border-radius:2px}.b{border-radius:4px}.c{border-radius:6px}.d{border-radius:8px}.e{border-radius:12px}";
    let few = ".a{border-radius:4px}.b{border-radius:8px}.c{border-radius:4px}";
    fires("inconsistent-scale", "a.css", radii, few);
    let sizes = r#"<p class="text-xs"></p><p class="text-sm"></p><p class="text-base"></p><p class="text-lg"></p><p class="text-xl"></p><p class="text-2xl"></p><p class="text-3xl"></p><p class="text-4xl"></p>"#;
    let scale = r#"<p class="text-sm"></p><p class="text-base"></p><p class="text-2xl"></p>"#;
    fires("inconsistent-scale", "a.html", sizes, scale);
}

#[test]
fn every_rule_has_a_fixture() {
    let source = include_str!("tests.rs");
    for rule in &RULES {
        assert!(
            source.contains(&format!("\"{}\"", rule.id)),
            "{} has no fixture",
            rule.id
        );
    }
    let mut ids: Vec<&str> = RULES.iter().map(|r| r.id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), RULES.len(), "rule ids are unique");
}

#[test]
fn comments_are_not_checked() {
    assert!(ids("a.css", "/* .x { font-size: 8px; } */").is_empty());
    assert!(ids("a.html", "<!-- <img src=\"x.png\"> -->").is_empty());
    assert!(ids("a.tsx", "// <img src=\"x.png\" />\nconst a = 1;").is_empty());
}

#[test]
fn findings_carry_rule_severity_place_and_words() {
    let (found, _) = check_source("src/Card.tsx", "const a = 1;\n<img src=\"/a.png\" />\n");
    let f = found
        .iter()
        .find(|f| f.rule == "img-missing-alt")
        .expect("finding");
    assert_eq!(f.severity, DesignSeverity::Error);
    assert_eq!(f.origin, RuleOrigin::Shodh);
    assert_eq!(
        (f.path.as_str(), f.line, f.end_line),
        ("src/Card.tsx", 2, 2)
    );
    assert!(f.explanation.contains("screen reader"));
    assert!(!f.fix.is_empty());
    assert!(f.evidence.contains("<img"));
}

#[test]
fn a_fix_is_offered_only_for_a_line_found_once() {
    let twice = ".a {\n  font-size: 10px;\n}\n.b {\n  font-size: 10px;\n}\n";
    let found = check_source("a.css", twice).0;
    assert_eq!(found.iter().filter(|f| f.rule == "tiny-text").count(), 2);
    assert!(
        found.iter().all(|f| f.replace.is_none()),
        "the edit could land on either line"
    );
}

#[test]
fn the_check_is_deterministic() {
    let text = include_str!("fixtures/slop.html");
    let first = check_source("slop.html", text);
    for _ in 0..5 {
        assert_eq!(check_source("slop.html", text), first);
    }
    assert!(
        first.0.len() >= 8,
        "the fixture is full of slop: {:?}",
        first.0
    );
    // Worst first, then by place.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("slop.html"), text).unwrap();
    std::fs::write(dir.path().join("b.css"), ".x { font-size: 9px; }\n").unwrap();
    let a = check_files(dir.path(), &["slop.html".into(), "b.css".into()]);
    let b = check_files(
        dir.path(),
        &["b.css".into(), "slop.html".into(), "slop.html".into()],
    );
    assert_eq!(a, b);
    assert_eq!(a.files_checked, 2);
    assert!(a
        .findings
        .windows(2)
        .all(|w| w[0].severity <= w[1].severity));
    assert_eq!(a.total(), a.findings.len() + a.left_out);
}

#[test]
fn checks_are_bounded_and_skip_what_they_must_not_read() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
    std::fs::write(
        root.join("node_modules/pkg/a.css"),
        ".x { font-size: 9px; }",
    )
    .unwrap();
    std::fs::write(root.join("app.tsx"), "<img src=\"a.png\" />").unwrap();
    std::fs::write(root.join("app.min.css"), ".x{font-size:9px}").unwrap();
    std::fs::write(root.join("notes.md"), "Lorem ipsum").unwrap();
    std::fs::write(root.join("big.css"), "a{}".repeat(200_000)).unwrap();
    let files = ui_files(root, ".");
    assert_eq!(files, vec!["app.tsx".to_string(), "big.css".to_string()]);
    let report = check_files(root, &files);
    assert_eq!(report.files_checked, 1);
    assert_eq!(
        report.skipped.len(),
        1,
        "the large file is skipped, and says why"
    );
    assert_eq!(ui_files(root, "app.tsx"), vec!["app.tsx".to_string()]);
    // Many findings of one rule in one file are capped and counted.
    let many = "<img src=\"a.png\">\n".repeat(MAX_PER_RULE + 5);
    let (found, left) = check_source("m.html", &many);
    assert_eq!(
        found.iter().filter(|f| f.rule == "img-missing-alt").count(),
        MAX_PER_RULE
    );
    assert_eq!(left.error, 5);
}

#[test]
fn ported_rules_keep_impeccables_words() {
    // Word for word from impeccable's registry.
    assert_eq!(
        rule("side-tab").unwrap().description,
        "Thick colored border on one side of a card — the most recognizable tell of AI-generated UIs. Use a subtler accent or remove it entirely."
    );
    let ported = RULES
        .iter()
        .filter(|r| r.origin == RuleOrigin::Impeccable)
        .count();
    assert!(ported >= 25, "{ported} rules ported");
    assert_eq!(rules::IMPECCABLE_COMMIT.len(), 40);
}

// ─── Fix round on #159 ───────────────────────────────────────────────────────

/// H1: a `/*` or `//` inside a string, an attribute, a template literal, a
/// regular expression or a URL never starts a comment, so the rules after it
/// still fire.
#[test]
fn comment_markers_inside_strings_do_not_silence_the_file() {
    let tsx = r#"const icons = import.meta.glob('./icons/*.svg');
const api = () => fetch('/api/*');
const marker = '<!--';
const glob = `src/**/*.tsx`;
const re = /a\/*b/;
export const Upload = () => (
  <div>
    <input type="file" accept="image/*" onChange={pick} />
    <img src={preview} />
    <p className="text-[10px]">Lorem ipsum</p>
    <a href="https://example.com/*">docs</a>
  </div>
);
"#;
    let found = ids("upload.tsx", tsx);
    for rule in [
        "control-missing-label",
        "img-missing-alt",
        "tiny-text",
        "placeholder-text",
    ] {
        assert!(found.contains(&rule), "{rule} went silent: {found:?}");
    }
    // Inside a `<script>` of a page, and in a CSS string.
    let html = r#"<script>const g = "/*";</script><img src="a.png">"#;
    assert!(ids("a.html", html).contains(&"img-missing-alt"));
    let css = ".a::before { content: \"/*\"; }\n.b { font-size: 9px; }";
    assert!(ids("a.css", css).contains(&"tiny-text"));
    // Real comments are still blanked.
    assert!(!ids("a.tsx", "const a = 1; /* <img src=\"x.png\" /> */").contains(&"img-missing-alt"));
    assert!(!ids("a.tsx", "{/* <img src=\"x.png\" /> */}").contains(&"img-missing-alt"));
}

/// L12: `//` in element text is text, not a comment.
#[test]
fn double_slash_in_element_text_is_not_a_comment() {
    let found = ids("a.tsx", "<p><code> // note</code><img src=\"a.png\" /></p>");
    assert!(found.contains(&"img-missing-alt"), "{found:?}");
    assert!(!ids("a.tsx", "  // <img src=\"a.png\" />\n").contains(&"img-missing-alt"));
}

/// H2: every way a small size is written.
#[test]
fn tiny_text_reads_every_way_a_size_is_written() {
    let css = |value: &str| format!(".a {{ font-size: {value}; }}");
    for small in [
        "0.7em",
        "70%",
        "10px !important",
        "x-small",
        "xx-small",
        "clamp(9px, 1vw, 11px)",
        "min(10px, 2vw)",
        "max(10px, 11px)",
        "0.6rem",
    ] {
        assert!(ids("a.css", &css(small)).contains(&"tiny-text"), "{small}");
    }
    for fine in [
        "0.75em",
        "75%",
        "12px !important",
        "small",
        "clamp(12px, 2vw, 16px)",
        "max(10px, 2vw)",
        "calc(0.5rem + 2px)",
        "var(--size)",
        "smaller",
    ] {
        assert!(!ids("a.css", &css(fine)).contains(&"tiny-text"), "{fine}");
    }
    fires(
        "tiny-text",
        "a.css",
        "body { font: 600 10px/1.2 system-ui; }",
        "body { font: 600 14px/1.4 system-ui; }",
    );
    for (bad, good) in [
        ("text-[10px]/4", "text-[12px]/4"),
        ("text-[11px]/[14px]", "text-sm/[14px]"),
        ("text-[0.65em]", "text-[0.8em]"),
        ("text-[length:10px]", "text-[length:12px]"),
        ("text-[10px]!", "text-xs!"),
    ] {
        fires(
            "tiny-text",
            "a.tsx",
            &format!(r#"<span className="{bad}">x</span>"#),
            &format!(r#"<span className="{good}">x</span>"#),
        );
    }
    fires(
        "tiny-text",
        "a.tsx",
        "<p style={{ fontSize: '0.7em' }}>x</p>",
        "<p style={{ fontSize: '1em' }}>x</p>",
    );
    // The fixes keep the rest of the value or class.
    assert_eq!(
        fix_of("tiny-text", "a.css", ".a { font-size: 10px !important; }").new,
        ".a { font-size: 12px !important; }"
    );
    assert_eq!(
        fix_of("tiny-text", "a.css", ".a { font-size: 0.7em; }").new,
        ".a { font-size: 0.75em; }"
    );
    assert_eq!(
        fix_of("tiny-text", "a.css", "b { font: 600 10px/1.2 system-ui; }").new,
        "b { font: 600 12px/1.2 system-ui; }"
    );
    assert_eq!(
        fix_of(
            "tiny-text",
            "a.tsx",
            "<i className=\"text-[10px]/4\">x</i>\n"
        )
        .new,
        "<i className=\"text-xs/4\">x</i>"
    );
}

/// H3: the sup/sub exemption is the element, never a class that starts so.
#[test]
fn only_the_sup_and_sub_elements_are_small_print() {
    for selector in [
        ".card-subtitle",
        ".subtle-note",
        ".support-link",
        "#subscribe small",
        ".submenu a",
    ] {
        let css = format!("{selector} {{ font-size: 10px; }}");
        assert!(ids("a.css", &css).contains(&"tiny-text"), "{selector}");
    }
    for selector in ["sup", "sub", ".note sub", "p > sup.ref"] {
        let css = format!("{selector} {{ font-size: 10px; }}");
        assert!(!ids("a.css", &css).contains(&"tiny-text"), "{selector}");
    }
}

/// M9: backdrops, handlers that only stop the event, hidden and
/// presentational elements are not clickable controls; the rule is a warning.
#[test]
fn non_semantic_button_skips_what_is_not_a_control() {
    for fine in [
        r#"<div className="panel" onClick={e => e.stopPropagation()}>Form</div>"#,
        r#"<div className="panel" onClick={(e: React.MouseEvent) => { e.stopPropagation(); }}>Form</div>"#,
        r#"<div className="fixed inset-0 bg-black/40" onClick={close} />"#,
        r#"<div aria-hidden="true" onClick={close}>x</div>"#,
        r#"<span role="presentation" onClick={close}>x</span>"#,
    ] {
        assert!(
            !ids("a.tsx", fine).contains(&"non-semantic-button"),
            "{fine}"
        );
    }
    assert_eq!(
        rule("non-semantic-button").unwrap().severity,
        DesignSeverity::Warning
    );
}

/// M6: the taste rules never outrank a user's request: advisory.
#[test]
fn taste_rules_are_advisory() {
    for id in ["gradient-text", "ai-color-palette", "overused-font"] {
        assert_eq!(rule(id).unwrap().severity, DesignSeverity::Advisory, "{id}");
    }
}

/// L1: findings in JSX text sit on their own line, past `{…}` expressions.
#[test]
fn text_findings_keep_their_lines_past_expressions() {
    let tsx = "<p>\n  {user.name}\n  {' '}Your text goes here\n</p>\n";
    let (found, _) = check_source("a.tsx", tsx);
    let filler = found
        .iter()
        .find(|f| f.rule == "placeholder-text")
        .expect("filler");
    assert_eq!(filler.line, 3);
    let html = "<p>one &mdash; two</p>\n<p>Supercharge your team</p>\n";
    let (found, _) = check_source("a.html", html);
    let buzz = found
        .iter()
        .find(|f| f.rule == "marketing-buzzword")
        .expect("buzzword");
    assert_eq!(buzz.line, 2);
}

/// L2: two fixes on one line become one edit of that line.
#[test]
fn fixes_on_one_line_are_merged() {
    let line = "<button className=\"outline-none text-[10px]\">Go</button>\n";
    let (found, _) = check_source("a.tsx", line);
    let fixes: Vec<&DesignReplacement> = found.iter().filter_map(|f| f.replace.as_ref()).collect();
    assert_eq!(fixes.len(), 2);
    let merged = "<button className=\"outline-none focus-visible:ring-2 focus-visible:ring-offset-2 text-xs\">Go</button>";
    assert!(fixes.iter().all(|f| f.new == merged), "{fixes:?}");
}

/// L2: a fix applies only while its line is in the file exactly once.
#[test]
fn a_fix_applies_only_to_the_file_as_checked() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.css");
    std::fs::write(&file, ".a {\n  font-size: 10px;\n}\n").unwrap();
    assert!(fix_applies(dir.path(), "a.css", "  font-size: 10px;").unwrap());
    std::fs::write(
        &file,
        ".a {\n  font-size: 10px;\n}\n.b {\n  font-size: 10px;\n}\n",
    )
    .unwrap();
    assert!(
        !fix_applies(dir.path(), "a.css", "  font-size: 10px;").unwrap(),
        "now twice"
    );
    std::fs::write(&file, ".a { font-size: 12px; }\n").unwrap();
    assert!(
        !fix_applies(dir.path(), "a.css", "  font-size: 10px;").unwrap(),
        "gone"
    );
    assert!(fix_applies(dir.path(), "../a.css", "x").is_err());
    assert!(fix_applies(dir.path(), ".env", "x").is_err());
}

/// L4: the byte budget is never passed, even by one file.
#[test]
fn the_byte_budget_holds() {
    let dir = tempfile::tempdir().unwrap();
    let chunk = ".a{}\n".repeat(100_000); // 500 KB
    let mut paths = Vec::new();
    for i in 0..17 {
        let name = format!("f{i:02}.css");
        std::fs::write(dir.path().join(&name), &chunk).unwrap();
        paths.push(name);
    }
    let report = check_files(dir.path(), &paths);
    assert!(report.partial);
    assert!(report.files_checked as u64 * chunk.len() as u64 <= MAX_TOTAL_BYTES);
    assert_eq!(report.files_checked, 16);
    // Under the budget: every file, not partial.
    let report = check_files(dir.path(), &paths[..3]);
    assert!(!report.partial);
    assert_eq!(report.files_checked, 3);
}

/// L8: lucide's `<Image>` icon has no source and is not an image.
#[test]
fn an_icon_component_named_image_is_not_an_image() {
    fires(
        "img-missing-alt",
        "a.tsx",
        r#"<Image src="/hero.png" width={800} height={400} />"#,
        r#"<Image className="w-4 h-4" />"#,
    );
}

/// L9: tests, stories and fixtures are not the product's interface.
#[test]
fn tests_stories_and_fixtures_are_left_out_of_folders() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for path in [
        "src/Card.tsx",
        "src/Card.test.tsx",
        "src/Card.spec.tsx",
        "src/Card.stories.tsx",
        "src/fixtures/page.html",
        "src/__tests__/x.tsx",
    ] {
        std::fs::create_dir_all(root.join(path).parent().unwrap()).unwrap();
        std::fs::write(root.join(path), "<p>Lorem ipsum</p>").unwrap();
    }
    assert_eq!(ui_files(root, "."), vec!["src/Card.tsx".to_string()]);
    // Asked for by name, it is still checked.
    assert_eq!(
        ui_files(root, "src/Card.test.tsx"),
        vec!["src/Card.test.tsx".to_string()]
    );
}

/// L10: real UI copy that names a placeholder is not filler.
#[test]
fn copy_that_names_a_placeholder_is_not_filler() {
    for copy in ["<label>Placeholder text</label>", "<p>Sample text size</p>"] {
        assert!(!ids("a.html", copy).contains(&"placeholder-text"), "{copy}");
    }
    assert!(ids("a.html", "<p>Insert content here</p>").contains(&"placeholder-text"));
}

/// L11: near-white gray on a coloured fill is the recommended fix, not a flaw.
#[test]
fn near_white_gray_on_colour_is_fine() {
    for shade in [50, 100, 200] {
        let tag = format!(r#"<p className="text-gray-{shade} bg-blue-600">Hi</p>"#);
        assert!(!ids("a.tsx", &tag).contains(&"gray-on-color"), "{shade}");
    }
    assert!(ids(
        "a.tsx",
        r#"<p className="text-gray-400 bg-blue-600">Hi</p>"#
    )
    .contains(&"gray-on-color"));
}

/// M8: of the files an agent wrote, only UI files outside build, dependency
/// and test folders, that still exist.
#[test]
fn written_files_skip_build_output_and_tests() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for path in [
        "src/App.tsx",
        "dist/assets/index-abc.css",
        "build/a.html",
        "out/b.html",
        "coverage/c.html",
        "src/App.test.tsx",
        "src/lib.rs",
    ] {
        std::fs::create_dir_all(root.join(path).parent().unwrap()).unwrap();
        std::fs::write(root.join(path), "x").unwrap();
    }
    let absolute = root.join("src/App.tsx").display().to_string();
    let written: Vec<String> = [
        absolute.as_str(),
        "./src/App.tsx",
        "dist/assets/index-abc.css",
        "build/a.html",
        "out/b.html",
        "coverage/c.html",
        "src/App.test.tsx",
        "src/lib.rs",
        "src/Gone.tsx",
        "../outside.tsx",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(
        written_ui_files(root, &written),
        vec!["src/App.tsx".to_string()]
    );
}

// ─── Exact parsing, Tailwind contrast, merged CSS rules ──────────────────────

/// Strings, template literals, regexes and JSX text come from the grammar:
/// nothing in a string is a comment or a tag, and code between components is
/// never element text.
#[test]
fn the_parser_tells_strings_comments_and_jsx_text_apart() {
    let tsx = r#"const html = "<img src='x.png'>";
const note = `<!-- ${a} -->`;
export function A() {
  return <p className="x">Lorem ipsum</p>;
}
const notText = "Supercharge your workflow";
export function B() {
  return <div>Plain words</div>;
}
"#;
    let found = ids("a.tsx", tsx);
    assert!(
        !found.contains(&"img-missing-alt"),
        "a tag inside a string is not a tag: {found:?}"
    );
    assert!(found.contains(&"placeholder-text"));
    assert!(
        !found.contains(&"marketing-buzzword"),
        "a string is not element text: {found:?}"
    );
    // Vue and Svelte templates, with their script and style parsed by their own grammars.
    let vue = "<template><img :src=\"s\"></template>\n<script setup lang=\"ts\">const g = '/* not a comment';</script>\n<style>.a { font-size: 9px; }</style>\n";
    let found = ids("a.vue", vue);
    assert!(
        found.contains(&"img-missing-alt") && found.contains(&"tiny-text"),
        "{found:?}"
    );
    let svelte = "<script>let n = 0;</script>\n<button>{n} 🗑️</button>\n<p>Your text goes here</p>";
    assert!(ids("a.svelte", svelte).contains(&"placeholder-text"));
    // SCSS `//` comments, never inside a string.
    let scss = "// .x { font-size: 8px; }\n.a::after { content: '//'; }\n.b { font-size: 9px; }";
    let found = check_source("a.scss", scss).0;
    let tiny: Vec<usize> = found
        .iter()
        .filter(|f| f.rule == "tiny-text")
        .map(|f| f.line)
        .collect();
    assert_eq!(tiny, vec![3]);
}

/// The parse has the code map's budget, and minified files are not read.
#[test]
fn minified_files_are_skipped_with_the_reason() {
    let dir = tempfile::tempdir().unwrap();
    let line = ".a{font-size:9px}".repeat(2_000);
    std::fs::write(dir.path().join("bundle.css"), &line).unwrap();
    std::fs::write(dir.path().join("ok.css"), ".a { font-size: 9px; }\n").unwrap();
    let report = check_files(dir.path(), &["bundle.css".into(), "ok.css".into()]);
    assert_eq!(report.files_checked, 1);
    assert!(report
        .skipped
        .iter()
        .any(|s| s.path == "bundle.css" && s.reason.contains("minified")));
    assert_eq!(crate::code_map::PARSE_BUDGET.as_secs(), 2);
}

/// Limit 2: Tailwind text and background classes on one element, against
/// Tailwind's palette and opacity modifiers, at WCAG's 4.5:1 and 3:1.
#[test]
fn tailwind_colour_pairs_meet_wcag() {
    fires(
        "low-contrast",
        "a.tsx",
        r#"<p className="text-gray-400 bg-white">Note</p>"#,
        r#"<p className="text-gray-700 bg-white">Note</p>"#,
    );
    fires(
        "low-contrast",
        "a.tsx",
        r#"<p className="text-black/40 bg-white">Note</p>"#,
        r#"<p className="text-black/90 bg-white">Note</p>"#,
    );
    fires(
        "low-contrast",
        "a.html",
        r#"<p class="text-white bg-yellow-400">Sale</p>"#,
        r#"<p class="text-white bg-blue-700">Sale</p>"#,
    );
    // gray-500 on white is 4.8:1: fine for body text; gray-400 is 2.5:1 even when large.
    assert!(
        !ids("a.tsx", r#"<p className="text-gray-500 bg-white">x</p>"#).contains(&"low-contrast")
    );
    // Large text needs 3:1: amber-600 on white (3.2:1) passes at text-2xl, fails at body size.
    assert!(!ids(
        "a.tsx",
        r#"<h2 className="text-2xl text-amber-600 bg-white">x</h2>"#
    )
    .contains(&"low-contrast"));
    assert!(
        ids("a.tsx", r#"<p className="text-amber-600 bg-white">x</p>"#).contains(&"low-contrast")
    );
    // What cannot be resolved is not judged: a see-through background, a
    // variable, a colour behind a variant, an unknown name.
    for fine in [
        r#"<p className="text-gray-400 bg-white/50">x</p>"#,
        r#"<p className="text-[var(--ink)] bg-white">x</p>"#,
        r#"<p className="dark:text-gray-400 bg-white">x</p>"#,
        r#"<p className="text-brand-500 bg-white">x</p>"#,
    ] {
        assert!(!ids("a.tsx", fine).contains(&"low-contrast"), "{fine}");
    }
    // Arbitrary colours resolve.
    assert!(ids(
        "a.tsx",
        r#"<p className="text-[#bbbbbb] bg-[#ffffff]">x</p>"#
    )
    .contains(&"low-contrast"));
}

/// Limit 2: the project's own Tailwind colours, when they are plain literals.
#[test]
fn tailwind_config_colours_are_read() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("tailwind.config.js"),
        "module.exports = { theme: { extend: { colors: { brand: { DEFAULT: '#2563eb', pale: '#c7d2fe' }, ink: \"#111111\", glow: ({ theme }) => theme('x') } } } };\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("a.tsx"),
        "<p className=\"text-brand-pale bg-white\">x</p>\n<p className=\"text-ink bg-white\">y</p>\n<p className=\"text-glow bg-white\">z</p>\n",
    )
    .unwrap();
    let context = Context::of_project(dir.path());
    assert!(context.colors.contains_key("brand") && context.colors.contains_key("brand-pale"));
    assert!(
        !context.colors.contains_key("glow"),
        "a function is not a literal"
    );
    let report = check_files(dir.path(), &["a.tsx".into()]);
    let lines: Vec<usize> = report
        .findings
        .iter()
        .filter(|f| f.rule == "low-contrast")
        .map(|f| f.line)
        .collect();
    assert_eq!(
        lines,
        vec![1],
        "brand-pale on white is low, ink is fine, glow is unknown"
    );
}

/// Limit 3: rules with the same selector in one file are one box.
#[test]
fn side_tab_reads_every_rule_of_its_selector_in_the_file() {
    fires(
        "side-tab",
        "a.css",
        ".card { border-left: 4px solid #3b82f6; }\n.card { border-radius: 12px; }",
        ".card { border-left: 4px solid #3b82f6; }\n.other { border-radius: 12px; }",
    );
    assert!(ids(
        "a.css",
        ".card, .panel { border-radius: 12px; }\n.card { border-left: 4px solid #3b82f6; }"
    )
    .contains(&"side-tab"));
    // A later rule that squares the box wins, as in the cascade.
    assert!(!ids("a.css", ".card { border-radius: 12px; border-left: 4px solid #3b82f6; }\n.card { border-radius: 0; }").contains(&"side-tab"));
    // In a page's <style> too.
    let html =
        "<style>.card { border-left: 4px solid #3b82f6; }\n.card { border-radius: 12px; }</style>";
    assert!(ids("a.html", html).contains(&"side-tab"));
}
