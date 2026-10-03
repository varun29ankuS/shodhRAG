//! # shodh-ontology
//!
//! The explicit, versioned ontology every structured thing in Shodh is typed against:
//! records, entities, graph edges, memory statements, snippets and research results.
//!
//! - **Authoring:** TOML files (`ontology/core.toml`, `ontology/packs/*.toml`, workspace
//!   extensions) merged by [`OntologyBuilder`] under an add-only rule. Errors name the
//!   file and line.
//! - **Validation:** [`Ontology::validate`] turns an extractor's [`Statement`] into a
//!   [`ValidStatement`] or a list of [`Violation`]s. Statements without provenance are
//!   rejected.
//! - **Supersede:** [`Ontology::supersedes`] decides deterministically how a new statement
//!   relates to an existing one (temporal functional properties supersede with history).
//! - **Dynamics:** every class carries memory [`Dynamics`] (decay half-life,
//!   reinforcement, expiry) for the memory module.
//! - **Slicing:** [`Ontology::slice_for`] selects the classes relevant to a text for
//!   constrained extraction, rendered compactly by [`OntologySlice::render_prompt`].
//! - **Versioning:** [`OntologyDiff::between`] lists changed terms and affected classes.
//! - **Export:** [`Ontology::to_turtle`] (OWL 2) and [`Ontology::statements_to_turtle`]
//!   (statements with PROV-O). OWL/Turtle import is not implemented.
//!
//! There is no RDF/OWL runtime: queries run through typed tools over LanceDB/SQLite.

pub mod builtin;
mod diff;
mod error;
mod loader;
mod model;
mod slice;
mod statement;
mod supersede;
mod turtle;
mod value;

pub use diff::{ClassField, Compatibility, OntologyDiff, PropertyField, TermChange};
pub use error::{LoadError, LoadErrorKind, LoadErrors, SourceLocation};
pub use loader::{OntologyBuilder, ROOT_CLASS};
pub use model::{
    Cardinality, Class, Datatype, Dynamics, Expiry, Layer, Namespace, Ontology, Property, Range,
    SourceInfo,
};
pub use slice::{OntologySlice, SliceClass, SliceMatch, SliceRole};
pub use statement::{
    Extractor, ExtractorKind, Provenance, Statement, TextSpan, ValidStatement, Violation,
};
pub use supersede::{IndependenceReason, PropertyChange, SubjectMatch, SupersedeDecision};
pub use turtle::{StatementExportOptions, SHODH_PROV};
pub use value::{Decimal, DecimalError, EntityRef, Money, RawMoney, RawValue, Value};
