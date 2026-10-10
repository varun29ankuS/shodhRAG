//! Display equations of a paper with ids, numbers and LaTeX.
//!
//! The LaTeX comes from one of two places, and every equation says which:
//! - **source**: the paper's own `.tex` is in the library and the equation was matched to
//!   it (by its printed number, confirmed by content, or by content alone). The source is
//!   used only when enough of the paper's equations match it, so an unrelated `.tex` in
//!   the same folder is never taken for the paper's source.
//! - **reconstructed**: rebuilt from the PDF's text layer, mapping math glyphs (Greek,
//!   operators, relations, styled mathematical letters, Unicode sub- and superscripts,
//!   combining accents) to LaTeX. Layout the text layer does not keep (fraction bars,
//!   the position of a sub- or superscript) cannot be recovered, so such an equation is a
//!   faithful transcription of its symbols rather than of its typesetting.

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

use crate::processing::document_model::{BBox, BlockKind, StructuredDocument};

/// Where an equation's LaTeX comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EquationOrigin {
    /// Rebuilt from the PDF text layer.
    Reconstructed,
    /// Taken from the paper's LaTeX source.
    Source,
}

/// A display equation as the agent and the app show it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Equation {
    /// Stable within one paper: `eq-<number>` (e.g. `eq-3`, `eq-2.1`), or
    /// `eq-p<page>-<k>` for unnumbered or repeated numbers.
    pub id: String,
    /// The number as printed ("3", "2.1", "A.4"), when the equation has one.
    pub number: Option<String>,
    /// The equation's text as extracted from the PDF, without its number.
    pub text: String,
    pub latex: String,
    pub origin: EquationOrigin,
    /// The `.tex` file the LaTeX was taken from.
    pub source_file: Option<String>,
    /// 1-based page (absent for unpaged documents).
    pub page: Option<u32>,
    pub bbox: Option<BBox>,
    /// The prose that introduces the equation (the end of the paragraph before it), in
    /// the paper's words: what claims citing the equation are checked against.
    pub intro: String,
    /// The start of the next paragraph when it explains the symbols ("where …").
    pub explanation: String,
}

impl Equation {
    /// The equation as a checkable passage: its label, the prose introducing it, its
    /// text as printed and the explanation of its symbols.
    pub fn passage(&self) -> String {
        let label = self
            .number
            .as_deref()
            .map_or_else(|| "Equation".to_string(), |n| format!("Equation ({n})"));
        let place = self.page.map(|p| format!(", page {p}")).unwrap_or_default();
        [
            self.intro.as_str(),
            self.text.as_str(),
            self.explanation.as_str(),
        ]
        .into_iter()
        .filter(|t| !t.is_empty())
        .fold(format!("{label}{place}:"), |acc, t| format!("{acc} {t}"))
    }
}

static RE_TRAILING_NUMBER: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"\(\s*([A-Z]?\d{1,3}(?:\.\d{1,3})?[a-z]?)\s*\)\s*$").ok());

/// Splits a trailing equation number off: "E = mc² (3)" → ("E = mc²", Some("3")).
pub fn split_number(text: &str) -> (String, Option<String>) {
    let trimmed = text.trim();
    if let Some(c) = RE_TRAILING_NUMBER
        .as_ref()
        .and_then(|re| re.captures(trimmed))
    {
        if let Some(whole) = c.get(0) {
            let body = trimmed[..whole.start()].trim_end().to_string();
            if !body.is_empty() {
                return (body, Some(c[1].to_string()));
            }
        }
    }
    (trimmed.to_string(), None)
}

// ------------------------------------------------------------- glyphs → LaTeX

