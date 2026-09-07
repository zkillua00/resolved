//! CSS-backed application theming.
//!
//! Parsing is deliberately independent from GPUI mutation: callers can validate
//! an untrusted theme completely before atomically applying it.

use std::sync::Arc;

use gpui::{App, BorrowAppContext as _, Global, Hsla, px};
use gpui_component::Theme;

use crate::core::{SavedTheme, ThemeSettings};

mod css;
mod intelligence;
mod palette;
mod presets;

pub(crate) use presets::install_bundled_themes;
mod schema;

pub use css::ThemeCssError as ThemeError;
pub(crate) use intelligence::{ThemeCssIntelligence, theme_css_diagnostics};
#[allow(unused_imports)]
pub use palette::{ApiPalette, ApiTheme};

/// The active parsed Resolved theme stored in GPUI global state.
#[derive(Clone, Debug)]
pub struct GlobalApiTheme(Arc<ApiTheme>);

impl Global for GlobalApiTheme {}

impl GlobalApiTheme {
    pub fn get(cx: &App) -> &Arc<ApiTheme> {
        &cx.global::<Self>().0
    }
}

/// Typography zoom applied on top of the active theme's baseline font sizes.
///
/// Kept out of the CSS theme so changing zoom never rewrites a user's saved
/// theme source, and stored as a GPUI global so every [`apply`] call —
/// startup, theme switches, and live CSS edits — honors the current zoom
/// automatically.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThemeZoom {
    /// Interface zoom multiplier applied to the theme's `.app` font size.
    pub ui: f32,
    /// Editor zoom multiplier applied to the theme's `.editor` font size.
    pub editor: f32,
}

impl Default for ThemeZoom {
    fn default() -> Self {
        Self {
            ui: 1.0,
            editor: 1.0,
        }
    }
}

impl Global for ThemeZoom {}

impl ThemeZoom {
    /// Publish the zoom used by subsequent [`apply`] calls. The caller keeps
    /// responsibility for re-applying the active theme source; see
    /// [`set_zoom`].
    pub fn set(zoom: Self, cx: &mut App) {
        if cx.has_global::<Self>() {
            cx.update_global::<Self, _>(|current, _| *current = zoom);
        } else {
            cx.set_global(zoom);
        }
    }

    /// The currently published zoom, or unity when nothing published yet.
    pub fn current(cx: &App) -> Self {
        cx.try_global::<Self>().copied().unwrap_or_default()
    }
}

/// Publish a new typography zoom. Follow this with a re-application of the
/// active theme source (via [`parse_and_apply`] or [`configure`]) so the new
/// scale takes effect immediately.
pub fn set_zoom(zoom: ThemeZoom, cx: &mut App) {
    ThemeZoom::set(zoom, cx);
}

/// Context-aware access to application-specific semantic colors.
#[allow(dead_code)]
pub trait ApiThemeExt {
    fn api_theme(&self) -> &ApiTheme;

    fn api_surface(&self) -> Hsla {
        self.api_theme().palette.surface
    }

    fn api_surface_lowest(&self) -> Hsla {
        self.api_theme().palette.surface_lowest
    }

    fn api_surface_low(&self) -> Hsla {
        self.api_theme().palette.surface_low
    }

    fn api_surface_container(&self) -> Hsla {
        self.api_theme().palette.surface_container
    }

    fn api_surface_high(&self) -> Hsla {
        self.api_theme().palette.surface_high
    }

    fn api_surface_highest(&self) -> Hsla {
        self.api_theme().palette.surface_highest
    }

    fn api_outline_variant(&self) -> Hsla {
        self.api_theme().palette.outline_variant
    }

    fn api_primary_bright(&self) -> Hsla {
        self.api_theme().palette.primary_bright
    }

    fn api_primary_lavender(&self) -> Hsla {
        self.api_theme().palette.primary_lavender
    }

    fn api_selected_container(&self) -> Hsla {
        self.api_theme().palette.selected_container
    }
}

impl ApiThemeExt for App {
    fn api_theme(&self) -> &ApiTheme {
        GlobalApiTheme::get(self).as_ref()
    }
}

