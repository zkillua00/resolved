use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use chrono::Utc;
use serde::{Deserialize, Serialize};

use super::UpstreamSettings;

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
    pub editor: EditorSettings,
    pub formatter: FormatterSettings,
    pub zoom: ZoomSettings,
    pub script: ScriptSettings,
    pub mcp: McpSettings,
    pub navigation_compact: bool,
    pub metrics_position: MetricsPosition,
    pub upstreams: UpstreamSettings,
    /// Preserve fields written by a newer application version when an older
    /// build changes a setting it understands.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// Local Model Context Protocol exposure. The transport is absent unless
/// `enabled` is true, and every call is checked against `enabled_tools` inside
/// the desktop process as well as filtered from MCP discovery.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct McpSettings {
    pub enabled: bool,
    /// Allow workspace-scoped MCP tools to inspect or change connected server
    /// workspaces. Server RBAC remains authoritative when this is enabled.
    pub allow_remote_workspaces: bool,
    #[serde(default = "default_mcp_tools")]
    pub enabled_tools: std::collections::BTreeSet<String>,
    /// Preserve fields written by a newer application version.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl McpSettings {
    pub fn tool_enabled(&self, name: &str) -> bool {
        self.enabled && self.enabled_tools.contains(name)
    }
}

impl Default for McpSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            allow_remote_workspaces: false,
            enabled_tools: default_mcp_tools(),
            extra: BTreeMap::new(),
        }
    }
}

fn default_mcp_tools() -> std::collections::BTreeSet<String> {
    crate::control_tools::CONTROL_TOOLS
        .iter()
        .map(|tool| tool.name.to_owned())
        .collect()
}

const DEFAULT_EDITOR_TAB_SIZE: u8 = 2;
const DEFAULT_FORMATTER_INDENT_SIZE: u8 = 2;
const DEFAULT_FORMATTER_LINE_WIDTH: u16 = 80;
const MIN_INDENT_SIZE: u64 = 1;
const MAX_INDENT_SIZE: u64 = 16;
const MIN_FORMATTER_LINE_WIDTH: u64 = 40;
const MAX_FORMATTER_LINE_WIDTH: u64 = 240;
const DEFAULT_SCRIPT_TIMEOUT_MS: u64 = 30_000;
/// Neutral zoom: baseline theme typography, no up- or down-scaling.
pub const DEFAULT_ZOOM_PERCENT: u16 = 100;
/// Minimum zoom as a percentage of the baseline theme font size.
pub const MIN_ZOOM_PERCENT: u16 = 50;
/// Maximum zoom as a percentage of the baseline theme font size.
pub const MAX_ZOOM_PERCENT: u16 = 200;
/// Amount each zoom-in/zoom-out step changes the percentage.
pub const ZOOM_STEP_PERCENT: u16 = 10;

fn default_script_timeout_ms() -> u64 {
    DEFAULT_SCRIPT_TIMEOUT_MS
}

/// Preferences for running request pre/post-response scripts, including any
/// awaited `api.requests.execute(...)` chains.
///
/// The timeout is a per-phase execution budget (with a generous default), not a
/// hard-coded cap: a phase that legitimately takes longer than a second — e.g.
/// a login chain that refreshes a token over the network — is no longer killed
/// at 1s.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct ScriptSettings {
    /// Maximum time a script phase (and anything it `await`s, including the
    /// full awaited request pipeline and its own pre/post scripts) may run, in
    /// milliseconds.
    #[serde(default = "default_script_timeout_ms")]
    pub timeout_ms: u64,
    /// Preserve fields written by a newer application version.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl ScriptSettings {
    /// The effective script deadline; never zero, so a mistyped setting
    /// cannot cause scripts to time out instantly.
    pub fn timeout(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.timeout_ms.max(1))
    }
}

impl Default for ScriptSettings {
    fn default() -> Self {
        Self {
            timeout_ms: DEFAULT_SCRIPT_TIMEOUT_MS,
            extra: BTreeMap::new(),
        }
    }
}

