//! Structure from markup sources: LaTeX and Markdown are turned into the same
//! block model as PDFs (headings, paragraphs, theorems, equations, tables,
//! figures, list items, code, bibliography entries) without page numbers.

use std::sync::LazyLock;

use regex::Regex;

use super::document_model::{collapse_ws, Block, BlockKind, StructuredDocument};

// ── LaTeX ───────────────────────────────────────────────────────────────────

static RE_TEX_SECTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^\\(part|chapter|section|subsection|subsubsection|paragraph)\*?\s*(?:\[[^\]]*\])?\s*\{",
    )
    .expect("static regex")
});
static RE_TEX_BEGIN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\\begin\{([A-Za-z*]+)\}").expect("static regex"));
static RE_TEX_INLINE_CMD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\\(?:textbf|textit|emph|texttt|textsc|underline|mathrm|text)\{([^{}]*)\}")
        .expect("static regex")
});
static RE_TEX_REF_CMD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\(?:label|index)\{[^{}]*\}").expect("static regex"));
static RE_TEX_CITE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\\(?:cite[pt]?|citep|citet|ref|eqref|autoref|cref|Cref)\{([^{}]*)\}")
        .expect("static regex")
});

const THEOREM_ENVS: &[(&str, &str)] = &[
    ("theorem", "Theorem"),
    ("lemma", "Lemma"),
    ("proposition", "Proposition"),
    ("corollary", "Corollary"),
    ("claim", "Claim"),
    ("conjecture", "Conjecture"),
    ("assumption", "Assumption"),
    ("remark", "Remark"),
    ("example", "Example"),
    ("hypothesis", "Hypothesis"),
    ("thm", "Theorem"),
    ("lem", "Lemma"),
    ("prop", "Proposition"),
    ("cor", "Corollary"),
];
const DEFINITION_ENVS: &[&str] = &["definition", "defn", "def"];
const EQUATION_ENVS: &[&str] = &[
    "equation",
    "equation*",
    "align",
    "align*",
    "gather",
    "gather*",
    "multline",
    "multline*",
    "eqnarray",
    "eqnarray*",
    "displaymath",
    "math",
];
const CODE_ENVS: &[&str] = &[
    "verbatim",
    "lstlisting",
    "minted",
    "Verbatim",
    "algorithmic",
];

/// Remove `%` comments (not `\%`).
fn strip_tex_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'%' && (i == 0 || bytes[i - 1] != b'\\') {
            return &line[..i];
        }
    }
    line
}

/// Content of the brace group starting at `open` (index of `{`), honouring
/// nesting. Returns the content and the index after the closing brace.
fn brace_group(text: &str, open: usize) -> Option<(String, usize)> {
    let mut depth = 0usize;
    let mut start = None;
    for (i, c) in text[open..].char_indices() {
        let i = open + i;
        match c {
            '{' => {
                depth += 1;
                if depth == 1 {
                    start = Some(i + 1);
                }
            }
            '}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some((text[start?..i].to_string(), i + 1));
                }
            }
            _ => {}
        }
    }
    None
}

/// Light cleanup of inline LaTeX for retrieval text: keep the argument of
/// formatting commands, render citations/references as `[key]`, drop labels.
fn clean_tex_inline(text: &str) -> String {
    let mut s = RE_TEX_REF_CMD.replace_all(text, "").to_string();
    for _ in 0..3 {
        let next = RE_TEX_INLINE_CMD.replace_all(&s, "$1").to_string();
        if next == s {
            break;
        }
        s = next;
    }
    s = RE_TEX_CITE.replace_all(&s, "[$1]").to_string();
    s = s.replace("~", " ").replace("\\\\", " ");
    collapse_ws(&s)
}

