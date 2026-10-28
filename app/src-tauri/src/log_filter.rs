//! Which lines reach the log: Shodh's own from INFO, the libraries' internals only
//! from WARN.
//!
//! Measured on a real log (two days, one user): 93 % of the lines and 82 % of the
//! bytes were the libraries' INFO lines. Lance and DataFusion log one line for every
//! query plan, index part, dataset open and commit; ONNX Runtime logs its session
//! set-up and every memory arena allocation each time a model loads; the PDF reader
//! logs every embedded font. None of it says what Shodh did or why it failed, and it
//! pushed Shodh's own lines out of the size-bounded log.
//!
//! `RUST_LOG`, when set, replaces all of this (for a developer's session).

use tracing_subscriber::EnvFilter;

/// The default filter's directives, in `RUST_LOG` syntax. A target matches a directive
/// when it starts with the directive's target, so `lance` covers `lance::execution`,
/// `lance_index` and `lancedb`.
pub const DEFAULT_DIRECTIVES: &[&str] = &[
    // Shodh's own lines (and any library not named below).
    "info",
    // Lance (`lance::execution`, `lance::io_events`, `lance::dataset_events`,
    // `lance::file_audit`, `lance::io::commit`) and DataFusion: per-query internals.
    "lance=warn",
    "lancedb=warn",
    "datafusion=warn",
    // ONNX Runtime through `ort` (`ort::logging`): session set-up, graph optimisation
    // passes and arena allocations.
    "ort=warn",
    // The PDF reader: one line per embedded font and character map.
    "pdf_oxide=warn",
    // tao's Windows event loop warns when the OS delivers its events in an unexpected
    // order; nothing is lost, and tao 0.37 logs the same lines at debug. Only that
    // module is quieted: tao's other warnings still reach the log.
    "tao::platform_impl::platform::event_loop::runner=error",
    // The updater plugin logs a refused check as an ERROR before returning it, which
    // is the normal state while no release is published. `updater.rs` logs every
    // outcome of a check and every failed install itself, at the level it deserves.
    "tauri_plugin_updater=off",
];

/// The default filter.
pub fn default_filter() -> EnvFilter {
    EnvFilter::new(DEFAULT_DIRECTIVES.join(","))
}

/// `RUST_LOG` when it is set and valid, else [`default_filter`].
pub fn from_env() -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| default_filter())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tracing::{Event, Subscriber};
    use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

    use super::*;

    /// Records `LEVEL target` for every event the filter lets through.
    #[derive(Clone, Default)]
    struct Recorder(Arc<Mutex<Vec<String>>>);

    impl<S: Subscriber> Layer<S> for Recorder {
        fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
            let meta = event.metadata();
            self.0
                .lock()
                .unwrap()
                .push(format!("{} {}", meta.level(), meta.target()));
        }
    }

    fn passed(emit: impl FnOnce()) -> Vec<String> {
        let recorder = Recorder::default();
        let subscriber = tracing_subscriber::registry()
            .with(default_filter())
            .with(recorder.clone());
        tracing::subscriber::with_default(subscriber, emit);
        let lines = recorder.0.lock().unwrap().clone();
        lines
    }

    #[test]
    fn every_directive_parses() {
        for directive in DEFAULT_DIRECTIVES {
            assert!(
                EnvFilter::try_new(directive).is_ok(),
                "{directive} does not parse"
            );
        }
        assert!(EnvFilter::try_new(DEFAULT_DIRECTIVES.join(",")).is_ok());
    }

    #[test]
    fn shodhs_own_info_lines_are_kept() {
        let lines = passed(|| {
            tracing::info!(target: "shodh_rag::rag_engine", "kept");
            tracing::info!(target: "shodh::telegram", "kept");
            tracing::info!(target: "shodh_rag_app::updater", "kept");
            tracing::debug!(target: "shodh_rag::rag_engine", "dropped");
        });
        assert_eq!(
            lines,
            [
                "INFO shodh_rag::rag_engine",
                "INFO shodh::telegram",
                "INFO shodh_rag_app::updater",
            ]
        );
    }

    /// The targets of the lines that filled the live log.
    #[test]
    fn library_internals_are_kept_only_from_warn() {
        let lines = passed(|| {
            tracing::info!(target: "lance::execution", "plan_run");
            tracing::info!(target: "lance::io_events", "load_scalar_part");
            tracing::info!(target: "lance::dataset_events", "loading");
            tracing::info!(target: "lance::file_audit", "create");
            tracing::info!(target: "lance::io::commit", "Auto cleanup triggered");
            tracing::info!(target: "lance_index::scalar", "loaded");
            tracing::info!(target: "lancedb::table", "opened");
            tracing::info!(target: "datafusion::physical_plan", "plan");
            tracing::info!(target: "ort::logging", "Reserving memory in BFCArena");
            tracing::info!(target: "pdf_oxide::fonts::font_dict", "ToUnicode CMap");
            tracing::warn!(target: "lance::io::commit", "kept");
            tracing::warn!(target: "ort::logging", "kept");
            tracing::error!(target: "pdf_oxide::document", "kept");
        });
        assert_eq!(
            lines,
            [
                "WARN lance::io::commit",
                "WARN ort::logging",
                "ERROR pdf_oxide::document",
            ]
        );
    }

    #[test]
    fn only_taos_event_order_warning_is_quieted() {
        let lines = passed(|| {
            tracing::warn!(
                target: "tao::platform_impl::platform::event_loop::runner",
                "NewEvents emitted without explicit RedrawEventsCleared"
            );
            tracing::error!(
                target: "tao::platform_impl::platform::event_loop::runner",
                "kept"
            );
            tracing::warn!(target: "tao::platform_impl::platform::window", "kept");
        });
        assert_eq!(
            lines,
            [
                "ERROR tao::platform_impl::platform::event_loop::runner",
                "WARN tao::platform_impl::platform::window",
            ]
        );
    }

    #[test]
    fn the_updater_plugins_own_lines_are_dropped() {
        let lines = passed(|| {
            tracing::error!(
                target: "tauri_plugin_updater::updater",
                "update endpoint did not respond with a successful status code"
            );
            tracing::info!(target: "shodh_rag_app::updater", "kept");
        });
        assert_eq!(lines, ["INFO shodh_rag_app::updater"]);
    }

    /// The live log's two-day mix, by line count: what the filter keeps.
    #[test]
    fn the_live_logs_mix_shrinks_to_shodhs_lines() {
        let lines = passed(|| {
            for _ in 0..100 {
                tracing::info!(target: "lance::execution", "plan_run");
                tracing::info!(target: "ort::logging", "GraphTransformer");
            }
            for _ in 0..3 {
                tracing::warn!(
                    target: "tao::platform_impl::platform::event_loop::runner",
                    "RedrawEventsCleared emitted without explicit MainEventsCleared"
                );
            }
            tracing::info!(target: "shodh::telegram", "connected to Telegram");
        });
        assert_eq!(lines, ["INFO shodh::telegram"]);
    }
}
