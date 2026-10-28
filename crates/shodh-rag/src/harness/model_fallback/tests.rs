//! The fallback with a scripted fake provider and a fake clock: no network, no sleeps.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::*;
use crate::harness::model_catalog::{
    parse_openrouter, CatalogModel, ModelRef, ModelTier, PrivacyNote, ProviderId, ToolSupport,
};
use crate::harness::model_picks::{PickTable, ProviderPicks};
use crate::harness::provider_error::{classify, ProviderErrorKind};
use crate::llm::failure::CallContext;
use crate::llm::ProviderFailure;
use crate::research::results::{ModelReply, TextModel};

const T0: u64 = 1_761_000_000_000;
const MIN: u64 = 60_000;

// ── Fakes ──────────────────────────────────────────────────────────

#[derive(Default)]
struct FakeClock(AtomicU64);

impl FakeClock {
    fn at(ms: u64) -> Arc<Self> {
        Arc::new(Self(AtomicU64::new(ms)))
    }
    fn advance(&self, ms: u64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
    fn now(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

impl Clock for FakeClock {
    fn now_ms(&self) -> u64 {
        self.now()
    }
}

/// Waits by moving the fake clock; records each wait.
struct FakePause {
    clock: Arc<FakeClock>,
    waits: Mutex<Vec<Duration>>,
}

#[async_trait::async_trait]
impl Pause for FakePause {
    async fn pause(&self, wait: Duration) {
        self.waits.lock().unwrap().push(wait);
        self.clock.advance(u64::try_from(wait.as_millis()).unwrap());
    }
}

/// How a scripted call ends.
#[derive(Clone)]
enum Script {
    Answer(&'static str),
    Fail(Fixture),
}

/// Real provider failures, classified the way the app classifies them.
#[derive(Clone, Copy, Debug)]
enum Fixture {
    Unavailable404,
    Server500,
    Timeout,
    PerMinute429 { retry_after_secs: u64 },
    UpstreamFree429,
    Daily429,
    BadKey401,
    Moderation403,
    ContextTooLong,
}

fn openrouter_headers(reset_ms: u64) -> String {
    format!(
        r#"{{"headers":{{"X-RateLimit-Limit":"20","X-RateLimit-Remaining":"0","X-RateLimit-Reset":"{reset_ms}"}}}}"#
    )
}

fn failure(fixture: Fixture, model: &ModelRef, now_ms: u64) -> ProviderFailure {
    let ctx = CallContext::new(
        model.provider.label(),
        model.model.clone(),
        "sk-or-v1-test-secret",
    );
    match fixture {
        Fixture::Unavailable404 => ctx.http(
            404,
            r#"{"error":{"message":"No endpoints found for nvidia/nemotron-3-super-120b-a12b:free.","code":404}}"#,
        ),
        Fixture::Server500 => ctx.http(500, "Internal Server Error"),
        Fixture::Timeout => ctx.failure(ProviderErrorKind::Timeout, None, "operation timed out"),
        Fixture::PerMinute429 { retry_after_secs } => ctx.http(
            429,
            &format!(
                r#"{{"error":{{"message":"Rate limit exceeded: free-models-per-min. ","code":429,"metadata":{}}}}}"#,
                openrouter_headers(now_ms + retry_after_secs * 1_000)
            ),
        ),
        Fixture::UpstreamFree429 => ctx.http(
            429,
            r#"{"error":{"message":"Provider returned error","code":429,"metadata":{"raw":"google/gemma-4-31b-it:free is temporarily rate-limited upstream. Please retry shortly.","provider_name":"Google AI Studio"}}}"#,
        ),
        Fixture::Daily429 => ctx.http(
            429,
            &format!(
                r#"{{"error":{{"message":"Rate limit exceeded: free-models-per-day. Add 10 credits to unlock 1000 free model requests per day","code":429,"metadata":{}}}}}"#,
                openrouter_headers(now_ms + 5 * 60 * MIN)
            ),
        ),
        Fixture::BadKey401 => ctx.http(401, r#"{"error":{"message":"No auth credentials found","code":401}}"#),
        Fixture::Moderation403 => ctx.http(
            403,
            r#"{"error":{"message":"Input requires moderation","code":403}}"#,
        ),
        Fixture::ContextTooLong => ctx.http(
            400,
            r#"{"error":{"message":"This endpoint's maximum context length is 32768 tokens.","code":400}}"#,
        ),
    }
}

/// A provider scripted per model: each call takes the model's next script entry (the
/// last one repeats). Each call takes `call_ms` of fake time.
struct FakeProvider {
    clock: Arc<FakeClock>,
    scripts: Mutex<HashMap<String, VecDeque<Script>>>,
    calls: Mutex<Vec<String>>,
    call_ms: u64,
}

impl FakeProvider {
    fn new(clock: Arc<FakeClock>, scripts: &[(&str, &[Script])]) -> Arc<Self> {
        Arc::new(Self {
            clock,
            scripts: Mutex::new(
                scripts
                    .iter()
                    .map(|(id, s)| (id.to_string(), s.iter().cloned().collect()))
                    .collect(),
            ),
            calls: Mutex::new(Vec::new()),
            call_ms: 1_000,
        })
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn next(&self, model: &ModelRef) -> Result<String, ProviderFailure> {
        self.calls.lock().unwrap().push(model.model.clone());
        self.clock.advance(self.call_ms);
        let script = {
            let mut scripts = self.scripts.lock().unwrap();
            let queue = scripts
                .get_mut(&model.model)
                .unwrap_or_else(|| panic!("no script for {}", model.model));
            if queue.len() > 1 {
                queue.pop_front().unwrap()
            } else {
                queue.front().cloned().unwrap()
            }
        };
        match script {
            Script::Answer(text) => Ok(text.to_string()),
            Script::Fail(fixture) => Err(failure(fixture, model, self.clock.now())),
        }
    }
}

#[async_trait::async_trait]
impl CandidateModel for FakeProvider {
    async fn complete(
        &self,
        candidate: &Candidate,
        _prompt: &str,
        _max_tokens: usize,
    ) -> Result<ModelReply, ProviderFailure> {
        self.next(&candidate.model).map(|text| ModelReply {
            text,
            truncated: false,
        })
    }
}

#[derive(Default)]
struct Reports(Mutex<Vec<CallReport>>);

impl FallbackObserver for Reports {
    fn observe(&self, report: &CallReport) {
        self.0.lock().unwrap().push(report.clone());
    }
}

// ── Catalog and plans ──────────────────────────────────────────────

const NEMOTRON: &str = "nvidia/nemotron-3-super-120b-a12b:free";
const GEMMA: &str = "google/gemma-4-31b-it:free";
const LIGHTNING: &str = "nvidia/nemotron-3.5-lightning:free";
const HAIKU_OR: &str = "anthropic/claude-haiku-4.5";

const OPENROUTER_LIST: &str = r#"{"data":[
    {"id":"nvidia/nemotron-3-super-120b-a12b:free","name":"Nemotron Super","context_length":262144,
     "pricing":{"prompt":"0","completion":"0"},"supported_parameters":["tools"],
     "architecture":{"input_modalities":["text"]}},
    {"id":"google/gemma-4-31b-it:free","name":"Gemma 4 31B","context_length":131072,
     "pricing":{"prompt":"0","completion":"0"},"supported_parameters":["tools"],
     "architecture":{"input_modalities":["image","text","video"]}},
    {"id":"nvidia/nemotron-3.5-lightning:free","name":"Nemotron Lightning","context_length":1000000,
     "pricing":{"prompt":"0","completion":"0"},"supported_parameters":["tools"],
     "architecture":{"input_modalities":["text"]}},
    {"id":"small/tiny-chat:free","name":"Tiny Chat","context_length":8192,
     "pricing":{"prompt":"0","completion":"0"},"supported_parameters":["temperature"]},
    {"id":"anthropic/claude-haiku-4.5","name":"Claude Haiku 4.5","context_length":200000,
     "pricing":{"prompt":"0.000001","completion":"0.000005"},"supported_parameters":["tools"],
     "architecture":{"input_modalities":["image","text"]}}
]}"#;

fn catalog() -> Vec<CatalogModel> {
    let mut models = parse_openrouter(OPENROUTER_LIST).unwrap();
    models.push(CatalogModel {
        provider: ProviderId::Google,
        id: "gemini-3-flash-preview".into(),
        name: "Gemini 3 Flash".into(),
        context_length: Some(1_000_000),
        prompt_per_million: Some(0.3),
        completion_per_million: Some(2.5),
        tools: ToolSupport::Yes,
        tier: ModelTier::Paid,
        privacy: PrivacyNote::ProviderTerms,
        stealth: false,
        image_input: Some(true),
    });
    models.push(CatalogModel {
        provider: ProviderId::LmStudio,
        id: "qwen3-8b".into(),
        name: "qwen3-8b".into(),
        context_length: None,
        prompt_per_million: Some(0.0),
        completion_per_million: Some(0.0),
        tools: ToolSupport::Unknown,
        tier: ModelTier::Local,
        privacy: PrivacyNote::OnDevice,
        stealth: false,
        image_input: None,
    });
    models
}

fn table() -> PickTable {
    let mut providers = BTreeMap::new();
    providers.insert(
        ProviderId::Google,
        ProviderPicks {
            fast: vec!["gemini-3-flash-preview".into()],
            ..ProviderPicks::default()
        },
    );
    providers.insert(
        ProviderId::GeminiSub,
        ProviderPicks {
            fast: vec!["gemini-3-flash-preview".into()],
            ..ProviderPicks::default()
        },
    );
    PickTable {
        order: vec![
            ProviderId::GeminiSub,
            ProviderId::Google,
            ProviderId::OpenRouter,
            ProviderId::LmStudio,
        ],
        providers,
    }
}

fn or(id: &str) -> ModelRef {
    ModelRef::new(ProviderId::OpenRouter, id)
}

fn gemini() -> ModelRef {
    ModelRef::new(ProviderId::Google, "gemini-3-flash-preview")
}

struct Setup {
    catalog: Vec<CatalogModel>,
    table: PickTable,
    requested: ModelRef,
    chosen: Option<ModelRef>,
    pick: Vec<ModelRef>,
    connected: Vec<ProviderId>,
    order: Vec<ProviderId>,
    local_only: bool,
    accepted: Vec<String>,
    needs: CallNeeds,
}

impl Setup {
    fn new(requested: ModelRef) -> Self {
        Self {
            catalog: catalog(),
            table: table(),
            requested,
            chosen: None,
            pick: Vec::new(),
            connected: vec![ProviderId::OpenRouter],
            order: vec![
                ProviderId::GeminiSub,
                ProviderId::Google,
                ProviderId::OpenRouter,
                ProviderId::LmStudio,
            ],
            local_only: false,
            accepted: Vec::new(),
            needs: CallNeeds::direct("What does the report say about costs?"),
        }
    }

    fn inputs(&self) -> ResolveInputs<'_> {
        ResolveInputs {
            requested: &self.requested,
            chosen_fallback: self.chosen.as_ref(),
            pick_chain: &self.pick,
            catalog: &self.catalog,
            table: &self.table,
            order: &self.order,
            connected: &self.connected,
            local_only: self.local_only,
            allow_stealth: false,
            stealth_accepted: &self.accepted,
            needs: self.needs,
        }
    }

    fn plan(&self, health: &HealthBook, availability: &AvailabilityBook, now: u64) -> Plan {
        resolve(&self.inputs(), health, availability, now)
    }
}

fn ids(plan: &Plan) -> Vec<String> {
    plan.candidates().map(|c| c.model.model.clone()).collect()
}

struct Rig {
    clock: Arc<FakeClock>,
    pause: Arc<FakePause>,
    health: Arc<HealthBook>,
    provider: Arc<FakeProvider>,
    reports: Arc<Reports>,
    model: FallbackTextModel,
}

fn make_rig(plan: Plan, scripts: &[(&str, &[Script])]) -> Rig {
    rig_with(
        plan,
        scripts,
        DEFAULT_BOUNDS,
        Arc::new(HealthBook::new()),
        FakeClock::at(T0),
    )
}

fn rig_with(
    plan: Plan,
    scripts: &[(&str, &[Script])],
    bounds: Bounds,
    health: Arc<HealthBook>,
    clock: Arc<FakeClock>,
) -> Rig {
    let pause = Arc::new(FakePause {
        clock: clock.clone(),
        waits: Mutex::new(Vec::new()),
    });
    let provider = FakeProvider::new(clock.clone(), scripts);
    let reports = Arc::new(Reports::default());
    let planned = plan.clone();
    let model = FallbackTextModel::new(
        Arc::new(move |_needs: CallNeeds| planned.clone()),
        provider.clone(),
        health.clone(),
        Timing {
            clock: clock.clone(),
            pause: pause.clone(),
            bounds,
        },
        Some(reports.clone() as Arc<dyn FallbackObserver>),
    );
    Rig {
        clock,
        pause,
        health,
        provider,
        reports,
        model,
    }
}

/// A plan of OpenRouter free models plus Gemini (another provider), in this order.
fn free_plan(requested: &str, fallbacks: &[&str]) -> Plan {
    let mut setup = Setup::new(or(requested));
    setup.connected = vec![ProviderId::OpenRouter, ProviderId::Google];
    let base = setup.plan(&HealthBook::new(), &AvailabilityBook::new(), T0);
    let find = |id: &str| -> Candidate {
        base.candidates()
            .find(|c| c.model.model == id)
            .cloned()
            .unwrap_or_else(|| panic!("{id} is not a candidate"))
    };
    Plan {
        requested: base.requested.clone(),
        fallbacks: fallbacks.iter().map(|id| find(id)).collect(),
    }
}

// ── Ordering ───────────────────────────────────────────────────────

#[test]
fn candidates_come_requested_then_chosen_then_pick_then_automatic() {
    let mut setup = Setup::new(or(NEMOTRON));
    setup.connected = vec![
        ProviderId::OpenRouter,
        ProviderId::Google,
        ProviderId::LmStudio,
    ];
    setup.chosen = Some(or(LIGHTNING));
    // The pick repeats the requested and chosen models: each is tried once.
    setup.pick = vec![or(NEMOTRON), or(GEMMA), or(LIGHTNING)];
    let plan = setup.plan(&HealthBook::new(), &AvailabilityBook::new(), T0);
    let order = ids(&plan);
    assert_eq!(order[..3], [NEMOTRON, LIGHTNING, GEMMA]);
    let origins: Vec<Origin> = plan.candidates().map(|c| c.origin).collect();
    assert_eq!(
        origins[..3],
        [Origin::Requested, Origin::ChosenFallback, Origin::PickChain]
    );
    // Automatic: the direct provider's fast model, the fast model through OpenRouter,
    // the local model, then free models (largest context first, already used ones
    // left out); capped.
    assert_eq!(
        order[3..],
        [
            "gemini-3-flash-preview",
            HAIKU_OR,
            "qwen3-8b",
            "small/tiny-chat:free"
        ][..MAX_AUTOMATIC]
    );
    assert!(plan.fallbacks[2..]
        .iter()
        .all(|c| c.origin == Origin::Automatic));
}

#[test]
fn automatic_candidates_rank_by_health_then_uptime_then_catalog_order() {
    let mut setup = Setup::new(or(NEMOTRON));
    // Only free models besides the requested one, so the ranking shows on them.
    setup.catalog.retain(|m| m.tier == ModelTier::Free);
    let health = HealthBook::new();
    let availability = AvailabilityBook::new();
    let plan = setup.plan(&health, &availability, T0);
    assert_eq!(ids(&plan)[1..], [LIGHTNING, GEMMA, "small/tiny-chat:free"]);

    // Lightning's uptime is poor and Gemma's is perfect: Gemma first.
    availability.put(
        LIGHTNING,
        parse_endpoints(r#"{"data":{"endpoints":[{"status":0,"uptime_last_30m":62.5}]}}"#).unwrap(),
        T0,
    );
    availability.put(
        GEMMA,
        parse_endpoints(r#"{"data":{"endpoints":[{"status":0,"uptime_last_30m":100}]}}"#).unwrap(),
        T0,
    );
    let plan = setup.plan(&health, &availability, T0);
    assert_eq!(ids(&plan)[1..], [GEMMA, "small/tiny-chat:free", LIGHTNING]);

    // A failing model goes behind healthy ones, whatever its uptime.
    health.record_failure(&or(GEMMA), ProviderErrorKind::ModelUnavailable, T0);
    let plan = setup.plan(&health, &availability, T0);
    assert_eq!(ids(&plan)[1..], ["small/tiny-chat:free", LIGHTNING, GEMMA]);

    // Nobody serves Tiny Chat: it is never offered. Past the TTL the data is unused.
    availability.put(
        "small/tiny-chat:free",
        parse_endpoints(r#"{"data":{"endpoints":[]}}"#).unwrap(),
        T0,
    );
    let plan = setup.plan(&health, &availability, T0);
    assert!(!ids(&plan).contains(&"small/tiny-chat:free".to_string()));
    let later = setup.plan(&health, &availability, T0 + AVAILABILITY_TTL_MS);
    assert!(ids(&later).contains(&"small/tiny-chat:free".to_string()));
}

#[test]
fn availability_is_parsed_from_openrouters_endpoint_list() {
    let delisted = parse_endpoints(
        r#"{"data":{"id":"nvidia/nemotron-3-super-120b-a12b:free","architecture":{"input_modalities":["text"]},"endpoints":[]}}"#,
    )
    .unwrap();
    assert!(delisted.is_down());
    let mixed = parse_endpoints(
        r#"{"data":{"endpoints":[
            {"provider_name":"A","status":-2,"uptime_last_30m":100},
            {"provider_name":"B","status":0,"uptime_last_30m":97.5},
            {"provider_name":"C","uptime_last_30m":null}
        ]}}"#,
    )
    .unwrap();
    assert_eq!(mixed.endpoints, 3);
    assert_eq!(mixed.serving, 2, "status 0 or no status is serving");
    assert_eq!(
        mixed.uptime_30m,
        Some(97.5),
        "the best serving endpoint's uptime"
    );
    assert_eq!(mixed.rank(), 0);
    assert!(parse_endpoints("not json").is_err());

    assert_eq!(
        endpoints_url(NEMOTRON).as_deref(),
        Some(
            "https://openrouter.ai/api/v1/models/nvidia/nemotron-3-super-120b-a12b:free/endpoints"
        )
    );
    for unsafe_id in ["../admin", "a/../b", "/x", "x/", "a b", "x@evil.com", ""] {
        assert_eq!(endpoints_url(unsafe_id), None, "{unsafe_id:?}");
    }
}

#[test]
fn availability_fetches_are_claimed_once_per_ttl() {
    let book = AvailabilityBook::new();
    let wanted = vec![GEMMA.to_string(), LIGHTNING.to_string()];
    assert_eq!(book.claim_due(&wanted, T0), wanted);
    assert!(
        book.claim_due(&wanted, T0 + MIN).is_empty(),
        "a failed fetch is not retried at once"
    );
    assert_eq!(book.claim_due(&wanted, T0 + AVAILABILITY_TTL_MS), wanted);
}

#[test]
fn only_free_openrouter_models_get_availability_and_never_in_local_only_mode() {
    let mut setup = Setup::new(or(NEMOTRON));
    setup.connected = vec![ProviderId::OpenRouter, ProviderId::Google];
    let wanted = availability_ids(&setup.inputs());
    assert!(wanted.contains(&NEMOTRON.to_string()));
    assert!(wanted.iter().all(|id| id.ends_with(":free")));
    setup.local_only = true;
    assert!(availability_ids(&setup.inputs()).is_empty());
}

// ── Fit ────────────────────────────────────────────────────────────

#[test]
fn candidates_must_fit_tools_images_and_context() {
    let mut setup = Setup::new(or(NEMOTRON));
    setup.catalog.retain(|m| m.tier == ModelTier::Free);
    setup.needs = CallNeeds::agent();
    assert!(
        !ids(&setup.plan(&HealthBook::new(), &AvailabilityBook::new(), T0))
            .contains(&"small/tiny-chat:free".to_string()),
        "an agent run needs tool calling"
    );

    setup.needs = CallNeeds {
        images: true,
        ..CallNeeds::agent()
    };
    assert_eq!(
        ids(&setup.plan(&HealthBook::new(), &AvailabilityBook::new(), T0))[1..],
        [GEMMA],
        "images need a model that reads images"
    );

    // About 150k tokens: only the 1M-token model has room.
    setup.needs = CallNeeds::direct(&"x".repeat(600_000));
    assert_eq!(
        ids(&setup.plan(&HealthBook::new(), &AvailabilityBook::new(), T0))[1..],
        [LIGHTNING]
    );
}

#[test]
fn automatic_candidates_never_lower_privacy_but_the_users_own_choices_may() {
    // The requested model is a paid one under the provider's terms: free models (which
    // may log prompts) are not used automatically.
    let mut setup = Setup::new(or(HAIKU_OR));
    setup.connected = vec![ProviderId::OpenRouter, ProviderId::LmStudio];
    let plan = setup.plan(&HealthBook::new(), &AvailabilityBook::new(), T0);
    assert_eq!(ids(&plan)[1..], ["qwen3-8b"]);
    // The user chose a free fallback themselves: it is used.
    setup.chosen = Some(or(GEMMA));
    let plan = setup.plan(&HealthBook::new(), &AvailabilityBook::new(), T0);
    assert_eq!(ids(&plan)[1..], [GEMMA, "qwen3-8b"]);
}

#[test]
fn local_only_mode_keeps_every_fallback_on_this_computer() {
    let mut setup = Setup::new(ModelRef::new(ProviderId::LmStudio, "llama-3b"));
    setup.connected = vec![
        ProviderId::OpenRouter,
        ProviderId::Google,
        ProviderId::LmStudio,
    ];
    setup.chosen = Some(or(GEMMA));
    setup.local_only = true;
    let plan = setup.plan(&HealthBook::new(), &AvailabilityBook::new(), T0);
    assert_eq!(ids(&plan)[1..], ["qwen3-8b"]);
}

#[test]
fn the_caller_decides_which_providers_can_answer() {
    let mut setup = Setup::new(or(NEMOTRON));
    setup.connected = vec![
        ProviderId::OpenRouter,
        ProviderId::GeminiSub,
        ProviderId::Google,
    ];
    setup.chosen = Some(ModelRef::new(
        ProviderId::GeminiSub,
        "gemini-3-flash-preview",
    ));
    // A direct call cannot use a subscription (only the agent runtime holds it).
    let direct = setup.plan(&HealthBook::new(), &AvailabilityBook::new(), T0);
    assert!(direct
        .fallbacks
        .iter()
        .all(|c| c.model.provider != ProviderId::GeminiSub));
    setup.needs = CallNeeds::agent();
    let agent = setup.plan(&HealthBook::new(), &AvailabilityBook::new(), T0);
    assert_eq!(agent.fallbacks[0].model.provider, ProviderId::GeminiSub);
    // Ollama is not usable by the agent runtime.
    setup.connected.push(ProviderId::Ollama);
    setup.chosen = Some(ModelRef::new(ProviderId::Ollama, "qwen3:4b"));
    let agent = setup.plan(&HealthBook::new(), &AvailabilityBook::new(), T0);
    assert!(agent
        .fallbacks
        .iter()
        .all(|c| c.model.provider != ProviderId::Ollama));
    setup.needs = CallNeeds::direct("q");
    let direct = setup.plan(&HealthBook::new(), &AvailabilityBook::new(), T0);
    assert_eq!(direct.fallbacks[0].model.provider, ProviderId::Ollama);
}

#[test]
fn stealth_models_need_the_users_confirmation() {
    let mut setup = Setup::new(or(NEMOTRON));
    setup.chosen = Some(or("stealth/space-bunny"));
    let plan = setup.plan(&HealthBook::new(), &AvailabilityBook::new(), T0);
    assert!(!ids(&plan).contains(&"stealth/space-bunny".to_string()));
    setup.accepted = vec!["stealth/space-bunny".into()];
    let plan = setup.plan(&HealthBook::new(), &AvailabilityBook::new(), T0);
    assert_eq!(plan.fallbacks[0].model.model, "stealth/space-bunny");
}

// ── Health ─────────────────────────────────────────────────────────

#[test]
fn a_failing_model_cools_down_doubling_to_a_cap_then_one_call_retries_it() {
    let book = HealthBook::new();
    let m = or(NEMOTRON);
    book.record_failure(&m, ProviderErrorKind::ModelUnavailable, T0);
    assert!(
        matches!(book.health(&m, T0), Health::CoolingDown { until_ms, .. } if until_ms == T0 + 2 * MIN)
    );
    assert!(
        book.admit(&m, T0 + 2 * MIN - 1).is_none(),
        "skipped while cooling down"
    );

    // Half-open: one call probes; another call meanwhile skips it, never waits.
    let probe = book.admit(&m, T0 + 2 * MIN).expect("the wait is over");
    assert!(probe.is_probe());
    assert!(book.admit(&m, T0 + 2 * MIN + 1).is_none());
    // A 5xx classifies as unavailable.
    let server_error = classify("500 Internal Server Error", None, T0).kind;
    assert_eq!(server_error, ProviderErrorKind::ModelUnavailable);
    probe.failed(server_error, T0 + 2 * MIN + 10);
    assert!(matches!(
        book.health(&m, T0 + 2 * MIN + 10),
        Health::CoolingDown { until_ms, failures: 2, .. } if until_ms == T0 + 2 * MIN + 10 + 4 * MIN
    ));

    assert_eq!(cool_down_ms(1), 2 * MIN);
    assert_eq!(cool_down_ms(2), 4 * MIN);
    assert_eq!(cool_down_ms(4), 16 * MIN);
    assert_eq!(cool_down_ms(5), 30 * MIN, "capped at 30 minutes");
    assert_eq!(cool_down_ms(40), MAX_COOL_DOWN_MS);

    // A success closes it.
    let later = T0 + 60 * MIN;
    let probe = book.admit(&m, later).unwrap();
    probe.succeeded();
    assert_eq!(book.health(&m, later), Health::Healthy);
    assert_eq!(book.failing_since(&m), None);
}

#[test]
fn a_dropped_probe_frees_the_model_and_other_failures_do_not_trip_it() {
    let book = HealthBook::new();
    let m = or(GEMMA);
    book.record_failure(&m, ProviderErrorKind::Timeout, T0);
    {
        let probe = book.admit(&m, T0 + 2 * MIN).unwrap();
        assert!(probe.is_probe());
        // Cancelled (dropped) without a result.
    }
    assert!(
        book.admit(&m, T0 + 2 * MIN + 1).is_some(),
        "the next call may probe"
    );

    let other = or(LIGHTNING);
    for kind in [
        ProviderErrorKind::RateLimited,
        ProviderErrorKind::QuotaExhausted,
        ProviderErrorKind::Auth,
        ProviderErrorKind::ContentFiltered,
        ProviderErrorKind::ContextTooLong,
        ProviderErrorKind::InvalidRequest,
    ] {
        book.record_failure(&other, kind, T0);
        assert_eq!(book.health(&other, T0), Health::Healthy, "{kind:?}");
    }
    for kind in [
        ProviderErrorKind::ModelUnavailable,
        ProviderErrorKind::Timeout,
        ProviderErrorKind::Network,
    ] {
        assert!(trips_breaker(kind));
    }
}

#[test]
fn a_model_failing_for_over_an_hour_earns_one_notice() {
    let book = HealthBook::new();
    let m = or(NEMOTRON);
    book.record_failure(&m, ProviderErrorKind::ModelUnavailable, T0);
    assert!(!book.take_long_failure(&m, T0 + 59 * MIN));
    book.record_failure(&m, ProviderErrorKind::ModelUnavailable, T0 + 59 * MIN);
    assert!(book.take_long_failure(&m, T0 + LONG_FAILURE_MS));
    assert!(
        !book.take_long_failure(&m, T0 + 2 * LONG_FAILURE_MS),
        "once per failing streak"
    );
    book.record_success(&m);
    book.record_failure(
        &m,
        ProviderErrorKind::ModelUnavailable,
        T0 + 3 * LONG_FAILURE_MS,
    );
    assert!(
        book.take_long_failure(&m, T0 + 4 * LONG_FAILURE_MS),
        "a new streak notices again"
    );
}

// ── Running a plan ─────────────────────────────────────────────────

#[tokio::test]
async fn unavailable_5xx_and_timeouts_move_to_the_next_model_at_once() {
    for fixture in [
        Fixture::Unavailable404,
        Fixture::Server500,
        Fixture::Timeout,
    ] {
        let rig = make_rig(
            free_plan(NEMOTRON, &[GEMMA]),
            &[
                (NEMOTRON, &[Script::Fail(fixture)]),
                (GEMMA, &[Script::Answer("from gemma")]),
            ],
        );
        let reply = rig.model.complete("q", 100).await.unwrap();
        assert_eq!(reply, "from gemma", "{fixture:?}");
        let by = rig.model.answered_by().unwrap();
        assert_eq!(by.model, or(GEMMA));
        assert!(
            rig.pause.waits.lock().unwrap().is_empty(),
            "no wait before switching"
        );
        // The failed model now cools down.
        assert!(matches!(
            rig.health.health(&or(NEMOTRON), rig.clock.now()),
            Health::CoolingDown { .. }
        ));
    }
    let rig = make_rig(
        free_plan(NEMOTRON, &[GEMMA]),
        &[
            (NEMOTRON, &[Script::Fail(Fixture::Unavailable404)]),
            (GEMMA, &[Script::Answer("ok")]),
        ],
    );
    rig.model.complete("q", 100).await.unwrap();
    assert_eq!(
        rig.model.answered_by().unwrap().note().unwrap(),
        "Answered by Gemma 4 31B — Nemotron Super is unavailable"
    );
    let reports = rig.reports.0.lock().unwrap();
    assert!(matches!(&reports[0], CallReport::Answered(by) if by.fell_back()));
}

#[tokio::test]
async fn a_model_that_failed_moments_ago_is_skipped_without_a_call() {
    let health = Arc::new(HealthBook::new());
    health.record_failure(&or(NEMOTRON), ProviderErrorKind::ModelUnavailable, T0);
    let rig = rig_with(
        free_plan(NEMOTRON, &[GEMMA]),
        &[
            (NEMOTRON, &[Script::Answer("x")]),
            (GEMMA, &[Script::Answer("from gemma")]),
        ],
        DEFAULT_BOUNDS,
        health,
        FakeClock::at(T0 + MIN),
    );
    assert_eq!(rig.model.complete("q", 10).await.unwrap(), "from gemma");
    assert_eq!(rig.provider.calls(), [GEMMA]);
    let by = rig.model.answered_by().unwrap();
    assert!(matches!(
        by.switches[0].reason,
        SwitchReason::Skipped {
            last: Some(ProviderErrorKind::ModelUnavailable)
        }
    ));

    // Every model skipped: nothing is called, and the failure says so plainly.
    let health = Arc::new(HealthBook::new());
    health.record_failure(&or(NEMOTRON), ProviderErrorKind::ModelUnavailable, T0);
    let rig = rig_with(
        Plan::only(free_plan(NEMOTRON, &[]).requested),
        &[(NEMOTRON, &[Script::Answer("x")])],
        DEFAULT_BOUNDS,
        health,
        FakeClock::at(T0 + MIN),
    );
    let error = rig.model.complete("q", 10).await.unwrap_err();
    assert!(rig.provider.calls().is_empty());
    assert_eq!(
        classify(&error, None, T0).kind,
        ProviderErrorKind::ModelUnavailable,
        "{error}"
    );
}

#[tokio::test]
async fn an_account_rate_limit_skips_other_free_models_for_another_provider() {
    let rig = make_rig(
        free_plan(NEMOTRON, &[GEMMA, "gemini-3-flash-preview"]),
        &[
            (
                NEMOTRON,
                &[Script::Fail(Fixture::PerMinute429 {
                    retry_after_secs: 30,
                })],
            ),
            (GEMMA, &[Script::Answer("must not be called")]),
            ("gemini-3-flash-preview", &[Script::Answer("from gemini")]),
        ],
    );
    assert_eq!(rig.model.complete("q", 10).await.unwrap(), "from gemini");
    assert_eq!(rig.provider.calls(), [NEMOTRON, "gemini-3-flash-preview"]);
    assert_eq!(
        rig.health.health(&or(NEMOTRON), rig.clock.now()),
        Health::Healthy,
        "a rate limit is not ill health"
    );
}

#[tokio::test]
async fn an_account_rate_limit_with_only_free_models_waits_a_short_retry_after_once() {
    let rig = make_rig(
        free_plan(NEMOTRON, &[GEMMA]),
        &[
            (
                NEMOTRON,
                &[
                    Script::Fail(Fixture::PerMinute429 {
                        retry_after_secs: 5,
                    }),
                    Script::Answer("after the wait"),
                ],
            ),
            (GEMMA, &[Script::Answer("must not be called")]),
        ],
    );
    assert_eq!(rig.model.complete("q", 10).await.unwrap(), "after the wait");
    assert_eq!(rig.provider.calls(), [NEMOTRON, NEMOTRON]);
    assert_eq!(*rig.pause.waits.lock().unwrap(), [Duration::from_secs(5)]);

    // A long wait is not waited: the rate limit is reported.
    let rig = make_rig(
        free_plan(NEMOTRON, &[GEMMA]),
        &[
            (
                NEMOTRON,
                &[Script::Fail(Fixture::PerMinute429 {
                    retry_after_secs: 45,
                })],
            ),
            (GEMMA, &[Script::Answer("must not be called")]),
        ],
    );
    let error = rig.model.complete("q", 10).await.unwrap_err();
    assert_eq!(
        classify(&error, None, T0).kind,
        ProviderErrorKind::RateLimited
    );
    assert_eq!(rig.provider.calls(), [NEMOTRON]);
    assert!(rig.pause.waits.lock().unwrap().is_empty());

    // Waited once only: a second limit after the wait ends the call.
    let rig = make_rig(
        free_plan(NEMOTRON, &[]),
        &[(
            NEMOTRON,
            &[Script::Fail(Fixture::PerMinute429 {
                retry_after_secs: 3,
            })],
        )],
    );
    rig.model.complete("q", 10).await.unwrap_err();
    assert_eq!(rig.provider.calls(), [NEMOTRON, NEMOTRON]);
}

#[tokio::test]
async fn an_upstream_rate_limit_of_one_free_model_moves_to_another_free_model() {
    let rig = make_rig(
        free_plan(GEMMA, &[LIGHTNING]),
        &[
            (GEMMA, &[Script::Fail(Fixture::UpstreamFree429)]),
            (LIGHTNING, &[Script::Answer("from lightning")]),
        ],
    );
    assert_eq!(rig.model.complete("q", 10).await.unwrap(), "from lightning");
}

#[tokio::test]
async fn the_daily_free_quota_never_hops_free_models_and_says_when_it_resets() {
    let rig = make_rig(
        free_plan(NEMOTRON, &[GEMMA, LIGHTNING]),
        &[
            (NEMOTRON, &[Script::Fail(Fixture::Daily429)]),
            (GEMMA, &[Script::Answer("must not be called")]),
            (LIGHTNING, &[Script::Answer("must not be called")]),
        ],
    );
    let exhausted = rig.model.complete_with_fallback("q", 10).await.unwrap_err();
    assert_eq!(rig.provider.calls(), [NEMOTRON]);
    assert_eq!(exhausted.failure.kind, ProviderErrorKind::QuotaExhausted);
    assert!(is_daily_free_quota(&exhausted.failure));
    let now = rig.clock.now();
    assert_eq!(
        reset_at_ms(&exhausted.failure, now),
        Some(now + 5 * 60 * MIN)
    );
    // The text callers get keeps the reset time for their own message.
    let text = exhausted.failure.to_string();
    assert_eq!(
        crate::harness::provider_error::limit_reset_at_ms(&text, now),
        Some(now + 5 * 60 * MIN)
    );
    assert!(rig.pause.waits.lock().unwrap().is_empty());

    // Another provider is outside the free quota: it answers.
    let rig = make_rig(
        free_plan(NEMOTRON, &[GEMMA, "gemini-3-flash-preview"]),
        &[
            (NEMOTRON, &[Script::Fail(Fixture::Daily429)]),
            ("gemini-3-flash-preview", &[Script::Answer("from gemini")]),
        ],
    );
    assert_eq!(rig.model.complete("q", 10).await.unwrap(), "from gemini");
}

#[tokio::test]
async fn a_bad_key_or_a_refusal_never_falls_back() {
    for fixture in [Fixture::BadKey401, Fixture::Moderation403] {
        let rig = make_rig(
            free_plan(NEMOTRON, &[GEMMA]),
            &[
                (NEMOTRON, &[Script::Fail(fixture)]),
                (GEMMA, &[Script::Answer("must not be called")]),
            ],
        );
        let exhausted = rig.model.complete_with_fallback("q", 10).await.unwrap_err();
        assert_eq!(exhausted.reason, StopReason::NotHelpful, "{fixture:?}");
        assert_eq!(rig.provider.calls(), [NEMOTRON], "{fixture:?}");
    }
    let kind = |f| failure(f, &or(NEMOTRON), T0).kind;
    assert_eq!(kind(Fixture::BadKey401), ProviderErrorKind::Auth);
    assert_eq!(
        kind(Fixture::Moderation403),
        ProviderErrorKind::ContentFiltered
    );
}

#[tokio::test]
async fn a_request_too_long_goes_only_to_a_larger_context() {
    // Gemma (131k tokens) is too small: Tiny Chat (8k) is smaller still, Lightning (1M) is not.
    let mut plan = free_plan(GEMMA, &[]);
    let base = free_plan(NEMOTRON, &[LIGHTNING]);
    let mut tiny = base.requested.clone();
    tiny.model = or("small/tiny-chat:free");
    tiny.name = "Tiny Chat".into();
    tiny.context_length = Some(8_192);
    plan.fallbacks = vec![tiny, base.fallbacks[0].clone()];
    let rig = make_rig(
        plan,
        &[
            (GEMMA, &[Script::Fail(Fixture::ContextTooLong)]),
            (
                "small/tiny-chat:free",
                &[Script::Answer("must not be called")],
            ),
            (LIGHTNING, &[Script::Answer("from lightning")]),
        ],
    );
    assert_eq!(rig.model.complete("q", 10).await.unwrap(), "from lightning");
    assert_eq!(rig.provider.calls(), [GEMMA, LIGHTNING]);
}

#[tokio::test]
async fn at_most_two_fallbacks_are_tried() {
    let mut plan = free_plan(NEMOTRON, &[GEMMA, LIGHTNING]);
    let mut extra = plan.fallbacks[0].clone();
    extra.model = or("small/tiny-chat:free");
    plan.fallbacks.push(extra);
    let rig = make_rig(
        plan,
        &[
            (NEMOTRON, &[Script::Fail(Fixture::Unavailable404)]),
            (GEMMA, &[Script::Fail(Fixture::Server500)]),
            (LIGHTNING, &[Script::Fail(Fixture::Timeout)]),
            (
                "small/tiny-chat:free",
                &[Script::Answer("must not be called")],
            ),
        ],
    );
    let exhausted = rig.model.complete_with_fallback("q", 10).await.unwrap_err();
    assert_eq!(exhausted.reason, StopReason::MaxFallbacks);
    assert_eq!(rig.provider.calls(), [NEMOTRON, GEMMA, LIGHTNING]);
    assert_eq!(
        exhausted.failure.kind,
        ProviderErrorKind::Timeout,
        "the last failure is reported"
    );
}

#[tokio::test]
async fn no_fallback_starts_after_the_time_cap() {
    let bounds = Bounds {
        max_fallbacks: 2,
        time_cap: Duration::from_secs(30),
    };
    let rig = rig_with(
        free_plan(NEMOTRON, &[GEMMA]),
        &[
            (NEMOTRON, &[Script::Fail(Fixture::Timeout)]),
            (GEMMA, &[Script::Answer("must not be called")]),
        ],
        bounds,
        Arc::new(HealthBook::new()),
        FakeClock::at(T0),
    );
    // The requested model's call took 31 s of the fake clock.
    let provider = Arc::new(FakeProvider {
        clock: rig.clock.clone(),
        scripts: Mutex::new(rig.provider.scripts.lock().unwrap().clone()),
        calls: Mutex::new(Vec::new()),
        call_ms: 31_000,
    });
    let plan = free_plan(NEMOTRON, &[GEMMA]);
    let outcome = run_plan(
        &plan,
        &rig.health,
        rig.clock.as_ref(),
        rig.pause.as_ref(),
        bounds,
        |candidate| {
            let provider = provider.clone();
            async move {
                provider
                    .next(&candidate.model)
                    .map_err(AttemptFailure::from)
            }
        },
    )
    .await;
    let exhausted = outcome.unwrap_err();
    assert_eq!(exhausted.reason, StopReason::TimeCap);
    assert_eq!(provider.calls(), [NEMOTRON]);
}

#[tokio::test]
async fn nothing_switches_once_part_of_the_answer_reached_the_user() {
    let health = HealthBook::new();
    let clock = FakeClock::at(T0);
    let pause = FakePause {
        clock: clock.clone(),
        waits: Mutex::new(Vec::new()),
    };
    let plan = free_plan(NEMOTRON, &[GEMMA]);
    let calls = Mutex::new(Vec::new());
    let outcome: Result<Answered<String>, Exhausted> = run_plan(
        &plan,
        &health,
        clock.as_ref(),
        &pause,
        DEFAULT_BOUNDS,
        |candidate| {
            calls.lock().unwrap().push(candidate.model.model.clone());
            let failure = failure(Fixture::Server500, &candidate.model, T0);
            async move {
                Err(AttemptFailure {
                    failure,
                    streamed: true,
                })
            }
        },
    )
    .await;
    let exhausted = outcome.unwrap_err();
    assert_eq!(exhausted.reason, StopReason::Streamed);
    assert_eq!(*calls.lock().unwrap(), [NEMOTRON]);
    assert!(exhausted.switches.is_empty());
}

#[test]
fn failure_words_are_plain() {
    assert_eq!(
        reason_words(SwitchReason::Failed {
            kind: ProviderErrorKind::ModelUnavailable
        }),
        "is unavailable"
    );
    let by = AnsweredBy {
        model: or(NEMOTRON),
        name: "Nemotron Super".into(),
        requested: or(NEMOTRON),
        requested_name: "Nemotron Super".into(),
        switches: Vec::new(),
    };
    assert_eq!(by.note(), None, "no note when the requested model answered");
}

#[test]
fn a_rate_limit_decision_tells_upstream_from_account_limits() {
    let plan = free_plan(GEMMA, &[]);
    let upstream = failure(Fixture::UpstreamFree429, &or(GEMMA), T0);
    assert_eq!(upstream.kind, ProviderErrorKind::RateLimited);
    assert!(matches!(
        decide(&upstream, &plan.requested, T0),
        Decision::Switch {
            eligible: Eligible::Any,
            ..
        }
    ));
    let account = failure(
        Fixture::PerMinute429 {
            retry_after_secs: 7,
        },
        &or(GEMMA),
        T0,
    );
    assert_eq!(account.kind, ProviderErrorKind::RateLimited);
    match decide(&account, &plan.requested, T0) {
        Decision::Switch {
            eligible: Eligible::OutsideLimitOf(_),
            retry_after,
        } => assert_eq!(retry_after, Some(Duration::from_secs(7))),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_subscription_cannot_answer_a_direct_call_so_the_next_model_does() {
    let mut setup = Setup::new(ModelRef::new(ProviderId::ClaudeSub, "claude-opus-5-5"));
    setup.connected = vec![ProviderId::ClaudeSub, ProviderId::Google];
    let plan = setup.plan(&HealthBook::new(), &AvailabilityBook::new(), T0);
    assert!(
        !plan.requested.reachable,
        "only the agent runtime holds a subscription"
    );
    assert_eq!(ids(&plan)[1..], ["gemini-3-flash-preview"]);
    let rig = make_rig(
        plan,
        &[("gemini-3-flash-preview", &[Script::Answer("from gemini")])],
    );
    assert_eq!(rig.model.complete("q", 10).await.unwrap(), "from gemini");
    assert_eq!(rig.provider.calls(), ["gemini-3-flash-preview"]);
    let by = rig.model.answered_by().unwrap();
    assert_eq!(by.switches[0].reason, SwitchReason::Unreachable);
    assert_eq!(
        by.note().unwrap(),
        "Answered by Gemini 3 Flash — claude-opus-5-5 cannot answer here"
    );
}