/// Return the bundled default theme source.
pub fn bundled_css() -> &'static str {
    css::BUILTIN_THEME_CSS
}

/// Parse and validate a Resolved CSS theme without changing application state.
pub fn parse_css(source: &str) -> Result<ApiTheme, ThemeError> {
    css::parse_theme_css(source)
}

/// Reconcile the active compatibility snapshot with the saved-theme catalog.
///
/// Builds before the catalog stored only `css_source` and `source_path`. A
/// valid legacy snapshot becomes a saved theme once, while an older build
/// selecting the built-in theme clears a stale catalog selection.
pub(crate) fn reconcile_catalog(settings: &mut ThemeSettings) -> Result<bool, ThemeError> {
    let Some(source) = settings.css_source.as_deref() else {
        let changed = settings.active_theme_id.take().is_some() || settings.source_path.is_some();
        settings.source_path = None;
        return Ok(changed);
    };

    if let Some(active_id) = settings.active_theme_id.as_deref()
        && settings.saved_theme(active_id).is_some_and(|saved| {
            saved.css_source == source && saved.source_path == settings.source_path
        })
    {
        return Ok(false);
    }

    if let Some(saved) = settings
        .saved_themes
        .iter()
        .find(|saved| saved.css_source == source && saved.source_path == settings.source_path)
    {
        let changed = settings.active_theme_id.as_deref() != Some(saved.id.as_str());
        settings.active_theme_id = Some(saved.id.clone());
        return Ok(changed);
    }

    let parsed = parse_css(source)?;
    if let Some(source_path) = settings.source_path.as_ref() {
        let active_index = settings.active_theme_id.as_deref().and_then(|active_id| {
            settings.saved_themes.iter().position(|saved| {
                saved.id == active_id && saved.source_path.as_ref() == Some(source_path)
            })
        });
        if let Some(index) = active_index.or_else(|| {
            settings
                .saved_themes
                .iter()
                .position(|saved| saved.source_path.as_ref() == Some(source_path))
        }) {
            let saved = &mut settings.saved_themes[index];
            let changed = saved.css_source != source
                || settings.active_theme_id.as_deref() != Some(saved.id.as_str());
            saved.css_source = source.to_owned();
            settings.active_theme_id = Some(saved.id.clone());
            return Ok(changed);
        }
    }

    let name = unique_catalog_name(&settings.saved_themes, parsed.name.as_ref());
    let saved = SavedTheme::new(name, source.to_owned(), settings.source_path.clone());
    settings.active_theme_id = Some(saved.id.clone());
    settings.saved_themes.push(saved);
    Ok(true)
}

fn unique_catalog_name(saved_themes: &[SavedTheme], requested: &str) -> String {
    const MAX_NAME_CHARS: usize = 80;
    let requested = requested
        .trim()
        .chars()
        .take(MAX_NAME_CHARS)
        .collect::<String>();
    let requested = if requested.is_empty() {
        "Untitled theme".to_owned()
    } else {
        requested
    };
    if !saved_themes
        .iter()
        .any(|saved| saved.name.eq_ignore_ascii_case(&requested))
    {
        return requested;
    }
    (2usize..)
        .map(|suffix| {
            let suffix = format!(" ({suffix})");
            let keep = MAX_NAME_CHARS.saturating_sub(suffix.chars().count());
            let base = requested.chars().take(keep).collect::<String>();
            format!("{}{suffix}", base.trim_end())
        })
        .find(|candidate| {
            !saved_themes
                .iter()
                .any(|saved| saved.name.eq_ignore_ascii_case(candidate))
        })
        .expect("theme name suffix search is unbounded")
}