/// Parse LaTeX source into blocks. Only the document body is read when a
/// `\begin{document}` is present.
pub fn parse_latex(source: &str) -> StructuredDocument {
    let body = match source.find("\\begin{document}") {
        Some(start) => {
            let rest = &source[start + "\\begin{document}".len()..];
            match rest.find("\\end{document}") {
                Some(end) => &rest[..end],
                None => rest,
            }
        }
        None => source,
    };
    let lines: Vec<&str> = body.lines().map(strip_tex_comment).collect();
    let mut blocks: Vec<Block> = Vec::new();
    let mut paragraph: Vec<String> = Vec::new();

    let flush = |paragraph: &mut Vec<String>, blocks: &mut Vec<Block>| {
        let text = clean_tex_inline(&paragraph.join(" "));
        paragraph.clear();
        if !text.is_empty() {
            blocks.push(Block::new(BlockKind::Paragraph, text));
        }
    };

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim();
        if line.is_empty() {
            flush(&mut paragraph, &mut blocks);
            i += 1;
            continue;
        }
        if let Some(c) = RE_TEX_SECTION.captures(line) {
            flush(&mut paragraph, &mut blocks);
            let level = match &c[1] {
                "part" | "chapter" => 1,
                "section" => 1,
                "subsection" => 2,
                "subsubsection" => 3,
                _ => 4,
            };
            let open = c.get(0).map(|m| m.end() - 1).unwrap_or(0);
            let (title, after) = brace_group(line, open).unwrap_or_default();
            blocks.push(Block::new(
                BlockKind::Heading { level },
                clean_tex_inline(&title),
            ));
            let rest = line.get(after..).unwrap_or("").trim();
            if !rest.is_empty() {
                paragraph.push(rest.to_string());
            }
            i += 1;
            continue;
        }
        if line.starts_with("\\bibitem") {
            flush(&mut paragraph, &mut blocks);
            let mut entry = vec![line.to_string()];
            i += 1;
            while i < lines.len() {
                let next = lines[i].trim();
                if next.is_empty() || next.starts_with("\\bibitem") || next.starts_with("\\end{") {
                    break;
                }
                entry.push(next.to_string());
                i += 1;
            }
            let joined = entry.join(" ");
            let without_cmd = joined.trim_start_matches("\\bibitem");
            let without_key = match without_cmd.trim_start().strip_prefix('[') {
                Some(rest) => rest.split_once(']').map(|(_, r)| r).unwrap_or(rest),
                None => without_cmd,
            };
            let without_key = match without_key.trim_start().strip_prefix('{') {
                Some(rest) => rest.split_once('}').map(|(_, r)| r).unwrap_or(rest),
                None => without_key,
            };
            blocks.push(Block::new(
                BlockKind::ReferenceEntry,
                clean_tex_inline(without_key),
            ));
            continue;
        }
        if let Some(c) = RE_TEX_BEGIN.captures(line) {
            let env = c[1].to_string();
            let is_list = matches!(env.as_str(), "itemize" | "enumerate" | "description");
            let known = is_list
                || env == "figure"
                || env == "figure*"
                || env == "table"
                || env == "table*"
                || env == "abstract"
                || env == "proof"
                || env == "thebibliography"
                || EQUATION_ENVS.contains(&env.as_str())
                || CODE_ENVS.contains(&env.as_str())
                || DEFINITION_ENVS.contains(&env.as_str())
                || THEOREM_ENVS.iter().any(|(e, _)| *e == env);
            if !known {
                paragraph.push(line.to_string());
                i += 1;
                continue;
            }
            flush(&mut paragraph, &mut blocks);
            if env == "thebibliography" {
                blocks.push(Block::new(BlockKind::Heading { level: 1 }, "References"));
                i += 1;
                continue;
            }
            if env == "abstract" {
                blocks.push(Block::new(BlockKind::Heading { level: 1 }, "Abstract"));
                i += 1;
                continue;
            }
            // Collect the environment body up to the matching \end{env}.
            let end_tag = format!("\\end{{{env}}}");
            let mut inner: Vec<String> = Vec::new();
            let first_rest = line[c.get(0).map(|m| m.end()).unwrap_or(0)..].to_string();
            if !first_rest.trim().is_empty() {
                inner.push(first_rest);
            }
            i += 1;
            while i < lines.len() && !lines[i].contains(&end_tag) {
                inner.push(lines[i].to_string());
                i += 1;
            }
            if i < lines.len() {
                let last = lines[i];
                if let Some(pos) = last.find(&end_tag) {
                    let before = &last[..pos];
                    if !before.trim().is_empty() {
                        inner.push(before.to_string());
                    }
                }
                i += 1;
            }
            push_tex_environment(&env, &inner, &mut blocks);
            continue;
        }
        if line.starts_with("\\end{") {
            flush(&mut paragraph, &mut blocks);
            i += 1;
            continue;
        }
        if line.starts_with("\\[") || line.starts_with("$$") {
            flush(&mut paragraph, &mut blocks);
            let closer = if line.starts_with("\\[") { "\\]" } else { "$$" };
            let mut eq = vec![line.to_string()];
            let closed_inline = line.len() > 2 && line[2..].contains(closer);
            i += 1;
            if !closed_inline {
                while i < lines.len() {
                    let l = lines[i];
                    eq.push(l.to_string());
                    i += 1;
                    if l.contains(closer) {
                        break;
                    }
                }
            }
            blocks.push(Block::new(
                BlockKind::Equation,
                eq.join("\n").trim().to_string(),
            ));
            continue;
        }
        if line.starts_with("\\maketitle") || line.starts_with("\\tableofcontents") {
            i += 1;
            continue;
        }
        paragraph.push(line.to_string());
        i += 1;
    }
    flush(&mut paragraph, &mut blocks);

    // Title from the preamble, when declared.
    if let Some(pos) = source.find("\\title") {
        let open = source[pos..].find('{').map(|o| pos + o);
        if let Some((title, _)) = open.and_then(|o| brace_group(source, o)) {
            let title = clean_tex_inline(&title);
            if !title.is_empty() {
                blocks.insert(0, Block::new(BlockKind::Title, title));
            }
        }
    }

    let mut doc = StructuredDocument {
        pages: Vec::new(),
        blocks,
    };
    doc.finalize();
    doc
}

