//! Tailwind CSS's default colour palette (the values of `src/public/colors.js` at v3.4.17,
//! MIT, Copyright (c) Tailwind Labs, Inc.; see NOTICE), and colour
//! classes resolved against it and a project's own config colours.

use std::collections::HashMap;

use super::color::{self, Rgba};

/// The shades of every palette family, lightest first.
const SHADES: [u32; 11] = [50, 100, 200, 300, 400, 500, 600, 700, 800, 900, 950];

/// Tailwind's default palette: each family's hex values for [`SHADES`].
const PALETTE: [(&str, [&str; 11]); 22] = [
    (
        "slate",
        [
            "f8fafc", "f1f5f9", "e2e8f0", "cbd5e1", "94a3b8", "64748b", "475569", "334155",
            "1e293b", "0f172a", "020617",
        ],
    ),
    (
        "gray",
        [
            "f9fafb", "f3f4f6", "e5e7eb", "d1d5db", "9ca3af", "6b7280", "4b5563", "374151",
            "1f2937", "111827", "030712",
        ],
    ),
    (
        "zinc",
        [
            "fafafa", "f4f4f5", "e4e4e7", "d4d4d8", "a1a1aa", "71717a", "52525b", "3f3f46",
            "27272a", "18181b", "09090b",
        ],
    ),
    (
        "neutral",
        [
            "fafafa", "f5f5f5", "e5e5e5", "d4d4d4", "a3a3a3", "737373", "525252", "404040",
            "262626", "171717", "0a0a0a",
        ],
    ),
    (
        "stone",
        [
            "fafaf9", "f5f5f4", "e7e5e4", "d6d3d1", "a8a29e", "78716c", "57534e", "44403c",
            "292524", "1c1917", "0c0a09",
        ],
    ),
    (
        "red",
        [
            "fef2f2", "fee2e2", "fecaca", "fca5a5", "f87171", "ef4444", "dc2626", "b91c1c",
            "991b1b", "7f1d1d", "450a0a",
        ],
    ),
    (
        "orange",
        [
            "fff7ed", "ffedd5", "fed7aa", "fdba74", "fb923c", "f97316", "ea580c", "c2410c",
            "9a3412", "7c2d12", "431407",
        ],
    ),
    (
        "amber",
        [
            "fffbeb", "fef3c7", "fde68a", "fcd34d", "fbbf24", "f59e0b", "d97706", "b45309",
            "92400e", "78350f", "451a03",
        ],
    ),
    (
        "yellow",
        [
            "fefce8", "fef9c3", "fef08a", "fde047", "facc15", "eab308", "ca8a04", "a16207",
            "854d0e", "713f12", "422006",
        ],
    ),
    (
        "lime",
        [
            "f7fee7", "ecfccb", "d9f99d", "bef264", "a3e635", "84cc16", "65a30d", "4d7c0f",
            "3f6212", "365314", "1a2e05",
        ],
    ),
    (
        "green",
        [
            "f0fdf4", "dcfce7", "bbf7d0", "86efac", "4ade80", "22c55e", "16a34a", "15803d",
            "166534", "14532d", "052e16",
        ],
    ),
    (
        "emerald",
        [
            "ecfdf5", "d1fae5", "a7f3d0", "6ee7b7", "34d399", "10b981", "059669", "047857",
            "065f46", "064e3b", "022c22",
        ],
    ),
    (
        "teal",
        [
            "f0fdfa", "ccfbf1", "99f6e4", "5eead4", "2dd4bf", "14b8a6", "0d9488", "0f766e",
            "115e59", "134e4a", "042f2e",
        ],
    ),
    (
        "cyan",
        [
            "ecfeff", "cffafe", "a5f3fc", "67e8f9", "22d3ee", "06b6d4", "0891b2", "0e7490",
            "155e75", "164e63", "083344",
        ],
    ),
    (
        "sky",
        [
            "f0f9ff", "e0f2fe", "bae6fd", "7dd3fc", "38bdf8", "0ea5e9", "0284c7", "0369a1",
            "075985", "0c4a6e", "082f49",
        ],
    ),
    (
        "blue",
        [
            "eff6ff", "dbeafe", "bfdbfe", "93c5fd", "60a5fa", "3b82f6", "2563eb", "1d4ed8",
            "1e40af", "1e3a8a", "172554",
        ],
    ),
    (
        "indigo",
        [
            "eef2ff", "e0e7ff", "c7d2fe", "a5b4fc", "818cf8", "6366f1", "4f46e5", "4338ca",
            "3730a3", "312e81", "1e1b4b",
        ],
    ),
    (
        "violet",
        [
            "f5f3ff", "ede9fe", "ddd6fe", "c4b5fd", "a78bfa", "8b5cf6", "7c3aed", "6d28d9",
            "5b21b6", "4c1d95", "2e1065",
        ],
    ),
    (
        "purple",
        [
            "faf5ff", "f3e8ff", "e9d5ff", "d8b4fe", "c084fc", "a855f7", "9333ea", "7e22ce",
            "6b21a8", "581c87", "3b0764",
        ],
    ),
    (
        "fuchsia",
        [
            "fdf4ff", "fae8ff", "f5d0fe", "f0abfc", "e879f9", "d946ef", "c026d3", "a21caf",
            "86198f", "701a75", "4a044e",
        ],
    ),
    (
        "pink",
        [
            "fdf2f8", "fce7f3", "fbcfe8", "f9a8d4", "f472b6", "ec4899", "db2777", "be185d",
            "9d174d", "831843", "500724",
        ],
    ),
    (
        "rose",
        [
            "fff1f2", "ffe4e6", "fecdd3", "fda4af", "fb7185", "f43f5e", "e11d48", "be123c",
            "9f1239", "881337", "4c0519",
        ],
    ),
];