fn symbol(c: char) -> Option<&'static str> {
    Some(match c {
        'α' => "\\alpha",
        'β' => "\\beta",
        'γ' => "\\gamma",
        'δ' => "\\delta",
        'ε' => "\\varepsilon",
        'ϵ' => "\\epsilon",
        'ζ' => "\\zeta",
        'η' => "\\eta",
        'θ' => "\\theta",
        'ϑ' => "\\vartheta",
        'ι' => "\\iota",
        'κ' => "\\kappa",
        'ϰ' => "\\varkappa",
        'λ' => "\\lambda",
        'μ' | 'µ' => "\\mu",
        'ν' => "\\nu",
        'ξ' => "\\xi",
        'ο' => "o",
        'π' => "\\pi",
        'ϖ' => "\\varpi",
        'ρ' => "\\rho",
        'ϱ' => "\\varrho",
        'σ' => "\\sigma",
        'ς' => "\\varsigma",
        'τ' => "\\tau",
        'υ' => "\\upsilon",
        'φ' => "\\varphi",
        'ϕ' => "\\phi",
        'χ' => "\\chi",
        'ψ' => "\\psi",
        'ω' => "\\omega",
        'Γ' => "\\Gamma",
        'Δ' => "\\Delta",
        'Θ' | 'ϴ' => "\\Theta",
        'Λ' => "\\Lambda",
        'Ξ' => "\\Xi",
        'Π' => "\\Pi",
        'Σ' => "\\Sigma",
        'Υ' => "\\Upsilon",
        'Φ' => "\\Phi",
        'Ψ' => "\\Psi",
        'Ω' => "\\Omega",
        'Α' => "A",
        'Β' => "B",
        'Ε' => "E",
        'Ζ' => "Z",
        'Η' => "H",
        'Ι' => "I",
        'Κ' => "K",
        'Μ' => "M",
        'Ν' => "N",
        'Ο' => "O",
        'Ρ' => "P",
        'Τ' => "T",
        'Χ' => "X",
        '∑' => "\\sum",
        '∏' => "\\prod",
        '∐' => "\\coprod",
        '∫' => "\\int",
        '∬' => "\\iint",
        '∮' => "\\oint",
        '√' => "\\sqrt",
        '∂' => "\\partial",
        '∇' => "\\nabla",
        '∞' => "\\infty",
        '≤' | '⩽' => "\\leq",
        '≥' | '⩾' => "\\geq",
        '≠' => "\\neq",
        '≈' => "\\approx",
        '≡' => "\\equiv",
        '∼' => "\\sim",
        '≃' => "\\simeq",
        '≅' => "\\cong",
        '∝' => "\\propto",
        '≪' => "\\ll",
        '≫' => "\\gg",
        '≺' => "\\prec",
        '≻' => "\\succ",
        '⪯' => "\\preceq",
        '⪰' => "\\succeq",
        '±' => "\\pm",
        '∓' => "\\mp",
        '×' => "\\times",
        '÷' => "\\div",
        '·' | '⋅' => "\\cdot",
        '∘' => "\\circ",
        '⊗' => "\\otimes",
        '⊕' => "\\oplus",
        '⊙' => "\\odot",
        '⊘' => "\\oslash",
        '∗' => "*",
        '⋆' => "\\star",
        '∈' => "\\in",
        '∉' => "\\notin",
        '∋' => "\\ni",
        '⊂' => "\\subset",
        '⊆' => "\\subseteq",
        '⊃' => "\\supset",
        '⊇' => "\\supseteq",
        '∪' => "\\cup",
        '∩' => "\\cap",
        '⋃' => "\\bigcup",
        '⋂' => "\\bigcap",
        '∖' => "\\setminus",
        '∅' => "\\emptyset",
        '∀' => "\\forall",
        '∃' => "\\exists",
        '¬' => "\\neg",
        '∧' => "\\wedge",
        '∨' => "\\vee",
        '→' => "\\to",
        '←' => "\\leftarrow",
        '↔' => "\\leftrightarrow",
        '⇒' => "\\Rightarrow",
        '⇐' => "\\Leftarrow",
        '⇔' => "\\Leftrightarrow",
        '↦' => "\\mapsto",
        '⟶' => "\\longrightarrow",
        '↑' => "\\uparrow",
        '↓' => "\\downarrow",
        '⊤' | '⊺' => "\\top",
        '⊥' => "\\perp",
        '∥' | '‖' => "\\|",
        '∣' => "\\mid",
        '⟨' | '〈' => "\\langle",
        '⟩' | '〉' => "\\rangle",
        '⌊' => "\\lfloor",
        '⌋' => "\\rfloor",
        '⌈' => "\\lceil",
        '⌉' => "\\rceil",
        '−' | '–' => "-",
        '…' => "\\ldots",
        '⋯' => "\\cdots",
        '⋮' => "\\vdots",
        '⋱' => "\\ddots",
        '′' => "'",
        '″' => "''",
        'ℓ' => "\\ell",
        'ℏ' => "\\hbar",
        'ℎ' => "h",
        '℘' => "\\wp",
        'ℜ' => "\\Re",
        'ℑ' => "\\Im",
        'ℵ' => "\\aleph",
        '°' => "^{\\circ}",
        '∠' => "\\angle",
        '△' => "\\triangle",
        '□' => "\\square",
        '∎' => "\\blacksquare",
        '%' => "\\%",
        '&' => "\\&",
        '#' => "\\#",
        '$' => "\\$",
        '{' => "\\{",
        '}' => "\\}",
        '~' => "\\sim",
        '\\' => "\\backslash",
        _ => return None,
    })
}

/// Letterlike symbols that stand in for gaps of the mathematical alphanumeric block.
fn letterlike(c: char) -> Option<(&'static str, char)> {
    Some(match c {
        'ℂ' => ("mathbb", 'C'),
        'ℍ' => ("mathbb", 'H'),
        'ℕ' => ("mathbb", 'N'),
        'ℙ' => ("mathbb", 'P'),
        'ℚ' => ("mathbb", 'Q'),
        'ℝ' => ("mathbb", 'R'),
        'ℤ' => ("mathbb", 'Z'),
        'ℬ' => ("mathcal", 'B'),
        'ℰ' => ("mathcal", 'E'),
        'ℱ' => ("mathcal", 'F'),
        'ℋ' => ("mathcal", 'H'),
        'ℐ' => ("mathcal", 'I'),
        'ℒ' => ("mathcal", 'L'),
        'ℳ' => ("mathcal", 'M'),
        'ℛ' => ("mathcal", 'R'),
        'ℭ' => ("mathfrak", 'C'),
        'ℌ' => ("mathfrak", 'H'),
        'ℨ' => ("mathfrak", 'Z'),
        _ => return None,
    })
}

const GREEK_STYLE_ORDER: [char; 58] = [
    'Α', 'Β', 'Γ', 'Δ', 'Ε', 'Ζ', 'Η', 'Θ', 'Ι', 'Κ', 'Λ', 'Μ', 'Ν', 'Ξ', 'Ο', 'Π', 'Ρ', 'ϴ', 'Σ',
    'Τ', 'Υ', 'Φ', 'Χ', 'Ψ', 'Ω', '∇', 'α', 'β', 'γ', 'δ', 'ε', 'ζ', 'η', 'θ', 'ι', 'κ', 'λ', 'μ',
    'ν', 'ξ', 'ο', 'π', 'ρ', 'ς', 'σ', 'τ', 'υ', 'φ', 'χ', 'ψ', 'ω', '∂', 'ϵ', 'ϑ', 'ϰ', 'ϕ', 'ϱ',
    'ϖ',
];

