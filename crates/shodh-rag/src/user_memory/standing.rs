//! Core memories: the small set of standing context put in front of every question of a
//! scope, whatever it asks, selected by class and not by similarity.
//!
//! Recall admits a memory only when its cosine similarity to the question reaches the
//! relevance gate ([`super::DEFAULT_MIN_SIMILARITY`]). Standing context — who the user is,
//! their projects, their preferences and rules such as "keep answers short", the notes they
//! asked Shodh to keep — is close to no particular question, so the gate leaves it out
//! most often. The core is:
//!
//! - current memories (not superseded, archived, expired or forgotten) visible from the
//!   run's scope (its own and global ones: a workspace chat never sees another
//!   workspace's core);
//! - of the classes `Person` about the user (subject `person:self`), `Project`,
//!   `Preference` (which holds the user's rules: what they require or avoid), and `Note`
//!   in the user's own words (extractor `user`: typed in Settings, asked for in a
//!   conversation, or approved as written; notes the learner formulated are not core);
//! - read with a filtered query on the statement store (no vector search), ranked pinned
//!   first, then strongest now, then most important, at most [`CORE_MAX_ITEMS`]; the
//!   rendered block is capped at [`CORE_MAX_CHARS`] ([`render_core_counted`]).
//!
//! Selecting a memory into the core is not use: like every injected memory it is
//! reinforced only when the answer used it ([`super::MemoryService::record_answer_use`]).

use std::collections::HashSet;

use shodh_ontology::ExtractorKind;

use super::recall::{prefer_workspace, render_block};
use super::{memory_source_prefixes, MemoryResult, MemoryService, RecalledMemory};
use crate::statements::{identity_tokens, Scope, StatementQuery, StoredStatement, SELF_ENTITY_ID};

/// Most core memories put in front of one question.
pub const CORE_MAX_ITEMS: usize = 20;

/// Most characters of the core block (≈ 375 tokens), on top of the recalled memories'
/// [`super::MAX_INJECTION_CHARS`].
pub const CORE_MAX_CHARS: usize = 1_500;

/// Title of the core block.
pub const CORE_BLOCK_TITLE: &str = "What you always keep in mind about the user (may be outdated)";

/// Core classes (each with its subclasses) whatever their subject or origin.
const STANDING_CLASSES: [&str; 2] = ["Project", "Preference"];

/// The core class kept only when the user wrote or approved its words.
const NOTE_CLASS: &str = "Note";

/// The core class kept only when it is about the user.
const PERSON_CLASS: &str = "Person";

/// The core memories of `scope`, best first, at most [`CORE_MAX_ITEMS`]. Read-only.
pub(super) async fn select(
    service: &MemoryService,
    scope: &Scope,
) -> MemoryResult<Vec<RecalledMemory>> {
    let store = service.store();
    let ontology = store.ontology();
    let base = StatementQuery {
        scopes: scope.visible(),
        source_prefixes: memory_source_prefixes(),
        ..Default::default()
    };
    let mut classes: Vec<String> = STANDING_CLASSES.iter().map(|c| c.to_string()).collect();
    classes.push(NOTE_CLASS.to_string());
    let standing = store
        .query_all(&StatementQuery {
            classes,
            ..base.clone()
        })
        .await?;
    let about_user = store
        .query_all(&StatementQuery {
            classes: vec![PERSON_CLASS.to_string()],
            subject: Some(SELF_ENTITY_ID.to_string()),
            ..base
        })
        .await?;
    let now = store.now();
    let visible: HashSet<Scope> = scope.visible().into_iter().collect();
    let mut seen = HashSet::new();
    let candidates: Vec<StoredStatement> = standing
        .into_iter()
        .chain(about_user)
        .filter(|s| seen.insert(s.id().to_string()))
        .filter(|s| s.is_current_at(now) && visible.contains(&s.scope) && is_core(service, s))
        .collect();
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    let states = service.states_of(&candidates).await?;
    let mut ranked: Vec<(RecalledMemory, Vec<String>)> = candidates
        .iter()
        .map(|s| {
            let state = states
                .get(s.id())
                .cloned()
                .unwrap_or_else(|| super::default_state(s));
            let record = service.record(s, &state);
            let tokens = ontology
                .validate(&s.statement)
                .map(|valid| identity_tokens(ontology, &valid))
                .unwrap_or_default();
            let recalled = RecalledMemory {
                score: record.strength,
                relevance: 0.0,
                activation: record.strength,
                association: 0.0,
                memory: record,
            };
            (recalled, tokens)
        })
        .collect();
    ranked.sort_by(|a, b| core_order(&a.0, &b.0));
    let mut kept = prefer_workspace(ranked, scope);
    kept.truncate(CORE_MAX_ITEMS);
    Ok(kept)
}

/// Pinned first, then strongest now, then most important, then by id (stable).
fn core_order(a: &RecalledMemory, b: &RecalledMemory) -> std::cmp::Ordering {
    b.memory
        .pinned
        .cmp(&a.memory.pinned)
        .then_with(|| b.memory.strength.total_cmp(&a.memory.strength))
        .then_with(|| b.memory.importance.total_cmp(&a.memory.importance))
        .then_with(|| a.memory.id.cmp(&b.memory.id))
}

/// Whether a current memory of the queried classes belongs to the core.
fn is_core(service: &MemoryService, stored: &StoredStatement) -> bool {
    let ontology = service.store().ontology();
    let class = stored.statement.class.as_str();
    let is_a = |ancestor: &str| class == ancestor || ontology.is_subclass_of(class, ancestor);
    if STANDING_CLASSES.iter().any(|c| is_a(c)) {
        return true;
    }
    if is_a(PERSON_CLASS) {
        return stored
            .statement
            .subject
            .as_ref()
            .is_some_and(|s| s.id == SELF_ENTITY_ID);
    }
    if is_a(NOTE_CLASS) {
        return stored
            .statement
            .provenance
            .as_ref()
            .is_some_and(|p| matches!(p.extractor.kind, ExtractorKind::User));
    }
    false
}

/// The core block for `core` (best first) within [`CORE_MAX_CHARS`], and how many of
/// `core` (from the front) it holds. `None` when there is none.
pub fn render_core_counted(core: &[RecalledMemory]) -> Option<(String, usize)> {
    render_block(CORE_BLOCK_TITLE, core, CORE_MAX_CHARS)
}