/// Preferences shared by every native code-editor surface.
///
/// The defaults intentionally match the editor behavior that predates persisted
/// editor settings. Unknown fields are retained so an older build can change a
/// known preference without erasing settings written by a newer build.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct EditorSettings {
    #[serde(
        default = "default_editor_tab_size",
        deserialize_with = "deserialize_editor_tab_size"
    )]
    pub tab_size: u8,
    #[serde(
        default = "default_false",
        deserialize_with = "deserialize_bool_default_false"
    )]
    pub hard_tabs: bool,
    #[serde(
        default = "default_false",
        deserialize_with = "deserialize_bool_default_false"
    )]
    pub soft_wrap: bool,
    #[serde(
        default = "default_true",
        deserialize_with = "deserialize_bool_default_true"
    )]
    pub line_numbers: bool,
    #[serde(
        default = "default_true",
        deserialize_with = "deserialize_bool_default_true"
    )]
    pub indent_guides: bool,
    #[serde(
        default = "default_true",
        deserialize_with = "deserialize_bool_default_true"
    )]
    pub auto_close_pairs: bool,
    pub inline_action_placement: EditorInlineActionPlacement,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            tab_size: DEFAULT_EDITOR_TAB_SIZE,
            hard_tabs: false,
            soft_wrap: false,
            line_numbers: true,
            indent_guides: true,
            auto_close_pairs: true,
            inline_action_placement: EditorInlineActionPlacement::Above,
            extra: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EditorInlineActionPlacement {
    #[default]
    Above,
    After,
}

impl EditorInlineActionPlacement {
    pub const ALL: [Self; 2] = [Self::Above, Self::After];

    pub fn label(self) -> &'static str {
        match self {
            Self::Above => "Above record",
            Self::After => "After record",
        }
    }
}

impl EditorSettings {
    pub fn effective_tab_size(&self) -> usize {
        usize::from(
            self.tab_size
                .clamp(MIN_INDENT_SIZE as u8, MAX_INDENT_SIZE as u8),
        )
    }

    pub fn set_tab_size(&mut self, tab_size: u8) {
        self.tab_size = tab_size.clamp(MIN_INDENT_SIZE as u8, MAX_INDENT_SIZE as u8);
    }
}

/// Independent typography zoom for the application interface and for code
/// editors, stored as percentages of the active theme's baseline font sizes.
///
/// Kept separate from the CSS theme so changing zoom never rewrites a user's
/// saved theme source, and kept as integers so persisted settings round-trip
/// exactly and stay comparable.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct ZoomSettings {
    /// Interface zoom as a percentage of the theme's `.app` font size.
    #[serde(
        default = "default_zoom_percent",
        deserialize_with = "deserialize_zoom_ui"
    )]
    pub ui: u16,
    /// Code editor zoom as a percentage of the theme's `.editor` font size.
    #[serde(
        default = "default_zoom_percent",
        deserialize_with = "deserialize_zoom_editor"
    )]
    pub editor: u16,
    /// Preserve fields written by a newer application version.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl Default for ZoomSettings {
    fn default() -> Self {
        Self {
            ui: DEFAULT_ZOOM_PERCENT,
            editor: DEFAULT_ZOOM_PERCENT,
            extra: BTreeMap::new(),
        }
    }
}

impl ZoomSettings {
    /// Clamp a raw percentage into the supported zoom range.
    pub fn clamped(percent: u16) -> u16 {
        percent.clamp(MIN_ZOOM_PERCENT, MAX_ZOOM_PERCENT)
    }

    /// The interface zoom as a multiplier applied to the theme's UI font.
    pub fn effective_ui(&self) -> f32 {
        Self::clamped(self.ui) as f32 / 100.0
    }

    /// The editor zoom as a multiplier applied to the theme's editor font.
    pub fn effective_editor(&self) -> f32 {
        Self::clamped(self.editor) as f32 / 100.0
    }

    /// Adjust the interface zoom by `ZOOM_STEP_PERCENT` steps (`+1` in, `-1`
    /// out, `0` resets to the baseline), clamping to the supported range.
    pub fn step_ui(&mut self, steps: i16) {
        self.ui = Self::clamped(
            (i32::from(self.ui) + i32::from(steps) * i32::from(ZOOM_STEP_PERCENT))
                .clamp(i32::from(MIN_ZOOM_PERCENT), i32::from(MAX_ZOOM_PERCENT)) as u16,
        );
    }

