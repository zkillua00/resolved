use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use chrono::Utc;
use serde::{Deserialize, Serialize};

static NEXT_THEME_ID: AtomicU64 = AtomicU64::new(0);

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
    pub metrics_position: MetricsPosition,
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

/// Persisted corner used by the in-app performance HUD.
#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MetricsPosition {
    TopLeft,
    TopRight,
    BottomLeft,
    #[default]
    BottomRight,
}

impl MetricsPosition {
    pub const ALL: [Self; 4] = [
        Self::TopLeft,
        Self::TopRight,
        Self::BottomLeft,
        Self::BottomRight,
    ];

    pub const fn key(self) -> &'static str {
        match self {
            Self::TopLeft => "top_left",
            Self::TopRight => "top_right",
            Self::BottomLeft => "bottom_left",
            Self::BottomRight => "bottom_right",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::TopLeft => "Top Left",
            Self::TopRight => "Top Right",
            Self::BottomLeft => "Bottom Left",
            Self::BottomRight => "Bottom Right",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|position| position.key() == key)
    }
}

impl<'de> Deserialize<'de> for MetricsPosition {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let key = String::deserialize(deserializer)?;
        Ok(Self::from_key(&key).unwrap_or_default())
    }
}

/// One validated CSS theme retained in the user's theme catalog.
///
/// The identifier is stable across renames and edits. `css_source` is the
/// durable snapshot, while `source_path` is optional provenance for workflows
/// that also keep a CSS file on disk.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct SavedTheme {
    pub id: String,
    pub name: String,
    pub css_source: String,
    pub source_path: Option<PathBuf>,
    /// Unapplied in-app work for this saved theme.
    pub draft_source: Option<String>,
    /// File handed to an external editor for this saved theme.
    pub draft_path: Option<PathBuf>,
    /// Last contents known to match `draft_path`.
    pub draft_disk_source: Option<String>,
    /// Preserve metadata written by a newer application version.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl SavedTheme {
    pub fn new(
        name: impl Into<String>,
        css_source: impl Into<String>,
        source_path: Option<PathBuf>,
    ) -> Self {
        Self {
            id: new_theme_id(),
            name: name.into(),
            css_source: css_source.into(),
            source_path,
            draft_source: None,
            draft_path: None,
            draft_disk_source: None,
            extra: BTreeMap::new(),
        }
    }
}

/// CSS theme source retained by the application.
///
/// `saved_themes` owns the catalog. `active_theme_id` selects one of those
/// entries. When it is `None`, an empty `css_source` selects the built-in theme;
/// a populated source is a backward-compatible, unsaved custom snapshot. The
/// existing `css_source` and `source_path` fields remain the durable
/// active-theme projection used at startup.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct ThemeSettings {
    pub saved_themes: Vec<SavedTheme>,
    pub active_theme_id: Option<String>,
    pub source_path: Option<PathBuf>,
    pub css_source: Option<String>,
    /// Unapplied in-app work, kept separate from the last valid theme so a
    /// restart can recover the editor without changing the application UI.
    pub draft_source: Option<String>,
    /// File handed to an external editor before it becomes the active source.
    /// Keeping it here makes the preferred-editor workflow round-trip across
    /// restarts.
    pub draft_path: Option<PathBuf>,
    /// Last file contents known to match `draft_path` (or the active source
    /// while an in-app draft is open). This prevents both false conflicts
    /// after restart and accidental overwrites of later external edits.
    pub draft_disk_source: Option<String>,
    /// Preserve future theme metadata across read-modify-write cycles.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl ThemeSettings {
    pub fn saved_theme(&self, id: &str) -> Option<&SavedTheme> {
        self.saved_themes.iter().find(|theme| theme.id == id)
    }
}

