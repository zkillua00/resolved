//! CSS-backed application theming.
//!
//! Parsing is deliberately independent from GPUI mutation: callers can validate
//! an untrusted theme completely before atomically applying it.

use std::sync::Arc;

use gpui::{App, BorrowAppContext as _, Global, Hsla, px};
use gpui_component::Theme;

mod css;
mod palette;

pub use css::ThemeCssError as ThemeError;
#[allow(unused_imports)]
pub use palette::{ApiPalette, ApiTheme};

/// The active parsed API Tester theme stored in GPUI global state.
#[derive(Clone, Debug)]
pub struct GlobalApiTheme(Arc<ApiTheme>);

impl Global for GlobalApiTheme {}

impl GlobalApiTheme {
    pub fn get(cx: &App) -> &Arc<ApiTheme> {
        &cx.global::<Self>().0
    }
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

/// Parse and validate an API Tester CSS theme without changing application state.
pub fn parse_css(source: &str) -> Result<ApiTheme, ThemeError> {
    css::parse_theme_css(source)
}

/// Atomically install a previously validated theme and redraw all GPUI windows.
pub fn apply(theme: ApiTheme, cx: &mut App) {
    let theme = Arc::new(theme);
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
    let theme = parse_css(bundled_css()).expect("bundled API Tester theme must be valid");
    apply(theme, cx);
}

/// Parse and atomically apply a CSS theme.
pub fn parse_and_apply(source: &str, cx: &mut App) -> Result<(), ThemeError> {
    let theme = parse_css(source)?;
    apply(theme, cx);
    Ok(())
}