/// The plain character and LaTeX style of a mathematical alphanumeric symbol
/// (U+1D400–U+1D7FF): `("mathbf", 'x')` for 𝐱, `("", 'x')` for the italic 𝑥.
fn styled(c: char) -> Option<(&'static str, char)> {
    let code = c as u32;
    // Latin letters: 13 styles of 52 letters (A–Z, a–z).
    const LATIN_STYLES: [&str; 13] = [
        "mathbf",
        "",
        "boldsymbol",
        "mathcal",
        "mathcal",
        "mathfrak",
        "mathbb",
        "mathfrak",
        "mathsf",
        "mathbf",
        "mathsf",
        "mathbf",
        "mathtt",
    ];
    if (0x1D400..0x1D400 + 13 * 52).contains(&code) {
        let offset = code - 0x1D400;
        let style = LATIN_STYLES[(offset / 52) as usize];
        let index = offset % 52;
        let base = if index < 26 {
            char::from_u32(u32::from(b'A') + index)?
        } else {
            char::from_u32(u32::from(b'a') + index - 26)?
        };
        return Some((style, base));
    }
    match code {
        0x1D6A4 => return Some(("", 'ı')),
        0x1D6A5 => return Some(("", 'ȷ')),
        _ => {}
    }
    // Greek: 5 styles of 58 symbols from U+1D6A8.
    const GREEK_STYLES: [&str; 5] = ["boldsymbol", "", "boldsymbol", "boldsymbol", "boldsymbol"];
    if (0x1D6A8..0x1D6A8 + 5 * 58).contains(&code) {
        let offset = code - 0x1D6A8;
        let style = GREEK_STYLES[(offset / 58) as usize];
        return Some((style, GREEK_STYLE_ORDER[(offset % 58) as usize]));
    }
    // Digits: 5 styles of 10 from U+1D7CE.
    if (0x1D7CE..0x1D7CE + 50).contains(&code) {
        let offset = code - 0x1D7CE;
        let style = ["mathbf", "mathbb", "mathsf", "mathbf", "mathtt"][(offset / 10) as usize];
        return Some((style, char::from_u32(u32::from(b'0') + offset % 10)?));
    }
    None
}

fn superscript(c: char) -> Option<char> {
    Some(match c {
        '⁰' => '0',
        '¹' => '1',
        '²' => '2',
        '³' => '3',
        '⁴' => '4',
        '⁵' => '5',
        '⁶' => '6',
        '⁷' => '7',
        '⁸' => '8',
        '⁹' => '9',
        '⁺' => '+',
        '⁻' => '-',
        '⁼' => '=',
        '⁽' => '(',
        '⁾' => ')',
        'ⁿ' => 'n',
        'ⁱ' => 'i',
        'ᵀ' => 'T',
        'ᵗ' => 't',
        'ᵏ' => 'k',
        _ => return None,
    })
}

fn subscript(c: char) -> Option<char> {
    Some(match c {
        '₀' => '0',
        '₁' => '1',
        '₂' => '2',
        '₃' => '3',
        '₄' => '4',
        '₅' => '5',
        '₆' => '6',
        '₇' => '7',
        '₈' => '8',
        '₉' => '9',
        '₊' => '+',
        '₋' => '-',
        '₌' => '=',
        '₍' => '(',
        '₎' => ')',
        'ₐ' => 'a',
        'ₑ' => 'e',
        'ₒ' => 'o',
        'ₓ' => 'x',
        'ₕ' => 'h',
        'ₖ' => 'k',
        'ₗ' => 'l',
        'ₘ' => 'm',
        'ₙ' => 'n',
        'ₚ' => 'p',
        'ₛ' => 's',
        'ₜ' => 't',
        'ᵢ' => 'i',
        'ⱼ' => 'j',
        'ᵣ' => 'r',
        'ᵤ' => 'u',
        'ᵥ' => 'v',
        _ => return None,
    })
}

fn accent(c: char) -> Option<&'static str> {
    Some(match c {
        '\u{0302}' => "hat",
        '\u{0303}' => "tilde",
        '\u{0304}' | '\u{0305}' => "bar",
        '\u{0307}' => "dot",
        '\u{0308}' => "ddot",
        '\u{20D7}' => "vec",
        _ => return None,
    })
}

/// Operator names KaTeX knows as commands.
const FUNCTION_WORDS: [&str; 22] = [
    "sin", "cos", "tan", "exp", "log", "ln", "max", "min", "sup", "inf", "lim", "det", "arg",
    "deg", "dim", "ker", "tanh", "sinh", "cosh", "arcsin", "arccos", "arctan",
];
/// Words set as text inside an equation.
const TEXT_WORDS: [&str; 12] = [
    "where",
    "if",
    "otherwise",
    "for",
    "and",
    "or",
    "with",
    "all",
    "else",
    "s.t.",
    "such",
    "that",
];

/// One unit of LaTeX output: a token that a following sub/superscript or accent
/// attaches to.
fn push_atom(out: &mut String, atom: &str) {
    // Commands followed by a letter need a separating space.
    if out.ends_with(|c: char| c.is_ascii_alphabetic())
        && out
            .rsplit('\\')
            .next()
            .is_some_and(|tail| tail.chars().all(|c| c.is_ascii_alphabetic()))
        && out.contains('\\')
        && atom.starts_with(|c: char| c.is_ascii_alphabetic())
    {
        out.push(' ');
    }
    out.push_str(atom);
}

fn flush_word(word: &mut String, out: &mut String) {
    if word.is_empty() {
        return;
    }
    let w = std::mem::take(word);
    let lower = w.to_ascii_lowercase();
    if FUNCTION_WORDS.contains(&w.as_str()) {
        push_atom(out, &format!("\\{w}"));
    } else if TEXT_WORDS.contains(&lower.as_str()) {
        push_atom(out, &format!("\\text{{ {w} }}"));
    } else if w.chars().count() >= 3 && w.chars().all(|c| c.is_ascii_lowercase()) {
        push_atom(out, &format!("\\operatorname{{{w}}}"));
    } else {
        push_atom(out, &w);
    }
}

