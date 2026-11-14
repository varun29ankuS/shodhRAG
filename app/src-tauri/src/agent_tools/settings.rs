//! Settings tools: `get_settings` (read) and `update_setting` (write) over
//! the explicit allowlist in [`crate::app_settings::AGENT_WRITABLE`].
//!
//! Secrets, approvals, audit policy, Local-only mode, web access and model
//! or provider choice are never reachable here; see
//! [`crate::app_settings::AGENT_DENIED`] for each reason.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use shodh_rag::audit::AuditEventType;
use shodh_rag::harness::events::NavigationTarget;
use shodh_rag::harness::profile::app_tools;
use shodh_rag::harness::tools::navigate::{emit_shown, SHOWN_TO_USER};
use shodh_rag::harness::tools::{
    ApprovalPreview, HostTool, RegistryError, ToolContext, ToolError, ToolOutput, ToolRegistry,
};
use shodh_rag::harness::RiskTier;

use super::{invalid, str_arg, AgentHost};
use crate::app_settings::{
    denied_reason, preference_change_payload, preference_value, set_preference, SettingKey,
    SettingsError, SettingsStore, AGENT_WRITABLE,
};

pub(super) fn register(
    registry: &mut ToolRegistry,
    host: &Arc<AgentHost>,
) -> Result<(), RegistryError> {
    registry.register(Arc::new(GetSettingsTool { host: host.clone() }))?;
    registry.register(Arc::new(UpdateSettingTool { host: host.clone() }))?;
    registry.register(Arc::new(ShowSettingTool { host: host.clone() }))?;
    Ok(())
}

fn store(host: &AgentHost) -> SettingsStore {
    SettingsStore::in_dir(&host.data_dir)
}

fn tool_error(tool: &str, error: SettingsError) -> ToolError {
    match error {
        SettingsError::InvalidValue { key, reason } => invalid(tool, format!("{key} {reason}")),
        other => ToolError::Failed(other.to_string()),
    }
}

fn writable_keys() -> String {
    AGENT_WRITABLE
        .iter()
        .map(|k| format!("{} ({})", k.as_str(), k.describe()))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Resolve the `key` argument, refusing withheld settings by name.
fn resolve_key(tool: &str, args: &Value) -> Result<SettingKey, ToolError> {
    let key = str_arg(args, "key").ok_or_else(|| invalid(tool, "`key` is required"))?;
    if let Some(why) = denied_reason(key) {
        return Err(ToolError::Forbidden(format!(
            "{key} cannot be changed by the assistant: {why} The user can change it in Settings."
        )));
    }
    SettingKey::parse(key).ok_or_else(|| {
        invalid(
            tool,
            format!("unknown setting {key}; you can change: {}", writable_keys()),
        )
    })
}

pub struct GetSettingsTool {
    host: Arc<AgentHost>,
}

#[async_trait]
impl HostTool for GetSettingsTool {
    fn name(&self) -> &'static str {
        app_tools::GET_SETTINGS
    }
    fn label(&self) -> &'static str {
        "Read settings"
    }
    fn label_template(&self) -> &'static str {
        "Checking settings"
    }
    fn description(&self) -> &'static str {
        "Read the app settings you may change (theme, passages per search), plus, read-only, \
         whether Local-only mode and web access are on and which model answers. API keys and \
         other secrets are never shown."
    }
    fn schema(&self) -> Value {
        json!({"type": "object", "properties": {}, "additionalProperties": false})
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }

    async fn execute(&self, _args: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let tool = app_tools::GET_SETTINGS;
        let settings = store(&self.host).load().map_err(|e| tool_error(tool, e))?;
        let mut writable = serde_json::Map::new();
        for key in AGENT_WRITABLE {
            writable.insert(
                key.as_str().to_string(),
                preference_value(&settings.preferences, key),
            );
        }
        let model = self.host.effects.model_info();
        let detail = json!({
            "changeable": writable,
            "readOnly": {
                "localOnly": settings.policy.local_only,
                "webAccess": settings.policy.web_access,
                "webAvailable": settings.policy.web_block_reason().is_none(),
                "model": model,
            },
        });
        Ok(ToolOutput {
            text_for_model: format!(
                "{detail}\nYou can change: {}. Everything under readOnly is the user's to change.",
                writable_keys()
            ),
            summary_for_ui: "Read settings".to_string(),
            detail: Some(detail),
        })
    }
}

