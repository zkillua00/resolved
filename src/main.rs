#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::{borrow::Cow, cell::Cell, rc::Rc};

use gpui::{
    App, AppContext as _, Application, AssetSource, Bounds, Entity, SharedString, WindowBounds,
    WindowOptions, px, size,
};
use gpui_component::{Root, WindowExt as _};

mod app;
mod brand;
mod code_editor;
mod control_server;
mod control_tools;
mod core;
mod debug_overlay;
mod documentation_body;
mod documentation_intelligence;
mod documentation_references;
mod editor_util;
mod gateway;
mod instance_guard;
mod io;
mod platform;
mod request_dirty;
mod script_intelligence;
mod shortcuts;
mod snippet_intelligence;
mod syntax_languages;
mod template_intelligence;
mod theme;
mod tls;
mod typescript_service;
mod update_download;
mod update_install;
mod web_preview;

use app::ApiTester;
use brand::{ICON_ASSET_PATH, PRODUCT_NAME};
use core::DatabaseStore;
use instance_guard::InstanceGuard;
use shortcuts::{
    ActivateNextRequestTab, ActivatePreviousRequestTab, CheckForUpdates, CloseRequestTab,
    FocusRequestUrl, FormatRawBody, NewRequestTab, QuickSendWebSocketTemplate, QuitApp,
    SaveRequest, SaveRequestAs, SendOrCancelRequest, ShowAboutResolved, ShowCollections,
    ShowEnvironments, ShowHistory, ShowSettings, ToggleMetrics, ToggleNavigation, ZoomEditorIn,
    ZoomEditorOut, ZoomEditorReset, ZoomUiIn, ZoomUiOut, ZoomUiReset,
};

struct AppAssets;

/// Closing is a foreground continuation, never a synchronous wait for the
/// worker. Recheck after the barrier because edits may arrive while it drains.
fn request_app_exit(view: &gpui::WeakEntity<ApiTester>, pending: &Rc<Cell<bool>>, cx: &mut App) {
    if pending.replace(true) {
        return;
    }
    let view = view.clone();
    let pending = Rc::clone(pending);
    let download = match view.update(cx, |view, cx| {
        let download = view.prepare_update_download_exit(cx)?;
        view.flush_local_state(cx);
        view.local_persistence_ready_to_close(cx);
        Ok(download)
    }) {
        Ok(Ok(download)) => download,
        Ok(Err(())) => {
            pending.set(false);
            return;
        }
        Err(error) => {
            tracing::error!("could not begin flushing local state: {error}");
            pending.set(false);
            return;
        }
    };
    let barrier = io::flush();
    cx.spawn(async move |cx| {
        // GPUI's final quit hook has a short deadline. Let the updater cancel,
        // clean its transport/partial files, and reap before entering that hook.
        if let Some(mut finished) = download {
            while !*finished.borrow_and_update() {
                if finished.changed().await.is_err() {
                    break;
                }
            }
        }
        let result = barrier.await;
        let _ = cx.update(|cx| {
            pending.set(false);
            if let Err(error) = result {
                tracing::error!("could not drain I/O before quit: {error}");
                let _ = view.update(cx, |view, cx| view.finish_update_download_exit(cx));
                return;
            }
            let saved = view
                .update(cx, |view, cx| {
                    view.finish_update_download_exit(cx);
                    view.flush_local_state(cx) && view.local_persistence_ready_to_close(cx)
                })
                .unwrap_or_else(|error| {
                    tracing::error!("could not verify local state before quit: {error}");
                    false
                });
            if saved {
                cx.quit();
            }
        });
    })
    .detach();
}