/// LaTeX for an equation's text-layer transcription. Every output has balanced braces.
pub fn glyphs_to_latex(text: &str) -> String {
    let (body, _) = split_number(text);
    let chars: Vec<char> = body.chars().collect();
    let mut out = String::new();
    let mut word = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        // Runs of Unicode superscripts / subscripts.
        if superscript(c).is_some() || subscript(c).is_some() {
            flush_word(&mut word, &mut out);
            let sup = superscript(c).is_some();
            let mut run = String::new();
            while let Some(m) =
                chars
                    .get(i)
                    .and_then(|&d| if sup { superscript(d) } else { subscript(d) })
            {
                run.push(m);
                i += 1;
            }
            out.push_str(if sup { "^{" } else { "_{" });
            out.push_str(&run);
            out.push('}');
            continue;
        }
        // A letter with combining accents.
        let accents: Vec<&str> = chars[i + 1..].iter().map_while(|&d| accent(d)).collect();
        if c.is_ascii_alphabetic() && accents.is_empty() {
            word.push(c);
            i += 1;
            continue;
        }
        if c == '.' && !word.is_empty() && word == "s.t" {
            word.push(c);
            i += 1;
            continue;
        }
        flush_word(&mut word, &mut out);
        let atom: Option<String> = if let Some((style, base)) = styled(c).or_else(|| letterlike(c))
        {
            let inner = symbol(base).map_or_else(|| base.to_string(), str::to_string);
            Some(if style.is_empty() {
                inner
            } else {
                format!("\\{style}{{{inner}}}")
            })
        } else if let Some(s) = symbol(c) {
            Some(s.to_string())
        } else if c.is_ascii_alphanumeric() || "+-=<>()[]|/!,;:.'*^_".contains(c) {
            Some(c.to_string())
        } else if c.is_whitespace() {
            None
        } else if c.is_alphabetic() {
            Some(format!("\\text{{{c}}}"))
        } else {
            None
        };
        i += 1 + accents.len();
        let Some(mut atom) = atom else {
            if c.is_whitespace() && !out.ends_with(' ') && !out.is_empty() {
                out.push(' ');
            }
            continue;
        };
        for a in accents {
            atom = format!("\\{a}{{{atom}}}");
        }
        push_atom(&mut out, &atom);
    }
    flush_word(&mut word, &mut out);
    let collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    // A stray "^" or "_" at the end of the text layer must not dangle.
    collapsed.trim_end_matches(['^', '_', ' ']).to_string()
}

// ------------------------------------------------------------- LaTeX source

/// One numbered or unnumbered display of the paper's LaTeX source.
#[derive(Debug, Clone, PartialEq)]
pub struct TexEquation {
    /// LaTeX of one row (aligned rows wrapped in `aligned`).
    pub latex: String,
    /// The number LaTeX gives it (counting numbered displays, `\tag` as written).
    pub number: Option<String>,
    pub label: Option<String>,
}

const NUMBERED_ENVS: [&str; 7] = [
    "equation", "align", "gather", "multline", "eqnarray", "flalign", "alignat",
];
const MULTI_ROW_ENVS: [&str; 5] = ["align", "gather", "eqnarray", "flalign", "alignat"];
/// Largest source read for equations, in bytes.
pub const MAX_TEX_BYTES: usize = 4 * 1024 * 1024;
/// Macros expanded, and expansion rounds.
const MAX_MACROS: usize = 400;
const MACRO_ROUNDS: usize = 6;

fn strip_comments(source: &str) -> String {
    source
        .lines()
        .map(|line| {
            let bytes = line.as_bytes();
            let mut cut = line.len();
            for (i, b) in bytes.iter().enumerate() {
                if *b == b'%' && (i == 0 || bytes[i - 1] != b'\\') {
                    cut = i;
                    break;
                }
            }
            &line[..cut]
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The brace group starting at byte `open` (which must be `{`): its inside and the byte
/// after the closing brace.
fn brace_group(text: &str, open: usize) -> Option<(String, usize)> {
    if text.as_bytes().get(open) != Some(&b'{') {
        return None;
    }
    let mut depth = 0usize;
    let mut escaped = false;
    for (i, c) in text[open..].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' => escaped = true,
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some((text[open + 1..open + i].to_string(), open + i + 1));
                }
            }
            _ => {}
        }
    }
    None
}

struct Macro {
    args: usize,
    body: String,
}

static RE_NEWCOMMAND: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(r"\\(?:re)?newcommand\*?\s*\{?\\([A-Za-z]+)\}?\s*(?:\[(\d)\])?\s*").ok()
});
static RE_DEF: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"\\def\s*\\([A-Za-z]+)\s*").ok());
static RE_LABEL: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"\\label\s*\{([^}]*)\}").ok());
static RE_TAG: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"\\tag\*?\s*\{([^}]*)\}").ok());
static RE_NONUMBER: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"\\(?:nonumber|notag)\b").ok());

/// `\newcommand`/`\def` macros without optional arguments.
fn macros_of(source: &str) -> HashMap<String, Macro> {
    let mut macros = HashMap::new();
    let mut collect = |re: &Option<Regex>, with_args: bool| {
        let Some(re) = re else { return };
        for c in re.captures_iter(source) {
            if macros.len() >= MAX_MACROS {
                return;
            }
            let (Some(whole), Some(name)) = (c.get(0), c.get(1)) else {
                continue;
            };
            let args = if with_args {
                c.get(2)
                    .and_then(|a| a.as_str().parse::<usize>().ok())
                    .unwrap_or(0)
            } else {
                0
            };
            // A default for the first argument ("[2][x]") is not supported.
            if source[whole.end()..].starts_with('[') {
                continue;
            }
            if let Some((body, _)) = brace_group(source, whole.end()) {
                macros.insert(name.as_str().to_string(), Macro { args, body });
            }
        }
    };
    collect(&RE_NEWCOMMAND, true);
    collect(&RE_DEF, false);
    macros
}

