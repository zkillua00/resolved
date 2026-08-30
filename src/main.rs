#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::borrow::Cow;

use gpui::{
    App, AppContext as _, Application, AssetSource, Bounds, Entity, Menu, MenuItem, SharedString,
    WindowBounds, WindowOptions, px, size,
};
#[cfg(target_os = "macos")]
use gpui::SystemMenuType;
use gpui_component::{Root, WindowExt as _};

mod app;
mod brand;
mod code_editor;
mod core;
mod debug_overlay;
mod editor_util;
mod instance_guard;
mod platform;
mod request_dirty;
mod script_intelligence;
mod shortcuts;
mod snippet_intelligence;
mod syntax_languages;
mod template_intelligence;
mod theme;
mod typescript_service;
mod web_preview;

use app::ApiTester;
use brand::{ICON_ASSET_PATH, PRODUCT_NAME};
use core::DatabaseStore;
use instance_guard::InstanceGuard;
use shortcuts::{
    ActivateNextRequestTab, ActivatePreviousRequestTab, CloseRequestTab, FocusRequestUrl,
    FormatRawBody, NewRequestTab, QuitApp, SaveRequest, SaveRequestAs, SendOrCancelRequest,
    ShowCollections, ShowEnvironments, ShowHistory, ShowSettings, ToggleMetrics, ToggleNavigation,
};

struct AppAssets;

