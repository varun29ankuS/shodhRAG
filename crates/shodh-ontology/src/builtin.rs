//! Ontology sources shipped with the crate.

/// Source name used in error locations for the built-in core.
pub const CORE_NAME: &str = "ontology/core.toml";

/// The built-in core ontology (TOML).
pub const CORE_TOML: &str = include_str!("../ontology/core.toml");

/// The built-in research pack (TOML).
pub const RESEARCH_TOML: &str = include_str!("../ontology/packs/research.toml");

/// Built-in packs: (name, source name, TOML text).
pub const PACKS: &[(&str, &str, &str)] = &[("research", "ontology/packs/research.toml", RESEARCH_TOML)];

/// Looks up a built-in pack by name, returning its source name and text.
pub fn pack(name: &str) -> Option<(&'static str, &'static str)> {
    PACKS
        .iter()
        .find(|(pack, _, _)| *pack == name)
        .map(|(_, file, text)| (*file, *text))
}