/// Expands the paper's own macros (KaTeX does not know them).
fn expand(latex: &str, macros: &HashMap<String, Macro>) -> String {
    if macros.is_empty() {
        return latex.to_string();
    }
    let mut text = latex.to_string();
    for _ in 0..MACRO_ROUNDS {
        let mut out = String::with_capacity(text.len());
        let mut changed = false;
        let mut i = 0;
        let bytes = text.as_bytes();
        while i < text.len() {
            if bytes[i] == b'\\' {
                let name_len = text[i + 1..]
                    .bytes()
                    .take_while(u8::is_ascii_alphabetic)
                    .count();
                let name = &text[i + 1..i + 1 + name_len];
                if let Some(m) = macros.get(name).filter(|_| name_len > 0) {
                    let mut at = i + 1 + name_len;
                    let mut args = Vec::new();
                    for _ in 0..m.args {
                        while text[at..].starts_with(' ') {
                            at += 1;
                        }
                        match brace_group(&text, at) {
                            Some((arg, next)) => {
                                args.push(arg);
                                at = next;
                            }
                            None => match text[at..].chars().next() {
                                Some(c) => {
                                    args.push(c.to_string());
                                    at += c.len_utf8();
                                }
                                None => break,
                            },
                        }
                    }
                    if args.len() == m.args {
                        let mut body = m.body.clone();
                        for (k, arg) in args.iter().enumerate().rev() {
                            body = body.replace(&format!("#{}", k + 1), arg);
                        }
                        out.push('{');
                        out.push_str(&body);
                        out.push('}');
                        i = at;
                        changed = true;
                        continue;
                    }
                }
                if name_len == 0 {
                    // An escaped character ("\{", "\\"): copy both bytes.
                    let next = text[i + 1..].chars().next().map_or(0, char::len_utf8);
                    out.push_str(&text[i..i + 1 + next]);
                    i += 1 + next;
                } else {
                    out.push_str(&text[i..i + 1 + name_len]);
                    i += 1 + name_len;
                }
                continue;
            }
            let c = text[i..].chars().next().unwrap_or(' ');
            out.push(c);
            i += c.len_utf8();
        }
        text = out;
        if !changed {
            break;
        }
    }
    text
}

/// Splits an environment body into rows at top-level `\\`.
fn rows_of(body: &str) -> Vec<String> {
    let mut rows = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if bytes.get(i + 1) == Some(&b'\\') => {
                if depth == 0 {
                    rows.push(body[start..i].to_string());
                    start = i + 2;
                }
                i += 2;
                continue;
            }
            b'\\' => {
                i += 2;
                continue;
            }
            b'{' => depth += 1,
            b'}' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    rows.push(body[start..].to_string());
    rows.into_iter()
        .map(|r| r.trim().to_string())
        .filter(|r| !r.is_empty())
        .collect()
}

fn clean_row(row: &str) -> (String, Option<String>, Option<String>, bool) {
    let label = RE_LABEL
        .as_ref()
        .and_then(|re| re.captures(row))
        .map(|c| c[1].trim().to_string());
    let tag = RE_TAG
        .as_ref()
        .and_then(|re| re.captures(row))
        .map(|c| c[1].trim().to_string());
    let unnumbered = RE_NONUMBER.as_ref().is_some_and(|re| re.is_match(row));
    let mut text = row.to_string();
    for re in [&RE_LABEL, &RE_TAG, &RE_NONUMBER]
        .into_iter()
        .filter_map(|re| re.as_ref())
    {
        text = re.replace_all(&text, "").to_string();
    }
    (text.trim().to_string(), label, tag, unnumbered)
}

fn wrap_aligned(latex: &str) -> String {
    if latex.contains('&') {
        format!("\\begin{{aligned}}{latex}\\end{{aligned}}")
    } else {
        latex.to_string()
    }
}

/// Every display equation of a LaTeX source, rows of multi-row environments separately,
/// with the numbers LaTeX gives them.
pub fn tex_equations(source: &str) -> Vec<TexEquation> {
    let source = strip_comments(source);
    let macros = macros_of(&source);
    let body_start = source.find("\\begin{document}").unwrap_or(0);
    let text = &source[body_start..];
    let mut out = Vec::new();
    let mut counter = 0u32;
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        let next_env = rest.find("\\begin{");
        let next_bracket = rest.find("\\[");
        let next_dollars = rest.find("$$");
        let first = [next_env, next_bracket, next_dollars]
            .into_iter()
            .flatten()
            .min();
        let Some(at) = first else { break };
        let start = i + at;
        if Some(at) == next_env {
            let Some((env, after)) = brace_group(text, start + "\\begin".len()) else {
                i = start + 7;
                continue;
            };
            let base = env.trim_end_matches('*');
            let starred = env.ends_with('*');
            let math = NUMBERED_ENVS.contains(&base) || base == "displaymath";
            let close = format!("\\end{{{env}}}");
            let Some(end) = text[after..].find(&close).map(|e| after + e) else {
                i = after;
                continue;
            };
            if !math {
                i = after;
                continue;
            }
            let mut inner = &text[after..end];
            if base == "alignat" {
                // Skip the column count argument.
                if let Some((_, next)) = brace_group(inner, 0) {
                    inner = &inner[next..];
                }
            }
            let numbered_env = NUMBERED_ENVS.contains(&base) && !starred;
            let rows = if MULTI_ROW_ENVS.contains(&base) {
                rows_of(inner)
            } else {
                vec![inner.trim().to_string()]
            };
            for row in rows {
                let (latex, label, tag, unnumbered) = clean_row(&row);
                if latex.is_empty() {
                    continue;
                }
                let number = if let Some(tag) = tag {
                    Some(tag)
                } else if numbered_env && !unnumbered {
                    counter += 1;
                    Some(counter.to_string())
                } else {
                    None
                };
                out.push(TexEquation {
                    latex: wrap_aligned(&expand(&latex, &macros)),
                    number,
                    label,
                });
            }
            i = end + close.len();
        } else {
            let (open, close) = if Some(at) == next_bracket {
                ("\\[", "\\]")
            } else {
                ("$$", "$$")
            };
            let after = start + open.len();
            let Some(end) = text[after..].find(close).map(|e| after + e) else {
                break;
            };
            let (latex, label, _, _) = clean_row(&text[after..end]);
            if !latex.is_empty() {
                out.push(TexEquation {
                    latex: wrap_aligned(&expand(&latex, &macros)),
                    number: None,
                    label,
                });
            }
            i = end + close.len();
        }
    }
    out
}

// ------------------------------------------------------------- matching

