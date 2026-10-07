//! Optional models (reranker, answer checking, table structure) loaded on first use and
//! dropped again after an idle period, so a session that never needs them does not keep
//! hundreds of megabytes resident. The embedding model is not handled here: search and
//! indexing need it all the time.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

type Loader<T> = Arc<dyn Fn() -> anyhow::Result<T> + Send + Sync>;

/// Idle time after which a loaded optional model is dropped, unless the
/// `SHODH_MODEL_IDLE_MINUTES` environment variable sets another (`0` keeps models loaded).
pub const DEFAULT_IDLE: Duration = Duration::from_secs(10 * 60);

/// The idle period from `SHODH_MODEL_IDLE_MINUTES`, else [`DEFAULT_IDLE`]. `None`: never
/// unload.
pub fn idle_period() -> Option<Duration> {
    idle_period_from(std::env::var("SHODH_MODEL_IDLE_MINUTES").ok().as_deref())
}

fn idle_period_from(value: Option<&str>) -> Option<Duration> {
    match value.map(str::trim).map(str::parse::<u64>) {
        Some(Ok(0)) => None,
        Some(Ok(minutes)) => Some(Duration::from_secs(minutes * 60)),
        Some(Err(_)) => {
            tracing::warn!(
                "SHODH_MODEL_IDLE_MINUTES is not a number of minutes; the default is used"
            );
            Some(DEFAULT_IDLE)
        }
        None => Some(DEFAULT_IDLE),
    }
}

struct State<T> {
    loader: Option<Loader<T>>,
    model: Option<Arc<T>>,
    last_used: Instant,
}

/// A model that is installed (it has a loader) and loaded only while it is used.
pub struct LazyModel<T> {
    name: &'static str,
    state: Mutex<State<T>>,
}

impl<T> std::fmt::Debug for LazyModel<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LazyModel")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl<T> LazyModel<T> {
    /// A model that is not installed yet.
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            state: Mutex::new(State {
                loader: None,
                model: None,
                last_used: Instant::now(),
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State<T>> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The model is installed: `loader` loads it on first use. A model loaded before is
    /// dropped (a new install replaces it).
    pub fn set_loader(&self, loader: impl Fn() -> anyhow::Result<T> + Send + Sync + 'static) {
        let mut state = self.lock();
        state.loader = Some(Arc::new(loader));
        state.model = None;
    }

    /// The model is no longer installed; a loaded one is dropped.
    pub fn uninstall(&self) {
        let mut state = self.lock();
        state.loader = None;
        state.model = None;
    }

    /// Whether the model is installed (loaded or loadable).
    pub fn available(&self) -> bool {
        self.lock().loader.is_some()
    }

    /// Whether the model is in memory now.
    pub fn is_loaded(&self) -> bool {
        self.lock().model.is_some()
    }

    /// The model, loaded first if needed. Blocking on first use (loading takes seconds):
    /// call from a blocking thread where possible. `None` when the model is not installed
    /// or fails to load; a model that fails to load is logged and treated as not
    /// installed until it is installed again.
    pub fn get(&self) -> Option<Arc<T>> {
        // The lock is held while loading, so concurrent first uses load once.
        let mut state = self.lock();
        state.last_used = Instant::now();
        if let Some(model) = &state.model {
            return Some(model.clone());
        }
        let loader = state.loader.clone()?;
        let started = Instant::now();
        match loader() {
            Ok(model) => {
                let model = Arc::new(model);
                state.model = Some(model.clone());
                state.last_used = Instant::now();
                tracing::info!(
                    model = self.name,
                    load_ms = started.elapsed().as_millis() as u64,
                    "Model loaded on first use"
                );
                Some(model)
            }
            Err(e) => {
                tracing::warn!(model = self.name, error = %format!("{e:#}"), "Model failed to load; it is not used");
                state.loader = None;
                None
            }
        }
    }

    /// Drops the model when it was not used for `idle`. Holders of a handle from
    /// [`Self::get`] keep theirs until they drop it. Returns whether it was dropped.
    pub fn unload_if_idle(&self, idle: Duration) -> bool {
        let Ok(mut state) = self.state.try_lock() else {
            // Loading or in use right now: not idle.
            return false;
        };
        if state.model.is_some() && state.last_used.elapsed() >= idle {
            state.model = None;
            tracing::info!(model = self.name, "Idle model unloaded");
            return true;
        }
        false
    }
}

/// Unloads `models` that were idle for `idle`, checking every minute (or every `idle`,
/// if shorter), for as long as the runtime runs.
pub async fn unload_idle_models(models: Vec<Arc<dyn IdleUnload>>, idle: Duration) {
    let mut tick = tokio::time::interval(idle.min(Duration::from_secs(60)));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        for model in &models {
            model.unload_if_idle(idle);
        }
    }
}

/// A model [`unload_idle_models`] can unload.
pub trait IdleUnload: Send + Sync {
    /// See [`LazyModel::unload_if_idle`].
    fn unload_if_idle(&self, idle: Duration) -> bool;
}

