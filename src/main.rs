use std::borrow::Cow;

use gpui::{
    App, AppContext as _, Application, AssetSource, Bounds, Entity, Menu, MenuItem, SharedString,
    SystemMenuType, WindowBounds, WindowOptions, px, size,
};
use gpui_component::{Root, WindowExt as _};

mod app;
mod brand;
mod code_editor;
mod core;
mod debug_overlay;
mod instance_guard;
mod request_dirty;
mod script_intelligence;
mod shortcuts;
mod template_intelligence;
mod theme;
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

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "api_tester=info".into()),
        )
        .init();

    let lock_path = DatabaseStore::default_path().with_file_name("api-tester.lock");
    let _instance_guard = match InstanceGuard::acquire(&lock_path) {
        Ok(guard) => guard,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            eprintln!("{PRODUCT_NAME} is already running.");
            return;
        }
        Err(error) => panic!(
            "failed to lock {PRODUCT_NAME} workspace at {}: {error}",
            lock_path.display()
        ),
    };

    Application::new()
        .with_assets(AppAssets)
        .run(|cx: &mut App| {
            gpui_component::init(cx);
            let base_key_bindings = shortcuts::capture_base_key_bindings(cx);
            theme::configure(cx);

            cx.on_window_closed(|cx| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            let bounds = Bounds::centered(None, size(px(1440.0), px(900.0)), cx);
            cx.open_window(
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
                            .unwrap_or(true)
                    });
                    let view_for_quit = view.downgrade();
                    cx.on_action(move |_: &QuitApp, cx| {
                        let saved = view_for_quit
                            .update(cx, |view, cx| view.flush_local_state(cx))
                            .unwrap_or(true);
                        if saved {
                            cx.quit();
                        }
                    });
                    cx.new(|cx| Root::new(view, window, cx))
                },
            )
            .expect("failed to open main window");

            cx.activate(true);
        });
}
