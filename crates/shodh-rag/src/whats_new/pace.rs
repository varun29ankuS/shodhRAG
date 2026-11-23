//! Spacing between requests to each scholarly API, across every paper alert of the
//! process: arXiv asks for 3 seconds between calls (its API manual); OpenAlex and
//! Semantic Scholar get 1 second, well inside OpenAlex's keyless 10 requests a second
//! and gentle on Semantic Scholar's shared keyless pool.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio::time::Instant;

use crate::harness::web::papers::PaperApi;

/// The least time between two requests to `api`.
pub fn spacing(api: PaperApi) -> Duration {
    match api {
        PaperApi::Arxiv => Duration::from_secs(3),
        PaperApi::OpenAlex | PaperApi::SemanticScholar => Duration::from_secs(1),
    }
}

/// When each API was last asked.
#[derive(Debug, Default)]
pub struct ApiPacer {
    last: Mutex<HashMap<PaperApi, Instant>>,
}

impl ApiPacer {
    /// The pacer every paper alert of this process shares.
    pub fn shared() -> &'static ApiPacer {
        static PACER: OnceLock<ApiPacer> = OnceLock::new();
        PACER.get_or_init(ApiPacer::default)
    }

    /// Waits until `api` may be asked again, then records the request as made now.
    pub async fn wait_turn(&self, api: PaperApi) {
        let mut last = self.last.lock().await;
        if let Some(at) = last.get(&api) {
            tokio::time::sleep_until(*at + spacing(api)).await;
        }
        last.insert(api, Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn the_pacer_spaces_arxiv_by_three_seconds_and_the_others_by_one() {
        let pacer = ApiPacer::default();
        let start = Instant::now();
        pacer.wait_turn(PaperApi::Arxiv).await;
        assert_eq!(
            start.elapsed(),
            Duration::ZERO,
            "the first request waits for nothing"
        );
        pacer.wait_turn(PaperApi::Arxiv).await;
        assert!(start.elapsed() >= Duration::from_secs(3));
        let before = Instant::now();
        pacer.wait_turn(PaperApi::OpenAlex).await;
        assert_eq!(
            before.elapsed(),
            Duration::ZERO,
            "each API has its own spacing"
        );
        pacer.wait_turn(PaperApi::OpenAlex).await;
        assert!(before.elapsed() >= Duration::from_secs(1));
        assert!(before.elapsed() < Duration::from_secs(3));
    }
}