fn push_tex_environment(env: &str, inner: &[String], blocks: &mut Vec<Block>) {
    let raw = inner.join("\n");
    if EQUATION_ENVS.contains(&env) {
        let text = RE_TEX_REF_CMD.replace_all(raw.trim(), "").to_string();
        blocks.push(Block::new(BlockKind::Equation, text.trim().to_string()));
        return;
    }
    if CODE_ENVS.contains(&env) {
        blocks.push(Block::new(
            BlockKind::Code,
            raw.trim_matches('\n').to_string(),
        ));
        return;
    }
    if let Some((_, name)) = THEOREM_ENVS.iter().find(|(e, _)| *e == env) {
        let (note, body) = optional_argument(&raw);
        let label = name.to_string();
        let text = match note {
            Some(note) => format!(
                "{label} ({}). {}",
                clean_tex_inline(&note),
                clean_tex_inline(&body)
            ),
            None => format!("{label}. {}", clean_tex_inline(&body)),
        };
        blocks.push(Block::new(BlockKind::Theorem { label }, text));
        return;
    }
    if DEFINITION_ENVS.contains(&env) {
        let (note, body) = optional_argument(&raw);
        let text = match note {
            Some(note) => format!(
                "Definition ({}). {}",
                clean_tex_inline(&note),
                clean_tex_inline(&body)
            ),
            None => format!("Definition. {}", clean_tex_inline(&body)),
        };
        blocks.push(Block::new(
            BlockKind::Definition {
                label: "Definition".to_string(),
            },
            text,
        ));
        return;
    }
    if env == "proof" {
        blocks.push(Block::new(
            BlockKind::Proof,
            format!("Proof. {}", clean_tex_inline(&raw)),
        ));
        return;
    }
    if matches!(env, "itemize" | "enumerate" | "description") {
        for item in raw.split("\\item").skip(1) {
            let text = clean_tex_inline(item);
            if !text.is_empty() {
                blocks.push(Block::new(BlockKind::ListItem, text));
            }
        }
        return;
    }
    if env.starts_with("figure") {
        if let Some(caption) = tex_caption(&raw) {
            blocks.push(Block::new(
                BlockKind::Figure {
                    caption: format!("Figure: {caption}"),
                },
                String::new(),
            ));
        }
        return;
    }
    if env.starts_with("table") {
        let caption = tex_caption(&raw).map(|c| format!("Table: {c}"));
        let mut rows: Vec<Vec<String>> = Vec::new();
        if let Some(start) = raw.find("\\begin{tabular") {
            let tab = &raw[start..];
            let tab = tab.find('\n').map(|n| &tab[n + 1..]).unwrap_or("");
            let tab = tab.split("\\end{tabular").next().unwrap_or(tab);
            for row in tab.split("\\\\") {
                let row = row
                    .replace("\\hline", "")
                    .replace("\\toprule", "")
                    .replace("\\midrule", "")
                    .replace("\\bottomrule", "");
                let cells: Vec<String> = row.split('&').map(clean_tex_inline).collect();
                if cells.iter().any(|c| !c.is_empty()) {
                    rows.push(cells);
                }
            }
        }
        if rows.is_empty() {
            if let Some(caption) = caption {
                blocks.push(Block::new(BlockKind::Paragraph, caption));
            }
            return;
        }
        let header = rows.remove(0);
        blocks.push(Block::new(
            BlockKind::Table {
                header,
                rows,
                caption,
                cell_boxes: Vec::new(),
                cell_coverage: None,
            },
            String::new(),
        ));
    }
}

