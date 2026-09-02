//! Application-level keyboard shortcuts.
//!
//! GPUI Component owns the normal text-editing keymap. This module only
//! describes Resolved's actions and installs their effective bindings on top
//! of a caller-provided snapshot of that base keymap.

use std::{collections::BTreeMap, error::Error, fmt};

#[cfg(test)]
use std::collections::BTreeSet;

use gpui::{App, KeyBinding, Keystroke, actions};

use crate::core::{AppSettings, ShortcutOverride};

pub const APP_KEY_CONTEXT: &str = "ApiTester";

actions!(
    api_tester,
    [
        NewRequestTab,
        CloseRequestTab,
        ActivateNextRequestTab,
        ActivatePreviousRequestTab,
        SendOrCancelRequest,
        QuickSendWebSocketTemplate,
        SaveRequest,
        SaveRequestAs,
        FocusRequestUrl,
        FormatRawBody,
        ShowCollections,
        ShowEnvironments,
        ShowHistory,
        ShowSettings,
        ToggleNavigation,
        ToggleMetrics,
        ZoomUiIn,
        ZoomUiOut,
        ZoomUiReset,
        ZoomEditorIn,
        ZoomEditorOut,
        ZoomEditorReset,
        QuitApp,
    ]
);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ShortcutId {
    NewRequestTab,
    CloseRequestTab,
    ActivateNextRequestTab,
    ActivatePreviousRequestTab,
    SendOrCancelRequest,
    QuickSendWebSocketTemplate,
    SaveRequest,
    SaveRequestAs,
    FocusRequestUrl,
    FormatRawBody,
    ShowCollections,
    ShowEnvironments,
    ShowHistory,
    ShowSettings,
    ToggleNavigation,
    ToggleMetrics,
    ZoomUiIn,
    ZoomUiOut,
    ZoomUiReset,
    ZoomEditorIn,
    ZoomEditorOut,
    ZoomEditorReset,
    QuitApp,
}

impl ShortcutId {
    pub const fn key(self) -> &'static str {
        match self {
            Self::NewRequestTab => "request.new_tab",
            Self::CloseRequestTab => "request.close_tab",
            Self::ActivateNextRequestTab => "request.next_tab",
            Self::ActivatePreviousRequestTab => "request.previous_tab",
            Self::SendOrCancelRequest => "request.send_or_cancel",
            Self::QuickSendWebSocketTemplate => "websocket.quick_send_template",
            Self::SaveRequest => "request.save",
            Self::SaveRequestAs => "request.save_as",
            Self::FocusRequestUrl => "request.focus_url",
            Self::FormatRawBody => "request.format_raw_body",
            Self::ShowCollections => "navigation.collections",
            Self::ShowEnvironments => "navigation.environments",
            Self::ShowHistory => "navigation.history",
            Self::ShowSettings => "navigation.settings",
            Self::ToggleNavigation => "view.toggle_navigation",
            Self::ToggleMetrics => "view.toggle_metrics",
            Self::ZoomUiIn => "view.zoom_ui_in",
            Self::ZoomUiOut => "view.zoom_ui_out",
            Self::ZoomUiReset => "view.zoom_ui_reset",
            Self::ZoomEditorIn => "editor.zoom_in",
            Self::ZoomEditorOut => "editor.zoom_out",
            Self::ZoomEditorReset => "editor.zoom_reset",
            Self::QuitApp => "app.quit",
        }
    }

    #[cfg(test)]
    pub fn from_key(key: &str) -> Option<Self> {
        SHORTCUT_DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.id.key() == key)
            .map(|descriptor| descriptor.id)
    }
}

impl fmt::Display for ShortcutId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.key())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ShortcutCategory {
    RequestTabs,
    ActiveRequest,
    Navigation,
    Interface,
    Application,
}

