//! Text normalisation shared by the reference parser, local paper identification and the
//! matching rules: ligatures, line-break hyphenation, diacritics and title comparison.

/// Replaces typographic ligatures and look-alike punctuation that PDF text extraction
/// produces (`ﬁ`, `ﬂ`, curly quotes, non-breaking spaces) with plain characters.
pub fn expand_ligatures(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\u{FB00}' => out.push_str("ff"),
            '\u{FB01}' => out.push_str("fi"),
            '\u{FB02}' => out.push_str("fl"),
            '\u{FB03}' => out.push_str("ffi"),
            '\u{FB04}' => out.push_str("ffl"),
            '\u{FB05}' | '\u{FB06}' => out.push_str("st"),
            '\u{00A0}' | '\u{2002}' | '\u{2003}' | '\u{2009}' | '\u{202F}' => out.push(' '),
            // Soft hyphens, and spacing diacritics that PDF extraction separates from
            // their letter (`Re ´`, `Jorg ¨`).
            '\u{00AD}' | '\u{00B4}' | '\u{00A8}' | '\u{02DC}' | '\u{02C6}' | '\u{00B8}'
            | '\u{02DA}' | '\u{02C7}' | '\u{02D8}' | '\u{02DD}' => {}
            '\u{2010}' | '\u{2011}' => out.push('-'),
            other => out.push(other),
        }
    }
    out
}

/// One line of text from a block that may span several lines: ligatures expanded, a word
/// broken across lines by a hyphen joined (`trans-\nformers` → `transformers`, while
/// `Kolmogorov-\nArnold` keeps its hyphen), and all whitespace collapsed to single spaces.
pub fn clean_block_text(text: &str) -> String {
    let text = expand_ligatures(text);
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '-' {
            // A hyphen at a line end: look at the letters around the break.
            let mut j = i + 1;
            let mut newline = false;
            while j < chars.len() && chars[j].is_whitespace() {
                newline |= chars[j] == '\n' || chars[j] == '\r';
                j += 1;
            }
            if newline {
                let before = i.checked_sub(1).map(|k| chars[k]);
                let after = chars.get(j).copied();
                let joins = before.is_some_and(|b| b.is_lowercase())
                    && after.is_some_and(|a| a.is_lowercase());
                if !joins {
                    out.push('-');
                }
                i = j;
                continue;
            }
        }
        if c.is_whitespace() {
            if !out.ends_with(' ') && !out.is_empty() {
                out.push(' ');
            }
        } else {
            out.push(c);
        }
        i += 1;
    }
    out.trim().to_string()
}

/// The ASCII form of a Latin letter with diacritics (`ü` → `u`, `ł` → `l`, `ß` → `ss`);
/// other characters are returned unchanged.
fn fold_char(c: char, out: &mut String) {
    let folded: &str = match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
        'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' | 'Ā' | 'Ă' | 'Ą' => "A",
        'æ' => "ae",
        'Æ' => "AE",
        'ç' | 'ć' | 'č' | 'ĉ' | 'ċ' => "c",
        'Ç' | 'Ć' | 'Č' | 'Ĉ' | 'Ċ' => "C",
        'ď' | 'đ' | 'ð' => "d",
        'Ď' | 'Đ' | 'Ð' => "D",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => "e",
        'È' | 'É' | 'Ê' | 'Ë' | 'Ē' | 'Ĕ' | 'Ė' | 'Ę' | 'Ě' => "E",
        'ğ' | 'ĝ' | 'ġ' | 'ģ' => "g",
        'Ğ' | 'Ĝ' | 'Ġ' | 'Ģ' => "G",
        'ì' | 'í' | 'î' | 'ï' | 'ī' | 'ĭ' | 'į' | 'ı' => "i",
        'Ì' | 'Í' | 'Î' | 'Ï' | 'Ī' | 'Ĭ' | 'Į' | 'İ' => "I",
        'ķ' => "k",
        'Ķ' => "K",
        'ł' | 'ľ' | 'ĺ' | 'ļ' => "l",
        'Ł' | 'Ľ' | 'Ĺ' | 'Ļ' => "L",
        'ñ' | 'ń' | 'ň' | 'ņ' => "n",
        'Ñ' | 'Ń' | 'Ň' | 'Ņ' => "N",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => "o",
        'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' | 'Ø' | 'Ō' | 'Ŏ' | 'Ő' => "O",
        'œ' => "oe",
        'Œ' => "OE",
        'ŕ' | 'ř' | 'ŗ' => "r",
        'Ŕ' | 'Ř' | 'Ŗ' => "R",
        'ś' | 'š' | 'ş' | 'ŝ' | 'ș' => "s",
        'Ś' | 'Š' | 'Ş' | 'Ŝ' | 'Ș' => "S",
        'ß' => "ss",
        'ť' | 'ţ' | 'ț' => "t",
        'Ť' | 'Ţ' | 'Ț' => "T",
        'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => "u",
        'Ù' | 'Ú' | 'Û' | 'Ü' | 'Ū' | 'Ŭ' | 'Ů' | 'Ű' | 'Ų' => "U",
        'ý' | 'ÿ' => "y",
        'Ý' | 'Ÿ' => "Y",
        'ź' | 'ż' | 'ž' => "z",
        'Ź' | 'Ż' | 'Ž' => "Z",
        'þ' => "th",
        'Þ' => "TH",
        other => {
            out.push(other);
            return;
        }
    };
    out.push_str(folded);
}