/// Symbols of a LaTeX string for comparing two spellings of one equation: control words
/// and alphanumerics and operators, with styling, spacing and grouping dropped.
fn symbols(latex: &str) -> Vec<String> {
    const IGNORED: [&str; 24] = [
        "mathbf",
        "mathrm",
        "mathit",
        "mathsf",
        "mathtt",
        "mathcal",
        "mathbb",
        "mathfrak",
        "boldsymbol",
        "bm",
        "operatorname",
        "text",
        "left",
        "right",
        "big",
        "Big",
        "bigg",
        "Bigg",
        "quad",
        "qquad",
        "displaystyle",
        "begin",
        "end",
        "aligned",
    ];
    let mut out = Vec::new();
    let chars: Vec<char> = latex.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' {
            let name: String = chars[i + 1..]
                .iter()
                .take_while(|c| c.is_ascii_alphabetic())
                .collect();
            i += 1 + name.chars().count().max(1);
            if name == "begin" || name == "end" {
                // Skip the environment name.
                if chars.get(i) == Some(&'{') {
                    while i < chars.len() && chars[i] != '}' {
                        i += 1;
                    }
                    i += 1;
                }
                continue;
            }
            if !name.is_empty() && !IGNORED.contains(&name.as_str()) {
                let normalized = match name.as_str() {
                    "le" => "leq",
                    "ge" => "geq",
                    "ne" => "neq",
                    "rightarrow" => "to",
                    "epsilon" => "varepsilon",
                    "phi" => "varphi",
                    "dots" | "cdots" => "ldots",
                    "intercal" | "T" => "top",
                    n => n,
                };
                out.push(normalized.to_string());
            }
            continue;
        }
        if c.is_alphanumeric() || "+-=<>()[]|/!,*".contains(c) {
            out.push(c.to_string());
        }
        i += 1;
    }
    out
}

/// Dice similarity of the symbol multisets of two LaTeX strings, in [0, 1].
pub fn similarity(a: &str, b: &str) -> f32 {
    let (sa, sb) = (symbols(a), symbols(b));
    if sa.is_empty() || sb.is_empty() {
        return 0.0;
    }
    let mut counts: HashMap<&str, i32> = HashMap::new();
    for s in &sa {
        *counts.entry(s).or_default() += 1;
    }
    let mut shared = 0;
    for s in &sb {
        if let Some(n) = counts.get_mut(s.as_str()) {
            if *n > 0 {
                *n -= 1;
                shared += 1;
            }
        }
    }
    (2 * shared) as f32 / (sa.len() + sb.len()) as f32
}

/// Least similarity for a number match to be confirmed by content.
const NUMBER_CONFIRM: f32 = 0.3;
/// Least similarity for a match by content alone.
const CONTENT_MATCH: f32 = 0.55;
/// Share of the paper's equations that must match a source for it to be the paper's.
const SOURCE_ACCEPT: f32 = 0.4;

/// The source equation for one PDF equation: by number (confirmed by content), else the
/// most similar one when it is similar enough.
fn match_source<'t>(
    reconstructed: &str,
    number: Option<&str>,
    source: &'t [TexEquation],
) -> Option<&'t TexEquation> {
    if let Some(n) = number {
        if let Some(eq) = source.iter().find(|e| e.number.as_deref() == Some(n)) {
            if similarity(reconstructed, &eq.latex) >= NUMBER_CONFIRM {
                return Some(eq);
            }
        }
    }
    source
        .iter()
        .map(|e| (similarity(reconstructed, &e.latex), e))
        .filter(|(s, _)| *s >= CONTENT_MATCH)
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, e)| e)
}

/// Characters of introducing prose and of a following "where" clause kept per equation.
const INTRO_CHARS: usize = 300;
const WHERE_CHARS: usize = 240;

fn is_prose(kind: &BlockKind) -> bool {
    matches!(
        kind,
        BlockKind::Paragraph
            | BlockKind::ListItem
            | BlockKind::Theorem { .. }
            | BlockKind::Definition { .. }
    )
}

fn flat(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The end of the paragraph before the equation at `index` (other equations of the same
/// display skipped), within its section.
fn equation_intro(doc: &StructuredDocument, index: usize) -> String {
    doc.blocks[..index]
        .iter()
        .rev()
        .take_while(|b| !matches!(b.kind, BlockKind::Heading { .. } | BlockKind::Title))
        .find(|b| is_prose(&b.kind))
        .map(|b| {
            let text = flat(&b.text);
            let chars: Vec<char> = text.chars().collect();
            if chars.len() <= INTRO_CHARS {
                return text;
            }
            let tail: String = chars[chars.len() - INTRO_CHARS..].iter().collect();
            // Start at a sentence or word boundary.
            let start = tail
                .find(". ")
                .map(|i| i + 2)
                .filter(|i| *i < tail.len() / 2)
                .or_else(|| tail.find(' ').map(|i| i + 1))
                .unwrap_or(0);
            format!("…{}", &tail[start..])
        })
        .unwrap_or_default()
}

/// The start of the paragraph after the equation at `index` when it explains the symbols.
fn equation_explanation(doc: &StructuredDocument, index: usize) -> String {
    doc.blocks[index + 1..]
        .iter()
        .find(|b| !matches!(b.kind, BlockKind::Equation))
        .filter(|b| is_prose(&b.kind))
        .map(|b| flat(&b.text))
        .filter(|t| t.to_lowercase().starts_with("where") || t.to_lowercase().starts_with("here"))
        .map(|t| {
            if t.chars().count() <= WHERE_CHARS {
                t
            } else {
                let mut cut: String = t.chars().take(WHERE_CHARS - 1).collect();
                cut.push('…');
                cut
            }
        })
        .unwrap_or_default()
}

/// Every display equation of a document, with LaTeX from `source` (a `.tex` path and
/// its equations) when that source matches the paper.
pub fn equations_of(
    doc: &StructuredDocument,
    source: Option<(&str, &[TexEquation])>,
) -> Vec<Equation> {
    let mut found: Vec<Equation> = doc
        .blocks
        .iter()
        .enumerate()
        .filter(|(_, b)| matches!(b.kind, BlockKind::Equation))
        .filter(|(_, b)| !b.text.trim().is_empty())
        .map(|(index, b)| {
            let (text, number) = split_number(&b.text);
            Equation {
                id: String::new(),
                latex: glyphs_to_latex(&text),
                text,
                number,
                origin: EquationOrigin::Reconstructed,
                source_file: None,
                page: b.page,
                bbox: b.bbox.map(|bb| bb.rounded()),
                intro: equation_intro(doc, index),
                explanation: equation_explanation(doc, index),
            }
        })
        .collect();
    if let Some((path, tex)) = source.filter(|(_, tex)| !tex.is_empty()) {
        let matches: Vec<Option<&TexEquation>> = found
            .iter()
            .map(|e| match_source(&e.latex, e.number.as_deref(), tex))
            .collect();
        let matched = matches.iter().filter(|m| m.is_some()).count();
        if !found.is_empty() && matched as f32 >= SOURCE_ACCEPT * found.len() as f32 {
            for (eq, m) in found.iter_mut().zip(matches) {
                if let Some(m) = m {
                    eq.latex = m.latex.clone();
                    eq.origin = EquationOrigin::Source;
                    eq.source_file = Some(path.to_string());
                }
            }
        }
    }
    let mut per_page: HashMap<Option<u32>, usize> = HashMap::new();
    let numbers: Vec<Option<String>> = found.iter().map(|e| e.number.clone()).collect();
    for eq in &mut found {
        let k = {
            let k = per_page.entry(eq.page).or_default();
            *k += 1;
            *k
        };
        let unique = eq
            .number
            .as_deref()
            .filter(|n| numbers.iter().filter(|m| m.as_deref() == Some(*n)).count() == 1);
        eq.id = match (unique, eq.page) {
            (Some(n), _) => format!("eq-{}", n.to_ascii_lowercase()),
            (None, Some(p)) => format!("eq-p{p}-{k}"),
            (None, None) => format!("eq-{k}"),
        };
    }
    found
}

static RE_QUERY_NUMBER: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(r"^(?i:(?:equation|eqn?\.?)\s*)?\(?\s*([A-Za-z]?\d{1,3}(?:\.\d{1,3})?[a-z]?)\s*\)?$")
        .ok()
});

