//! The expression language of model-written plots and simulations, checked the way the
//! app's renderer reads it (`app/src/features/ask/visual/expr.ts`): same tokens, grammar,
//! limits, names and functions. Nothing is evaluated here; a formula that passes would
//! compile in the renderer and one that fails would not.
//!
//! Grammar (lowest precedence first): comparison (`<`, `<=`, `>`, `>=`, not chained),
//! additive, product, unary minus/plus, power (`^` or `**`, right associative), and
//! primaries: number, name, `name(args)`, `(expr)`.

use serde_json::Value;

/// Longest expression accepted, in UTF-16 code units (JavaScript string length).
pub const MAX_EXPR_CHARS: usize = 600;
/// Deepest nesting accepted (parentheses, operators, calls).
pub const MAX_EXPR_DEPTH: usize = 48;
/// Longest variable name.
const MAX_NAME_CHARS: usize = 24;

/// Constants every expression may read (a variable of the same name takes precedence).
const CONSTANTS: [&str; 3] = ["pi", "e", "g"];

/// Functions with their least and greatest number of arguments.
const FUNCTIONS: [(&str, usize, usize); 23] = [
    ("sin", 1, 1),
    ("cos", 1, 1),
    ("tan", 1, 1),
    ("asin", 1, 1),
    ("acos", 1, 1),
    ("atan", 1, 1),
    ("atan2", 2, 2),
    ("sinh", 1, 1),
    ("cosh", 1, 1),
    ("tanh", 1, 1),
    ("sqrt", 1, 1),
    ("abs", 1, 1),
    ("sign", 1, 1),
    ("exp", 1, 1),
    ("ln", 1, 1),
    ("log", 1, 1),
    ("min", 1, 16),
    ("max", 1, 16),
    ("hypot", 1, 16),
    ("floor", 1, 1),
    ("ceil", 1, 1),
    ("round", 1, 1),
    ("mod", 2, 2),
];

fn function(name: &str) -> Option<(usize, usize)> {
    FUNCTIONS
        .iter()
        .find(|(n, _, _)| *n == name)
        .map(|(_, min, max)| (*min, *max))
}

/// Names a model may not use for its own variables: functions, `pi` and `e`.
pub fn is_reserved_name(name: &str) -> bool {
    function(name).is_some() || name == "pi" || name == "e"
}

/// A usable variable name: a letter, then up to 23 letters, digits or `_`, and not a
/// function or fixed constant.
pub fn is_valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    let well_formed = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && name.len() <= MAX_NAME_CHARS;
    well_formed && !is_reserved_name(name)
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Num,
    Name(String),
    Op(&'static str),
    LParen,
    RParen,
    Comma,
    End,
}

impl Token {
    fn text(&self) -> String {
        match self {
            Token::Num => "number".to_string(),
            Token::Name(n) => n.clone(),
            Token::Op(o) => (*o).to_string(),
            Token::LParen => "(".to_string(),
            Token::RParen => ")".to_string(),
            Token::Comma => ",".to_string(),
            Token::End => String::new(),
        }
    }
}

/// Length of the number at the start of `rest`: `\d+\.?\d*` or `\.\d+`, then an optional
/// exponent `[eE][+-]?\d+`.
fn number_len(rest: &[char]) -> usize {
    let digits = |from: usize| {
        rest[from..]
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .count()
    };
    let mut i;
    if rest.first().is_some_and(char::is_ascii_digit) {
        i = digits(0);
        if rest.get(i) == Some(&'.') {
            i += 1;
            i += digits(i);
        }
    } else {
        // A leading "." followed by a digit (checked by the caller).
        i = 1 + digits(1);
    }
    if matches!(rest.get(i), Some('e' | 'E')) {
        let mut j = i + 1;
        if matches!(rest.get(j), Some('+' | '-')) {
            j += 1;
        }
        let exp = digits(j.min(rest.len()));
        if exp > 0 {
            i = j + exp;
        }
    }
    i
}

