//! The log is a plain file kept for weeks and attached to bug reports: indexing a file
//! and searching it must write neither the question nor the file's name at INFO.
//!
//! Its own test binary (its own process, one test): the capturing subscriber is the
//! global default before anything logs, so every log line registers with it in place.
//! In the library's test binary, tests running in parallel on other threads could reach
//! a line first and leave it cached as unwanted, and the capture missed it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use shodh_rag::config::RAGConfig;
use shodh_rag::embeddings::EmbeddingModel;
use shodh_rag::rag_engine::{RAGEngine, SearchModels};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

const DIM: usize = 256;

/// Each distinct lower-cased word gets its own dimension, L2-normalised: texts sharing
/// words are similar.
#[derive(Default)]
struct WordEmbedder {
    vocabulary: Mutex<HashMap<String, usize>>,
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

/// Records every event at INFO or above with its fields.
struct CapturedLog(Arc<Mutex<Vec<String>>>);

impl<S: tracing::Subscriber> Layer<S> for CapturedLog {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        struct Fields(String);
        impl tracing::field::Visit for Fields {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                self.0.push_str(&format!(" {}={:?}", field.name(), value));
            }
        }
        let mut fields = Fields(String::new());
        event.record(&mut fields);
        let meta = event.metadata();
        self.0
            .lock()
            .unwrap()
            .push(format!("{} {}{}", meta.level(), meta.target(), fields.0));
    }
}

const PAPER: &str = r"\documentclass{article}
\begin{document}
\section{Introduction}
We show how the delta rule update of linear transformers is parallelized over
sequence length with a chunkwise form, so training on long sequences is efficient.
\section{Experiments}
Language models trained on long documents reach lower perplexity.
\end{document}
";

/// "Indexed file" named the source and "Hybrid search candidates" carried the query
/// and every hit's path.
#[test]
fn indexing_and_search_log_no_question_and_no_file_name() {
    let lines = Arc::new(Mutex::new(Vec::new()));
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry()
            .with(tracing_subscriber::filter::LevelFilter::INFO)
            .with(CapturedLog(lines.clone())),
    )
    .expect("the only subscriber of this test binary");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let results = runtime.block_on(async {
        let mut config = RAGConfig::default();
        config.data_dir = dir.path().join("data");
        config.embedding.model_dir = dir.path().join("models");
        config.embedding.use_e5 = false;
        config.embedding.dimension = DIM;
        config.search.min_score_threshold = 0.0;
        let mut engine = RAGEngine::new(config).await.unwrap();
        engine
            .attach_search_models(SearchModels::from_embedder(Arc::new(
                WordEmbedder::default(),
            )))
            .unwrap();
        let paper = dir.path().join("deltanet.tex");
        std::fs::write(&paper, PAPER).unwrap();
        engine
            .add_document_from_file(&paper, HashMap::new())
            .await
            .unwrap();
        Box::pin(engine.search_comprehensive(
            "delta rule parallelized over sequence length",
            5,
            None,
        ))
        .await
        .unwrap()
    });
    assert!(!results.is_empty());

    let lines = lines.lock().unwrap().clone();
    assert!(
        lines.iter().any(|l| l.contains("Indexed file")),
        "{lines:#?}"
    );
    assert!(
        lines.iter().any(|l| l.contains("Hybrid search candidates")),
        "{lines:#?}"
    );
    for line in &lines {
        assert!(!line.contains("deltanet"), "names the file: {line}");
        assert!(
            !line.contains("parallelized"),
            "carries the question: {line}"
        );
    }
}