/// Equations matching `query`: a number ("3", "Eq. (3)", "equation 2.1") or text and
/// symbols of the equation, best first.
pub fn find_equations<'e>(equations: &'e [Equation], query: &str) -> Vec<&'e Equation> {
    let query = query.trim();
    if let Some(n) = RE_QUERY_NUMBER
        .as_ref()
        .and_then(|re| re.captures(query))
        .map(|c| c[1].to_string())
    {
        return equations
            .iter()
            .filter(|e| {
                e.number
                    .as_deref()
                    .is_some_and(|m| m.eq_ignore_ascii_case(&n))
            })
            .collect();
    }
    let wanted = glyphs_to_latex(query);
    let mut scored: Vec<(f32, &Equation)> = equations
        .iter()
        .map(|e| {
            let by_latex = similarity(&wanted, &e.latex);
            let by_text = similarity(&wanted, &glyphs_to_latex(&e.text));
            (by_latex.max(by_text), e)
        })
        .filter(|(s, _)| *s >= 0.4)
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored.into_iter().map(|(_, e)| e).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::processing::document_model::Block;

    #[test]
    fn numbers_are_split_off() {
        assert_eq!(
            split_number("E = mc² (3)"),
            ("E = mc²".to_string(), Some("3".to_string()))
        );
        assert_eq!(
            split_number("x = y   ( 2.1 )"),
            ("x = y".to_string(), Some("2.1".to_string()))
        );
        assert_eq!(split_number("f(x) = (x)"), ("f(x) = (x)".to_string(), None));
        assert_eq!(split_number("(A.4)").1, None);
    }

    #[test]
    fn glyphs_map_to_latex() {
        assert_eq!(
            glyphs_to_latex("α + β ≤ γ"),
            "\\alpha + \\beta \\leq \\gamma"
        );
        assert_eq!(glyphs_to_latex("x² + yᵢ"), "x^{2} + y_{i}");
        assert_eq!(
            glyphs_to_latex("𝐱 ∈ ℝⁿ"),
            "\\mathbf{x} \\in \\mathbb{R}^{n}"
        );
        // Italic math letters are plain letters; bold Greek is \boldsymbol.
        assert_eq!(glyphs_to_latex("𝑓(𝑥)"), "f(x)");
        assert_eq!(glyphs_to_latex("𝝓"), "\\boldsymbol{\\phi}");
        assert_eq!(glyphs_to_latex("x̂ = Wᵀ h"), "\\hat{x} = W^{T} h");
        assert_eq!(glyphs_to_latex("∑ exp(z) − 1"), "\\sum \\exp(z) - 1");
        assert_eq!(
            glyphs_to_latex("softmax(QKᵀ) where Q ∈ ℝ"),
            "\\operatorname{softmax}(QK^{T}) \\text{ where } Q \\in \\mathbb{R}"
        );
        assert_eq!(glyphs_to_latex("50% {a}"), "50\\% \\{a\\}");
        // The equation number is not part of the LaTeX.
        assert_eq!(glyphs_to_latex("y = Φq x (4)"), "y = \\Phi q x");
        // A command followed by a letter keeps them apart.
        assert_eq!(glyphs_to_latex("αx"), "\\alpha x");
        for text in ["{{", "x^", "√}", "∑_", "a }{ b"] {
            let latex = glyphs_to_latex(text);
            let opens = latex.matches('{').count() - latex.matches("\\{").count();
            let closes = latex.matches('}').count() - latex.matches("\\}").count();
            assert_eq!(opens, closes, "{text} → {latex}");
        }
    }

    const PAPER_TEX: &str = r"\documentclass{article}
