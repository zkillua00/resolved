use std::borrow::Cow;

use gpui::{
    App, AppContext as _, Application, AssetSource, Bounds, SharedString, WindowBounds,
    WindowOptions, px, size,
};
use gpui_component::Root;

mod app;
mod code_editor;
mod core;
mod debug_overlay;
mod instance_guard;
mod request_dirty;
mod script_intelligence;
mod template_intelligence;
mod theme;
mod web_preview;

use app::ApiTester;
use core::DatabaseStore;
use instance_guard::InstanceGuard;

struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        let bytes: &'static [u8] = match path {
            "icons/folder-open.svg" => include_bytes!("../assets/icons/folder-open.svg"),
            "icons/gallery-vertical-end.svg" => {
                include_bytes!("../assets/icons/gallery-vertical-end.svg")
            }
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
            eprintln!("API Tester is already running.");
            return;
        }
        Err(error) => panic!(
            "failed to lock API Tester workspace at {}: {error}",
            lock_path.display()
        ),
    };

    Application::new()
        .with_assets(AppAssets)
        .run(|cx: &mut App| {
            gpui_component::init(cx);
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
                        title: Some("API Tester".into()),
                        appears_transparent: true,
                        traffic_light_position: Some(gpui::point(px(16.0), px(16.0))),
                    }),
                    ..Default::default()
                },
                |window, cx| {
                    let view = cx.new(|cx| ApiTester::new(window, cx));
                    cx.new(|cx| Root::new(view, window, cx))
                },
            )
            .expect("failed to open main window");

            cx.activate(true);
        });
}
