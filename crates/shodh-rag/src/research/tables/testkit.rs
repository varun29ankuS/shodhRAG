//! Test doubles shared by the fill and job tests: a paper index, a model that replays
//! recorded replies, and a clock that moves only when a test moves it.

use std::collections::VecDeque;
use std::sync::Mutex;

use chrono::{DateTime, FixedOffset, TimeDelta, TimeZone, Utc};

use super::calls::Clock;
use super::passages::PaperIndex;
use super::Passage;
use crate::research::path_key;
use crate::research::results::TextModel;

pub(crate) fn passage(id: &str, page: u32, text: &str) -> Passage {
    Passage {
        chunk_id: id.to_string(),
        text: text.to_string(),
        page: Some(page),
        bboxes: Some(format!(
            r#"[{{"page":{page},"x0":1.0,"y0":2.0,"x1":3.0,"y1":4.0}}]"#
        )),
    }
}

/// The same passages for every query unless a keyword of the query selects others;
/// records every search. Papers listed as not indexed have no chunks at all, and a search
/// error, when set, fails every search.
pub(crate) struct Index {
    pub(crate) first: Option<Passage>,
    pub(crate) passages: Vec<Passage>,
    pub(crate) by_query: Vec<(&'static str, Vec<Passage>)>,
    pub(crate) searches: Mutex<Vec<(String, String)>>,
    pub(crate) unindexed: Vec<String>,
    pub(crate) search_error: Option<String>,
}

impl Index {
    pub(crate) fn new(first: Option<Passage>, passages: Vec<Passage>) -> Self {
        Self {
            first,
            passages,
            by_query: Vec::new(),
            searches: Mutex::new(Vec::new()),
            unindexed: Vec::new(),
            search_error: None,
        }
    }

    pub(crate) fn with_query(mut self, keyword: &'static str, passages: Vec<Passage>) -> Self {
        self.by_query.push((keyword, passages));
        self
    }

    /// `source` has no chunks: no first passage and no search hit.
    pub(crate) fn without(mut self, source: &str) -> Self {
        self.unindexed.push(path_key(source));
        self
    }

    /// Every search fails with `error`, as when the search models are missing.
    pub(crate) fn failing(mut self, error: &str) -> Self {
        self.search_error = Some(error.to_string());
        self
    }