/// The default palette colour `name` (`gray-400`, `white`, `black`).
pub fn default_color(name: &str) -> Option<Rgba> {
    match name {
        "white" => return color::parse("#ffffff"),
        "black" => return color::parse("#000000"),
        _ => {}
    }
    let (family, shade) = name.rsplit_once('-')?;
    let shade: u32 = shade.parse().ok()?;
    let index = SHADES.iter().position(|s| *s == shade)?;
    let (_, hexes) = PALETTE.iter().find(|(f, _)| *f == family)?;
    color::parse(&format!("#{}", hexes[index]))
}

/// The colour a class's colour part names (what follows `text-` or `bg-`):
/// a project colour, a default palette colour, or an arbitrary value
/// (`[#999]`, `[rgb(0,0,0)]`), with an opacity modifier (`/50`, `/[0.3]`)
/// applied to its alpha. `None` when it is not a colour that can be resolved
/// (`current`, `inherit`, `[var(--x)]`, a size such as `sm`).
pub fn resolve(part: &str, project: &HashMap<String, Rgba>) -> Option<Rgba> {
    let (name, opacity) = if part.starts_with('[') {
        let close = part.find(']')?;
        (&part[..=close], part[close + 1..].strip_prefix('/'))
    } else {
        match part.split_once('/') {
            Some((name, alpha)) => (name, Some(alpha)),
            None => (part, None),
        }
    };
    let base = if let Some(inner) = name.strip_prefix('[').and_then(|n| n.strip_suffix(']')) {
        let inner = inner.strip_prefix("color:").unwrap_or(inner);
        color::parse(&inner.replace('_', " "))?
    } else {
        project.get(name).copied().or_else(|| default_color(name))?
    };
    let alpha = match opacity {
        None => 1.0,
        Some(a) => {
            let a = a.trim_start_matches('[').trim_end_matches(']');
            let value: f64 = a.trim_end_matches('%').parse().ok()?;
            if a.ends_with('%') || value > 1.0 {
                value / 100.0
            } else {
                value
            }
        }
    };
    Some(Rgba {
        a: base.a * alpha.clamp(0.0, 1.0),
        ..base
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_colours_and_modifiers_resolve() {
        let none = HashMap::new();
        let gray = resolve("gray-400", &none).unwrap();
        assert_eq!((gray.r, gray.g, gray.b, gray.a), (156.0, 163.0, 175.0, 1.0));
        assert_eq!(resolve("white/50", &none).unwrap().a, 0.5);
        assert_eq!(resolve("black/[0.3]", &none).unwrap().a, 0.3);
        assert_eq!(resolve("[#999]", &none).unwrap().r, 153.0);
        for unknown in [
            "sm",
            "current",
            "inherit",
            "[var(--x)]",
            "gray-450",
            "brand-500",
        ] {
            assert!(resolve(unknown, &none).is_none(), "{unknown}");
        }
        let mut project = HashMap::new();
        project.insert("brand-500".to_string(), color::parse("#123456").unwrap());
        assert_eq!(resolve("brand-500", &project).unwrap().b, 86.0);
        assert_eq!(PALETTE.len(), 22);
    }
}
