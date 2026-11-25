//! Spacing between requests to each scholarly API, across every paper alert of the
//! process: arXiv asks for 3 seconds between calls (its API manual); OpenAlex and
//! Semantic Scholar get 1 second, well inside OpenAlex's keyless 10 requests a second
//! and gentle on Semantic Scholar's shared keyless pool.
//!
//! Each API has its own turn, so arXiv's wait never holds the others up. A turn is held
//! while its request runs and the spacing counts from when that request ended, so a
//! slow answer is never followed straight away by the next request. Alert requests
//! follow no redirect, so one request is one hop.

use std::sync::OnceLock;
use std::time::Duration;

use tokio::sync::{Mutex, MutexGuard};
use tokio::time::Instant;

use crate::harness::web::papers::PaperApi;

/// The least time between the end of one request to `api` and the start of the next.
pub fn spacing(api: PaperApi) -> Duration {
    match api {
        PaperApi::Arxiv => Duration::from_secs(3),
        PaperApi::OpenAlex | PaperApi::SemanticScholar => Duration::from_secs(1),
    }
}

/// When each API's last request ended.
#[derive(Debug, Default)]
pub struct ApiPacer {
    arxiv: Mutex<Option<Instant>>,
    openalex: Mutex<Option<Instant>>,
    semantic_scholar: Mutex<Option<Instant>>,
    /// Asks back to back (tests over in-process HTTP).
    unspaced: bool,
}

/// One API's turn: held while its request runs; dropping it records the request as
/// ended now.
pub struct Turn<'a> {
    last: MutexGuard<'a, Option<Instant>>,
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        *self.last = Some(Instant::now());
    }
}

impl ApiPacer {
    /// The pacer every paper alert of this process shares.
    pub fn shared() -> &'static ApiPacer {
        static PACER: OnceLock<ApiPacer> = OnceLock::new();
        PACER.get_or_init(ApiPacer::default)
    }

    /// A pacer that never waits: for tests that answer in process.
    pub fn without_spacing() -> Self {
        Self {
            unspaced: true,
            ..Self::default()
        }
    }

    fn slot(&self, api: PaperApi) -> &Mutex<Option<Instant>> {
        match api {
            PaperApi::Arxiv => &self.arxiv,
            PaperApi::OpenAlex => &self.openalex,
            PaperApi::SemanticScholar => &self.semantic_scholar,
        }
    }

    /// Waits until `api` may be asked again and takes its turn; hold the turn until
    /// the request has ended.
    pub async fn wait_turn(&self, api: PaperApi) -> Turn<'_> {
        let last = self.slot(api).lock().await;
        if !self.unspaced {
            if let Some(ended) = *last {
                tokio::time::sleep_until(ended + spacing(api)).await;
            }
        }
        Turn { last }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn the_pacer_spaces_arxiv_by_three_seconds_from_the_end_of_a_request() {
        let pacer = ApiPacer::default();
        let start = Instant::now();
        let turn = pacer.wait_turn(PaperApi::Arxiv).await;
        assert_eq!(
            start.elapsed(),
            Duration::ZERO,
            "the first request waits for nothing"
        );
        // A slow answer: the spacing counts from its end.
        tokio::time::sleep(Duration::from_secs(10)).await;
        drop(turn);
        let ended = Instant::now();
        drop(pacer.wait_turn(PaperApi::Arxiv).await);
        assert!(ended.elapsed() >= Duration::from_secs(3));
        assert!(ended.elapsed() < Duration::from_secs(4));
    }

    #[tokio::test(start_paused = true)]
    async fn each_api_waits_only_for_itself() {
        let pacer = ApiPacer::default();
        let arxiv = pacer.wait_turn(PaperApi::Arxiv).await;
        // arXiv's request is still running: OpenAlex is not held up by it.
        let before = Instant::now();
        drop(pacer.wait_turn(PaperApi::OpenAlex).await);
        assert_eq!(before.elapsed(), Duration::ZERO);
        drop(arxiv);
        let before = Instant::now();
        drop(pacer.wait_turn(PaperApi::OpenAlex).await);
        assert!(before.elapsed() >= Duration::from_secs(1));
        assert!(before.elapsed() < Duration::from_secs(3));
        let unspaced = ApiPacer::without_spacing();
        drop(unspaced.wait_turn(PaperApi::Arxiv).await);
        let before = Instant::now();
        drop(unspaced.wait_turn(PaperApi::Arxiv).await);
        assert_eq!(before.elapsed(), Duration::ZERO);
    }
}