fn new_theme_id() -> String {
    let created_at = Utc::now();
    let sequence = NEXT_THEME_ID.fetch_add(1, Ordering::Relaxed);
    format!(
        "theme-{}-{}-{sequence}",
        created_at.timestamp_micros(),
        std::process::id()
    )
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
        assert_eq!(settings.metrics_position, MetricsPosition::BottomRight);
    }

    #[test]
    fn settings_round_trip_preserves_stable_override_representations() {
        let saved_theme = SavedTheme::new(
            "Material Dark",
            ":root { --api-background: #14121a; }",
            Some(PathBuf::from("/tmp/api-tester-theme.css")),
        );
        let settings = AppSettings {
            shortcuts: BTreeMap::from([
                (
                    "request.send".to_owned(),
                    ShortcutOverride::Custom("cmd-enter".to_owned()),
                ),
                ("request.close_tab".to_owned(), ShortcutOverride::Disabled),
            ]),
            theme: ThemeSettings {
                saved_themes: vec![saved_theme.clone()],
                active_theme_id: Some(saved_theme.id),
                source_path: Some(PathBuf::from("/tmp/api-tester-theme.css")),
                css_source: Some(":root { --api-background: #14121a; }".to_owned()),
                draft_source: Some(":root { --api-background: #20202a; }".to_owned()),
                draft_path: Some(PathBuf::from("/tmp/api-tester-theme-draft.css")),
                draft_disk_source: Some(":root { --api-background: #14121a; }".to_owned()),
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
        assert_eq!(settings.metrics_position, MetricsPosition::BottomRight);

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

    #[test]
    fn pre_catalog_theme_settings_remain_readable() {
        let settings: ThemeSettings = serde_json::from_str(
            r#"{
                "source_path": "/tmp/legacy-theme.css",
                "css_source": ":root { --api-theme-name: \"Legacy\"; }",
                "draft_source": ":root { --api-theme-name: \"Draft\"; }"
            }"#,
        )
        .unwrap();

        assert!(settings.saved_themes.is_empty());
        assert_eq!(settings.active_theme_id, None);
        assert_eq!(
            settings.source_path,
            Some(PathBuf::from("/tmp/legacy-theme.css"))
        );
        assert_eq!(
            settings.css_source.as_deref(),
            Some(":root { --api-theme-name: \"Legacy\"; }")
        );
        assert_eq!(
            settings.draft_source.as_deref(),
            Some(":root { --api-theme-name: \"Draft\"; }")
        );
    }

    #[test]
    fn metrics_positions_round_trip_with_a_backward_compatible_default() {
        for position in MetricsPosition::ALL {
            let settings = AppSettings {
                metrics_position: position,
                ..Default::default()
            };
            let encoded = serde_json::to_value(&settings).unwrap();
            assert_eq!(
                encoded.get("metrics_position"),
                Some(&serde_json::json!(position.key()))
            );
            assert_eq!(
                serde_json::from_value::<AppSettings>(encoded)
                    .unwrap()
                    .metrics_position,
                position
            );
            assert_eq!(MetricsPosition::from_key(position.key()), Some(position));
        }
        assert_eq!(MetricsPosition::from_key("center"), None);

        let unknown: AppSettings =
            serde_json::from_str(r#"{"metrics_position":"future_corner"}"#).unwrap();
        assert_eq!(unknown.metrics_position, MetricsPosition::BottomRight);
    }

    #[test]
    fn saved_theme_ids_are_unique_and_lookup_is_stable() {
        let first = SavedTheme::new("First", "first source", None);
        let second = SavedTheme::new(
            "Second",
            "second source",
            Some(PathBuf::from("/tmp/second.css")),
        );

        assert!(first.id.starts_with("theme-"));
        assert_ne!(first.id, second.id);

        let settings = ThemeSettings {
            saved_themes: vec![first.clone(), second.clone()],
            active_theme_id: Some(second.id.clone()),
            ..Default::default()
        };
        assert_eq!(settings.saved_theme(&first.id), Some(&first));
        assert_eq!(settings.saved_theme(&second.id), Some(&second));
        assert_eq!(settings.saved_theme("missing"), None);
    }

    #[test]
    fn saved_theme_round_trip_preserves_future_metadata() {
        let theme: SavedTheme = serde_json::from_str(
            r#"{
                "id": "theme-existing",
                "name": "Ocean",
                "css_source": ":root {}",
                "source_path": "/tmp/ocean.css",
                "future_theme_property": { "revision": 3 }
            }"#,
        )
        .unwrap();

        assert_eq!(
            theme.extra.get("future_theme_property"),
            Some(&serde_json::json!({ "revision": 3 }))
        );
        let encoded = serde_json::to_value(theme).unwrap();
        assert_eq!(
            encoded.get("future_theme_property"),
            Some(&serde_json::json!({ "revision": 3 }))
        );
    }

    #[test]
    fn saved_theme_round_trip_keeps_its_own_editor_draft() {
        let mut theme = SavedTheme::new(
            "Ocean",
            ":root { --api-theme-name: \"Ocean\"; }",
            Some(PathBuf::from("/tmp/ocean.css")),
        );
        theme.draft_source = Some(":root { --api-theme-name: \"Ocean Draft\"; }".to_owned());
        theme.draft_path = Some(PathBuf::from("/tmp/ocean-draft.css"));
        theme.draft_disk_source = theme.draft_source.clone();

        let encoded = serde_json::to_string(&theme).unwrap();
        let decoded: SavedTheme = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, theme);
    }
}
