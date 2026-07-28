use std::{collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Serialize};

/// Persisted application preferences that are independent from request data.
///
/// Every field uses a serde default so settings written by an older build
/// remain readable when new preferences are introduced.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct AppSettings {
    pub shortcuts: BTreeMap<String, ShortcutOverride>,
    pub theme: ThemeSettings,
    pub navigation_compact: bool,
    /// Preserve fields written by a newer application version when an older
    /// build changes a setting it understands.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// A user override for one stable shortcut command identifier.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ShortcutOverride {
    Custom(String),
    Disabled,
}

/// CSS theme source retained by the application.
///
/// `css_source` is the durable snapshot used at startup. `source_path` is
/// optional provenance for reload/reveal workflows and is not required for the
/// selected theme to remain usable.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct ThemeSettings {
    pub source_path: Option<PathBuf>,
    pub css_source: Option<String>,
    /// Preserve future theme metadata across read-modify-write cycles.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_empty_and_backward_compatible() {
        let settings: AppSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings, AppSettings::default());
        assert!(settings.shortcuts.is_empty());
        assert_eq!(settings.theme, ThemeSettings::default());
        assert!(!settings.navigation_compact);
    }

    #[test]
    fn settings_round_trip_preserves_stable_override_representations() {
        let settings = AppSettings {
            shortcuts: BTreeMap::from([
                (
                    "request.send".to_owned(),
                    ShortcutOverride::Custom("cmd-enter".to_owned()),
                ),
                ("request.close_tab".to_owned(), ShortcutOverride::Disabled),
            ]),
            theme: ThemeSettings {
                source_path: Some(PathBuf::from("/tmp/api-tester-theme.css")),
                css_source: Some(":root { --api-background: #14121a; }".to_owned()),
                ..Default::default()
            },
            navigation_compact: true,
            ..Default::default()
        };

        let encoded = serde_json::to_string(&settings).unwrap();
        assert!(encoded.contains(r#""request.send":{"custom":"cmd-enter"}"#));
        assert!(encoded.contains(r#""request.close_tab":"disabled""#));
        assert_eq!(
            serde_json::from_str::<AppSettings>(&encoded).unwrap(),
            settings
        );
    }

    #[test]
    fn missing_new_fields_use_defaults_without_losing_existing_values() {
        let settings: AppSettings = serde_json::from_str(
            r#"{
                "shortcuts": {
                    "workspace.settings": { "custom": "cmd-," }
                },
                "future_top_level": { "enabled": true },
                "theme": {
                    "future_theme_field": ["ocean", 2]
                }
            }"#,
        )
        .unwrap();

        assert_eq!(
            settings.shortcuts.get("workspace.settings"),
            Some(&ShortcutOverride::Custom("cmd-,".to_owned()))
        );
        assert_eq!(
            settings.extra.get("future_top_level"),
            Some(&serde_json::json!({ "enabled": true }))
        );
        assert_eq!(
            settings.theme.extra.get("future_theme_field"),
            Some(&serde_json::json!(["ocean", 2]))
        );
        assert!(!settings.navigation_compact);

        let encoded = serde_json::to_string(&settings).unwrap();
        let decoded: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(
            decoded.get("future_top_level"),
            Some(&serde_json::json!({ "enabled": true }))
        );
        assert_eq!(
            decoded
                .get("theme")
                .and_then(|theme| theme.get("future_theme_field")),
            Some(&serde_json::json!(["ocean", 2]))
        );
    }
}
