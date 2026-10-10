//! Test support: a deterministic bag-of-words embedder and a settable clock.

use std::sync::{Arc, Mutex};

use chrono::{DateTime, TimeZone, Utc};
use semver::Version;
use shodh_ontology::{Extractor, ExtractorKind, Ontology, Provenance, RawValue, Statement};

use super::{Clock, DynamicsStore, EmbedderSource, StatementResult, StatementStore};
use crate::embeddings::EmbeddingModel;

pub(crate) const DIM: usize = 256;

/// Gives each distinct lower-cased word its own dimension (a per-embedder vocabulary) and
/// L2-normalises: texts sharing words are similar, texts sharing none are orthogonal.
#[derive(Default)]
pub(crate) struct WordEmbedder {
    vocabulary: Mutex<std::collections::HashMap<String, usize>>,
}

impl WordEmbedder {
    fn embed(&self, text: &str) -> Vec<f32> {
        let mut v = vec![0.0f32; DIM];
        let mut vocabulary = self.vocabulary.lock().unwrap();
        for word in text
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() > 1)
        {
            let next = vocabulary.len();
            let index = *vocabulary.entry(word.to_lowercase()).or_insert(next);
            assert!(index < DIM, "test vocabulary exceeded {DIM} words");
            v[index] += 1.0;
        }
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm == 0.0 {
            v[DIM - 1] = 1.0;
        } else {
            v.iter_mut().for_each(|x| *x /= norm);
        }
        v
    }
}

impl EmbeddingModel for WordEmbedder {
    fn embed_query(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        Ok(self.embed(text))
    }
    fn embed_document(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        Ok(self.embed(text))
    }
    fn dimension(&self) -> usize {
        DIM
    }
}

/// Hashes each lower-cased word into one of [`DIM`] dimensions and L2-normalises: any
/// vocabulary size (for fixtures larger than [`WordEmbedder`] allows).
#[derive(Default)]
pub(crate) struct HashEmbedder;

impl EmbeddingModel for HashEmbedder {
    fn embed_query(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        self.embed_document(text)
    }
    fn embed_document(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        use std::hash::{Hash, Hasher};
        let mut v = vec![0.0f32; DIM];
        for word in text.split(|c: char| !c.is_alphanumeric()) {
            if word.is_empty() {
                continue;
            }
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            word.to_lowercase().hash(&mut hasher);
            v[(hasher.finish() % DIM as u64) as usize] += 1.0;
        }
        let norm = v
            .iter()
            .map(|x| x * x)
            .sum::<f32>()
            .sqrt()
            .max(f32::EPSILON);
        Ok(v.into_iter().map(|x| x / norm).collect())
    }
    fn dimension(&self) -> usize {
        DIM
    }
}

pub(crate) struct FixedEmbedder(pub Arc<dyn EmbeddingModel>);

#[async_trait::async_trait]
impl EmbedderSource for FixedEmbedder {
    async fn embedder(&self) -> StatementResult<Arc<dyn EmbeddingModel>> {
        Ok(self.0.clone())
    }
}

/// A clock tests move by hand.
pub(crate) struct TestClock(Mutex<DateTime<Utc>>);

impl TestClock {
    pub(crate) fn at(start: DateTime<Utc>) -> Arc<Self> {
        Arc::new(Self(Mutex::new(start)))
    }
    pub(crate) fn set(&self, at: DateTime<Utc>) {
        *self.0.lock().unwrap() = at;
    }
    pub(crate) fn advance_days(&self, days: i64) {
        let mut now = self.0.lock().unwrap();
        *now += chrono::Duration::days(days);
    }
}

impl Clock for TestClock {
    fn now(&self) -> DateTime<Utc> {
        *self.0.lock().unwrap()
    }
}

pub(crate) fn t0() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, 10, 9, 0, 0).unwrap()
}

pub(crate) fn ontology() -> Arc<Ontology> {
    Arc::new(Ontology::builtin().unwrap())
}

pub(crate) struct Fixture {
    pub dir: tempfile::TempDir,
    pub clock: Arc<TestClock>,
    pub store: Arc<StatementStore>,
}

pub(crate) async fn fixture() -> Fixture {
    fixture_with(Arc::new(WordEmbedder::default())).await
}

/// A fixture embedding with `embedder` (a real model in model-backed tests).
pub(crate) async fn fixture_with(embedder: Arc<dyn EmbeddingModel>) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let clock = TestClock::at(t0());
    let dynamics = Arc::new(DynamicsStore::open(&dir.path().join("shodh.db"), None).unwrap());
    let dimension = embedder.dimension();
    let store = StatementStore::open(
        &dir.path().join("lance_data"),
        dimension,
        ontology(),
        dynamics,
        Arc::new(FixedEmbedder(embedder)),
        clock.clone(),
    )
    .await
    .unwrap();
    Fixture {
        dir,
        clock,
        store: Arc::new(store),
    }
}

/// A user-stated statement extracted at `at`.
pub(crate) fn statement(
    id: &str,
    class: &str,
    properties: &[(&str, RawValue)],
    at: DateTime<Utc>,
) -> Statement {
    Statement {
        id: id.to_string(),
        class: class.to_string(),
        subject: None,
        properties: properties
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
        ontology_version: Version::new(1, 0, 0),
        valid_from: None,
        provenance: Some(Provenance {
            source: "conversation://c1/turn/r1".to_string(),
            generation: 0,
            page: None,
            span: None,
            extractor: Extractor {
                kind: ExtractorKind::User,
                version: "test".to_string(),
            },
            confidence: 1.0,
            extracted_at: at,
        }),
    }
}

pub(crate) fn preference(id: &str, topic: &str, value: &str, at: DateTime<Utc>) -> Statement {
    statement(
        id,
        "Preference",
        &[
            (
                "preferenceHolder",
                RawValue::Entity(shodh_ontology::EntityRef::typed("person:self", "Person")),
            ),
            ("preferenceTopic", RawValue::text(topic)),
            ("preferenceValue", RawValue::text(value)),
        ],
        at,
    )
}

pub(crate) fn note(id: &str, text: &str, at: DateTime<Utc>) -> Statement {
    statement(id, "Note", &[("noteText", RawValue::text(text))], at)
}
