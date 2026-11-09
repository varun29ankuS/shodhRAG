//! Colours as source text writes them: hex, `rgb()`, `hsl()` and the common
//! named colours, with the WCAG contrast ratio and the hue tests the rules use.

use std::sync::LazyLock;

use regex::Regex;

/// A colour, channels 0–255 and alpha 0–1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub a: f64,
}

const NAMED: [(&str, [u8; 3]); 44] = [
    ("white", [255, 255, 255]),
    ("black", [0, 0, 0]),
    ("red", [255, 0, 0]),
    ("green", [0, 128, 0]),
    ("blue", [0, 0, 255]),
    ("yellow", [255, 255, 0]),
    ("orange", [255, 165, 0]),
    ("purple", [128, 0, 128]),
    ("violet", [238, 130, 238]),
    ("indigo", [75, 0, 130]),
    ("cyan", [0, 255, 255]),
    ("aqua", [0, 255, 255]),
    ("magenta", [255, 0, 255]),
    ("fuchsia", [255, 0, 255]),
    ("pink", [255, 192, 203]),
    ("hotpink", [255, 105, 180]),
    ("gray", [128, 128, 128]),
    ("grey", [128, 128, 128]),
    ("silver", [192, 192, 192]),
    ("lightgray", [211, 211, 211]),
    ("lightgrey", [211, 211, 211]),
    ("darkgray", [169, 169, 169]),
    ("darkgrey", [169, 169, 169]),
    ("dimgray", [105, 105, 105]),
    ("gainsboro", [220, 220, 220]),
    ("whitesmoke", [245, 245, 245]),
    ("navy", [0, 0, 128]),
    ("teal", [0, 128, 128]),
    ("lime", [0, 255, 0]),
    ("maroon", [128, 0, 0]),
    ("olive", [128, 128, 0]),
    ("rebeccapurple", [102, 51, 153]),
    ("blueviolet", [138, 43, 226]),
    ("mediumpurple", [147, 112, 219]),
    ("darkviolet", [148, 0, 211]),
    ("slateblue", [106, 90, 205]),
    ("mediumslateblue", [123, 104, 238]),
    ("beige", [245, 245, 220]),
    ("ivory", [255, 255, 240]),
    ("linen", [250, 240, 230]),
    ("cornsilk", [255, 248, 220]),
    ("oldlace", [253, 245, 230]),
    ("antiquewhite", [250, 235, 215]),
    ("wheat", [245, 222, 179]),
];

static COLOR_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)#[0-9a-f]{3,8}\b|(?:rgba?|hsla?)\([^)]*\)|\b[a-z]+\b").expect("colour pattern")
});

fn num(text: &str) -> Option<f64> {
    text.trim().trim_end_matches('%').parse::<f64>().ok()
}

/// One colour written as `text` (the whole of it).
pub fn parse(text: &str) -> Option<Rgba> {
    let t = text.trim().to_ascii_lowercase();
    if t == "transparent" {
        return Some(Rgba {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        });
    }
    if let Some(hex) = t.strip_prefix('#') {
        let digits: Vec<f64> = hex
            .chars()
            .map(|c| c.to_digit(16).map(f64::from))
            .collect::<Option<Vec<_>>>()?;
        let (rgb, a) = match digits.len() {
            3 | 4 => (
                [digits[0] * 17.0, digits[1] * 17.0, digits[2] * 17.0],
                digits.get(3).map(|d| d * 17.0 / 255.0).unwrap_or(1.0),
            ),
            6 | 8 => (
                [
                    digits[0] * 16.0 + digits[1],
                    digits[2] * 16.0 + digits[3],
                    digits[4] * 16.0 + digits[5],
                ],
                if digits.len() == 8 {
                    (digits[6] * 16.0 + digits[7]) / 255.0
                } else {
                    1.0
                },
            ),
            _ => return None,
        };
        return Some(Rgba {
            r: rgb[0],
            g: rgb[1],
            b: rgb[2],
            a,
        });
    }
    if let Some(open) = t.find('(') {
        let func = &t[..open];
        let inner = t[open + 1..].trim_end_matches(')');
        let parts: Vec<&str> = inner
            .split(|c: char| c == ',' || c == '/' || c.is_whitespace())
            .filter(|p| !p.is_empty())
            .collect();
        if parts.len() < 3 {
            return None;
        }
        let alpha = parts
            .get(3)
            .and_then(|a| {
                let v = num(a)?;
                Some(if a.ends_with('%') { v / 100.0 } else { v })
            })
            .unwrap_or(1.0);
        return match func {
            "rgb" | "rgba" => Some(Rgba {
                r: num(parts[0])?,
                g: num(parts[1])?,
                b: num(parts[2])?,
                a: alpha,
            }),
            "hsl" | "hsla" => {
                let h = parts[0].trim_end_matches("deg").parse::<f64>().ok()?;
                let (r, g, b) = hsl_to_rgb(h, num(parts[1])? / 100.0, num(parts[2])? / 100.0);
                Some(Rgba { r, g, b, a: alpha })
            }
            _ => None,
        };
    }
    NAMED
        .iter()
        .find(|(name, _)| *name == t)
        .map(|(_, [r, g, b])| Rgba {
            r: f64::from(*r),
            g: f64::from(*g),
            b: f64::from(*b),
            a: 1.0,
        })
}

