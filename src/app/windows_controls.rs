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
    App, div, px, svg, AnyElement, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Styled as _, Window, WindowControlArea,
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

const WINDOW_CONTROL_WIDTH: Pixels = px(46.);
const GLYPH_SIZE: Pixels = px(14.);

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

    let glyph_color = glyph_color(cx);
    div()
        .flex()
        .flex_shrink_0()
        .child(control_button(
            "title-window-minimize",
            IconName::WindowMinimize,
            WindowControlArea::Min,
            glyph_color,
        ))
        .child(control_button(
            "title-window-maximize",
            if window.is_maximized() {
                IconName::WindowRestore
            } else {
                IconName::WindowMaximize
            },
            WindowControlArea::Max,
            glyph_color,
        ))
        .child(control_button(
            "title-window-close",
            IconName::WindowClose,
            WindowControlArea::Close,
            glyph_color,
        ))
        .into_any_element()
}

fn glyph_color(cx: &App) -> Hsla {
    cx.theme().muted_foreground
}

fn control_button(
    id: &'static str,
    icon: IconName,
    area: WindowControlArea,
    color: Hsla,
) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .justify_center()
        .items_center()
        .w(WINDOW_CONTROL_WIDTH)
        .h_full()
        .flex_shrink_0()
        .window_control_area(area)
        .text_color(color)
        .child(svg().path(icon.path()).size(GLYPH_SIZE).text_color(color))
}
