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
    fires(
        "emoji-icon",
        "a.html",
        "<ul><li>🚀 Fast builds</li></ul>",
        "<ul><li>Fast builds</li><li>✓ Done</li></ul>",
    );
    assert!(!ids("a.html", "<p>We shipped it 🎉</p>").contains(&"emoji-icon"));
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
    fires(
        "side-tab",
        "a.tsx",
        r#"<div className="border-l-4 border-blue-500 p-4">Note</div>"#,
        r#"<div className="border-l-2 p-4">Note</div>"#,
    );
    assert!(ids(
        "a.tsx",
        r#"<div className="rounded-lg border-l-2 p-4">Note</div>"#
    )
    .contains(&"side-tab"));
    fires(
        "side-tab",
        "a.css",
        ".card { border-left: 4px solid #3b82f6; }",
        "blockquote { border-left: 4px solid #cccccc; }",
    );
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
    let mixed: String = (0..5)
        .map(|_| p("text-center"))
        .chain([p("text-left")])
        .collect();
    fires("centered-everything", "a.html", &all, &mixed);
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
