//! Compile-time desktop platform plug-ins.
//!
//! The application and feature modules call this facade instead of selecting
//! operating systems themselves. Each backend owns its native launch policy,
//! menus, window integration, shortcut convention, diagnostics, and webview
//! behavior. Only this module selects the active backend with `cfg`.

use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};

use gpui::{App, Keystroke, Menu, MenuItem, Pixels, Task, WindowOptions, px};
use wry::{WebView, WebViewBuilder};

use crate::shortcuts::{
    CloseRequestTab, NewRequestTab, QuitApp, SaveRequest, SaveRequestAs, SendOrCancelRequest,
    ShowSettings,
};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
type ActiveBackend = linux::LinuxBackend;
#[cfg(target_os = "macos")]
type ActiveBackend = macos::MacOsBackend;
#[cfg(target_os = "windows")]
type ActiveBackend = windows::WindowsBackend;

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
compile_error!("Resolved currently supports Linux, macOS, and Windows desktop targets");

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TitleBarIntegration {
    pub leading_inset: Pixels,
    pub trailing_inset: Pixels,
    pub client_controls: bool,
    pub draggable_content: bool,
}

pub(crate) trait PlatformBackend {
    fn launch_blocker() -> Result<(), String> {
        Ok(())
    }

    fn configure_menus(cx: &mut App);

    fn install_runtime_hooks() {}

    fn log_diagnostic(_message: &str) {}

    fn normalize_keystroke(_keystroke: &mut Keystroke) {}

    fn configure_main_window(_options: &mut WindowOptions) {}

    fn title_bar_integration() -> TitleBarIntegration;

    fn configure_webview<'a>(builder: WebViewBuilder<'a>) -> WebViewBuilder<'a> {
        builder.with_incognito(true)
    }

    fn start_webview_runtime(_cx: &App) -> Result<Option<Task<()>>, String> {
        Ok(None)
    }

    fn prepare_preview_webview(_webview: &WebView) -> Result<(), String> {
        Ok(())
    }

    fn load_preview_document(
        webview: &WebView,
        document: &str,
    ) -> wry::Result<Option<Arc<AtomicBool>>> {
        webview.load_html(document)?;
        Ok(None)
    }

    fn preview_data_directory() -> Option<PathBuf> {
        None
    }
}

pub(crate) fn launch_blocker() -> Result<(), String> {
    ActiveBackend::launch_blocker()
}

pub(crate) fn configure_menus(cx: &mut App) {
    ActiveBackend::configure_menus(cx);
}

pub(crate) fn install_runtime_hooks() {
    ActiveBackend::install_runtime_hooks();
}

pub(crate) fn log_diagnostic(message: &str) {
    ActiveBackend::log_diagnostic(message);
}

pub(crate) fn normalize_keystroke(keystroke: &mut Keystroke) {
    ActiveBackend::normalize_keystroke(keystroke);
}

pub(crate) fn configure_main_window(options: &mut WindowOptions) {
    ActiveBackend::configure_main_window(options);
}

pub(crate) fn title_bar_integration() -> TitleBarIntegration {
    ActiveBackend::title_bar_integration()
}

pub(crate) fn configure_webview(builder: WebViewBuilder<'_>) -> WebViewBuilder<'_> {
    ActiveBackend::configure_webview(builder)
}

pub(crate) fn start_webview_runtime(cx: &App) -> Result<Option<Task<()>>, String> {
    ActiveBackend::start_webview_runtime(cx)
}

pub(crate) fn prepare_preview_webview(webview: &WebView) -> Result<(), String> {
    ActiveBackend::prepare_preview_webview(webview)
}

pub(crate) fn load_preview_document(
    webview: &WebView,
    document: &str,
) -> wry::Result<Option<Arc<AtomicBool>>> {
    ActiveBackend::load_preview_document(webview, document)
}

pub(crate) fn preview_data_directory() -> Option<PathBuf> {
    ActiveBackend::preview_data_directory()
}

fn configure_desktop_menus(cx: &mut App) {
    cx.set_menus(vec![
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("Settings…", ShowSettings),
                MenuItem::separator(),
                MenuItem::action("New Request Tab", NewRequestTab),
                MenuItem::action("Close Active Tab", CloseRequestTab),
                MenuItem::separator(),
                MenuItem::action("Save", SaveRequest),
                MenuItem::action("Save As…", SaveRequestAs),
                MenuItem::separator(),
                MenuItem::action("Quit", QuitApp),
            ],
        },
        Menu {
            name: "Request".into(),
            items: vec![MenuItem::action(
                "Send or Cancel Request",
                SendOrCancelRequest,
            )],
        },
    ]);
}

fn control_shortcut(keystroke: &mut Keystroke) {
    if keystroke.modifiers.platform {
        keystroke.modifiers.platform = false;
        keystroke.modifiers.control = true;
    }
}

fn native_title_bar() -> TitleBarIntegration {
    TitleBarIntegration {
        leading_inset: px(12.),
        trailing_inset: px(24.),
        client_controls: false,
        draggable_content: false,
    }
}