/// Every colour written in `value`, in order.
pub fn colors_in(value: &str) -> Vec<Rgba> {
    COLOR_RE
        .find_iter(value)
        .filter_map(|m| parse(m.as_str()))
        .collect()
}

fn hsl_to_rgb(h: f64, s: f64, l: f64) -> (f64, f64, f64) {
    let h = h.rem_euclid(360.0) / 360.0;
    if s <= 0.0 {
        let v = l * 255.0;
        return (v, v, v);
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let channel = |t: f64| {
        let t = t.rem_euclid(1.0);
        let v = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        v * 255.0
    };
    (channel(h + 1.0 / 3.0), channel(h), channel(h - 1.0 / 3.0))
}

impl Rgba {
    /// Hue in degrees, saturation and lightness 0–1.
    pub fn hsl(&self) -> (f64, f64, f64) {
        let (r, g, b) = (self.r / 255.0, self.g / 255.0, self.b / 255.0);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let l = (max + min) / 2.0;
        if (max - min).abs() < f64::EPSILON {
            return (0.0, 0.0, l);
        }
        let d = max - min;
        let s = if l > 0.5 {
            d / (2.0 - max - min)
        } else {
            d / (max + min)
        };
        let h = if max == r {
            (g - b) / d + if g < b { 6.0 } else { 0.0 }
        } else if max == g {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        };
        (h * 60.0, s, l)
    }

    /// No real hue: the channels are within 30 of each other (impeccable's
    /// neutral test), or it is nearly invisible.
    pub fn is_neutral(&self) -> bool {
        let max = self.r.max(self.g).max(self.b);
        let min = self.r.min(self.g).min(self.b);
        max - min < 30.0 || self.a < 0.1
    }

    /// A saturated colour that reads as a hue, not a tint or a near-black.
    pub fn is_chromatic(&self) -> bool {
        let (_, s, l) = self.hsl();
        !self.is_neutral() && s >= 0.25 && (0.15..=0.92).contains(&l)
    }

    /// Purple, violet or indigo: the generated-UI default hues.
    pub fn is_purple(&self) -> bool {
        let (h, _, _) = self.hsl();
        self.is_chromatic() && (240.0..=300.0).contains(&h)
    }

    /// A warm cream or beige: light, warm and only lightly saturated.
    pub fn is_cream(&self) -> bool {
        let (h, s, l) = self.hsl();
        !self.is_neutral_strict()
            && (25.0..=60.0).contains(&h)
            && (0.15..=0.9).contains(&s)
            && l >= 0.86
            && l < 0.98
    }

    fn is_neutral_strict(&self) -> bool {
        let max = self.r.max(self.g).max(self.b);
        let min = self.r.min(self.g).min(self.b);
        max - min < 6.0
    }

    /// WCAG relative luminance.
    pub fn luminance(&self) -> f64 {
        let lin = |c: f64| {
            let c = c / 255.0;
            if c <= 0.039_28 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(self.r) + 0.7152 * lin(self.g) + 0.0722 * lin(self.b)
    }

    /// This colour painted over the opaque `ground`.
    pub fn over(&self, ground: &Rgba) -> Rgba {
        let a = self.a.clamp(0.0, 1.0);
        Rgba {
            r: self.r * a + ground.r * (1.0 - a),
            g: self.g * a + ground.g * (1.0 - a),
            b: self.b * a + ground.b * (1.0 - a),
            a: 1.0,
        }
    }
}

/// The WCAG contrast ratio of two opaque colours.
pub fn contrast(a: &Rgba, b: &Rgba) -> f64 {
    let (la, lb) = (a.luminance(), b.luminance());
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}