fn tokenize(src: &str) -> Result<Vec<(Token, usize)>, String> {
    let chars: Vec<char> = src.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if matches!(c, ' ' | '\t' | '\n' | '\r') {
            i += 1;
            continue;
        }
        if c.is_ascii_digit() || (c == '.' && chars.get(i + 1).is_some_and(char::is_ascii_digit)) {
            let len = number_len(&chars[i..]);
            let text: String = chars[i..i + len].iter().collect();
            let value: f64 = text
                .parse()
                .map_err(|_| format!("Unreadable number at position {}.", i + 1))?;
            if !value.is_finite() {
                return Err(format!("Number out of range at position {}.", i + 1));
            }
            tokens.push((Token::Num, i));
            i += len;
            if chars
                .get(i)
                .is_some_and(|c| c.is_ascii_alphabetic() || *c == '_' || *c == '(')
            {
                return Err(format!(
                    "Write \"*\" between {text} and what follows (position {}).",
                    i + 1
                ));
            }
            continue;
        }
        if c.is_ascii_alphabetic() || c == '_' {
            let len = chars[i..]
                .iter()
                .take_while(|c| c.is_ascii_alphanumeric() || **c == '_')
                .count();
            tokens.push((Token::Name(chars[i..i + len].iter().collect()), i));
            i += len;
            continue;
        }
        let next = chars.get(i + 1).copied();
        let (token, len) = match c {
            '<' if next == Some('=') => (Token::Op("<="), 2),
            '>' if next == Some('=') => (Token::Op(">="), 2),
            '<' => (Token::Op("<"), 1),
            '>' => (Token::Op(">"), 1),
            '*' if next == Some('*') => (Token::Op("^"), 2),
            '+' => (Token::Op("+"), 1),
            '-' => (Token::Op("-"), 1),
            '*' => (Token::Op("*"), 1),
            '/' => (Token::Op("/"), 1),
            '^' => (Token::Op("^"), 1),
            '(' => (Token::LParen, 1),
            ')' => (Token::RParen, 1),
            ',' => (Token::Comma, 1),
            other => {
                return Err(format!(
                    "Unexpected character \"{other}\" at position {}.",
                    i + 1
                ))
            }
        };
        tokens.push((token, i));
        i += len;
    }
    tokens.push((Token::End, chars.len()));
    Ok(tokens)
}

/// Parsed expression tree (only what name resolution needs).
#[derive(Debug)]
enum Expr {
    Num,
    Var(String),
    Neg(Box<Expr>),
    Bin(Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
}

fn binding_power(op: &str) -> Option<u8> {
    match op {
        "<" | "<=" | ">" | ">=" => Some(10),
        "+" | "-" => Some(20),
        "*" | "/" => Some(30),
        "^" => Some(50),
        _ => None,
    }
}
const COMPARISON_BP: u8 = 10;
const UNARY_BP: u8 = 40;

struct Parser {
    tokens: Vec<(Token, usize)>,
    index: usize,
    depth: usize,
}

impl Parser {
    fn peek(&self) -> &(Token, usize) {
        &self.tokens[self.index.min(self.tokens.len() - 1)]
    }

    fn next(&mut self) -> (Token, usize) {
        let token = self.peek().clone();
        self.index += 1;
        token
    }

    fn parse_all(&mut self) -> Result<Expr, String> {
        let expr = self.parse(0)?;
        let (tail, pos) = self.peek();
        if *tail != Token::End {
            return Err(format!(
                "Unexpected \"{}\" at position {}.",
                tail.text(),
                pos + 1
            ));
        }
        Ok(expr)
    }

