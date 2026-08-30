//! Windows decorations for Resolved's custom title bars.
//!
//! The macOS `.app` draws traffic lights, so each title bar reserves a wide
//! leading inset and nothing else. Windows has no system chrome under
//! `appears_transparent`, so the bars themselves must provide:
//!
//! * minimize / maximize-restore / close buttons at the trailing edge, and
//! * a caption region over the non-interactive remainder of the bar.
//!
//! Control regions are marked with gpui's `WindowControlArea`: the platform
//! interprets presses there as non-client commands (caption drag, HTMINBUTTON,
//! HTMAXBUTTON, HTCLOSE). Interactive content stays outside the marked
//! elements — a Drag region wrapping a button would swallow its press.
//! Close is routed through `WM_CLOSE`, so the app's dirty-close guard stays
//! in charge.

use gpui::{
    AnyElement, App, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    StatefulInteractiveElement as _, Styled as _, Window, WindowControlArea, div, px, svg,
};
use gpui_component::{ActiveTheme as _, IconName, IconNamed as _};

/// Whether the Windows control buttons participate in title bars.
pub const fn uses_windows_window_controls() -> bool {
    cfg!(target_os = "windows")
}

/// The title bar's leading inset: macOS traffic-light clearance, or a compact
/// caption strip on Windows.
pub const fn leading_inset() -> Pixels {
    if cfg!(target_os = "macos") {
        px(92.)
    } else {
        px(12.)
    }
}

/// Right-side title-bar breathing room on platforms without client-drawn
/// Windows controls. On Windows the control cluster must touch the window's
/// trailing edge so its native hit targets stay aligned across workspaces.
pub const fn trailing_inset() -> Pixels {
    if cfg!(target_os = "windows") {
        px(0.)
    } else {
        px(24.)
    }
}

const WINDOW_CONTROL_WIDTH: Pixels = px(46.);
const GLYPH_SIZE: Pixels = px(12.);

#[derive(Clone, Copy)]
enum ControlKind {
    Standard,
    Close,
}

/// The trailing window-control cluster on Windows; produces nothing
/// elsewhere.
///
/// Appended as the last child of a title bar row. The three buttons are
/// marked non-client areas, so presses reach the OS without click handlers;
/// `window.is_maximized()` picks the restore/maximize glyph. The glyph color
/// follows the active theme's muted foreground — gpui's `Svg` paints nothing
/// without an explicit text color, and a fixed gray is invisible on light
/// title bars.
pub(crate) fn windows_window_controls(window: &Window, cx: &App) -> AnyElement {
    if !uses_windows_window_controls() {
        return div().into_any_element();
    }

    let colors = ControlColors::new(cx);
    div()
        .flex()
        .items_center()
        .h(px(super::APP_TITLE_BAR_HEIGHT))
        .flex_shrink_0()
        .child(control_button(
            "title-window-minimize",
            IconName::WindowMinimize,
            WindowControlArea::Min,
            ControlKind::Standard,
            colors,
        ))
        .child(control_button(
            "title-window-maximize",
            if window.is_maximized() {
                IconName::WindowRestore
            } else {
                IconName::WindowMaximize
            },
            WindowControlArea::Max,
            ControlKind::Standard,
            colors,
        ))
        .child(control_button(
            "title-window-close",
            IconName::WindowClose,
            WindowControlArea::Close,
            ControlKind::Close,
            colors,
        ))
        .into_any_element()
}

#[derive(Clone, Copy)]
struct ControlColors {
    foreground: Hsla,
    hover_background: Hsla,
    active_background: Hsla,
    close_hover_background: Hsla,
    close_active_background: Hsla,
    close_foreground: Hsla,
}

impl ControlColors {
    fn new(cx: &App) -> Self {
        Self {
            foreground: cx.theme().foreground,
            hover_background: cx.theme().secondary_hover,
            active_background: cx.theme().secondary_active,
            close_hover_background: cx.theme().danger,
            close_active_background: cx.theme().danger_active,
            close_foreground: cx.theme().danger_foreground,
        }
    }
}

fn control_button(
    id: &'static str,
    icon: IconName,
    area: WindowControlArea,
    kind: ControlKind,
    colors: ControlColors,
) -> impl IntoElement {
    let (hover_background, active_background, hover_foreground) = match kind {
        ControlKind::Standard => (
            colors.hover_background,
            colors.active_background,
            colors.foreground,
        ),
        ControlKind::Close => (
            colors.close_hover_background,
            colors.close_active_background,
            colors.close_foreground,
        ),
    };

    div()
        .id(id)
        .group(id)
        .flex()
        .justify_center()
        .items_center()
        .w(WINDOW_CONTROL_WIDTH)
        .h_full()
        .flex_shrink_0()
        .window_control_area(area)
        .text_color(colors.foreground)
        .hover(|style| style.bg(hover_background).text_color(hover_foreground))
        .active(|style| style.bg(active_background).text_color(hover_foreground))
        .child(
            svg()
                .path(icon.path())
                .size(GLYPH_SIZE)
                .text_color(colors.foreground)
                .group_hover(id, |style| style.text_color(hover_foreground)),
        )
}