    pub(crate) fn searches(&self) -> Vec<(String, String)> {
        self.searches
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn indexed(&self, source: &str) -> bool {
        !self.unindexed.iter().any(|s| *s == path_key(source))
    }
}

#[async_trait::async_trait]
impl PaperIndex for Index {
    async fn search_paper(
        &self,
        source: &str,
        query: &str,
        k: usize,
    ) -> Result<Vec<Passage>, String> {
        self.searches
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((source.to_string(), query.to_string()));
        if let Some(error) = &self.search_error {
            return Err(error.clone());
        }
        if !self.indexed(source) {
            return Ok(Vec::new());
        }
        let lower = query.to_lowercase();
        let chosen = self
            .by_query
            .iter()
            .find(|(keyword, _)| lower.contains(keyword))
            .map_or(&self.passages, |(_, passages)| passages);
        Ok(chosen.iter().take(k).cloned().collect())
    }
    async fn first_passage(&self, source: &str) -> Result<Option<Passage>, String> {
        if !self.indexed(source) {
            return Ok(None);
        }
        Ok(self.first.clone())
    }
}

pub(crate) fn paper_index() -> Index {
    Index::new(
        Some(passage(
            "p0",
            1,
            "Dense Retrieval at Scale. Abstract: We pre-train a bi-encoder on LAION-400M with a contrastive loss.",
        )),
        vec![
            passage(
                "p1",
                4,
                "Table 2 reports Recall@10 of 0.912 on MS MARCO with 48 GB of GPU memory.",
            ),
            passage(
                "p2",
                6,
                "Our code is available at the project page. The study is a benchmark evaluation.",
            ),
        ],
    )
}

/// Four passages of about 2,000 tokens each: more than one call holds for four columns.
pub(crate) fn big_index() -> Index {
    let big = "word ".repeat(1_200);
    Index::new(
        None,
        (1..=4)
            .map(|i| passage(&format!("big{i}"), i, &big))
            .collect(),
    )
}

/// The characters one output token is worth in the double below (the same estimate the
/// budget uses).
pub(crate) const CHARS_PER_TOKEN: usize = 3;

/// Replays recorded replies in order, keeps the prompts and output limits it was sent,
/// and cuts a reply at the limit the way a provider does. `before_reply` runs when a call
/// arrives, as the user would act while the model is thinking.
pub(crate) struct Replies {
    pub(crate) replies: Mutex<VecDeque<Result<String, String>>>,
    pub(crate) prompts: Mutex<Vec<String>>,
    pub(crate) limits: Mutex<Vec<usize>>,
    pub(crate) before_reply: Option<Box<dyn Fn() + Send + Sync>>,
}

impl Replies {
    pub(crate) fn new(replies: Vec<Result<&str, &str>>) -> Self {
        Self {
            replies: Mutex::new(
                replies
                    .into_iter()
                    .map(|r| r.map(str::to_string).map_err(str::to_string))
                    .collect(),
            ),
            prompts: Mutex::new(Vec::new()),
            limits: Mutex::new(Vec::new()),
            before_reply: None,
        }
    }
    pub(crate) fn prompts(&self) -> Vec<String> {
        self.prompts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    pub(crate) fn limits(&self) -> Vec<usize> {
        self.limits
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

#[async_trait::async_trait]
impl TextModel for Replies {
    fn model_id(&self) -> String {
        "recorded-model".to_string()
    }
    async fn complete(&self, prompt: &str, max_tokens: usize) -> Result<String, String> {
        self.prompts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(prompt.to_string());
        self.limits
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(max_tokens);
        if let Some(act) = &self.before_reply {
            act();
        }
        let reply = self
            .replies
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop_front()
            .unwrap_or_else(|| Err("no recorded reply left".to_string()))?;
        Ok(reply.chars().take(max_tokens * CHARS_PER_TOKEN).collect())
    }
}

/// An instant in UTC.
pub(crate) fn utc(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(year, month, day, hour, minute, 0)
        .single()
        .unwrap()
}

/// A clock that stands still until a test moves it, in a time zone given as offsets
/// (seconds east of UTC) that apply from given instants on.
pub(crate) struct ManualClock {
    now: Mutex<DateTime<Utc>>,
    zone: Vec<(DateTime<Utc>, FixedOffset)>,
}

impl ManualClock {
    pub(crate) fn new(now: DateTime<Utc>, zone: &[(DateTime<Utc>, i32)]) -> Self {
        let mut zone: Vec<(DateTime<Utc>, FixedOffset)> = zone
            .iter()
            .map(|&(from, east)| (from, FixedOffset::east_opt(east).unwrap()))
            .collect();
        zone.sort_by_key(|(from, _)| *from);
        Self {
            now: Mutex::new(now),
            zone,
        }
    }

    /// A clock in UTC.
    pub(crate) fn utc_at(now: DateTime<Utc>) -> Self {
        Self::new(now, &[])
    }

    pub(crate) fn set(&self, now: DateTime<Utc>) {
        *self.now.lock().unwrap_or_else(|e| e.into_inner()) = now;
    }

    pub(crate) fn advance(&self, by: TimeDelta) {
        let mut now = self.now.lock().unwrap_or_else(|e| e.into_inner());
        *now += by;
    }
}

impl Clock for ManualClock {
    fn now(&self) -> DateTime<Utc> {
        *self.now.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn offset_at(&self, at: DateTime<Utc>) -> FixedOffset {
        self.zone
            .iter()
            .rev()
            .find(|(from, _)| *from <= at)
            .map(|(_, offset)| *offset)
            .unwrap_or_else(|| FixedOffset::east_opt(0).unwrap())
    }
}