#[cfg(target_os = "macos")]
fn configure_menus(cx: &mut App) {
    cx.set_menus(vec![
        Menu {
            name: PRODUCT_NAME.into(),
            items: vec![
                MenuItem::action("Settings…", ShowSettings),
                MenuItem::separator(),
                MenuItem::os_submenu("Services", SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action(format!("Quit {PRODUCT_NAME}"), QuitApp),
            ],
        },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("New Request Tab", NewRequestTab),
                MenuItem::action("Close Active Tab", CloseRequestTab),
                MenuItem::separator(),
                MenuItem::action("Save Request", SaveRequest),
                MenuItem::action("Save Request As…", SaveRequestAs),
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

#[cfg(not(target_os = "macos"))]
fn configure_menus(cx: &mut App) {
    // The Services submenu and top-level app menu are macOS concepts; on
    // Windows these menus surface through gpui-component's AppMenuBar.
    cx.set_menus(vec![
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("Settings…", ShowSettings),
                MenuItem::separator(),
                MenuItem::action("New Request Tab", NewRequestTab),
                MenuItem::action("Close Active Tab", CloseRequestTab),
                MenuItem::separator(),
                MenuItem::action("Save Request", SaveRequest),
                MenuItem::action("Save Request As…", SaveRequestAs),
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

fn register_app_action_handlers(view: &Entity<ApiTester>, cx: &mut App) {
    macro_rules! register {
        ($action:ty, $handler:ident) => {{
            let view = view.downgrade();
            cx.on_action(move |action: &$action, cx| {
                let Some(window_handle) = cx.active_window() else {
                    tracing::error!(
                        "could not dispatch {} because there is no active window",
                        stringify!($action)
                    );
                    return;
                };
                let action = action.clone();
                let view = view.clone();
                // Key and menu actions are dispatched while the active window
                // is already on GPUI's update stack. Wait until the end of
                // that effect cycle before borrowing the window again.
                cx.defer(move |cx| {
                    let result = window_handle.update(cx, |_, window, cx| {
                        if window.has_active_dialog(cx) {
                            return Ok(());
                        }
                        view.update(cx, |view, cx| {
                            view.cancel_shortcut_recording(cx);
                            view.$handler(&action, window, cx);
                        })
                    });
                    match result {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => tracing::error!(
                            "could not dispatch {} to {PRODUCT_NAME}: {error}",
                            stringify!($action)
                        ),
                        Err(error) => tracing::error!(
                            "could not update {PRODUCT_NAME} window for {}: {error}",
                            stringify!($action)
                        ),
                    }
                });
            });
        }};
    }

    register!(NewRequestTab, on_new_request_tab);
    register!(CloseRequestTab, on_close_request_tab);
    register!(ActivateNextRequestTab, on_activate_next_request_tab);
    register!(ActivatePreviousRequestTab, on_activate_previous_request_tab);
    register!(SendOrCancelRequest, on_send_or_cancel_request);
    register!(SaveRequest, on_save_request);
    register!(SaveRequestAs, on_save_request_as);
    register!(FocusRequestUrl, on_focus_request_url);
    register!(FormatRawBody, on_format_raw_body);
    register!(ShowCollections, on_show_collections);
    register!(ShowEnvironments, on_show_environments);
    register!(ShowHistory, on_show_history);
    register!(ShowSettings, on_show_settings);
    register!(ToggleNavigation, on_toggle_navigation);
    register!(ToggleMetrics, on_toggle_metrics);
}

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        let bytes: &'static [u8] = match path {
            ICON_ASSET_PATH => include_bytes!("../assets/brand/resolved-runtime.png"),
            "icons/folder-open.svg" => include_bytes!("../assets/icons/folder-open.svg"),
            "icons/gallery-vertical-end.svg" => {
                include_bytes!("../assets/icons/gallery-vertical-end.svg")
            }
            "icons/globe.svg" => include_bytes!("../assets/icons/globe.svg"),
            "icons/palette.svg" => include_bytes!("../assets/icons/palette.svg"),
            "icons/settings-2.svg" => include_bytes!("../assets/icons/settings-2.svg"),
            "icons/chart-pie.svg" => include_bytes!("../assets/icons/chart-pie.svg"),
            "icons/case-sensitive.svg" => include_bytes!("../assets/icons/case-sensitive.svg"),
            "icons/check.svg" => include_bytes!("../assets/icons/check.svg"),
            "icons/chevron-down.svg" => include_bytes!("../assets/icons/chevron-down.svg"),
            "icons/chevron-left.svg" => include_bytes!("../assets/icons/chevron-left.svg"),
            "icons/chevron-right.svg" => include_bytes!("../assets/icons/chevron-right.svg"),
            "icons/circle-x.svg" => include_bytes!("../assets/icons/circle-x.svg"),
            "icons/close.svg" => include_bytes!("../assets/icons/close.svg"),
            "icons/eye.svg" => include_bytes!("../assets/icons/eye.svg"),
            "icons/eye-off.svg" => include_bytes!("../assets/icons/eye-off.svg"),
            "icons/ellipsis-vertical.svg" => {
                include_bytes!("../assets/icons/ellipsis-vertical.svg")
            }
            "icons/external-link.svg" => include_bytes!("../assets/icons/external-link.svg"),
            "icons/folder-closed.svg" => include_bytes!("../assets/icons/folder-closed.svg"),
            "icons/window-close.svg" => include_bytes!("../assets/icons/window-close.svg"),
            "icons/window-maximize.svg" => include_bytes!("../assets/icons/window-maximize.svg"),
            "icons/window-minimize.svg" => include_bytes!("../assets/icons/window-minimize.svg"),
            "icons/window-restore.svg" => include_bytes!("../assets/icons/window-restore.svg"),
            "icons/plus.svg" => include_bytes!("../assets/icons/plus.svg"),
            "icons/replace.svg" => include_bytes!("../assets/icons/replace.svg"),
            "icons/search.svg" => include_bytes!("../assets/icons/search.svg"),
            _ => return Ok(None),
        };
        Ok(Some(Cow::Borrowed(bytes)))
    }

    fn list(&self, _path: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}

/// Append a diagnostic line to the Windows panic log.
///
/// The packaged process has no stderr and swallows many webview/OS errors
/// into `Result`s, so subsystems report here; a `None` from any caller is
/// usually the only trace of a packaged-only failure. No-op elsewhere.
pub(crate) fn log_diagnostic(message: &str) {
    #[cfg(target_os = "windows")]
    {
        use std::io::Write as _;
        let path = DatabaseStore::default_path().with_file_name("api-tester-panic.log");
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(file, "{} {message}", chrono::Local::now().to_rfc3339());
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = message;
    }
}

fn main() {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    if let Err(error) = platform::launch_blocker() {
        eprintln!("{error}");
        std::process::exit(1);
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "api_tester=info".into()),
        )
        .init();

    // A packaged GUI process has no stderr, so a Rust panic in it would die
    // as an opaque fast-fail in Event Viewer. Route panics to a log file next
    // to the data directory instead. Windows only; macOS keeps the debugger
    // workflow (and this build's release profile strips symbols anyway).
    #[cfg(target_os = "windows")]
    std::panic::set_hook(Box::new(|info| {
        use std::io::Write as _;
        let path = DatabaseStore::default_path()
            .with_file_name("api-tester-panic.log");
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(
                file,
                "{}\n{info}\nbacktrace:\n{}",
                chrono::Local::now().to_rfc3339(),
                std::backtrace::Backtrace::force_capture()
            );
        }
        eprintln!("{info}");
    }));

    let lock_path = DatabaseStore::default_path().with_file_name("api-tester.lock");
    let _instance_guard = match InstanceGuard::acquire(&lock_path) {
        Ok(guard) => guard,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            eprintln!("{PRODUCT_NAME} is already running.");
            return;
        }
        Err(error) => {
            eprintln!(
                "{PRODUCT_NAME} could not lock its workspace at {}: {error}",
                lock_path.display()
            );
            std::process::exit(1);
        }
    };

    // GPUI's DirectComposition visual is composited above native child HWNDs
    // such as the WebView2 response preview. The child still receives input,
    // but its pixels are hidden behind the GPUI surface. Select GPUI's HWND
    // swap-chain renderer before the Windows platform is initialized so the
    // native child participates in normal window z-order and clipping.
    #[cfg(target_os = "windows")]
    // SAFETY: this runs on the single startup thread before `Application`
    // creates GPUI's platform or any worker threads.
    unsafe {
        std::env::set_var("GPUI_DISABLE_DIRECT_COMPOSITION", "1");
    }

    Application::new()
        .with_assets(AppAssets)
        .run(|cx: &mut App| {
            gpui_component::init(cx);
            syntax_languages::register();
            let base_key_bindings = shortcuts::capture_base_key_bindings(cx);
            theme::configure(cx);

            cx.on_window_closed(|cx| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            let bounds = Bounds::centered(None, size(px(1440.0), px(900.0)), cx);
            match cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(1100.0), px(720.0))),
                    titlebar: Some(gpui::TitlebarOptions {
                        title: Some(PRODUCT_NAME.into()),
                        appears_transparent: true,
                        traffic_light_position: Some(gpui::point(px(16.0), px(16.0))),
                    }),
                    ..Default::default()
                },
                |window, cx| {
                    let view = cx.new(|cx| ApiTester::new(base_key_bindings.clone(), window, cx));
                    register_app_action_handlers(&view, cx);
                    configure_menus(cx);
                    let view_for_close = view.downgrade();
                    window.on_window_should_close(cx, move |_, cx| {
                        view_for_close
                            .update(cx, |view, cx| view.flush_local_state(cx))
                            .unwrap_or_else(|error| {
                                tracing::error!(
                                    "could not flush local state before close: {error}"
                                );
                                true
                            })
                    });
                    let view_for_quit = view.downgrade();
                    cx.on_action(move |_: &QuitApp, cx| {
                        let saved = view_for_quit
                            .update(cx, |view, cx| view.flush_local_state(cx))
                            .unwrap_or_else(|error| {
                                tracing::error!(
                                    "could not flush local state before quit: {error}"
                                );
                                true
                            });
                        if saved {
                            cx.quit();
                        }
                    });
                    cx.new(|cx| Root::new(view, window, cx))
                },
            ) {
                Ok(_) => {}
                Err(error) => {
                    eprintln!("{PRODUCT_NAME} could not open its main window: {error}");
                    std::process::exit(1);
                }
            }

            cx.activate(true);
        });
}