/// Split `[note] body` (a theorem's optional argument) from the body.
fn optional_argument(raw: &str) -> (Option<String>, String) {
    let trimmed = raw.trim_start();
    if let Some(rest) = trimmed.strip_prefix('[') {
        if let Some(end) = rest.find(']') {
            return (Some(rest[..end].to_string()), rest[end + 1..].to_string());
        }
    }
    (None, raw.to_string())
}

fn tex_caption(raw: &str) -> Option<String> {
    let pos = raw.find("\\caption")?;
    let open = raw[pos..].find('{').map(|o| pos + o)?;
    brace_group(raw, open)
        .map(|(c, _)| clean_tex_inline(&c))
        .filter(|c| !c.is_empty())
}

// ── Markdown ────────────────────────────────────────────────────────────────

static RE_MD_HEADING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(#{1,6})\s+(.+?)\s*#*\s*$").expect("static regex"));
static RE_MD_LIST: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s{0,6}(?:[-*+]|\d{1,3}[.)])\s+").expect("static regex"));

/// Parse Markdown into blocks: ATX headings, fenced code, pipe tables, list
/// items, `$$` display math and paragraphs.
pub fn parse_markdown(source: &str) -> StructuredDocument {
    let lines: Vec<&str> = source.lines().collect();
    let mut blocks: Vec<Block> = Vec::new();
    let mut paragraph: Vec<&str> = Vec::new();
    let flush = |paragraph: &mut Vec<&str>, blocks: &mut Vec<Block>| {
        let text = collapse_ws(&paragraph.join(" "));
        paragraph.clear();
        if !text.is_empty() {
            blocks.push(Block::new(BlockKind::Paragraph, text));
        }
    };
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        if trimmed.is_empty() {
            flush(&mut paragraph, &mut blocks);
            i += 1;
            continue;
        }
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            flush(&mut paragraph, &mut blocks);
            let fence = &trimmed[..3];
            let mut code = Vec::new();
            i += 1;
            while i < lines.len() && !lines[i].trim_start().starts_with(fence) {
                code.push(lines[i]);
                i += 1;
            }
            i += 1;
            blocks.push(Block::new(BlockKind::Code, code.join("\n")));
            continue;
        }
        if trimmed.starts_with("$$") {
            flush(&mut paragraph, &mut blocks);
            let mut eq = vec![trimmed];
            let closed = trimmed
                .strip_prefix("$$")
                .is_some_and(|rest| rest.contains("$$"));
            i += 1;
            if !closed {
                while i < lines.len() {
                    eq.push(lines[i].trim());
                    i += 1;
                    if lines[i - 1].contains("$$") {
                        break;
                    }
                }
            }
            blocks.push(Block::new(BlockKind::Equation, eq.join("\n")));
            continue;
        }
        if let Some(c) = RE_MD_HEADING.captures(trimmed) {
            flush(&mut paragraph, &mut blocks);
            let level = c[1].len() as u8;
            blocks.push(Block::new(BlockKind::Heading { level }, c[2].to_string()));
            i += 1;
            continue;
        }
        let is_separator = |l: &str| {
            let l = l.trim();
            l.starts_with('|') && l.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
        };
        if trimmed.starts_with('|') && i + 1 < lines.len() && is_separator(lines[i + 1]) {
            flush(&mut paragraph, &mut blocks);
            let split = |l: &str| -> Vec<String> {
                l.trim()
                    .trim_matches('|')
                    .split('|')
                    .map(|c| c.trim().to_string())
                    .collect()
            };
            let header = split(trimmed);
            let mut rows = Vec::new();
            i += 2;
            while i < lines.len() && lines[i].trim().starts_with('|') {
                rows.push(split(lines[i]));
                i += 1;
            }
            blocks.push(Block::new(
                BlockKind::Table {
                    header,
                    rows,
                    caption: None,
                    cell_boxes: Vec::new(),
                    cell_coverage: None,
                },
                String::new(),
            ));
            continue;
        }
        if RE_MD_LIST.is_match(line) {
            flush(&mut paragraph, &mut blocks);
            let mut item = vec![RE_MD_LIST.replace(line, "").to_string()];
            i += 1;
            while i < lines.len() {
                let next = lines[i];
                if next.trim().is_empty() || RE_MD_LIST.is_match(next) || !next.starts_with(' ') {
                    break;
                }
                item.push(next.trim().to_string());
                i += 1;
            }
            blocks.push(Block::new(
                BlockKind::ListItem,
                collapse_ws(&item.join(" ")),
            ));
            continue;
        }
        paragraph.push(trimmed);
        i += 1;
    }
    flush(&mut paragraph, &mut blocks);
    let mut doc = StructuredDocument {
        pages: Vec::new(),
        blocks,
    };
    doc.finalize();
    doc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latex_sections_theorems_equations_tables_and_bibliography() {
        let src = r#"
\documentclass{article}
\title{Prime Helices}
\begin{document}
\maketitle
\section{Introduction}
Primes are \emph{interesting} \cite{euler}. % a comment
They spiral.

\subsection{Setup}
\begin{theorem}[Density]\label{thm:d}
Let $p$ be prime. Then the helix is dense.
\end{theorem}
\begin{proof}
Follows from Dirichlet.
\end{proof}
\begin{equation}
\theta(p) = 2\pi p / \log p
\end{equation}
\begin{table}
\caption{Counts}
\begin{tabular}{ll}
N & primes \\ \hline
10 & 4 \\
100 & 25 \\
\end{tabular}
\end{table}
\begin{thebibliography}{9}
\bibitem{euler} L. Euler. Variae observationes. 1737.
\bibitem{dir} P. Dirichlet. Beweis. 1837.
\end{thebibliography}
\end{document}
"#;
        let doc = parse_latex(src);
        let kinds: Vec<&str> = doc.blocks.iter().map(|b| b.kind.name()).collect();
        assert_eq!(
            kinds,
            vec![
                "title",
                "heading",
                "paragraph",
                "heading",
                "theorem",
                "proof",
                "equation",
                "table",
                "heading",
                "reference_entry",
                "reference_entry"
            ]
        );
        assert_eq!(
            doc.blocks[2].text,
            "Primes are interesting [euler]. They spiral."
        );
        assert_eq!(doc.blocks[4].section_path, vec!["Introduction", "Setup"]);
        assert!(doc.blocks[4]
            .text
            .starts_with("Theorem (Density). Let $p$ be prime."));
        assert!(!doc.blocks[6].text.contains("label"));
        match &doc.blocks[7].kind {
            BlockKind::Table {
                header,
                rows,
                caption,
                ..
            } => {
                assert_eq!(header, &vec!["N".to_string(), "primes".to_string()]);
                assert_eq!(rows.len(), 2);
                assert_eq!(caption.as_deref(), Some("Table: Counts"));
            }
            other => panic!("expected table, got {other:?}"),
        }
        assert_eq!(doc.blocks[9].text, "L. Euler. Variae observationes. 1737.");
    }

    #[test]
    fn markdown_blocks() {
        let src = "# Title\n\nIntro text\ncontinues.\n\n## Part\n\n- one\n- two\n\n```rust\nfn main() {}\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n$$\nx = y\n$$\n";
        let doc = parse_markdown(src);
        let kinds: Vec<&str> = doc.blocks.iter().map(|b| b.kind.name()).collect();
        assert_eq!(
            kinds,
            vec![
                "heading",
                "paragraph",
                "heading",
                "list_item",
                "list_item",
                "code",
                "table",
                "equation"
            ]
        );
        assert_eq!(doc.blocks[1].text, "Intro text continues.");
        assert_eq!(doc.blocks[3].section_path, vec!["Title", "Part"]);
        assert_eq!(doc.blocks[5].text, "fn main() {}");
    }
}