\newcommand{\R}{\mathbb{R}}
\newcommand{\norm}[1]{\lVert #1 \rVert}
\begin{document}
\begin{equation}\label{eq:update}
S_t = S_{t-1} + \beta_t (v_t - S_{t-1} k_t) k_t^\top
\end{equation}
Some text with $x$ inline. % \begin{equation} commented out \end{equation}
\begin{align}
o_t &= S_t q_t \\
h_t &= \norm{o_t} \nonumber \\
y &= W h_t \tag{3a}
\end{align}
\[ z = \sum_i z_i \]
\begin{equation*} a = b \end{equation*}
\begin{equation}
x \in \R^d
\end{equation}
\end{document}";

    #[test]
    fn tex_source_equations_are_numbered_like_latex() {
        let eqs = tex_equations(PAPER_TEX);
        let numbers: Vec<Option<&str>> = eqs.iter().map(|e| e.number.as_deref()).collect();
        assert_eq!(
            numbers,
            [
                Some("1"),
                Some("2"),
                None,
                Some("3a"),
                None,
                None,
                Some("3")
            ]
        );
        assert_eq!(eqs[0].label.as_deref(), Some("eq:update"));
        assert_eq!(
            eqs[0].latex,
            "S_t = S_{t-1} + \\beta_t (v_t - S_{t-1} k_t) k_t^\\top"
        );
        // Aligned rows stay renderable on their own; macros are expanded.
        assert_eq!(eqs[1].latex, "\\begin{aligned}o_t &= S_t q_t\\end{aligned}");
        assert!(
            eqs[2].latex.contains("\\lVert o_t \\rVert"),
            "{}",
            eqs[2].latex
        );
        assert_eq!(eqs[6].latex, "x \\in {\\mathbb{R}}^d");
    }

    fn paper(blocks: &[(&str, u32)]) -> StructuredDocument {
        StructuredDocument {
            pages: Vec::new(),
            blocks: blocks
                .iter()
                .map(|(t, p)| {
                    Block::new(BlockKind::Equation, *t)
                        .on_page(*p, Some(BBox::new(100.0, 400.0, 500.0, 420.0)))
                })
                .collect(),
        }
    }

    #[test]
    fn source_latex_replaces_reconstruction_when_the_source_matches() {
        let doc = paper(&[
            ("St = St−1 + βt(vt − St−1kt)k⊤t (1)", 3),
            ("ot = Stqt (2)", 3),
            ("x ∈ ℝd (3)", 4),
            ("ψ = ∫ ζ dλ", 5),
        ]);
        let tex = tex_equations(PAPER_TEX);
        let eqs = equations_of(&doc, Some(("C:/p/delta.tex", &tex)));
        assert_eq!(eqs[0].origin, EquationOrigin::Source);
        assert_eq!(eqs[0].id, "eq-1");
        assert!(eqs[0].latex.starts_with("S_t = S_{t-1}"));
        assert_eq!(eqs[0].source_file.as_deref(), Some("C:/p/delta.tex"));
        assert_eq!(eqs[1].origin, EquationOrigin::Source);
        // "(3)" in the PDF is the 4th numbered display of the source (3a was tagged), and
        // the content confirms the match.
        assert_eq!(eqs[2].origin, EquationOrigin::Source);
        assert_eq!(eqs[2].latex, "x \\in {\\mathbb{R}}^d");
        // An equation the source does not have stays reconstructed.
        assert_eq!(eqs[3].origin, EquationOrigin::Reconstructed);
        assert_eq!(eqs[3].id, "eq-p5-1");
        assert_eq!(eqs[3].latex, "\\psi = \\int \\zeta d\\lambda");
    }

    #[test]
    fn an_unrelated_source_is_never_used() {
        let doc = paper(&[
            ("L = −∑ y log p (1)", 2),
            ("∇θL = E[g] (2)", 2),
            ("p = σ(Wx + b) (3)", 3),
        ]);
        let tex = tex_equations(PAPER_TEX);
        let eqs = equations_of(&doc, Some(("C:/p/other.tex", &tex)));
        assert!(eqs
            .iter()
            .all(|e| e.origin == EquationOrigin::Reconstructed));
        assert!(eqs.iter().all(|e| e.source_file.is_none()));
    }

    #[test]
    fn equations_carry_the_prose_around_them() {
        let doc = StructuredDocument {
            pages: Vec::new(),
            blocks: vec![
                Block::new(BlockKind::Heading { level: 1 }, "2 Method"),
                Block::new(
                    BlockKind::Paragraph,
                    "The state is updated with the delta rule, which writes the error of the current prediction:",
                ),
                Block::new(BlockKind::Equation, "St = St−1 + βt(vt − St−1kt)k⊤t (1)"),
                Block::new(BlockKind::Paragraph, "where βt is the writing strength."),
                Block::new(BlockKind::Heading { level: 1 }, "3 Results"),
                Block::new(BlockKind::Equation, "x = y (2)"),
            ],
        };
        let eqs = equations_of(&doc, None);
        assert_eq!(
            eqs[0].intro,
            "The state is updated with the delta rule, which writes the error of the current prediction:"
        );
        assert_eq!(eqs[0].explanation, "where βt is the writing strength.");
        assert_eq!(
            eqs[0].passage(),
            "Equation (1): The state is updated with the delta rule, which writes the error of the current prediction: St = St−1 + βt(vt − St−1kt)k⊤t where βt is the writing strength."
        );
        // A heading ends the search: no prose from another section.
        assert_eq!(eqs[1].intro, "");
        assert_eq!(eqs[1].passage(), "Equation (2): x = y");
    }

    #[test]
    fn equations_are_found_by_number_or_content() {
        let doc = paper(&[("ot = Stqt (2)", 3), ("x ∈ ℝd (3.1)", 4), ("ψ = ∫ ζ dλ", 5)]);
        let eqs = equations_of(&doc, None);
        assert_eq!(find_equations(&eqs, "Eq. (2)")[0].id, "eq-2");
        assert_eq!(find_equations(&eqs, "equation 3.1")[0].id, "eq-3.1");
        assert_eq!(find_equations(&eqs, "3.1")[0].id, "eq-3.1");
        assert!(find_equations(&eqs, "(7)").is_empty());
        assert_eq!(find_equations(&eqs, "ψ = ∫ ζ")[0].id, "eq-p5-1");
        assert!(similarity("\\mathbf{x} \\le y", "x \\leq y") > 0.99);
    }
}