impl ShortcutCategory {
    pub const ALL: [Self; 5] = [
        Self::RequestTabs,
        Self::ActiveRequest,
        Self::Navigation,
        Self::Interface,
        Self::Application,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::RequestTabs => "Tabs",
            Self::ActiveRequest => "Active request",
            Self::Navigation => "Navigation",
            Self::Interface => "Interface",
            Self::Application => "Application",
        }
    }

    pub const fn description(self) -> &'static str {
        match self {
            Self::RequestTabs => "Create request tabs, then close and move between open tabs.",
            Self::ActiveRequest => "Send, save, focus, and format the active request.",
            Self::Navigation => "Open the primary Resolved workspaces.",
            Self::Interface => "Show or hide supporting interface surfaces.",
            Self::Application => "Application-wide commands.",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShortcutDescriptor {
    pub id: ShortcutId,
    pub label: &'static str,
    pub category: ShortcutCategory,
    /// GPUI's parseable, platform-specific default representation.
    pub default_binding: &'static str,
}

pub const SHORTCUT_DESCRIPTORS: &[ShortcutDescriptor] = &[
    ShortcutDescriptor {
        id: ShortcutId::NewRequestTab,
        label: "New request tab",
        category: ShortcutCategory::RequestTabs,
        default_binding: "cmd-t",
    },
    ShortcutDescriptor {
        id: ShortcutId::CloseRequestTab,
        label: "Close active tab",
        category: ShortcutCategory::RequestTabs,
        default_binding: "cmd-w",
    },
    ShortcutDescriptor {
        id: ShortcutId::ActivateNextRequestTab,
        label: "Next tab",
        category: ShortcutCategory::RequestTabs,
        default_binding: "ctrl-tab",
    },
    ShortcutDescriptor {
        id: ShortcutId::ActivatePreviousRequestTab,
        label: "Previous tab",
        category: ShortcutCategory::RequestTabs,
        default_binding: "ctrl-shift-tab",
    },
    ShortcutDescriptor {
        id: ShortcutId::SendOrCancelRequest,
        label: "Send or cancel request",
        category: ShortcutCategory::ActiveRequest,
        default_binding: "cmd-enter",
    },
    ShortcutDescriptor {
        id: ShortcutId::QuickSendWebSocketTemplate,
        label: "Quick send WebSocket template",
        category: ShortcutCategory::ActiveRequest,
        default_binding: "cmd-shift-enter",
    },
    ShortcutDescriptor {
        id: ShortcutId::FocusRequestUrl,
        label: "Focus request URL",
        category: ShortcutCategory::ActiveRequest,
        default_binding: "cmd-l",
    },
    ShortcutDescriptor {
        id: ShortcutId::FormatRawBody,
        label: "Format active request editor",
        category: ShortcutCategory::ActiveRequest,
        default_binding: "alt-shift-f",
    },
    ShortcutDescriptor {
        id: ShortcutId::ShowCollections,
        label: "Show collections",
        category: ShortcutCategory::Navigation,
        default_binding: "cmd-1",
    },
    ShortcutDescriptor {
        id: ShortcutId::ShowEnvironments,
        label: "Show environments",
        category: ShortcutCategory::Navigation,
        default_binding: "cmd-2",
    },
    ShortcutDescriptor {
        id: ShortcutId::ShowHistory,
        label: "Show history",
        category: ShortcutCategory::Navigation,
        default_binding: "cmd-3",
    },
    ShortcutDescriptor {
        id: ShortcutId::ShowSettings,
        label: "Show settings",
        category: ShortcutCategory::Navigation,
        default_binding: "cmd-,",
    },
    ShortcutDescriptor {
        id: ShortcutId::ToggleNavigation,
        label: "Toggle navigation size",
        category: ShortcutCategory::Interface,
        default_binding: "cmd-\\",
    },
    ShortcutDescriptor {
        id: ShortcutId::ToggleMetrics,
        label: "Toggle metrics",
        category: ShortcutCategory::Interface,
        default_binding: "cmd-shift-m",
    },
    ShortcutDescriptor {
        id: ShortcutId::ZoomUiIn,
        label: "Zoom interface in",
        category: ShortcutCategory::Interface,
        default_binding: "cmd-=",
    },
    ShortcutDescriptor {
        id: ShortcutId::ZoomUiOut,
        label: "Zoom interface out",
        category: ShortcutCategory::Interface,
        default_binding: "cmd--",
    },
    ShortcutDescriptor {
        id: ShortcutId::ZoomUiReset,
        label: "Reset interface zoom",
        category: ShortcutCategory::Interface,
        default_binding: "cmd-0",
    },
    ShortcutDescriptor {
        id: ShortcutId::ZoomEditorIn,
        label: "Zoom editor in",
        category: ShortcutCategory::Interface,
        default_binding: "cmd-alt-=",
    },
    ShortcutDescriptor {
        id: ShortcutId::ZoomEditorOut,
        label: "Zoom editor out",
        category: ShortcutCategory::Interface,
        default_binding: "cmd-alt--",
    },
    ShortcutDescriptor {
        id: ShortcutId::ZoomEditorReset,
        label: "Reset editor zoom",
        category: ShortcutCategory::Interface,
        default_binding: "cmd-alt-0",
    },
    ShortcutDescriptor {
        id: ShortcutId::SaveRequest,
        label: "Save active context",
        category: ShortcutCategory::Application,
        default_binding: "cmd-s",
    },
    ShortcutDescriptor {
        id: ShortcutId::SaveRequestAs,
        label: "Save active context as",
        category: ShortcutCategory::Application,
        default_binding: "cmd-shift-s",
    },
    ShortcutDescriptor {
        id: ShortcutId::QuitApp,
        label: "Quit Resolved",
        category: ShortcutCategory::Application,
        default_binding: "cmd-q",
    },
];

pub fn shortcut_descriptors() -> &'static [ShortcutDescriptor] {
    SHORTCUT_DESCRIPTORS
}

