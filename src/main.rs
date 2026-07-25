use gpui::{App, AppContext as _, Application, Bounds, WindowBounds, WindowOptions, px, size};
use gpui_component::Root;

mod app;
mod core;
mod web_preview;

use app::ApiTester;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "api_tester=info".into()),
        )
        .init();

    Application::new().run(|cx: &mut App| {
        gpui_component::init(cx);

        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        let bounds = Bounds::centered(None, size(px(1180.0), px(760.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(900.0), px(600.0))),
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
