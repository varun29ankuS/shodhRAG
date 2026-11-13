//! The main window's first-load handshake. The frontend reports once it has
//! reached every startup milestone (`app/src/lib/startupTiming.ts`): its modules
//! ran, the shell is on screen, the index answered `initialize_rag`, and the model
//! status, the statistics and the saved conversations were loaded. Its command
//! only answers once the backend's setup has finished (the states it reads are
//! managed at the end of setup), so the log line it writes says both sides are
//! up. Installed builds have no console: the line, with the milestone times,
//! tells a slow or stuck start from a normal one. The release pipeline's macOS
//! launch test waits for it (release.yml).

use serde::Deserialize;

/// The start of the log line the frontend's report writes.
pub const READY_LINE: &str = "Main window ready";

/// When each startup milestone was reached, in milliseconds since the WebView
/// started loading the page.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartupTimings {
    pub modules_evaluated: u32,
    pub shell_mounted: u32,
    pub index_ready: u32,
    pub model_checked: u32,
    pub stats_loaded: u32,
    pub conversations_loaded: u32,
}

impl StartupTimings {
    /// The milestone times as `name=ms` pairs, in the order they are reached.
    fn summary(&self) -> String {
        format!(
            "modules-evaluated={} shell-mounted={} index-ready={} model-checked={} stats-loaded={} conversations-loaded={}",
            self.modules_evaluated,
            self.shell_mounted,
            self.index_ready,
            self.model_checked,
            self.stats_loaded,
            self.conversations_loaded,
        )
    }
}

/// The main window reached every startup milestone (each page load reports
/// once: the first, and a reload).
#[tauri::command]
pub fn startup_ready(timings: StartupTimings) {
    tracing::info!(
        "{READY_LINE} (ms since the page started loading: {})",
        timings.summary()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_report_names_every_milestone_in_order() {
        let timings: StartupTimings = serde_json::from_value(serde_json::json!({
            "modulesEvaluated": 120,
            "shellMounted": 310,
            "indexReady": 450,
            "modelChecked": 470,
            "statsLoaded": 480,
            "conversationsLoaded": 400,
        }))
        .expect("the frontend's payload");
        assert_eq!(
            timings.summary(),
            "modules-evaluated=120 shell-mounted=310 index-ready=450 model-checked=470 stats-loaded=480 conversations-loaded=400"
        );
    }

    #[test]
    fn a_partial_or_unknown_report_is_refused() {
        let missing = serde_json::json!({ "modulesEvaluated": 1, "shellMounted": 2 });
        assert!(serde_json::from_value::<StartupTimings>(missing).is_err());
        let unknown = serde_json::json!({
            "modulesEvaluated": 1, "shellMounted": 2, "indexReady": 3, "modelChecked": 4,
            "statsLoaded": 5, "conversationsLoaded": 6, "extra": 7,
        });
        assert!(serde_json::from_value::<StartupTimings>(unknown).is_err());
    }
}