fn request_update_restart(view: &gpui::WeakEntity<ApiTester>, cx: &mut App) {
    let (cancel, cancelled) = tokio::sync::watch::channel(false);
    let (finished, completion) = tokio::sync::watch::channel(false);
    let request = view.update(cx, |view, cx| {
        let request = view.begin_update_restart(cancel, completion, cx)?;
        view.flush_local_state(cx);
        view.local_persistence_ready_to_close(cx);
        Some(request)
    });
    let Ok(Some(request)) = request else { return };
    let view = view.clone();
    let barrier = io::flush();
    cx.spawn(async move |cx| {
        if barrier.await.is_err() {
            let _ = cx.update(|cx| {
                view.update(cx, |view, cx| {
                    view.fail_update_restart(
                        "Could not save local state before updating.".to_owned(),
                        cx,
                    );
                })
            });
            return;
        }
        let task = cx.update(|cx| {
            view.update(cx, |view, cx| {
                if !view.flush_local_state(cx) || !view.local_persistence_ready_to_close(cx) {
                    view.fail_update_restart(
                        "Finish active work and save local changes before restarting.".to_owned(),
                        cx,
                    );
                    return None;
                }
                Some(view.spawn_update_installer(request, cancelled, finished))
            })
        });
        let Ok(Ok(Some(task))) = task else { return };
        let ticket = match task.await {
            Ok(Ok(ticket)) => ticket,
            result => {
                let message = match result {
                    Ok(Err(error)) => error,
                    Err(_) => "The installer supervisor stopped before readiness.".to_owned(),
                    _ => unreachable!(),
                };
                let _ = cx
                    .update(|cx| view.update(cx, |view, cx| view.fail_update_restart(message, cx)));
                return;
            }
        };
        // Preparation is asynchronous. Drain again because edits may have arrived
        // while the helper revalidated and staged the signed application.
        let flushed = cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.flush_local_state(cx);
                view.local_persistence_ready_to_close(cx);
            })
        });
        if flushed.is_err() || io::flush().await.is_err() {
            drop(ticket);
            let _ = cx.update(|cx| {
                view.update(cx, |view, cx| {
                    view.fail_update_restart(
                        "Could not finish saving before restart.".to_owned(),
                        cx,
                    );
                })
            });
            return;
        }
        let _ = cx.update(|cx| {
            let committed = view
                .update(cx, |view, cx| {
                    if !view.flush_local_state(cx) || !view.local_persistence_ready_to_close(cx) {
                        drop(ticket);
                        view.fail_update_restart(
                            "Local work changed during update preparation. Save it and retry."
                                .to_owned(),
                            cx,
                        );
                        return false;
                    }
                    match ticket.commit() {
                        Ok(()) => {
                            view.commit_update_restart(cx);
                            true
                        }
                        Err(error) => {
                            view.fail_update_restart(error, cx);
                            false
                        }
                    }
                })
                .unwrap_or(false);
            // No await or event dispatch occurs between the final durability
            // check, nonblocking commit write, and normal app termination.
            if committed {
                cx.quit();
            }
        });
    })
    .detach();
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
    register!(QuickSendWebSocketTemplate, on_quick_send_websocket_template);
    register!(SaveRequest, on_save_request);
    register!(SaveRequestAs, on_save_request_as);
    register!(FocusRequestUrl, on_focus_request_url);
    register!(FormatRawBody, on_format_raw_body);
    register!(ShowCollections, on_show_collections);
    register!(ShowEnvironments, on_show_environments);
    register!(ShowHistory, on_show_history);
    register!(ShowAboutResolved, on_show_about_resolved);
    register!(CheckForUpdates, on_check_for_updates);
    register!(ShowSettings, on_show_settings);
    register!(ToggleNavigation, on_toggle_navigation);
    register!(ToggleMetrics, on_toggle_metrics);
    register!(ZoomUiIn, on_zoom_ui_in);
    register!(ZoomUiOut, on_zoom_ui_out);
    register!(ZoomUiReset, on_zoom_ui_reset);
    register!(ZoomEditorIn, on_zoom_editor_in);
    register!(ZoomEditorOut, on_zoom_editor_out);
    register!(ZoomEditorReset, on_zoom_editor_reset);
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
            "icons/inspector.svg" => include_bytes!("../assets/icons/inspector.svg"),
            "icons/layout-dashboard.svg" => {
                include_bytes!("../assets/icons/layout-dashboard.svg")
            }
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

/// Send native-integration diagnostics to the active platform backend.
pub(crate) fn log_diagnostic(message: &str) {
    platform::log_diagnostic(message);
}

fn main() {
    if let Err(error) = platform::launch_blocker() {
        eprintln!("{error}");
        std::process::exit(1);
    }

    // Platform selection and process hooks must run before GPUI creates its
    // display client or background executors.
    platform::install_runtime_hooks();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "api_tester=info".into()),
        )
        .init();

    if let Err(error) = tls::install_crypto_provider() {
        eprintln!("{PRODUCT_NAME} {error}.");
        std::process::exit(1);
    }

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

            let initial_state = ApiTester::load_initial_state();
            cx.spawn(async move |cx| {
                let initial_state = match initial_state.await {
                    Ok(state) => state,
                    Err(error) => {
                        tracing::error!("could not load initial application state: {error}");
                        let _ = cx.update(|cx| cx.quit());
                        return;
                    }
                };
                let _ = cx.update(move |cx| {
                    let bounds = Bounds::centered(None, size(px(1440.0), px(900.0)), cx);
                    let mut window_options = WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        window_min_size: Some(size(px(1100.0), px(720.0))),
                        titlebar: Some(gpui::TitlebarOptions {
                            title: Some(PRODUCT_NAME.into()),
                            appears_transparent: true,
                            traffic_light_position: Some(gpui::point(px(16.0), px(16.0))),
                        }),
                        ..Default::default()
                    };
                    platform::configure_main_window(&mut window_options);
                    match cx.open_window(window_options, |window, cx| {
                        let view = cx.new(|cx| {
                            ApiTester::new_with_initial_state(
                                base_key_bindings,
                                initial_state,
                                window,
                                cx,
                            )
                        });
                        register_app_action_handlers(&view, cx);
                        platform::configure_menus(cx);
                        view.update(cx, |view, cx| view.start_update_recovery(cx));
                        let exit_pending = Rc::new(Cell::new(false));
                        let close_pending = Rc::clone(&exit_pending);
                        let view_for_close = view.downgrade();
                        window.on_window_should_close(cx, move |_, cx| {
                            request_app_exit(&view_for_close, &close_pending, cx);
                            false
                        });
                        let view_for_quit = view.downgrade();
                        cx.on_action(move |_: &QuitApp, cx| {
                            request_app_exit(&view_for_quit, &exit_pending, cx);
                        });
                        cx.new(|cx| Root::new(view, window, cx))
                    }) {
                        Ok(_) => {}
                        Err(error) => {
                            eprintln!("{PRODUCT_NAME} could not open its main window: {error}");
                            std::process::exit(1);
                        }
                    }

                    cx.activate(true);
                });
            })
            .detach();
        });
}

#[cfg(test)]
mod asset_tests {
    use super::*;

    #[test]
    fn navigation_icons_are_embedded() {
        for path in ["icons/inspector.svg", "icons/layout-dashboard.svg"] {
            assert!(
                AppAssets.load(path).expect("load embedded asset").is_some(),
                "{path} must be bundled or its navigation slot renders blank"
            );
        }
    }
}