pub fn shortcut_descriptor(id: ShortcutId) -> &'static ShortcutDescriptor {
    SHORTCUT_DESCRIPTORS
        .iter()
        .find(|descriptor| descriptor.id == id)
        .expect("every ShortcutId must have a descriptor")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShortcutValidationError {
    Empty,
    InvalidKeystroke { keystroke: String },
    ModifierOnly { keystroke: String },
    UnmodifiedPrintable { keystroke: String },
}

impl fmt::Display for ShortcutValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("a shortcut cannot be empty"),
            Self::InvalidKeystroke { keystroke } => {
                write!(formatter, "“{keystroke}” is not a valid keystroke")
            }
            Self::ModifierOnly { keystroke } => {
                write!(formatter, "“{keystroke}” contains only a modifier key")
            }
            Self::UnmodifiedPrintable { keystroke } => write!(
                formatter,
                "“{keystroke}” would intercept normal text input; add Command, Control, Option, or Function"
            ),
        }
    }
}

impl Error for ShortcutValidationError {}

/// Parse and canonicalize a GPUI key sequence.
///
/// GPUI accepts multi-stroke bindings separated by whitespace. Each stroke is
/// normalized with `Keystroke::unparse`, so aliases and modifier order compare
/// consistently during conflict detection.
pub fn normalize_binding(source: &str) -> Result<String, ShortcutValidationError> {
    let source = source.trim();
    if source.is_empty() {
        return Err(ShortcutValidationError::Empty);
    }

    source
        .split_whitespace()
        .map(|raw| {
            let keystroke =
                Keystroke::parse(raw).map_err(|_| ShortcutValidationError::InvalidKeystroke {
                    keystroke: raw.to_owned(),
                })?;
            validate_keystroke(&keystroke, raw)?;
            let mut keystroke = keystroke;
            normalize_keystroke_for_platform(&mut keystroke);
            Ok(keystroke.unparse())
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|strokes| strokes.join(" "))
}

/// Canonicalize a keystroke's modifier for the running platform.
///
/// Defaults are authored once using macOS `cmd-` spelling. The active desktop
/// backend maps that semantic modifier to the native user-facing convention;
/// Linux and Windows use Control while macOS retains Command.
fn normalize_keystroke_for_platform(keystroke: &mut Keystroke) {
    crate::platform::normalize_keystroke(keystroke);
}

fn validate_keystroke(keystroke: &Keystroke, source: &str) -> Result<(), ShortcutValidationError> {
    if matches!(
        keystroke.key.as_str(),
        "shift" | "control" | "alt" | "platform" | "function"
    ) {
        return Err(ShortcutValidationError::ModifierOnly {
            keystroke: source.to_owned(),
        });
    }

    let has_non_shift_modifier = keystroke.modifiers.platform
        || keystroke.modifiers.control
        || keystroke.modifiers.alt
        || keystroke.modifiers.function;
    let is_printable_or_input_key = keystroke.key.chars().count() == 1
        || matches!(keystroke.key.as_str(), "space" | "enter" | "tab");
    if !has_non_shift_modifier && is_printable_or_input_key {
        return Err(ShortcutValidationError::UnmodifiedPrintable {
            keystroke: source.to_owned(),
        });
    }

    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShortcutSource {
    Default,
    Custom,
    Disabled,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectiveShortcut {
    pub descriptor: &'static ShortcutDescriptor,
    pub binding: Option<String>,
    pub source: ShortcutSource,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidShortcutSetting {
    pub id: ShortcutId,
    pub value: String,
    pub error: ShortcutValidationError,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShortcutConflict {
    pub binding: String,
    pub first: ShortcutId,
    pub second: ShortcutId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShortcutConfigIssue {
    Invalid(InvalidShortcutSetting),
    Conflict(ShortcutConflict),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShortcutConfigError {
    pub issues: Vec<ShortcutConfigIssue>,
}

impl fmt::Display for ShortcutConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Some(first) = self.issues.first() else {
            return formatter.write_str("shortcut configuration is invalid");
        };
        match first {
            ShortcutConfigIssue::Invalid(invalid) => {
                write!(formatter, "{}: {}", invalid.id, invalid.error)?;
            }
            ShortcutConfigIssue::Conflict(conflict) => write!(
                formatter,
                "{} and {} both use {}",
                conflict.first, conflict.second, conflict.binding
            )?,
        }
        if self.issues.len() > 1 {
            write!(
                formatter,
                " (and {} more issue{})",
                self.issues.len() - 1,
                if self.issues.len() == 2 { "" } else { "s" }
            )?;
        }
        Ok(())
    }
}

impl Error for ShortcutConfigError {}

pub fn effective_binding(
    settings: &AppSettings,
    id: ShortcutId,
) -> Result<EffectiveShortcut, ShortcutValidationError> {
    let descriptor = shortcut_descriptor(id);
    match settings.shortcuts.get(id.key()) {
        Some(ShortcutOverride::Custom(binding)) => Ok(EffectiveShortcut {
            descriptor,
            binding: Some(normalize_binding(binding)?),
            source: ShortcutSource::Custom,
        }),
        Some(ShortcutOverride::Disabled) => Ok(EffectiveShortcut {
            descriptor,
            binding: None,
            source: ShortcutSource::Disabled,
        }),
        None => Ok(EffectiveShortcut {
            descriptor,
            binding: Some(
                normalize_binding(descriptor.default_binding).unwrap_or_else(|error| {
                    panic!(
                        "invalid built-in shortcut {} ({}): {error}",
                        id, descriptor.default_binding
                    )
                }),
            ),
            source: ShortcutSource::Default,
        }),
    }
}

/// Resolve every known action and report all invalid values and collisions.
///
/// Unknown stored IDs are intentionally ignored. They may belong to a newer
/// application version and should not make the known shortcut set unusable.
pub fn effective_shortcuts(
    settings: &AppSettings,
) -> Result<Vec<EffectiveShortcut>, ShortcutConfigError> {
    let mut effective = Vec::with_capacity(SHORTCUT_DESCRIPTORS.len());
    let mut issues = Vec::new();

    for descriptor in SHORTCUT_DESCRIPTORS {
        match effective_binding(settings, descriptor.id) {
            Ok(shortcut) => effective.push(shortcut),
            Err(error) => {
                let value = settings
                    .shortcuts
                    .get(descriptor.id.key())
                    .and_then(|value| match value {
                        ShortcutOverride::Custom(value) => Some(value.clone()),
                        ShortcutOverride::Disabled => None,
                    })
                    .unwrap_or_default();
                issues.push(ShortcutConfigIssue::Invalid(InvalidShortcutSetting {
                    id: descriptor.id,
                    value,
                    error,
                }));
            }
        }
    }

    issues.extend(
        detect_conflicts(&effective)
            .into_iter()
            .map(ShortcutConfigIssue::Conflict),
    );

    if issues.is_empty() {
        Ok(effective)
    } else {
        Err(ShortcutConfigError { issues })
    }
}

pub fn detect_conflicts(shortcuts: &[EffectiveShortcut]) -> Vec<ShortcutConflict> {
    let mut first_by_binding = BTreeMap::<&str, ShortcutId>::new();
    let mut conflicts = Vec::new();

    for shortcut in shortcuts {
        let Some(binding) = shortcut.binding.as_deref() else {
            continue;
        };
        if let Some(first) = first_by_binding.get(binding).copied() {
            conflicts.push(ShortcutConflict {
                binding: binding.to_owned(),
                first,
                second: shortcut.descriptor.id,
            });
        } else {
            first_by_binding.insert(binding, shortcut.descriptor.id);
        }
    }

    conflicts
}

#[cfg(test)]
pub fn unknown_shortcut_ids(settings: &AppSettings) -> Vec<&str> {
    let known = SHORTCUT_DESCRIPTORS
        .iter()
        .map(|descriptor| descriptor.id.key())
        .collect::<BTreeSet<_>>();
    settings
        .shortcuts
        .keys()
        .filter(|id| !known.contains(id.as_str()))
        .map(String::as_str)
        .collect()
}

pub fn build_app_key_bindings(
    settings: &AppSettings,
) -> Result<Vec<KeyBinding>, ShortcutConfigError> {
    effective_shortcuts(settings).map(|shortcuts| {
        let mut bindings = Vec::with_capacity(shortcuts.len());
        for shortcut in shortcuts {
            let Some(binding) = shortcut.binding else {
                continue;
            };
            // A binding without a context is available even when no GPUI
            // element currently owns focus. GPUI treats it at the deepest
            // context depth, and this application layer is installed after
            // GPUI Component's editor keymap so intentional user overrides
            // win without replacing the editor's remaining bindings.
            bindings.push(key_binding(shortcut.descriptor.id, &binding, None));
        }
        bindings
    })
}

/// Capture GPUI Component's initialized bindings before Resolved installs
/// its own layer. Keep this snapshot unchanged and pass it to
/// [`apply_key_bindings`] for every live settings update.
pub fn capture_base_key_bindings(cx: &App) -> Vec<KeyBinding> {
    cx.key_bindings().borrow().bindings().cloned().collect()
}

/// Replace only Resolved's keymap layer while preserving GPUI Component's
/// text editing, menu, dialog, and popover bindings.
///
/// `base_bindings` must be captured once, after component initialization and
/// before any Resolved bindings are installed. The candidate app keymap is
/// fully validated and built before the active keymap is changed.
pub fn apply_key_bindings(
    cx: &mut App,
    base_bindings: &[KeyBinding],
    settings: &AppSettings,
) -> Result<(), ShortcutConfigError> {
    let app_bindings = build_app_key_bindings(settings)?;
    cx.clear_key_bindings();
    cx.bind_keys(base_bindings.iter().cloned());
    cx.bind_keys(app_bindings);
    Ok(())
}

fn key_binding(id: ShortcutId, binding: &str, context: Option<&str>) -> KeyBinding {
    match id {
        ShortcutId::NewRequestTab => KeyBinding::new(binding, NewRequestTab, context),
        ShortcutId::CloseRequestTab => KeyBinding::new(binding, CloseRequestTab, context),
        ShortcutId::ActivateNextRequestTab => {
            KeyBinding::new(binding, ActivateNextRequestTab, context)
        }
        ShortcutId::ActivatePreviousRequestTab => {
            KeyBinding::new(binding, ActivatePreviousRequestTab, context)
        }
        ShortcutId::SendOrCancelRequest => KeyBinding::new(binding, SendOrCancelRequest, context),
        ShortcutId::QuickSendWebSocketTemplate => {
            KeyBinding::new(binding, QuickSendWebSocketTemplate, context)
        }
        ShortcutId::SaveRequest => KeyBinding::new(binding, SaveRequest, context),
        ShortcutId::SaveRequestAs => KeyBinding::new(binding, SaveRequestAs, context),
        ShortcutId::FocusRequestUrl => KeyBinding::new(binding, FocusRequestUrl, context),
        ShortcutId::FormatRawBody => KeyBinding::new(binding, FormatRawBody, context),
        ShortcutId::ShowCollections => KeyBinding::new(binding, ShowCollections, context),
        ShortcutId::ShowEnvironments => KeyBinding::new(binding, ShowEnvironments, context),
        ShortcutId::ShowHistory => KeyBinding::new(binding, ShowHistory, context),
        ShortcutId::ShowSettings => KeyBinding::new(binding, ShowSettings, context),
        ShortcutId::ToggleNavigation => KeyBinding::new(binding, ToggleNavigation, context),
        ShortcutId::ToggleMetrics => KeyBinding::new(binding, ToggleMetrics, context),
        ShortcutId::ZoomUiIn => KeyBinding::new(binding, ZoomUiIn, context),
        ShortcutId::ZoomUiOut => KeyBinding::new(binding, ZoomUiOut, context),
        ShortcutId::ZoomUiReset => KeyBinding::new(binding, ZoomUiReset, context),
        ShortcutId::ZoomEditorIn => KeyBinding::new(binding, ZoomEditorIn, context),
        ShortcutId::ZoomEditorOut => KeyBinding::new(binding, ZoomEditorOut, context),
        ShortcutId::ZoomEditorReset => KeyBinding::new(binding, ZoomEditorReset, context),
        ShortcutId::QuitApp => KeyBinding::new(binding, QuitApp, context),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings_with(id: ShortcutId, value: ShortcutOverride) -> AppSettings {
        let mut settings = AppSettings::default();
        settings.shortcuts.insert(id.key().to_owned(), value);
        settings
    }

    #[test]
    fn descriptors_have_stable_unique_ids_and_valid_conflict_free_defaults() {
        let settings = AppSettings::default();
        let ids = SHORTCUT_DESCRIPTORS
            .iter()
            .map(|descriptor| descriptor.id.key())
            .collect::<BTreeSet<_>>();

        assert_eq!(ids.len(), SHORTCUT_DESCRIPTORS.len());
        assert_eq!(
            effective_shortcuts(&settings).unwrap().len(),
            SHORTCUT_DESCRIPTORS.len()
        );
        assert!(unknown_shortcut_ids(&settings).is_empty());
        for descriptor in SHORTCUT_DESCRIPTORS {
            assert_eq!(
                ShortcutId::from_key(descriptor.id.key()),
                Some(descriptor.id)
            );
            assert!(normalize_binding(descriptor.default_binding).is_ok());
        }
    }

    #[test]
    fn descriptor_sections_are_nonempty_and_follow_the_declared_order() {
        let category_order = ShortcutCategory::ALL
            .into_iter()
            .enumerate()
            .map(|(index, category)| (category, index))
            .collect::<BTreeMap<_, _>>();
        let mut counts = BTreeMap::new();
        let mut previous = 0;

        for (descriptor_index, descriptor) in SHORTCUT_DESCRIPTORS.iter().enumerate() {
            let index = category_order[&descriptor.category];
            if descriptor_index > 0 {
                assert!(
                    index >= previous,
                    "{} appears outside its declared section order",
                    descriptor.id
                );
            }
            previous = index;
            *counts.entry(descriptor.category).or_insert(0usize) += 1;
        }

        for category in ShortcutCategory::ALL {
            assert!(
                counts.get(&category).is_some_and(|count| *count > 0),
                "{} must contain at least one shortcut",
                category.label()
            );
            assert!(!category.description().trim().is_empty());
        }
    }

    #[test]
    fn generalized_tab_labels_keep_persisted_shortcut_ids_compatible() {
        assert_eq!(ShortcutId::CloseRequestTab.key(), "request.close_tab");
        assert_eq!(ShortcutId::ActivateNextRequestTab.key(), "request.next_tab");
        assert_eq!(
            ShortcutId::ActivatePreviousRequestTab.key(),
            "request.previous_tab"
        );
        assert_eq!(
            shortcut_descriptor(ShortcutId::CloseRequestTab).label,
            "Close active tab"
        );
        assert_eq!(
            shortcut_descriptor(ShortcutId::ActivateNextRequestTab).label,
            "Next tab"
        );
        assert_eq!(
            shortcut_descriptor(ShortcutId::ActivatePreviousRequestTab).label,
            "Previous tab"
        );
    }

    #[test]
    fn normalization_canonicalizes_case_modifier_order_and_chords() {
        if cfg!(target_os = "macos") {
            assert_eq!(
                normalize_binding(" SHIFT-CMD-S   CTRL-TAB ").unwrap(),
                "cmd-shift-s ctrl-tab"
            );
        } else {
            // The macOS-authored Command modifier resolves to Control on
            // Linux and Windows so installed bindings match typed keystrokes.
            assert_eq!(
                normalize_binding(" SHIFT-CMD-S   CTRL-TAB ").unwrap(),
                "ctrl-shift-s ctrl-tab"
            );
        }
    }

    #[test]
    fn validation_rejects_empty_modifier_only_and_typing_bindings() {
        assert_eq!(normalize_binding(""), Err(ShortcutValidationError::Empty));
        assert!(matches!(
            normalize_binding("cmd"),
            Err(ShortcutValidationError::ModifierOnly { .. })
        ));
        assert!(matches!(
            normalize_binding("A"),
            Err(ShortcutValidationError::UnmodifiedPrintable { .. })
        ));
        assert!(normalize_binding("f5").is_ok());
        assert!(normalize_binding("cmd-a").is_ok());
    }

    #[test]
    fn custom_and_disabled_overrides_replace_the_default() {
        let custom = settings_with(
            ShortcutId::SaveRequest,
            ShortcutOverride::Custom("ctrl-alt-s".to_owned()),
        );
        let resolved = effective_binding(&custom, ShortcutId::SaveRequest).unwrap();
        assert_eq!(resolved.binding.as_deref(), Some("ctrl-alt-s"));
        assert_eq!(resolved.source, ShortcutSource::Custom);

        let disabled = settings_with(ShortcutId::SaveRequest, ShortcutOverride::Disabled);
        let resolved = effective_binding(&disabled, ShortcutId::SaveRequest).unwrap();
        assert_eq!(resolved.binding, None);
        assert_eq!(resolved.source, ShortcutSource::Disabled);
    }

    #[test]
    fn invalid_custom_binding_is_reported_without_building_a_partial_keymap() {
        let settings = settings_with(
            ShortcutId::SaveRequest,
            ShortcutOverride::Custom("definitely-not-a-keystroke".to_owned()),
        );
        let error = build_app_key_bindings(&settings).unwrap_err();

        assert_eq!(error.issues.len(), 1);
        assert!(matches!(
            &error.issues[0],
            ShortcutConfigIssue::Invalid(InvalidShortcutSetting {
                id: ShortcutId::SaveRequest,
                ..
            })
        ));
    }

    #[test]
    fn normalized_collisions_identify_both_actions() {
        let settings = settings_with(
            ShortcutId::SaveRequest,
            ShortcutOverride::Custom("CMD-t".to_owned()),
        );
        let error = effective_shortcuts(&settings).unwrap_err();

        let expected_binding = if cfg!(target_os = "macos") {
            "cmd-t".to_owned()
        } else {
            "ctrl-t".to_owned()
        };
        assert_eq!(
            error.issues,
            vec![ShortcutConfigIssue::Conflict(ShortcutConflict {
                binding: expected_binding,
                first: ShortcutId::NewRequestTab,
                second: ShortcutId::SaveRequest,
            })]
        );
    }

    #[test]
    fn disabling_one_side_resolves_a_conflict() {
        let mut settings = AppSettings::default();
        settings.shortcuts.insert(
            ShortcutId::SaveRequest.key().to_owned(),
            ShortcutOverride::Custom("cmd-t".to_owned()),
        );
        settings.shortcuts.insert(
            ShortcutId::NewRequestTab.key().to_owned(),
            ShortcutOverride::Disabled,
        );

        assert!(effective_shortcuts(&settings).is_ok());
    }

    #[test]
    fn unknown_stored_ids_are_visible_but_do_not_break_known_bindings() {
        let mut settings = AppSettings::default();
        settings.shortcuts.insert(
            "future.action".to_owned(),
            ShortcutOverride::Custom("cmd-y".to_owned()),
        );

        assert_eq!(unknown_shortcut_ids(&settings), ["future.action"]);
        assert!(effective_shortcuts(&settings).is_ok());
    }

    #[test]
    fn built_keymap_contains_only_enabled_global_actions() {
        let settings = settings_with(ShortcutId::ToggleMetrics, ShortcutOverride::Disabled);
        let bindings = build_app_key_bindings(&settings).unwrap();

        assert_eq!(bindings.len(), SHORTCUT_DESCRIPTORS.len() - 1);
        assert!(bindings.iter().all(|binding| binding.predicate().is_none()));
        assert!(
            !bindings
                .iter()
                .any(|binding| binding.action().partial_eq(&ToggleMetrics))
        );
    }

    #[test]
    fn custom_binding_matches_without_a_focused_key_context() {
        let settings = settings_with(
            ShortcutId::NewRequestTab,
            ShortcutOverride::Custom("ctrl-alt-n".to_owned()),
        );
        let keymap = gpui::Keymap::new(build_app_key_bindings(&settings).unwrap());
        let keystroke = Keystroke::parse("ctrl-alt-n").unwrap();
        let (matches, pending) = keymap.bindings_for_input(&[keystroke], &[]);

        assert!(!pending);
        assert_eq!(matches.len(), 1);
        assert!(matches[0].action().partial_eq(&NewRequestTab));
    }
}
