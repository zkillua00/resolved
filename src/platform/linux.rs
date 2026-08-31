use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use gdkx11::X11Display;
use gdkx11::glib::translate::ToGlibPtr as _;
use gpui::{App, Keystroke, Task, Timer, WindowDecorations, WindowOptions};
use gtk::prelude::*;
use wry::{WebViewBuilder, WebViewExtUnix as _};

use super::{
    PlatformBackend, TitleBarIntegration, configure_desktop_menus, control_shortcut,
    native_title_bar,
};

pub(super) struct LinuxBackend;

static GTK_EVENT_PUMP_STARTED: AtomicBool = AtomicBool::new(false);

impl PlatformBackend for LinuxBackend {
    fn install_runtime_hooks() {
        let has_display = std::env::var_os("DISPLAY").is_some_and(|value| !value.is_empty());
        let has_wayland =
            std::env::var_os("WAYLAND_DISPLAY").is_some_and(|value| !value.is_empty());
        if has_display {
            // SAFETY: main calls this before constructing GPUI's Application,
            // which is also before the app starts any worker threads. Wry's
            // embedded WebKitGTK child-window backend accepts X11 handles only.
            // Force both GPUI and GTK onto the session's X11/XWayland display;
            // otherwise an inherited `GDK_BACKEND=wayland` makes Wry panic when
            // it downcasts GTK's display after receiving GPUI's X11 handle.
            unsafe {
                std::env::set_var("GDK_BACKEND", "x11");
                if has_wayland {
                    std::env::remove_var("WAYLAND_DISPLAY");
                }
            }
        }
    }

    fn configure_menus(cx: &mut App) {
        configure_desktop_menus(cx);
    }

    fn normalize_keystroke(keystroke: &mut Keystroke) {
        control_shortcut(keystroke);
    }

    fn log_diagnostic(message: &str) {
        tracing::info!("{message}");
    }

    fn configure_main_window(options: &mut WindowOptions) {
        options.app_id = Some("io.github.zkillua00.resolved".to_owned());
        options.window_decorations = Some(WindowDecorations::Server);
        if let Some(titlebar) = &mut options.titlebar {
            titlebar.appears_transparent = false;
            titlebar.traffic_light_position = None;
        }
    }

    fn configure_webview<'a>(builder: WebViewBuilder<'a>) -> WebViewBuilder<'a> {
        // WebKitGTK creates a fresh ephemeral WebContext (and network helper)
        // for every incognito WebView, ignoring the process-level context the
        // caller supplied. Preview documents cannot navigate or use the
        // network, so reuse that context and its helper across tab reopenings.
        builder
    }

    fn start_webview_runtime(cx: &App) -> Result<Option<Task<()>>, String> {
        gtk::init()
            .map_err(|error| format!("could not initialize GTK for HTML Preview: {error}"))?;

        let display = gtk::gdk::Display::default()
            .ok_or_else(|| "GTK did not provide a display for HTML Preview".to_owned())?;
        if display.downcast_ref::<X11Display>().is_none() {
            return Err(
                "HTML Preview requires an X11 or XWayland display, but GTK selected a different backend"
                    .to_owned(),
            );
        }
        tracing::info!("web preview: GTK X11 runtime initialized");

        if !GTK_EVENT_PUMP_STARTED.swap(true, Ordering::AcqRel) {
            cx.spawn(async move |_| {
                loop {
                    while gtk::events_pending() {
                        gtk::main_iteration_do(false);
                    }
                    Timer::after(Duration::from_millis(16)).await;
                }
            })
            .detach();
        }

        // GTK is a process-level runtime. Keeping one event pump alive lets
        // native WebView destruction finish even after the Preview entity has
        // left GPUI's render tree, without accumulating one pump per reopen.
        Ok(None)
    }

    fn prepare_preview_webview(webview: &wry::WebView) -> Result<(), String> {
        let gtk_webview = webview.webview();
        let toplevel = gtk_webview
            .toplevel()
            .ok_or_else(|| "WebKitGTK did not create a top-level child window".to_owned())?;
        let gtk_window = toplevel
            .downcast::<gtk::Window>()
            .map_err(|_| "WebKitGTK's top-level widget was not a GTK window".to_owned())?;
        let gdk_window = gtk_window
            .window()
            .ok_or_else(|| "WebKitGTK's child window was not realized".to_owned())?;
        let x11_window = gdk_window
            .downcast::<gdkx11::X11Window>()
            .map_err(|_| "WebKitGTK's child window was not an X11 window".to_owned())?;
        let display = gtk::gdk::Display::default()
            .ok_or_else(|| "GTK did not provide the WebKitGTK display".to_owned())?
            .downcast::<X11Display>()
            .map_err(|_| "WebKitGTK's child window did not use an X11 display".to_owned())?;

        // Wry creates its X11 child with background pixel 0. GPUI uses an
        // ARGB visual, where that pixel is fully transparent, so the desktop
        // can show through between mapping the child and WebKit's first
        // composited frame. Give the container an opaque native background;
        // WebKit replaces it as soon as the captured document paints.
        unsafe {
            let raw_display = gdkx11::ffi::gdk_x11_display_get_xdisplay(display.to_glib_none().0);
            let xid = x11_window.xid();
            x11::xlib::XSetWindowBackground(raw_display, xid, u32::MAX.into());
            x11::xlib::XClearWindow(raw_display, xid);
            x11::xlib::XFlush(raw_display);
        }
        tracing::info!("web preview: native X11 background made opaque");
        Ok(())
    }

    fn title_bar_integration() -> TitleBarIntegration {
        native_title_bar()
    }
}

#[cfg(test)]
mod tests {
    use gpui::Keystroke;

    use super::*;

    #[test]
    fn command_authored_shortcuts_use_control() {
        let mut keystroke = Keystroke::parse("cmd-s").expect("valid shortcut");
        LinuxBackend::normalize_keystroke(&mut keystroke);
        assert_eq!(keystroke.unparse(), "ctrl-s");
    }
}