/// Atomically install a previously validated theme and redraw all GPUI windows.
///
/// The theme's typography is scaled by the active [`ThemeZoom`] global before
/// installation, so callers feed in a freshly parsed theme and the scale is
/// applied exactly once no matter how often zoom or theme changes re-apply it.
pub fn apply(theme: ApiTheme, cx: &mut App) {
    let theme = Arc::new(theme.scaled(ThemeZoom::current(cx)));
    if cx.has_global::<GlobalApiTheme>() {
        let next = Arc::clone(&theme);
        cx.update_global::<GlobalApiTheme, _>(|active, _| active.0 = next);
    } else {
        cx.set_global(GlobalApiTheme(Arc::clone(&theme)));
    }

    let component_theme = Theme::global_mut(cx);
    component_theme.mode = theme.mode;
    component_theme.colors = theme.palette.component_colors;
    component_theme.highlight_theme = Arc::clone(&theme.palette.highlight_theme);
    component_theme.font_family = theme.classes.app.font_family.clone();
    component_theme.font_size = theme.classes.app.font_size;
    component_theme.mono_font_size = theme.classes.editor.font_size;
    component_theme.button_style = theme.classes.button.clone();
    cx.refresh_windows();
}

/// Install the bundled theme and preserve the original application typography.
pub fn configure(cx: &mut App) {
    {
        let theme = Theme::global_mut(cx);
        theme.font_family = ".SystemUIFont".into();
        theme.font_size = px(16.);
        theme.mono_font_family = "Menlo".into();
        theme.mono_font_size = px(13.);
        theme.radius = px(8.);
        theme.radius_lg = px(12.);
        theme.shadow = false;
    }
    let theme = parse_css(bundled_css()).expect("bundled Resolved theme must be valid");
    apply(theme, cx);
}

/// Parse and atomically apply a CSS theme.
pub fn parse_and_apply(source: &str, cx: &mut App) -> Result<(), ThemeError> {
    let theme = parse_css(source)?;
    apply(theme, cx);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_active_snapshot_is_promoted_once() {
        let mut settings = ThemeSettings {
            source_path: Some("/tmp/legacy.css".into()),
            css_source: Some(bundled_css().to_owned()),
            ..Default::default()
        };

        assert!(reconcile_catalog(&mut settings).unwrap());
        let active_id = settings.active_theme_id.clone().unwrap();
        let active = settings.saved_theme(&active_id).unwrap();
        assert_eq!(active.css_source, bundled_css());
        assert_eq!(active.source_path, settings.source_path);
        assert!(!reconcile_catalog(&mut settings).unwrap());
        assert_eq!(settings.saved_themes.len(), 1);
    }

    #[test]
    fn built_in_projection_clears_a_stale_selection_without_losing_catalog() {
        let saved = SavedTheme::new("Saved", bundled_css(), None);
        let mut settings = ThemeSettings {
            active_theme_id: Some(saved.id.clone()),
            saved_themes: vec![saved],
            source_path: Some("/tmp/stale.css".into()),
            ..Default::default()
        };

        assert!(reconcile_catalog(&mut settings).unwrap());
        assert_eq!(settings.active_theme_id, None);
        assert_eq!(settings.source_path, None);
        assert_eq!(settings.saved_themes.len(), 1);
    }

    #[test]
    fn legacy_change_at_a_known_path_updates_the_existing_record() {
        let mut old_source = bundled_css().to_owned();
        old_source = old_source.replace("#141217", "#151218");
        let saved = SavedTheme::new("Existing", old_source, Some("/tmp/shared-theme.css".into()));
        let mut settings = ThemeSettings {
            saved_themes: vec![saved],
            source_path: Some("/tmp/shared-theme.css".into()),
            css_source: Some(bundled_css().to_owned()),
            ..Default::default()
        };

        assert!(reconcile_catalog(&mut settings).unwrap());
        assert_eq!(settings.saved_themes.len(), 1);
        assert_eq!(settings.saved_themes[0].css_source, bundled_css());
        assert_eq!(
            settings.active_theme_id.as_deref(),
            Some(settings.saved_themes[0].id.as_str())
        );
    }

    #[test]
    fn invalid_legacy_snapshot_is_left_untouched() {
        let mut settings = ThemeSettings {
            css_source: Some(":root {}".into()),
            ..Default::default()
        };
        let before = settings.clone();

        assert!(reconcile_catalog(&mut settings).is_err());
        assert_eq!(settings, before);
    }
}