pub struct UpdateSettingTool {
    host: Arc<AgentHost>,
}

#[async_trait]
impl HostTool for UpdateSettingTool {
    fn name(&self) -> &'static str {
        app_tools::UPDATE_SETTING
    }
    fn label(&self) -> &'static str {
        "Change setting"
    }
    fn label_template(&self) -> &'static str {
        "Changing {key!}[ to {value}]"
    }
    fn description(&self) -> &'static str {
        "Change one app setting. Allowed keys: theme (\"light\" or \"dark\") and \
         search_max_results (passages per document search, 3 to 20). Any other setting is refused."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "key": {"type": "string", "minLength": 1, "maxLength": 100},
                "value": {"type": ["string", "integer", "boolean"]}
            },
            "required": ["key", "value"],
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Write
    }
    async fn preview(&self, args: &Value) -> Result<ApprovalPreview, ToolError> {
        let tool = app_tools::UPDATE_SETTING;
        let key = resolve_key(tool, args)?;
        let value = args.get("value").cloned().unwrap_or(Value::Null);
        let mut prefs = store(&self.host)
            .load()
            .map_err(|e| tool_error(tool, e))?
            .preferences;
        let old = set_preference(&mut prefs, key, &value).map_err(|e| tool_error(tool, e))?;
        let new = preference_value(&prefs, key);
        if old == new {
            return Err(ToolError::Failed(format!(
                "{} is already {new}; nothing to change.",
                key.as_str()
            )));
        }
        Ok(ApprovalPreview {
            label: Some(format!("Change {} to {new}", key.as_str())),
            details: json!({ "changes": [{"field": key.as_str(), "before": old, "after": new}] }),
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let tool = app_tools::UPDATE_SETTING;
        let key = resolve_key(tool, &args)?;
        let value = args.get("value").cloned().unwrap_or(Value::Null);
        let (settings, old) = store(&self.host)
            .update(|s| set_preference(&mut s.preferences, key, &value))
            .map_err(|e| tool_error(tool, e))?;
        let new = preference_value(&settings.preferences, key);
        if old != new {
            ctx.audit(
                AuditEventType::SettingsChange,
                preference_change_payload(key, &old, &new, "agent"),
            );
            self.host.effects.settings_changed(&settings);
        }
        // The changed control, highlighted where Settings shows it.
        if let Some(section) = key.section() {
            reveal_setting(ctx, section, Some(key.as_str().to_string()), key.label());
        }
        Ok(ToolOutput {
            text_for_model: format!("{} is now {new} (was {old}).", key.as_str()),
            summary_for_ui: format!("Set {} to {new}", key.as_str()),
            detail: Some(json!({ "key": key.as_str(), "before": old, "after": new })),
        })
    }
}

/// The sections of the Settings view, in its order (`SettingsView.tsx`; the
/// coverage test checks the two agree).
pub const SETTINGS_SECTIONS: [&str; 12] = [
    "models",
    "search",
    "tools",
    "code",
    "tables",
    "general",
    "appearance",
    "notifications",
    "phone",
    "privacy",
    "memory",
    "data",
];

/// Open Settings at `section` with `item` (a setting key or a memory id) highlighted.
/// What Settings shows for `section`, in the user's words (`SettingsView.tsx`).
fn section_label(section: &str) -> &str {
    match section {
        "models" => "Models",
        "search" => "Search",
        "tools" => "Tools & connections",
        "code" => "Code",
        "tables" => "Tables",
        "general" => "General",
        "appearance" => "Appearance",
        "notifications" => "Notifications",
        "phone" => "Phone",
        "privacy" => "Privacy",
        "memory" => "Memory",
        "data" => "Data",
        other => other,
    }
}

fn reveal_setting(ctx: &ToolContext, section: &str, item: Option<String>, label: &str) {
    emit_shown(
        ctx,
        "settings",
        item.clone(),
        Some(NavigationTarget::Setting {
            section: section.to_string(),
            item,
        }),
        Some(label),
    );
}

/// `show_setting`: open a Settings section for the user, a setting or a memory
/// highlighted. It only shows: changing a setting is `update_setting` (with
/// approval), and the user-only settings stay the user's to change there.
pub struct ShowSettingTool {
    host: Arc<AgentHost>,
}

#[async_trait]
impl HostTool for ShowSettingTool {
    fn name(&self) -> &'static str {
        app_tools::SHOW_SETTING
    }
    fn label(&self) -> &'static str {
        "Show setting"
    }
    fn label_template(&self) -> &'static str {
        "Opening Settings[ {section}][ at {key}]"
    }
    fn description(&self) -> &'static str {
        "Open Settings for the user at a section: models, search, tools (tools, MCP servers and \
         skills), code (Code mode's building skills), tables, general (running in the background), appearance (text size), \
         notifications, phone (pair a phone: the pairing QR code, and the paired phones), privacy (Local-only mode and web access), memory or data. Give `key` \
         (a setting you may change) to highlight it, or `memory_id` (from recall) to highlight \
         a memory in the memory section. Use it to show the user where to change a setting \
         you may not change yourself."
    }
    fn schema(&self) -> Value {
        let keys: Vec<&str> = AGENT_WRITABLE
            .iter()
            .filter(|k| k.section().is_some())
            .map(|k| k.as_str())
            .collect();
        json!({
            "type": "object",
            "properties": {
                "section": {"type": "string", "enum": SETTINGS_SECTIONS},
                "key": {"type": "string", "enum": keys},
                "memory_id": {"type": "string", "minLength": 1, "maxLength": 200}
            },
            "additionalProperties": false
        })
    }
    fn tier(&self) -> RiskTier {
        RiskTier::Read
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let tool = app_tools::SHOW_SETTING;
        let requested = str_arg(&args, "section");
        if let Some(s) = requested {
            if !SETTINGS_SECTIONS.contains(&s) {
                return Err(invalid(tool, format!("unknown section {s}")));
            }
        }
        let (section, item) = if let Some(id) = str_arg(&args, "memory_id") {
            if requested.is_some_and(|s| s != "memory") {
                return Err(invalid(tool, "a memory is shown in the memory section"));
            }
            super::memory::check_visible(&self.host, ctx, tool, id).await?;
            ("memory", Some(id.to_string()))
        } else if let Some(raw) = str_arg(&args, "key") {
            let key = SettingKey::parse(raw)
                .ok_or_else(|| invalid(tool, format!("unknown setting {raw}")))?;
            let home = key.section().ok_or_else(|| {
                invalid(
                    tool,
                    format!("{raw} is not shown in Settings; it changes the whole window"),
                )
            })?;
            if requested.is_some_and(|s| s != home) {
                return Err(invalid(tool, format!("{raw} is in the {home} section")));
            }
            (home, Some(raw.to_string()))
        } else {
            let section =
                requested.ok_or_else(|| invalid(tool, "give a section, a key or a memory_id"))?;
            (section, None)
        };
        let label = match (&item, section) {
            (Some(_), "memory") => "a memory".to_string(),
            (Some(key), _) => SettingKey::parse(key)
                .map(|k| k.label().to_string())
                .unwrap_or_else(|| key.clone()),
            (None, _) => section_label(section).to_string(),
        };
        reveal_setting(ctx, section, item.clone(), &label);
        let what = match (&item, section) {
            (Some(_), "memory") => " with that memory highlighted".to_string(),
            (Some(key), _) => format!(" with {key} highlighted"),
            (None, _) => String::new(),
        };
        Ok(ToolOutput {
            text_for_model: format!("Settings at {section}{what}: {SHOWN_TO_USER}"),
            summary_for_ui: format!("Opened Settings at {section}"),
            detail: Some(json!({ "section": section, "item": item })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing;
    use super::*;
    use crate::app_settings::{Theme, AGENT_DENIED};

    #[tokio::test]
    async fn get_settings_reports_writable_values_and_policy_without_secrets() {
        let t = testing::host().await;
        let (ctx, _rx) = testing::ctx();
        let out = GetSettingsTool {
            host: t.host.clone(),
        }
        .execute(json!({}), &ctx)
        .await
        .unwrap();
        let detail = out.detail.unwrap();
        assert_eq!(detail["changeable"]["theme"], "dark");
        assert_eq!(detail["changeable"]["search_max_results"], 8);
        assert_eq!(detail["readOnly"]["webAvailable"], true);
        assert!(!out.text_for_model.to_lowercase().contains("api_key"));
    }

    #[tokio::test]
    async fn update_setting_previews_saves_audits_and_broadcasts() {
        let t = testing::host().await;
        let tool = UpdateSettingTool {
            host: t.host.clone(),
        };
        let args = json!({"key": "theme", "value": "light"});
        let preview = tool.preview(&args).await.unwrap();
        assert_eq!(preview.details["changes"][0]["before"], "dark");
        assert_eq!(preview.details["changes"][0]["after"], "light");
        let (ctx, _rx) = testing::ctx();
        tool.execute(args.clone(), &ctx).await.unwrap();
        let saved = SettingsStore::in_dir(&t.host.data_dir).load().unwrap();
        assert_eq!(saved.preferences.theme, Theme::Light);
        assert_eq!(t.effects.settings.lock().unwrap().len(), 1);
        assert!(tool.preview(&args).await.is_err(), "no-op");
        assert!(matches!(
            tool.preview(&json!({"key": "search_max_results", "value": 50}))
                .await,
            Err(ToolError::InvalidArguments { .. })
        ));
        assert!(matches!(
            tool.preview(&json!({"key": "font_size", "value": 3})).await,
            Err(ToolError::InvalidArguments { .. })
        ));
    }

    fn next_setting_target(
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<shodh_rag::harness::AgentEvent>,
        tool: &str,
    ) -> (String, Option<String>) {
        let shown = testing::expect_navigation(rx, tool);
        assert_eq!(shown.view, "settings");
        assert!(shown.label.is_some(), "it names what it shows");
        match shown.target {
            Some(NavigationTarget::Setting { section, item }) => (section, item),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn show_setting_opens_a_section_or_a_setting_and_checks_them() {
        let t = testing::host().await;
        let tool = ShowSettingTool {
            host: t.host.clone(),
        };
        let (ctx, mut rx) = testing::ctx();
        tool.execute(json!({"section": "privacy"}), &ctx)
            .await
            .unwrap();
        assert_eq!(
            next_setting_target(&mut rx, "show_setting"),
            ("privacy".into(), None)
        );
        let out = tool
            .execute(json!({"key": "search_max_results"}), &ctx)
            .await
            .unwrap();
        assert_eq!(
            next_setting_target(&mut rx, "show_setting"),
            ("search".into(), Some("search_max_results".into()))
        );
        assert!(out
            .text_for_model
            .contains("search_max_results highlighted"));
        for bad in [
            json!({}),
            json!({"section": "secrets"}),
            json!({"key": "theme"}),
            json!({"key": "local_only"}),
            json!({"section": "privacy", "key": "search_max_results"}),
            json!({"section": "search", "memory_id": "mem-1"}),
        ] {
            assert!(
                matches!(
                    tool.execute(bad.clone(), &ctx).await,
                    Err(ToolError::InvalidArguments { .. })
                ),
                "{bad}"
            );
        }
        assert!(tool
            .execute(json!({"memory_id": "mem-none"}), &ctx)
            .await
            .is_err());
        assert!(rx.try_recv().is_err(), "a refused call shows nothing");
    }

    #[tokio::test]
    async fn update_setting_shows_the_changed_control_after_saving() {
        let t = testing::host().await;
        let tool = UpdateSettingTool {
            host: t.host.clone(),
        };
        let (ctx, mut rx) = testing::ctx();
        tool.execute(json!({"key": "search_max_results", "value": 12}), &ctx)
            .await
            .unwrap();
        assert_eq!(
            next_setting_target(&mut rx, "update_setting"),
            ("search".into(), Some("search_max_results".into()))
        );
        // The theme changes the whole window; it has no control in Settings to show.
        tool.execute(json!({"key": "theme", "value": "light"}), &ctx)
            .await
            .unwrap();
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn every_denied_setting_is_refused_by_name_and_nothing_changes() {
        let t = testing::host().await;
        let tool = UpdateSettingTool {
            host: t.host.clone(),
        };
        let (ctx, _rx) = testing::ctx();
        let before = SettingsStore::in_dir(&t.host.data_dir).load().unwrap();
        for (key, _) in AGENT_DENIED {
            for value in [json!(true), json!(false), json!("x")] {
                let args = json!({"key": key, "value": value});
                assert!(
                    matches!(tool.preview(&args).await, Err(ToolError::Forbidden(_))),
                    "{key}"
                );
                assert!(
                    matches!(tool.execute(args, &ctx).await, Err(ToolError::Forbidden(_))),
                    "{key}"
                );
            }
        }
        let forbidden = tool
            .execute(json!({"key": "api_keys.openrouter", "value": "sk-x"}), &ctx)
            .await
            .unwrap_err();
        assert!(forbidden.to_string().contains("secrets"));
        assert_eq!(
            SettingsStore::in_dir(&t.host.data_dir).load().unwrap(),
            before
        );
        assert!(t.effects.settings.lock().unwrap().is_empty());
    }

    /// Agent-first: the assistant opens Settings → Phone, where the pairing QR code is
    /// shown, but no tool removes a phone (only the user does), and agent-coverage.json
    /// records both.
    #[tokio::test]
    async fn the_agent_can_open_pairing_but_not_remove_a_device() {
        let t = testing::host().await;
        let tool = ShowSettingTool {
            host: t.host.clone(),
        };
        let (ctx, mut rx) = testing::ctx();
        let out = tool
            .execute(json!({"section": "phone"}), &ctx)
            .await
            .unwrap();
        assert_eq!(
            next_setting_target(&mut rx, "show_setting"),
            ("phone".into(), None)
        );
        assert!(
            !out.text_for_model.contains("shodh-pair:"),
            "the pairing code never reaches the model"
        );

        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("agent-coverage.json");
        let manifest: Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let commands = &manifest["commands"];
        assert_eq!(commands["phone_pairing_start"]["tool"], "show_setting");
        assert_eq!(commands["phone_devices"]["tool"], "show_setting");
        assert!(commands["phone_device_remove"]["tool"].is_null());
        assert!(commands["phone_device_remove"]["excluded"]
            .as_str()
            .is_some_and(|why| why.contains("only the user")));

        let registry = super::super::build_registry(t.host.clone()).unwrap();
        let phone_tools: Vec<&str> = registry
            .names()
            .into_iter()
            .filter(|name| name.contains("phone") || name.contains("device"))
            .collect();
        assert!(
            phone_tools.is_empty(),
            "no tool reaches the phones: {phone_tools:?}"
        );
    }
}
