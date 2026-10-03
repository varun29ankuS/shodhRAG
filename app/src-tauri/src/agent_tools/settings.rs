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
use shodh_rag::harness::profile::app_tools;
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
    Ok(())
}

fn store(host: &AgentHost) -> SettingsStore {
    SettingsStore::in_dir(&host.data_dir)
}

fn tool_error(tool: &str, error: SettingsError) -> ToolError {
    match error {
        SettingsError::InvalidValue { key, reason } => invalid(tool, format!("{key} {reason}")),
        SettingsError::UnknownKey(key) => invalid(tool, format!("unknown setting {key}")),
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
        Ok(ToolOutput {
            text_for_model: format!("{} is now {new} (was {old}).", key.as_str()),
            summary_for_ui: format!("Set {} to {new}", key.as_str()),
            detail: Some(json!({ "key": key.as_str(), "before": old, "after": new })),
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
}