    fn parse(&mut self, min_bp: u8) -> Result<Expr, String> {
        self.depth += 1;
        if self.depth > MAX_EXPR_DEPTH {
            return Err("The expression is nested too deeply.".to_string());
        }
        let mut left = self.prefix()?;
        loop {
            let Token::Op(op) = self.peek().0 else {
                break;
            };
            let Some(bp) = binding_power(op) else {
                break;
            };
            if bp <= min_bp {
                break;
            }
            self.next();
            if bp == COMPARISON_BP {
                let right = self.parse(bp)?;
                if let Token::Op(after) = self.peek().0 {
                    if binding_power(after) == Some(COMPARISON_BP) {
                        return Err("Comparisons cannot be chained.".to_string());
                    }
                }
                left = Expr::Bin(Box::new(left), Box::new(right));
                continue;
            }
            let right = self.parse(if op == "^" { bp - 1 } else { bp })?;
            left = Expr::Bin(Box::new(left), Box::new(right));
        }
        self.depth -= 1;
        Ok(left)
    }

    fn prefix(&mut self) -> Result<Expr, String> {
        let (token, pos) = self.next();
        match token {
            Token::Num => Ok(Expr::Num),
            Token::Op(op) if op == "-" || op == "+" => {
                let arg = self.parse(UNARY_BP)?;
                Ok(if op == "-" {
                    Expr::Neg(Box::new(arg))
                } else {
                    arg
                })
            }
            Token::LParen => {
                let inner = self.parse(0)?;
                let (close, _) = self.next();
                if close != Token::RParen {
                    return Err(format!("Missing \")\" for \"(\" at position {}.", pos + 1));
                }
                Ok(inner)
            }
            Token::Name(name) => {
                if self.peek().0 != Token::LParen {
                    return Ok(Expr::Var(name));
                }
                self.next();
                let mut args = Vec::new();
                if self.peek().0 != Token::RParen {
                    loop {
                        args.push(self.parse(0)?);
                        if self.peek().0 == Token::Comma {
                            self.next();
                            continue;
                        }
                        break;
                    }
                }
                let (close, _) = self.next();
                if close != Token::RParen {
                    return Err(format!(
                        "Missing \")\" after the arguments of {name} (position {}).",
                        pos + 1
                    ));
                }
                Ok(Expr::Call(name, args))
            }
            Token::End => Err("The expression ends too early.".to_string()),
            other => Err(format!(
                "Unexpected \"{}\" at position {}.",
                other.text(),
                pos + 1
            )),
        }
    }
}

fn resolve(node: &Expr, names: &[&str]) -> Result<(), String> {
    match node {
        Expr::Num => Ok(()),
        Expr::Var(name) => {
            if names.contains(&name.as_str()) || CONSTANTS.contains(&name.as_str()) {
                Ok(())
            } else if function(name).is_some() {
                Err(format!("{name} is a function; call it as {name}(…)."))
            } else {
                Err(format!("Unknown name \"{name}\"."))
            }
        }
        Expr::Neg(arg) => resolve(arg, names),
        Expr::Bin(l, r) => {
            resolve(l, names)?;
            resolve(r, names)
        }
        Expr::Call(name, args) => {
            let Some((min, max)) = function(name) else {
                if names.contains(&name.as_str()) || CONSTANTS.contains(&name.as_str()) {
                    return Err(format!(
                        "{name} is not a function; write {name}*(…) to multiply."
                    ));
                }
                return Err(format!("Unknown function \"{name}\"."));
            };
            if args.len() < min || args.len() > max {
                let want = if min == max {
                    min.to_string()
                } else {
                    format!("{min} to {max}")
                };
                return Err(format!(
                    "{name} takes {want} argument{}, got {}.",
                    if max == 1 { "" } else { "s" },
                    args.len()
                ));
            }
            args.iter().try_for_each(|a| resolve(a, names))
        }
    }
}

/// Checks `source` as the renderer compiles it against the variables `names`.
pub fn check_expression(source: &str, names: &[&str]) -> Result<(), String> {
    if let Some(bad) = names.iter().find(|n| !is_valid_name(n)) {
        return Err(format!("\"{bad}\" cannot be used as a variable name."));
    }
    if source.encode_utf16().count() > MAX_EXPR_CHARS {
        return Err(format!(
            "The expression is longer than {MAX_EXPR_CHARS} characters."
        ));
    }
    if source.trim().is_empty() {
        return Err("The expression is empty.".to_string());
    }
    let tokens = tokenize(source)?;
    let tree = Parser {
        tokens,
        index: 0,
        depth: 0,
    }
    .parse_all()?;
    resolve(&tree, names)
}

/// Checks a JSON value used as an expression: text, or a finite number (always valid).
pub fn check_value(value: Option<&Value>, names: &[&str]) -> Result<(), String> {
    match value {
        Some(Value::String(text)) => check_expression(text, names),
        Some(Value::Number(n)) if n.as_f64().is_some_and(f64::is_finite) => {
            if let Some(bad) = names.iter().find(|n| !is_valid_name(n)) {
                return Err(format!("\"{bad}\" cannot be used as a variable name."));
            }
            Ok(())
        }
        _ => Err("An expression must be text or a number.".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(src: &str, names: &[&str]) -> bool {
        check_expression(src, names).is_ok()
    }

    #[test]
    fn grammar_matches_the_renderer() {
        assert!(ok("v0*x - g*x^2/2", &["v0", "x"]));
        assert!(ok("-x^2 + 2**3", &["x"]));
        assert!(ok("atan2(y, x) + max(1, 2, 3)", &["x", "y"]));
        assert!(ok(".5 + 1e-3 + 2.e2", &[]));
        assert!(ok("y < 0", &["y"]));
        assert!(!ok("2x", &["x"]));
        assert!(!ok("2(x)", &["x"]));
        assert!(!ok("0 < x < 1", &["x"]));
        assert!(!ok("sin", &[]));
        assert!(!ok("k(2)", &["k"]));
        assert!(!ok("atan2(1)", &[]));
        assert!(!ok("foo(1)", &[]));
        assert!(!ok("x +", &["x"]));
        assert!(!ok("(x", &["x"]));
        assert!(!ok("x $ 2", &["x"]));
        assert!(!ok("q", &[]));
        assert!(!ok("   ", &[]));
        assert!(!ok("1e999", &[]));
        // g is a constant unless a variable takes its name.
        assert!(ok("g", &[]));
        assert!(ok("g", &["g"]));
        assert!(!ok("1", &["pi"]));
    }

    #[test]
    fn limits_match_the_renderer() {
        let nested = |n: usize| format!("{}1{}", "(".repeat(n), ")".repeat(n));
        // The outer parse plus one per parenthesis.
        assert!(ok(&nested(MAX_EXPR_DEPTH - 1), &[]));
        assert!(!ok(&nested(MAX_EXPR_DEPTH), &[]));
        let long = format!("1{}", "+1".repeat((MAX_EXPR_CHARS - 1) / 2));
        assert_eq!(long.len(), MAX_EXPR_CHARS - 1);
        assert!(ok(&long, &[]));
        assert!(ok(&format!("{long} "), &[]));
        assert!(!ok(&format!("{long}+1"), &[]));
        assert!(is_valid_name("v0") && is_valid_name("a_b"));
        assert!(!is_valid_name("_a") && !is_valid_name("e") && !is_valid_name("sin"));
        assert!(!is_valid_name(&"a".repeat(25)));
    }

    #[test]
    fn json_numbers_are_expressions() {
        assert!(check_value(Some(&serde_json::json!(3.5)), &[]).is_ok());
        assert!(check_value(Some(&serde_json::json!("x")), &["x"]).is_ok());
        assert!(check_value(None, &[]).is_err());
        assert!(check_value(Some(&Value::Null), &[]).is_err());
        assert!(check_value(Some(&serde_json::json!([1])), &[]).is_err());
    }
}