    /// Adjust the editor zoom by `ZOOM_STEP_PERCENT` steps (see [`step_ui`]).
    pub fn step_editor(&mut self, steps: i16) {
        self.editor = Self::clamped(
            (i32::from(self.editor) + i32::from(steps) * i32::from(ZOOM_STEP_PERCENT))
                .clamp(i32::from(MIN_ZOOM_PERCENT), i32::from(MAX_ZOOM_PERCENT)) as u16,
        );
    }
}

const fn default_zoom_percent() -> u16 {
    DEFAULT_ZOOM_PERCENT
}

fn deserialize_zoom_ui<'de, D>(deserializer: D) -> Result<u16, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_zoom(deserializer)
}

fn deserialize_zoom_editor<'de, D>(deserializer: D) -> Result<u16, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_zoom(deserializer)
}

fn deserialize_zoom<'de, D>(deserializer: D) -> Result<u16, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(bounded_integer(
        &value,
        u64::from(DEFAULT_ZOOM_PERCENT),
        u64::from(MIN_ZOOM_PERCENT),
        u64::from(MAX_ZOOM_PERCENT),
    ) as u16)
}

/// Options consumed by the built-in JSON and JavaScript formatters.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct FormatterSettings {
    #[serde(
        default = "default_formatter_indent_size",
        deserialize_with = "deserialize_formatter_indent_size"
    )]
    pub indent_size: u8,
    #[serde(
        default = "default_false",
        deserialize_with = "deserialize_bool_default_false"
    )]
    pub hard_tabs: bool,
    #[serde(
        default = "default_formatter_line_width",
        deserialize_with = "deserialize_formatter_line_width"
    )]
    pub line_width: u16,
    pub quote_style: FormatterQuoteStyle,
    pub semicolons: FormatterSemicolons,
    pub trailing_commas: FormatterTrailingCommas,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl Default for FormatterSettings {
    fn default() -> Self {
        Self {
            indent_size: DEFAULT_FORMATTER_INDENT_SIZE,
            hard_tabs: false,
            line_width: DEFAULT_FORMATTER_LINE_WIDTH,
            quote_style: FormatterQuoteStyle::Double,
            semicolons: FormatterSemicolons::Prefer,
            trailing_commas: FormatterTrailingCommas::Never,
            extra: BTreeMap::new(),
        }
    }
}

impl FormatterSettings {
    pub fn effective_indent_size(&self) -> usize {
        usize::from(
            self.indent_size
                .clamp(MIN_INDENT_SIZE as u8, MAX_INDENT_SIZE as u8),
        )
    }

    pub fn effective_line_width(&self) -> usize {
        usize::from(self.line_width.clamp(
            MIN_FORMATTER_LINE_WIDTH as u16,
            MAX_FORMATTER_LINE_WIDTH as u16,
        ))
    }

    pub fn set_indent_size(&mut self, indent_size: u8) {
        self.indent_size = indent_size.clamp(MIN_INDENT_SIZE as u8, MAX_INDENT_SIZE as u8);
    }

    pub fn set_line_width(&mut self, line_width: u16) {
        self.line_width = line_width.clamp(
            MIN_FORMATTER_LINE_WIDTH as u16,
            MAX_FORMATTER_LINE_WIDTH as u16,
        );
    }

    pub fn set_quote_style_key(&mut self, key: &str) -> bool {
        let Some(value) = FormatterQuoteStyle::from_key(key) else {
            return false;
        };
        self.quote_style = value;
        true
    }

    pub fn set_semicolons_key(&mut self, key: &str) -> bool {
        let Some(value) = FormatterSemicolons::from_key(key) else {
            return false;
        };
        self.semicolons = value;
        true
    }