/// `text` with Latin diacritics removed.
pub fn fold_diacritics(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        fold_char(c, &mut out);
    }
    out
}

/// The comparable form of a title: ligatures expanded, diacritics folded, lower-cased,
/// every run of non-alphanumeric characters turned into one space.
pub fn normalize_title(title: &str) -> String {
    let folded = fold_diacritics(&expand_ligatures(title)).to_lowercase();
    let mut out = String::with_capacity(folded.len());
    for c in folded.chars() {
        if c.is_alphanumeric() {
            out.push(c);
        } else if !out.ends_with(' ') && !out.is_empty() {
            out.push(' ');
        }
    }
    out.trim_end().to_string()
}

/// The comparable form of a surname: diacritics folded, lower-cased, letters only
/// (`Schmidhuber`, `schmidhuber` and `Schmidhüber` agree; so do `O'Neil` and `ONeil`).
pub fn normalize_surname(name: &str) -> String {
    fold_diacritics(&expand_ligatures(name))
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphabetic())
        .collect()
}

/// Levenshtein distance between two character sequences.
fn levenshtein(a: &[char], b: &[char]) -> usize {
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        current[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let substitution = previous[j] + usize::from(ca != cb);
            current[j + 1] = substitution.min(previous[j + 1] + 1).min(current[j] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

/// Similarity of two titles in `[0, 1]`: one minus the edit distance of their normalised
/// forms over the longer length. `1.0` means the normalised titles are equal; two empty
/// titles are not similar (`0.0`).
pub fn title_similarity(a: &str, b: &str) -> f64 {
    let a: Vec<char> = normalize_title(a).chars().collect();
    let b: Vec<char> = normalize_title(b).chars().collect();
    let longest = a.len().max(b.len());
    if longest == 0 {
        return 0.0;
    }
    1.0 - levenshtein(&a, &b) as f64 / longest as f64
}

/// Whether a library paper's title (as read from its first page) and another title name
/// the same work: at least 0.9 similar, or the library title is a cut-off start of the
/// other of at least four words (a title set on two lines, of which the parser kept one).
pub fn titles_agree(library_title: &str, other: &str) -> bool {
    if title_similarity(library_title, other) >= 0.9 {
        return true;
    }
    let short = normalize_title(library_title);
    let long = normalize_title(other);
    short.split(' ').count() >= 4 && long.starts_with(&format!("{short} "))
}

/// A short, stable slug of a name or title: normalised words joined by `-`, at most
/// `max_words` words.
pub fn slug(text: &str, max_words: usize) -> String {
    normalize_title(text)
        .split(' ')
        .filter(|w| !w.is_empty())
        .take(max_words)
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_text_joins_hyphenated_words_and_keeps_compound_hyphens() {
        assert_eq!(
            clean_block_text("Linear trans-\nformers are secretly ﬁxed\nweight programmers"),
            "Linear transformers are secretly fixed weight programmers"
        );
        assert_eq!(
            clean_block_text("KAN: Kolmogorov-\nArnold networks"),
            "KAN: Kolmogorov-Arnold networks"
        );
        assert_eq!(clean_block_text("  a\u{00A0} b \n\n c "), "a b c");
    }

    #[test]
    fn titles_compare_without_case_punctuation_or_diacritics() {
        assert_eq!(
            normalize_title("Attention Is All You Need!"),
            "attention is all you need"
        );
        assert!(
            (title_similarity(
                "Linear Transformers Are Secretly Fast Weight Programmers",
                "Linear transformers are secretly fast weight programmers."
            ) - 1.0)
                .abs()
                < 1e-9
        );
        let near = title_similarity(
            "Parallelizing linear transformers with the delta rule over sequence length",
            "Parallelizing Linear Transformers with the Delta Rule over Sequence Length",
        );
        assert!(near > 0.99);
        assert!(title_similarity("Gated delta networks", "Gated linear attention") < 0.9);
        assert_eq!(title_similarity("", ""), 0.0);
        assert_eq!(normalize_surname("Schmidhüber"), "schmidhuber");
        assert_eq!(normalize_surname("Łukasz"), "lukasz");
        assert_eq!(slug("Songlin Yang", 4), "songlin-yang");
        assert!(titles_agree(
            "Linear Transformers Are Secretly",
            "Linear transformers are secretly fast weight programmers"
        ));
        assert!(!titles_agree(
            "Linear Transformers",
            "Linear transformers are RNNs"
        ));
    }
}
