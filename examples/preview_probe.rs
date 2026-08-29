//! Probe 2: reproduce Preview's exact flow — webview created inside a
//! deferred click handler after the window is interactive, then shown and
//! given HTML. Logs to %TEMP%\resolved-preview-probe.log. Run:
//! `cargo run --release --example preview_probe`
use std::sync::OnceLock;

use gpui::{
    App, AppContext as _, Application, Bounds, ClickEvent, Context, Entity, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, Window,
    WindowBounds, WindowOptions, div, px, size,
};
use gpui_wry::WebView as GpuiWebView;
use wry::{NewWindowResponse, WebViewBuilder};

static LOG_PATH: OnceLock<String> = OnceLock::new();

fn log(message: &str) {
    let Some(path) = LOG_PATH.get() else { return };
    use std::io::Write as _;
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{message}");
    }
}

struct ProbeRoot {
    webview: Option<Entity<GpuiWebView>>,
    clicked: bool,
}

impl Render for ProbeRoot {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let button = div()
            .id("make-preview")
            .child("click to create webview")
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                log("click handler entered");
                let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    WebViewBuilder::new()
                        .with_html("<p>probe click</p>")
                        .with_incognito(true)
                        .with_javascript_disabled()
                        .with_devtools(false)
                        .with_autoplay(false)
                        .with_drag_drop_handler(|_| true)
                        .with_navigation_handler(|url| url.starts_with("about:blank"))
                        .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
                        .with_download_started_handler(|_, _| false)
                        .build_as_child(window)
                }));
                match built {
                    Ok(Ok(webview)) => {
                        log("webview built OK from click");
                        // Nesting matches HtmlPreview::new's pattern: the
                        // outer cx.new enters the parent's Context, where
                        // creating the child is legal.
                        let managed = cx.new(|cx| GpuiWebView::new(webview, window, cx));
                        this.webview = Some(managed);
                        log("gpui entity created from click");
                    }
                    Ok(Err(error)) => log(&format!("webview build ERROR {error}")),
                    Err(_) => log("webview build PANICKED (caught)"),
                }
                this.clicked = true;
                cx.notify();
            }));

        let mut root = div().flex().flex_col().size_full();
        root = root.child(button);
        if let Some(webview) = self.webview.clone() {
            root = root.child(webview);
        }
        root
    }
}

fn main() {
    let path = std::env::temp_dir()
        .join("resolved-preview-probe.log")
        .to_string_lossy()
        .to_string();
    LOG_PATH.set(path.clone()).ok();
    std::fs::write(&path, b"").ok();

    std::panic::set_hook(Box::new(move |info| {
        log(&format!("PANIC {info}"));
    }));

    Application::new().run(|cx: &mut App| {
        log("app started");
        let bounds = Bounds::centered(None, size(px(800.), px(600.)), cx);
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| ProbeRoot { webview: None, clicked: false }),
        );
        log(&format!("open_window -> {opened:?}"));
        cx.activate(true);

        // Self-terminate so the probe never lingers.
        cx.spawn(async move |cx| {
            gpui::Timer::after(std::time::Duration::from_secs(30)).await;
            log("probe timeout (no click)");
            let _ = cx.update(|cx| cx.quit());
        })
        .detach();
    });
}