    pub fn set_trailing_commas_key(&mut self, key: &str) -> bool {
        let Some(value) = FormatterTrailingCommas::from_key(key) else {
            return false;
        };
        self.trailing_commas = value;
        true
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FormatterQuoteStyle {
    #[default]
    Double,
    Single,
}

impl FormatterQuoteStyle {
    pub const ALL: [Self; 2] = [Self::Double, Self::Single];

    pub const fn key(self) -> &'static str {
        match self {
            Self::Double => "double",
            Self::Single => "single",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Double => "Double quotes",
            Self::Single => "Single quotes",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|value| value.key() == key)
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FormatterSemicolons {
    #[default]
    Prefer,
    Asi,
}

impl FormatterSemicolons {
    pub const ALL: [Self; 2] = [Self::Prefer, Self::Asi];

    pub const fn key(self) -> &'static str {
        match self {
            Self::Prefer => "prefer",
            Self::Asi => "asi",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Prefer => "Prefer semicolons",
            Self::Asi => "Automatic insertion",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|value| value.key() == key)
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FormatterTrailingCommas {
    #[default]
    Never,
    MultiLine,
    Always,
}

impl FormatterTrailingCommas {
    pub const ALL: [Self; 3] = [Self::Never, Self::MultiLine, Self::Always];

    pub const fn key(self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::MultiLine => "multi_line",
            Self::Always => "always",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Never => "Never",
            Self::MultiLine => "Multi-line only",
            Self::Always => "Always",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|value| value.key() == key)
    }
}

impl<'de> Deserialize<'de> for FormatterQuoteStyle {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        Ok(value.as_str().and_then(Self::from_key).unwrap_or_default())
    }
}

impl<'de> Deserialize<'de> for FormatterSemicolons {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        Ok(value.as_str().and_then(Self::from_key).unwrap_or_default())
    }
}

impl<'de> Deserialize<'de> for FormatterTrailingCommas {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        Ok(value.as_str().and_then(Self::from_key).unwrap_or_default())
    }
}

const fn default_editor_tab_size() -> u8 {
    DEFAULT_EDITOR_TAB_SIZE
}

const fn default_formatter_indent_size() -> u8 {
    DEFAULT_FORMATTER_INDENT_SIZE
}

const fn default_formatter_line_width() -> u16 {
    DEFAULT_FORMATTER_LINE_WIDTH
}

const fn default_true() -> bool {
    true
}

const fn default_false() -> bool {
    false
}

fn deserialize_editor_tab_size<'de, D>(deserializer: D) -> Result<u8, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(bounded_integer(
        &value,
        u64::from(DEFAULT_EDITOR_TAB_SIZE),
        MIN_INDENT_SIZE,
        MAX_INDENT_SIZE,
    ) as u8)
}

fn deserialize_formatter_indent_size<'de, D>(deserializer: D) -> Result<u8, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(bounded_integer(
        &value,
        u64::from(DEFAULT_FORMATTER_INDENT_SIZE),
        MIN_INDENT_SIZE,
        MAX_INDENT_SIZE,
    ) as u8)
}

fn deserialize_formatter_line_width<'de, D>(deserializer: D) -> Result<u16, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(bounded_integer(
        &value,
        u64::from(DEFAULT_FORMATTER_LINE_WIDTH),
        MIN_FORMATTER_LINE_WIDTH,
        MAX_FORMATTER_LINE_WIDTH,
    ) as u16)
}

fn bounded_integer(value: &serde_json::Value, default: u64, minimum: u64, maximum: u64) -> u64 {
    let parsed = match value {
        serde_json::Value::Number(number) => number
            .as_u64()
            .map(|value| value as f64)
            .or_else(|| number.as_i64().map(|value| value as f64))
            .or_else(|| number.as_f64()),
        serde_json::Value::String(value) => value.parse::<f64>().ok(),
        _ => None,
    };
    let Some(parsed) = parsed.filter(|value| value.is_finite()) else {
        return default;
    };
    parsed.round().clamp(minimum as f64, maximum as f64) as u64
}

fn deserialize_bool_default_true<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_bool(deserializer, true)
}

fn deserialize_bool_default_false<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_bool(deserializer, false)
}

