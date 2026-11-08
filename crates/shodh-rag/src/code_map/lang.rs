//! The languages the code map reads, by file extension, with their
//! tree-sitter grammars.

use tree_sitter::Language;

/// A source language of the code map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Lang {
    Rust,
    TypeScript,
    Tsx,
    JavaScript,
    Jsx,
    Python,
    Go,
}

/// Every language, in the order Settings and reports list them.
pub const LANGS: [Lang; 7] = [
    Lang::Rust,
    Lang::TypeScript,
    Lang::Tsx,
    Lang::JavaScript,
    Lang::Jsx,
    Lang::Python,
    Lang::Go,
];

impl Lang {
    /// The language of `path` (relative, `/`-separated) by its extension.
    pub fn of_path(path: &str) -> Option<Lang> {
        let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
        let (_, ext) = name.rsplit_once('.')?;
        Some(match ext.to_ascii_lowercase().as_str() {
            "rs" => Lang::Rust,
            "ts" | "mts" | "cts" => Lang::TypeScript,
            "tsx" => Lang::Tsx,
            "js" | "mjs" | "cjs" => Lang::JavaScript,
            "jsx" => Lang::Jsx,
            "py" | "pyi" => Lang::Python,
            "go" => Lang::Go,
            _ => return None,
        })
    }

    /// The name stored and shown (also the code pane's highlighter name).
    pub fn name(self) -> &'static str {
        match self {
            Lang::Rust => "rust",
            Lang::TypeScript => "typescript",
            Lang::Tsx => "tsx",
            Lang::JavaScript => "javascript",
            Lang::Jsx => "jsx",
            Lang::Python => "python",
            Lang::Go => "go",
        }
    }

    pub fn from_name(name: &str) -> Option<Lang> {
        LANGS.into_iter().find(|lang| lang.name() == name)
    }

    /// TypeScript or JavaScript, with or without JSX.
    pub fn is_script(self) -> bool {
        matches!(
            self,
            Lang::TypeScript | Lang::Tsx | Lang::JavaScript | Lang::Jsx
        )
    }

    /// The tree-sitter grammar that parses it.
    pub fn grammar(self) -> Language {
        match self {
            Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
            Lang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Lang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Lang::JavaScript | Lang::Jsx => tree_sitter_javascript::LANGUAGE.into(),
            Lang::Python => tree_sitter_python::LANGUAGE.into(),
            Lang::Go => tree_sitter_go::LANGUAGE.into(),
        }
    }
}

/// A file the code map reads for how the project is put together, not for
/// its symbols: crate manifests, TypeScript path aliases, Go module paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConfigKind {
    /// `Cargo.toml`: a crate's name, roots and path dependencies.
    Cargo,
    /// `tsconfig*.json` / `jsconfig*.json`: `baseUrl`, `paths` and `extends`.
    TsConfig,
    /// `go.mod`: the module path.
    GoMod,
}

impl ConfigKind {
    /// The kind of config `path` is, by its file name.
    pub fn of_path(path: &str) -> Option<ConfigKind> {
        let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
        let lower = name.to_ascii_lowercase();
        if name == "Cargo.toml" {
            Some(ConfigKind::Cargo)
        } else if name == "go.mod" {
            Some(ConfigKind::GoMod)
        } else if (lower.starts_with("tsconfig") || lower.starts_with("jsconfig"))
            && lower.ends_with(".json")
        {
            Some(ConfigKind::TsConfig)
        } else {
            None
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ConfigKind::Cargo => "cargo",
            ConfigKind::TsConfig => "tsconfig",
            ConfigKind::GoMod => "gomod",
        }
    }

    pub fn from_name(name: &str) -> Option<ConfigKind> {
        [ConfigKind::Cargo, ConfigKind::TsConfig, ConfigKind::GoMod]
            .into_iter()
            .find(|kind| kind.name() == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_and_configs_are_known_by_their_names() {
        assert_eq!(Lang::of_path("src/a.rs"), Some(Lang::Rust));
        assert_eq!(Lang::of_path("web/App.TSX"), Some(Lang::Tsx));
        assert_eq!(Lang::of_path("x/types.d.ts"), Some(Lang::TypeScript));
        assert_eq!(Lang::of_path("tool.py"), Some(Lang::Python));
        assert_eq!(Lang::of_path("cmd/main.go"), Some(Lang::Go));
        assert_eq!(Lang::of_path("README.md"), None);
        assert_eq!(Lang::of_path("Makefile"), None);
        for lang in LANGS {
            assert_eq!(Lang::from_name(lang.name()), Some(lang));
        }
        assert_eq!(
            ConfigKind::of_path("crates/x/Cargo.toml"),
            Some(ConfigKind::Cargo)
        );
        assert_eq!(
            ConfigKind::of_path("app/tsconfig.app.json"),
            Some(ConfigKind::TsConfig)
        );
        assert_eq!(ConfigKind::of_path("go.mod"), Some(ConfigKind::GoMod));
        assert_eq!(ConfigKind::of_path("package.json"), None);
    }

    #[test]
    fn every_grammar_loads_into_a_parser() {
        for lang in LANGS {
            let mut parser = tree_sitter::Parser::new();
            parser
                .set_language(&lang.grammar())
                .unwrap_or_else(|e| panic!("{}: {e}", lang.name()));
        }
    }
}
