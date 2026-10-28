//! Live availability of OpenRouter models, for ranking automatic fallbacks.
//!
//! OpenRouter publishes each model's endpoints (the providers serving it), with a
//! status and the uptime of the last 30 minutes, at
//! `GET https://openrouter.ai/api/v1/models/{id}/endpoints`
//! (<https://openrouter.ai/docs/api/api-reference/endpoints/list-endpoints>). The request
//! needs no key and none is sent: only the model id (in the path) and the app's generic
//! User-Agent leave this computer. A model with no endpoint is served by nobody (that is
//! how a delisted free model looks: `"endpoints": []`).
//!
//! The app fetches in the background for the free candidates and keeps each answer for
//! [`AVAILABILITY_TTL_MS`]; an answer's critical path only reads the cache.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::harness::catalog_fetch::{get_bounded, FetchError};
use crate::harness::model::is_valid_model_id;
use crate::harness::model_catalog::CatalogError;

/// OpenRouter's per-model endpoint list: `{OPENROUTER_ENDPOINTS_BASE}/{id}/endpoints`.
pub const OPENROUTER_ENDPOINTS_BASE: &str = "https://openrouter.ai/api/v1/models";
/// How long a fetched availability is used (and a failed fetch is not repeated).
pub const AVAILABILITY_TTL_MS: u64 = 10 * 60 * 1_000;
/// Time allowed for one endpoint list.
pub const AVAILABILITY_TIMEOUT: Duration = Duration::from_secs(8);

/// What OpenRouter says about one model right now.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Availability {
    /// Endpoints listed.
    pub endpoints: u32,
    /// Endpoints whose status is OK (`status` 0, or no status given).
    pub serving: u32,
    /// The best uptime over the last 30 minutes among the serving endpoints, in
    /// percent; `None` when OpenRouter gave none.
    pub uptime_30m: Option<f64>,
}

impl Availability {
    /// Nobody serves the model: calling it cannot succeed.
    pub fn is_down(&self) -> bool {
        self.serving == 0
    }

    /// Ranking bucket (lower is better): 95–100 % uptime is 0, unknown counts as 1
    /// (like 90–95 %), each further 5 % one more.
    pub fn rank(&self) -> u32 {
        match self.uptime_30m {
            // Bounded to 0..=100 by the parser, so the cast cannot overflow.
            Some(uptime) => ((100.0 - uptime).max(0.0) / 5.0).floor() as u32,
            None => 1,
        }
    }
}

#[derive(Deserialize)]
struct EndpointsBody {
    data: EndpointsData,
}

#[derive(Deserialize)]
struct EndpointsData {
    #[serde(default)]
    endpoints: Vec<Endpoint>,
}

#[derive(Deserialize)]
struct Endpoint {
    #[serde(default)]
    status: Option<i64>,
    #[serde(default)]
    uptime_last_30m: Option<f64>,
}

/// Parses an endpoints response.
pub fn parse_endpoints(json: &str) -> Result<Availability, CatalogError> {
    let body: EndpointsBody =
        serde_json::from_str(json).map_err(|e| CatalogError::Parse(e.to_string()))?;
    let endpoints = &body.data.endpoints;
    let serving: Vec<&Endpoint> = endpoints
        .iter()
        .filter(|e| e.status.unwrap_or(0) == 0)
        .collect();
    let uptime_30m = serving
        .iter()
        .filter_map(|e| e.uptime_last_30m)
        .filter(|u| u.is_finite())
        .map(|u| u.clamp(0.0, 100.0))
        .reduce(f64::max);
    Ok(Availability {
        endpoints: u32::try_from(endpoints.len()).unwrap_or(u32::MAX),
        serving: u32::try_from(serving.len()).unwrap_or(u32::MAX),
        uptime_30m,
    })
}

/// The endpoints URL of `model_id`, or `None` for an id that is not safe in a path.
pub fn endpoints_url(model_id: &str) -> Option<String> {
    let id = model_id.trim();
    let safe = is_valid_model_id(id)
        && !id.contains("..")
        && !id.starts_with('/')
        && !id.ends_with('/')
        && !id.contains('@');
    safe.then(|| format!("{OPENROUTER_ENDPOINTS_BASE}/{id}/endpoints"))
}

/// Fetches `model_id`'s availability from OpenRouter (no key is sent).
pub async fn fetch_availability(model_id: &str) -> Result<Availability, FetchError> {
    let url = endpoints_url(model_id)
        .ok_or_else(|| FetchError::Parse(CatalogError::InvalidModel(model_id.to_string())))?;
    let body = get_bounded(&url, AVAILABILITY_TIMEOUT).await?;
    Ok(parse_endpoints(&body)?)
}

#[derive(Debug, Default)]
struct Books {
    known: HashMap<String, (Availability, u64)>,
    /// When each id was last asked for (fetched or failed).
    asked: HashMap<String, u64>,
}

/// Fetched availabilities by OpenRouter model id, each kept for
/// [`AVAILABILITY_TTL_MS`].
#[derive(Debug, Default)]
pub struct AvailabilityBook {
    books: Mutex<Books>,
}

impl AvailabilityBook {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, Books> {
        self.books.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `model_id`'s availability, when one fetched within the TTL is kept.
    pub fn get(&self, model_id: &str, now_ms: u64) -> Option<Availability> {
        self.lock()
            .known
            .get(model_id)
            .filter(|(_, at)| now_ms.saturating_sub(*at) < AVAILABILITY_TTL_MS)
            .map(|(a, _)| *a)
    }

    /// The ids among `ids` to fetch now: neither known nor asked for within the TTL.
    /// They are marked as asked, so concurrent callers do not fetch them twice.
    pub fn claim_due(&self, ids: &[String], now_ms: u64) -> Vec<String> {
        let mut books = self.lock();
        let mut due = Vec::new();
        for id in ids {
            let fresh = |at: Option<&u64>| {
                at.is_some_and(|at| now_ms.saturating_sub(*at) < AVAILABILITY_TTL_MS)
            };
            if fresh(books.known.get(id).map(|(_, at)| at)) || fresh(books.asked.get(id)) {
                continue;
            }
            books.asked.insert(id.clone(), now_ms);
            if !due.contains(id) {
                due.push(id.clone());
            }
        }
        due
    }

    /// Keeps a fetched availability.
    pub fn put(&self, model_id: &str, availability: Availability, now_ms: u64) {
        self.lock()
            .known
            .insert(model_id.to_string(), (availability, now_ms));
    }
}