impl<T: Send + Sync> IdleUnload for LazyModel<T> {
    fn unload_if_idle(&self, idle: Duration) -> bool {
        LazyModel::unload_if_idle(self, idle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn loads_on_first_use_once_and_unloads_when_idle() {
        let loads = Arc::new(AtomicUsize::new(0));
        let model = LazyModel::<String>::new("test");
        assert!(!model.available());
        assert!(model.get().is_none());

        let counter = loads.clone();
        model.set_loader(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok("weights".to_string())
        });
        assert!(model.available());
        assert!(!model.is_loaded(), "installing does not load");
        assert_eq!(model.get().as_deref().map(String::as_str), Some("weights"));
        assert_eq!(model.get().as_deref().map(String::as_str), Some("weights"));
        assert_eq!(loads.load(Ordering::SeqCst), 1);

        assert!(
            !model.unload_if_idle(Duration::from_secs(3600)),
            "recently used"
        );
        assert!(model.unload_if_idle(Duration::ZERO));
        assert!(!model.is_loaded());
        assert!(model.available(), "still installed");
        assert!(model.get().is_some());
        assert_eq!(loads.load(Ordering::SeqCst), 2, "loaded again on next use");
    }

    #[test]
    fn a_model_that_fails_to_load_is_not_retried() {
        let loads = Arc::new(AtomicUsize::new(0));
        let model = LazyModel::<String>::new("broken");
        let counter = loads.clone();
        model.set_loader(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            anyhow::bail!("corrupt file")
        });
        assert!(model.get().is_none());
        assert!(model.get().is_none());
        assert_eq!(loads.load(Ordering::SeqCst), 1);
        assert!(!model.available());
    }

    #[test]
    fn idle_period_is_configurable_and_zero_keeps_models() {
        assert_eq!(idle_period_from(None), Some(DEFAULT_IDLE));
        assert_eq!(idle_period_from(Some("3")), Some(Duration::from_secs(180)));
        assert_eq!(idle_period_from(Some("0")), None);
        assert_eq!(idle_period_from(Some("soon")), Some(DEFAULT_IDLE));
    }

    /// Working set and private bytes of this process, in MB.
    #[cfg(windows)]
    fn process_memory_mb() -> (f64, f64) {
        #[repr(C)]
        #[derive(Default)]
        struct ProcessMemoryCounters {
            cb: u32,
            page_fault_count: u32,
            peak_working_set_size: usize,
            working_set_size: usize,
            quota_peak_paged_pool_usage: usize,
            quota_paged_pool_usage: usize,
            quota_peak_non_paged_pool_usage: usize,
            quota_non_paged_pool_usage: usize,
            pagefile_usage: usize,
            peak_pagefile_usage: usize,
        }
        extern "system" {
            fn GetCurrentProcess() -> isize;
            fn K32GetProcessMemoryInfo(
                process: isize,
                counters: *mut ProcessMemoryCounters,
                cb: u32,
            ) -> i32;
        }
        let mut counters = ProcessMemoryCounters {
            cb: std::mem::size_of::<ProcessMemoryCounters>() as u32,
            ..Default::default()
        };
        // SAFETY: the pseudo handle of the current process is always valid and
        // `counters` is a correctly sized PROCESS_MEMORY_COUNTERS.
        let ok =
            unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) };
        assert_ne!(ok, 0, "GetProcessMemoryInfo failed");
        let mb = |bytes: usize| bytes as f64 / (1024.0 * 1024.0);
        (mb(counters.working_set_size), mb(counters.pagefile_usage))
    }

    /// Resident memory the optional models cost at startup when loaded eagerly, which
    /// lazy loading avoids until first use. Run with `SHODH_TEST_MODELS` set to the
    /// model root: `cargo test -p shodh-rag --lib -- --ignored
    /// optional_models_resident_memory --nocapture`.
    #[cfg(windows)]
    #[test]
    #[ignore = "measurement; requires SHODH_TEST_MODELS"]
    fn optional_models_resident_memory() {
        use crate::embeddings::model_store::{ANSWER_CHECK_DIR, RERANKER_DIR};
        let root = std::path::PathBuf::from(
            std::env::var_os("SHODH_TEST_MODELS").expect("SHODH_TEST_MODELS"),
        );
        let reranker = LazyModel::new("reranker");
        let dir = root.join(RERANKER_DIR);
        reranker.set_loader(move || crate::reranking::CrossEncoderReranker::new(&dir));
        let tables = LazyModel::new("table model");
        let dir = crate::processing::table_model::model_dir(&root);
        tables.set_loader(move || Ok(crate::processing::table_model::TableModel::load(&dir, 4)?));
        let nli = LazyModel::new("answer checking model");
        let dir = root.join(ANSWER_CHECK_DIR);
        if dir.exists() {
            nli.set_loader(move || crate::reranking::NliModel::new(&dir));
        }

        let (ws0, private0) = process_memory_mb();
        let loaded = [
            ("reranker", reranker.get().is_some()),
            ("table model", tables.get().is_some()),
            ("answer checking model", nli.get().is_some()),
        ];
        let (ws1, private1) = process_memory_mb();
        for model in [&reranker as &dyn IdleUnload, &tables, &nli] {
            model.unload_if_idle(Duration::ZERO);
        }
        let (ws2, private2) = process_memory_mb();
        eprintln!("loaded: {loaded:?}");
        eprintln!(
            "working set MB: {ws0:.0} before, {ws1:.0} loaded (+{:.0}), {ws2:.0} after unload",
            ws1 - ws0
        );
        eprintln!(
            "private MB: {private0:.0} before, {private1:.0} loaded (+{:.0}), {private2:.0} after unload",
            private1 - private0
        );
    }
}