fn deserialize_bool<'de, D>(deserializer: D, default: bool) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(match value {
        serde_json::Value::Bool(value) => value,
        serde_json::Value::Number(number) if number.as_i64() == Some(0) => false,
        serde_json::Value::Number(number) if number.as_i64() == Some(1) => true,
        serde_json::Value::String(value) if value.eq_ignore_ascii_case("true") => true,
        serde_json::Value::String(value) if value.eq_ignore_ascii_case("false") => false,
        _ => default,
    })
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
        // Lenient like the sibling formatter enums: an unknown, null, or
        // non-string persisted value falls back to the default instead of
        // bricking the whole settings row on load.
        let value = serde_json::Value::deserialize(deserializer)?;
        Ok(value.as_str().and_then(Self::from_key).unwrap_or_default())
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
        assert_eq!(settings.editor, EditorSettings::default());
        assert_eq!(settings.formatter, FormatterSettings::default());
        assert_eq!(settings.mcp, McpSettings::default());
        assert!(!settings.mcp.enabled);
        assert!(!settings.mcp.allow_remote_workspaces);
        assert_eq!(
            settings.mcp.enabled_tools.len(),
            crate::control_tools::CONTROL_TOOLS.len()
        );
        assert_eq!(settings.upstreams, UpstreamSettings::default());
        assert!(!settings.navigation_compact);
        assert_eq!(settings.metrics_position, MetricsPosition::BottomRight);
    }

    #[test]
    fn mcp_settings_round_trip_and_preserve_unknown_tools() {
        let settings: AppSettings = serde_json::from_value(serde_json::json!({
            "mcp": {
                "enabled": true,
                "allow_remote_workspaces": true,
                "enabled_tools": ["status", "future_tool"],
                "future_policy": "prompt"
            }
        }))
        .unwrap();

        assert!(settings.mcp.enabled);
        assert!(settings.mcp.allow_remote_workspaces);
        assert!(settings.mcp.tool_enabled("status"));
        assert!(!settings.mcp.tool_enabled("get_request"));
        assert!(settings.mcp.enabled_tools.contains("future_tool"));
        assert_eq!(
            settings.mcp.extra.get("future_policy"),
            Some(&serde_json::json!("prompt"))
        );
        let encoded = serde_json::to_value(&settings).unwrap();
        assert_eq!(
            serde_json::from_value::<AppSettings>(encoded).unwrap(),
            settings
        );
    }

    #[test]
    fn editor_and_formatter_settings_round_trip_with_future_metadata() {
        let settings: AppSettings = serde_json::from_str(
            r#"{
                "editor": {
                    "tab_size": 4,
                    "hard_tabs": true,
                    "soft_wrap": true,
                    "line_numbers": false,
                    "indent_guides": false,
                    "auto_close_pairs": false,
                    "future_editor_option": { "mode": "smart" }
                },
                "formatter": {
                    "indent_size": 4,
                    "hard_tabs": true,
                    "line_width": 120,
                    "quote_style": "single",
                    "semicolons": "asi",
                    "trailing_commas": "multi_line",
                    "future_formatter_option": ["imports"]
                }
            }"#,
        )
        .unwrap();

        assert_eq!(settings.editor.tab_size, 4);
        assert!(settings.editor.hard_tabs);
        assert!(settings.editor.soft_wrap);
        assert!(!settings.editor.line_numbers);
        assert!(!settings.editor.indent_guides);
        assert!(!settings.editor.auto_close_pairs);
        assert_eq!(
            settings.editor.extra.get("future_editor_option"),
            Some(&serde_json::json!({ "mode": "smart" }))
        );
        assert_eq!(settings.formatter.indent_size, 4);
        assert!(settings.formatter.hard_tabs);
        assert_eq!(settings.formatter.line_width, 120);
        assert_eq!(settings.formatter.quote_style, FormatterQuoteStyle::Single);
        assert_eq!(settings.formatter.semicolons, FormatterSemicolons::Asi);
        assert_eq!(
            settings.formatter.trailing_commas,
            FormatterTrailingCommas::MultiLine
        );
        assert_eq!(
            settings.formatter.extra.get("future_formatter_option"),
            Some(&serde_json::json!(["imports"]))
        );

        let encoded = serde_json::to_value(&settings).unwrap();
        assert_eq!(
            serde_json::from_value::<AppSettings>(encoded).unwrap(),
            settings
        );
    }

    #[test]
    fn malformed_editor_and_formatter_values_fall_back_or_clamp() {
        let settings: AppSettings = serde_json::from_str(
            r#"{
                "editor": {
                    "tab_size": 0,
                    "hard_tabs": "invalid",
                    "soft_wrap": 1,
                    "line_numbers": null,
                    "indent_guides": "false",
                    "auto_close_pairs": { "invalid": true }
                },
                "formatter": {
                    "indent_size": 999,
                    "hard_tabs": 1,
                    "line_width": 20,
                    "quote_style": "future_style",
                    "semicolons": 42,
                    "trailing_commas": null
                }
            }"#,
        )
        .unwrap();

        assert_eq!(settings.editor.tab_size, 1);
        assert!(!settings.editor.hard_tabs);
        assert!(settings.editor.soft_wrap);
        assert!(settings.editor.line_numbers);
        assert!(!settings.editor.indent_guides);
        assert!(settings.editor.auto_close_pairs);
        assert_eq!(settings.formatter.indent_size, 16);
        assert!(settings.formatter.hard_tabs);
        assert_eq!(settings.formatter.line_width, 40);
        assert_eq!(settings.formatter.quote_style, FormatterQuoteStyle::Double);
        assert_eq!(settings.formatter.semicolons, FormatterSemicolons::Prefer);
        assert_eq!(
            settings.formatter.trailing_commas,
            FormatterTrailingCommas::Never
        );
    }

    #[test]
    fn formatter_option_keys_are_stable() {
        for option in FormatterQuoteStyle::ALL {
            assert_eq!(FormatterQuoteStyle::from_key(option.key()), Some(option));
            assert!(!option.label().is_empty());
        }
        for option in FormatterSemicolons::ALL {
            assert_eq!(FormatterSemicolons::from_key(option.key()), Some(option));
            assert!(!option.label().is_empty());
        }
        for option in FormatterTrailingCommas::ALL {
            assert_eq!(
                FormatterTrailingCommas::from_key(option.key()),
                Some(option)
            );
            assert!(!option.label().is_empty());
        }
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
    fn inline_action_placement_round_trips_with_above_as_the_legacy_default() {
        let legacy: EditorSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(
            legacy.inline_action_placement,
            EditorInlineActionPlacement::Above
        );

        for placement in EditorInlineActionPlacement::ALL {
            let settings = EditorSettings {
                inline_action_placement: placement,
                ..Default::default()
            };
            let encoded = serde_json::to_value(&settings).unwrap();
            assert_eq!(
                serde_json::from_value::<EditorSettings>(encoded)
                    .unwrap()
                    .inline_action_placement,
                placement
            );
        }
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

    #[test]
    fn zoom_settings_default_clamp_and_round_trip() {
        let default = ZoomSettings::default();
        assert_eq!(default.ui, DEFAULT_ZOOM_PERCENT);
        assert_eq!(default.editor, DEFAULT_ZOOM_PERCENT);
        assert_eq!(default.effective_ui(), 1.0);
        assert_eq!(default.effective_editor(), 1.0);

        let mut zoom = ZoomSettings::default();
        zoom.step_ui(1);
        assert_eq!(zoom.ui, 110);
        zoom.step_editor(-3);
        assert_eq!(zoom.editor, 70);
        // Steps clamp to the supported range.
        zoom.step_ui(-20);
        assert_eq!(zoom.ui, 50);
        zoom.step_ui(400);
        assert_eq!(zoom.ui, 200);
        assert_eq!(zoom.effective_ui(), 2.0);

        // Malformed stored values fall back or clamp instead of bricking load.
        let settings: AppSettings = serde_json::from_str(
            r#"{
                "zoom": {
                    "ui": 10,
                    "editor": "999",
                    "future_zoom_option": { "smooth": true }
                }
            }"#,
        )
        .unwrap();
        assert_eq!(settings.zoom.ui, 50);
        assert_eq!(settings.zoom.editor, 200);
        assert_eq!(
            settings.zoom.extra.get("future_zoom_option"),
            Some(&serde_json::json!({ "smooth": true }))
        );

        // Old settings without a `zoom` key load with defaults.
        let from_old: ZoomSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(from_old, default);
        let encoded = serde_json::to_string(&default).unwrap();
        let decoded: ZoomSettings = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, default);
    }

    #[test]
    fn script_settings_default_and_clamp() {
        let default = ScriptSettings::default();
        assert_eq!(default.timeout_ms, DEFAULT_SCRIPT_TIMEOUT_MS);
        assert_eq!(
            default.timeout(),
            std::time::Duration::from_millis(DEFAULT_SCRIPT_TIMEOUT_MS)
        );
        // A mistyped zero must not make scripts time out instantly.
        let zero = ScriptSettings {
            timeout_ms: 0,
            ..Default::default()
        };
        assert_eq!(zero.timeout(), std::time::Duration::from_millis(1));
        // Round-trips through JSON with serde defaults intact.
        let enc = serde_json::to_string(&default).unwrap();
        let dec: ScriptSettings = serde_json::from_str(&enc).unwrap();
        assert_eq!(dec, default);
        // An old settings file with no `script` key still loads with defaults.
        let from_old: ScriptSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(from_old, default);
    }
}
