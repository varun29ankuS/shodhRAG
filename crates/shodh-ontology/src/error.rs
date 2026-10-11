//! Load-time errors. Every error that can be traced to authored TOML carries
//! the file name and the 1-based line and column of the offending definition.

use std::fmt;

/// A position inside an ontology source file.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
pub struct SourceLocation {
    /// Source name as given to the loader (usually the file path).
    pub file: String,
    /// 1-based line number.
    pub line: usize,
    /// 1-based column number (in characters).
    pub column: usize,
}

impl SourceLocation {
    /// Resolves a byte offset in `text` to a line/column location in `file`.
    pub fn from_offset(file: &str, text: &str, offset: usize) -> Self {
        let offset = offset.min(text.len());
        let mut line = 1;
        let mut line_start = 0;
        for (index, byte) in text.as_bytes().iter().enumerate().take(offset) {
            if *byte == b'\n' {
                line += 1;
                line_start = index + 1;
            }
        }
        let column = text
            .get(line_start..offset)
            .map(|prefix| prefix.chars().count() + 1)
            .unwrap_or(1);
        Self {
            file: file.to_owned(),
            line,
            column,
        }
    }
}

impl fmt::Display for SourceLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.file, self.line, self.column)
    }
}

/// What went wrong while loading or compiling an ontology.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum LoadErrorKind {
    /// The file could not be read.
    #[error("cannot read file: {0}")]
    Io(String),
    /// The TOML is malformed or does not match the authoring schema.
    #[error("invalid TOML: {0}")]
    Syntax(String),
    /// The file has none, or more than one, of the `[core]`, `[pack]`, `[extension]` headers.
    #[error("a source must declare exactly one of [core], [pack] or [extension]")]
    MissingHeader,
    /// No core ontology was supplied, or more than one.
    #[error("exactly one [core] source is required, found {0}")]
    CoreCount(usize),
    /// Two sources declare the same id.
    #[error("source id `{id}` is already used by {first}")]
    DuplicateSource {
        /// The duplicated source id.
        id: String,
        /// Where the id was first declared.
        first: SourceLocation,
    },
    /// A semantic version string did not parse.
    #[error("invalid semantic version `{value}`: {reason}")]
    InvalidVersion {
        /// The offending text.
        value: String,
        /// Parser message.
        reason: String,
    },
    /// A pack or extension requires a source that is absent or has an incompatible version.
    #[error("requires `{id}` {requirement}, but {found}")]
    UnmetRequirement {
        /// Required source id.
        id: String,
        /// Version requirement as written.
        requirement: String,
        /// What was actually loaded.
        found: String,
    },
    /// A namespace header is malformed.
    #[error("invalid namespace: {0}")]
    InvalidNamespace(String),
    /// A term id does not follow the naming rules.
    #[error("invalid {kind} id `{id}`: {rule}")]
    InvalidId {
        /// `class` or `property`.
        kind: &'static str,
        /// The offending id.
        id: String,
        /// The rule that was broken.
        rule: &'static str,
    },
    /// A term was defined twice, either in the same file or across layers.
    /// Packs and extensions can only add terms; they can never redefine one.
    #[error("`{id}` is already defined at {first}; packs and extensions may only add terms, never redefine them")]
    Redefinition {
        /// The redefined term.
        id: String,
        /// Where the term was first defined.
        first: SourceLocation,
    },
    /// A referenced class does not exist.
    #[error("unknown class `{0}`")]
    UnknownClass(String),
    /// A referenced property does not exist or does not apply to the class.
    #[error("property `{property}` does not apply to class `{class}`")]
    PropertyNotApplicable {
        /// The class being described.
        class: String,
        /// The referenced property.
        property: String,
    },
    /// The class hierarchy contains a cycle.
    #[error("class hierarchy cycle: {0}")]
    InheritanceCycle(String),
    /// A range names neither a datatype nor a known class.
    #[error("unknown range `{0}`: expected a datatype (String, Boolean, Integer, Decimal, Money, Date, DateTime, Url, Email, Enum) or a class id")]
    UnknownRange(String),
    /// A regular expression did not compile.
    #[error("invalid regular expression `{pattern}`: {reason}")]
    InvalidPattern {
        /// The pattern as written.
        pattern: String,
        /// Compiler message.
        reason: String,
    },
    /// A property definition is internally inconsistent.
    #[error("invalid property `{property}`: {reason}")]
    InvalidProperty {
        /// The property id.
        property: String,
        /// Why it is invalid.
        reason: String,
    },
    /// Memory dynamics parameters are out of range or inconsistent.
    #[error("invalid dynamics for class `{class}`: {reason}")]
    InvalidDynamics {
        /// The class id.
        class: String,
        /// Why they are invalid.
        reason: String,
    },
    /// A pack or extension tried to add a required property to a class it does not own,
    /// which would retroactively invalidate statements accepted under the owning layer.
    #[error("cannot add required property `{property}` to `{class}` (owned by `{owner}`); only the owning source may add required properties")]
    RequiredOnForeignClass {
        /// The property id.
        property: String,
        /// The class it targets.
        class: String,
        /// The source that defines the class.
        owner: String,
    },
    /// An unknown built-in pack was requested.
    #[error("unknown built-in pack `{0}`")]
    UnknownBuiltinPack(String),
}

/// A load error with its location, if the error can be traced to authored text.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub struct LoadError {
    /// Where the problem is. `None` only for errors about the set of sources as a whole.
    pub location: Option<SourceLocation>,
    /// What the problem is.
    pub kind: LoadErrorKind,
}

impl LoadError {
    pub(crate) fn at(location: SourceLocation, kind: LoadErrorKind) -> Self {
        Self {
            location: Some(location),
            kind,
        }
    }

    pub(crate) fn global(kind: LoadErrorKind) -> Self {
        Self {
            location: None,
            kind,
        }
    }
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.location {
            Some(location) => write!(f, "{location}: {}", self.kind),
            None => write!(f, "{}", self.kind),
        }
    }
}

/// All errors found while loading. The loader keeps going after the first error so
/// authors see every problem at once.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub struct LoadErrors(pub Vec<LoadError>);

impl LoadErrors {
    /// The individual errors, in source order.
    pub fn errors(&self) -> &[LoadError] {
        &self.0
    }
}

impl fmt::Display for LoadErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{} ontology error(s):", self.0.len())?;
        for error in &self.0 {
            writeln!(f, "  {error}")?;
        }
        Ok(())
    }
}
