use gpui::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _, Window,
    Timer, div,
};
use gpui_wry::WebView as GpuiWebView;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use wry::{NewWindowResponse, WebContext, WebViewBuilder};

/// The link-preview policy shared by every webview backend.
///
/// Only the Darwin builder exposes a real link-preview switch; the other
/// backends have no such behavior, so the policy compiles to a no-op there.
trait PreviewLinkPolicy: Sized {
    fn with_allow_link_preview(self, allow: bool) -> Self;
}

#[cfg(target_os = "macos")]
impl PreviewLinkPolicy for WebViewBuilder<'_> {
    fn with_allow_link_preview(self, allow: bool) -> Self {
        wry::WebViewBuilderExtDarwin::with_allow_link_preview(self, allow)
    }
}

#[cfg(not(target_os = "macos"))]
impl PreviewLinkPolicy for WebViewBuilder<'_> {
    fn with_allow_link_preview(self, _allow: bool) -> Self {
        self
    }
}

const CSP_META: &str = r#"<meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'none'; img-src data: blob:; style-src 'unsafe-inline'; font-src data:; media-src data: blob:; connect-src 'none'; frame-src 'none'; object-src 'none'; form-action 'none'; base-uri 'none'>"#;

pub struct HtmlPreview {
    webview: Option<Entity<GpuiWebView>>,
}

impl HtmlPreview {
    pub fn new(html: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // WebView2 keeps its user-data folder beside the host executable by
        // default; the Windows Store deployment puts that under
        // `C:\Program Files\WindowsApps\...`, which is read-only, so
        // environment creation fails and the pane renders nothing. Point the
        // web context at the app's own data directory on Windows (the loose
        // macOS flow is unaffected; WKWebView needs no folder there).
        // `with_incognito(true)` below still keeps the session in WebView2's
        // private mode on top of that folder.
        let mut web_context = WebContext::new(preview_data_directory());
        // Build the WKWebView before creating the entity so a creation failure
        // (rare but real: window-server/display errors) degrades to an unavailable
        // preview instead of panicking the whole app on a user action. The
        // packaged Windows process has no stderr, so the failure reason is
        // mirrored into the diagnostic log beside the data directory.
        // Prepare the captured response before creating the native child.
        // On Windows, WebView2's builder-time NavigateToString can report a
        // successful navigation yet finish with the default empty document.
        // Navigate only after gpui-wry owns the fully-created child below.
        let document = safe_html_document(html);
        crate::log_diagnostic(&format!(
            "web preview: initial document source_bytes={} document_bytes={}",
            html.len(),
            document.len()
        ));
        let webview = WebViewBuilder::new_with_web_context(&mut web_context)
            .with_incognito(true)
            // The child is created at the native default origin. Keep it
            // hidden until gpui-wry has assigned the preview pane's real
            // bounds, otherwise WebView2 flashes in the window's top-left
            // corner for its first frame.
            .with_visible(false)
            .with_background_color((255, 255, 255, 255))
            .with_javascript_disabled()
            .with_devtools(false)
            .with_autoplay(false)
            .with_allow_link_preview(false)
            .with_drag_drop_handler(|_| true)
            .with_navigation_handler(|url| url.starts_with("about:blank"))
            .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
            .with_download_started_handler(|_, _| false)
            .with_on_page_load_handler(|event, url| {
                let scheme = url.split_once(':').map_or("unknown", |(scheme, _)| scheme);
                let event = match event {
                    wry::PageLoadEvent::Started => "started",
                    wry::PageLoadEvent::Finished => "finished",
                };
                crate::log_diagnostic(&format!(
                    "web preview: page load {event} (scheme: {scheme})"
                ));
            })
            .build_as_child(window);
        let webview = match webview {
            Ok(webview) => {
                crate::log_diagnostic("web preview: webview created");
                Some(webview)
            }
            Err(error) => {
                crate::log_diagnostic(&format!("web preview: creation failed: {error}"));
                None
            }
        };
        // The preview is created only while its tab is active and is dropped
        // when hidden, so keep WebView2 visible through its first navigation
        // and nonzero layout. A hide/show cycle here can leave its child
        // composition surface black even though the document remains
        // interactive.
        let webview = webview.map(|raw| cx.new(|cx| GpuiWebView::new(raw, window, cx)));

        if let Some(initial_view) = webview.clone() {
            cx.spawn(async move |_, cx| {
                // Wait for gpui-wry's first prepaint to assign a nonempty pane
                // rectangle. Its logical visibility remains enabled so layout
                // proceeds while the native controller itself stays hidden.
                let mut stable_bounds = None;
                let mut stable_samples = 0;
                let mut positioned = false;
                for _ in 0..120 {
                    Timer::after(Duration::from_millis(16)).await;
                    match initial_view.update(cx, |view, _| view.bounds()) {
                        Ok(bounds) if bounds.is_empty() => {
                            stable_bounds = None;
                            stable_samples = 0;
                        }
                        Ok(bounds) => {
                            if stable_bounds == Some(bounds) {
                                stable_samples += 1;
                            } else {
                                stable_bounds = Some(bounds);
                                stable_samples = 1;
                            }
                            // A first nonempty layout can still be GPUI's
                            // provisional response-pane size. Require roughly
                            // 100 ms of unchanged bounds before exposing the
                            // native controller so none of that resize is
                            // visible to the user.
                            if stable_samples >= 6 {
                                positioned = true;
                                break;
                            }
                        }
                        Err(error) => {
                            crate::log_diagnostic(&format!(
                                "web preview: initial layout check failed: {error}"
                            ));
                            return;
                        }
                    }
                }
                if !positioned {
                    crate::log_diagnostic(
                        "web preview: initial layout never produced nonempty bounds",
                    );
                    return;
                }

                let result =
                    initial_view.update(cx, |view, _| load_document(view.raw(), &document));
                let ready = match result {
                    Ok(Ok(ready)) => {
                        crate::log_diagnostic("web preview: initial document submitted");
                        ready
                    }
                    Ok(Err(error)) => {
                        crate::log_diagnostic(&format!(
                            "web preview: initial document submission failed: {error}"
                        ));
                        return;
                    }
                    Err(error) => {
                        crate::log_diagnostic(&format!(
                            "web preview: initial document submission failed: {error}"
                        ));
                        return;
                    }
                };

                if let Some(ready) = ready {
                    let mut completed = false;
                    for _ in 0..120 {
                        if ready.load(Ordering::Acquire) {
                            completed = true;
                            break;
                        }
                        Timer::after(Duration::from_millis(16)).await;
                    }
                    if !completed {
                        crate::log_diagnostic(
                            "web preview: initial document did not complete before reveal",
                        );
                        return;
                    }
                }

                // ExecuteScript completion means the DOM write finished, not
                // that WebView2 has presented the corresponding compositor
                // frame. Wry does not expose WebView2's frame-presented event,
                // so retain the hidden controller through a short compositor
                // grace period before the final bounds-stability gate.
                Timer::after(Duration::from_millis(500)).await;

                // Host script execution and WebView2 painting are
                // asynchronous. Keep the controller hidden while the loaded
                // document gets its first layout, and require the host pane to
                // remain settled throughout that work before revealing it.
                stable_bounds = None;
                stable_samples = 0;
                positioned = false;
                for _ in 0..120 {
                    Timer::after(Duration::from_millis(16)).await;
                    match initial_view.update(cx, |view, _| view.bounds()) {
                        Ok(bounds) if bounds.is_empty() => {
                            stable_bounds = None;
                            stable_samples = 0;
                        }
                        Ok(bounds) => {
                            if stable_bounds == Some(bounds) {
                                stable_samples += 1;
                            } else {
                                stable_bounds = Some(bounds);
                                stable_samples = 1;
                            }
                            if stable_samples >= 6 {
                                positioned = true;
                                break;
                            }
                        }
                        Err(error) => {
                            crate::log_diagnostic(&format!(
                                "web preview: post-load layout check failed: {error}"
                            ));
                            return;
                        }
                    }
                }
                if !positioned {
                    crate::log_diagnostic(
                        "web preview: post-load layout never settled at nonempty bounds",
                    );
                    return;
                }

                if let Err(error) = initial_view.update(cx, |view, _| view.show()) {
                    crate::log_diagnostic(&format!(
                        "web preview: initial reveal failed: {error}"
                    ));
                }
            })
            .detach();
        }

        // Record the renderer's own view of the document after its first
        // layout. This deliberately logs only structure/layout metadata, not
        // response text. It distinguishes an empty/hidden DOM from a native
        // WebView2 composition failure when the preview surface is blank.
        cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_millis(750)).await;
            let Some(this) = this.upgrade() else {
                return;
            };
            this.update(cx, |this, cx| this.log_render_state(cx)).ok();
        })
        .detach();

        Self { webview }
    }

    fn log_render_state(&self, cx: &mut Context<Self>) {
        let Some(webview) = &self.webview else {
            return;
        };
        let script = r#"(() => {
            try {
                const root = document.documentElement;
                const body = document.body;
                const style = element => {
                    if (!element) return null;
                    const computed = getComputedStyle(element);
                    const rect = element.getBoundingClientRect();
                    return {
                        display: computed.display,
                        visibility: computed.visibility,
                        opacity: computed.opacity,
                        color: computed.color,
                        backgroundColor: computed.backgroundColor,
                        width: rect.width,
                        height: rect.height
                    };
                };
                return {
                    readyState: document.readyState,
                    titleLength: document.title.length,
                    textLength: body?.innerText?.length ?? -1,
                    childCount: body?.children?.length ?? -1,
                    elementCount: document.querySelectorAll('*').length,
                    markupLength: root?.outerHTML?.length ?? -1,
                    viewportWidth: innerWidth,
                    viewportHeight: innerHeight,
                    devicePixelRatio,
                    root: style(root),
                    body: style(body)
                };
            } catch (error) {
                return { error: String(error) };
            }
        })()"#;
        let result = webview.update(cx, |view, _| {
            view.raw().evaluate_script_with_callback(script, |state| {
                crate::log_diagnostic(&format!("web preview: render state {state}"));
            })
        });
        match result {
            Ok(()) => {}
            Err(error) => crate::log_diagnostic(&format!(
                "web preview: render-state probe failed: {error}"
            )),
        }
    }

    pub fn is_available(&self) -> bool {
        self.webview.is_some()
    }

    pub fn load_html(&mut self, html: &str, cx: &mut Context<Self>) -> wry::Result<()> {
        let Some(webview) = &self.webview else {
            // Creation failed; the caller has already surfaced the error.
            return Ok(());
        };
        let document = safe_html_document(html);
        crate::log_diagnostic(&format!(
            "web preview: reload document source_bytes={} document_bytes={}",
            html.len(),
            document.len()
        ));
        let webview = webview.clone();
        let ready = webview.update(cx, |view, _| {
            // Keep gpui-wry logically visible so prepaint continues updating
            // bounds, but hide the native controller while WebView2 replaces
            // and lays out the document.
            view.raw().set_visible(false)?;
            load_document(view.raw(), &document)
        })?;

        cx.spawn(async move |_, cx| {
            if let Some(ready) = ready {
                let mut completed = false;
                for _ in 0..120 {
                    if ready.load(Ordering::Acquire) {
                        completed = true;
                        break;
                    }
                    Timer::after(Duration::from_millis(16)).await;
                }
                if !completed {
                    crate::log_diagnostic(
                        "web preview: reload document did not complete before reveal",
                    );
                    return;
                }
            }

            Timer::after(Duration::from_millis(500)).await;

            let mut stable_bounds = None;
            let mut stable_samples = 0;
            for _ in 0..120 {
                Timer::after(Duration::from_millis(16)).await;
                match webview.update(cx, |view, _| view.bounds()) {
                    Ok(bounds) if bounds.is_empty() => {
                        stable_bounds = None;
                        stable_samples = 0;
                    }
                    Ok(bounds) => {
                        if stable_bounds == Some(bounds) {
                            stable_samples += 1;
                        } else {
                            stable_bounds = Some(bounds);
                            stable_samples = 1;
                        }
                        if stable_samples >= 6 {
                            if let Err(error) = webview.update(cx, |view, _| view.show()) {
                                crate::log_diagnostic(&format!(
                                    "web preview: reload reveal failed: {error}"
                                ));
                            }
                            return;
                        }
                    }
                    Err(error) => {
                        crate::log_diagnostic(&format!(
                            "web preview: reload layout check failed: {error}"
                        ));
                        return;
                    }
                }
            }
            crate::log_diagnostic("web preview: reload layout never settled at nonempty bounds");
        })
        .detach();

        cx.notify();
        Ok(())
    }

    pub fn hide(&mut self, cx: &mut Context<Self>) {
        if let Some(webview) = &self.webview {
            webview.update(cx, |view, _| view.hide());
        }
        cx.notify();
    }
}

/// Load a captured document without allowing its own scripts to execute.
///
/// WebView2's `NavigateToString` can successfully complete while leaving its
/// default empty document in a child WebView hosted by GPUI. Host-injected
/// script remains available when page JavaScript is disabled, so on Windows
/// replace the current document directly. Other engines keep their native
/// HTML loading path.
#[cfg(target_os = "windows")]
fn load_document(webview: &wry::WebView, document: &str) -> wry::Result<Option<Arc<AtomicBool>>> {
    let encoded = serde_json::to_string(document).expect("serializing an HTML string cannot fail");
    let ready = Arc::new(AtomicBool::new(false));
    let callback_ready = Arc::clone(&ready);
    webview.evaluate_script_with_callback(
        &format!("document.open();document.write({encoded});document.close();true"),
        move |_| callback_ready.store(true, Ordering::Release),
    )?;
    Ok(Some(ready))
}

#[cfg(not(target_os = "windows"))]
fn load_document(webview: &wry::WebView, document: &str) -> wry::Result<Option<Arc<AtomicBool>>> {
    webview.load_html(document)?;
    Ok(None)
}

impl Render for HtmlPreview {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let mut root = div().size_full();
        if let Some(webview) = self.webview.clone() {
            root = root.child(webview);
        }
        root
    }
}

/// The WebView2 user-data folder: the app's own data directory, where the
/// process has write access in every deployment (loose, unpackaged, or
/// WindowsApps). `None` elsewhere keeps the platform default.
fn preview_data_directory() -> Option<std::path::PathBuf> {
    if cfg!(target_os = "windows") {
        // The instance guard already creates this directory for the SQLite
        // workspace, so a fresh install can't race it here.
        Some(
            crate::core::DatabaseStore::default_path()
                .with_file_name("webview-data")
                .parent()
                .map(|directory| directory.join("webview-data"))
                .unwrap_or_else(|| std::env::temp_dir().join("resolved-webview-data")),
        )
    } else {
        None
    }
}

pub fn can_preview(content_type: Option<&str>, body: &[u8]) -> bool {
    let declared_html = content_type
        .map(|value| {
            let value = value.to_ascii_lowercase();
            value.contains("text/html") || value.contains("application/xhtml+xml")
        })
        .unwrap_or(false);

    if declared_html {
        return true;
    }

    let prefix = String::from_utf8_lossy(&body[..body.len().min(1024)]).to_ascii_lowercase();
    prefix.contains("<!doctype html")
        || prefix.contains("<html")
        || prefix.contains("<head")
        || prefix.contains("<body")
}

fn safe_html_document(html: &str) -> String {
    let lowercase = html.to_ascii_lowercase();

    if let Some(head_start) = lowercase.find("<head")
        && let Some(tag_end) = lowercase[head_start..].find('>')
    {
        let insert_at = head_start + tag_end + 1;
        let mut result = String::with_capacity(html.len() + CSP_META.len());
        result.push_str(&html[..insert_at]);
        result.push_str(CSP_META);
        result.push_str(&html[insert_at..]);
        return result;
    }

    if let Some(html_start) = lowercase.find("<html")
        && let Some(tag_end) = lowercase[html_start..].find('>')
    {
        let insert_at = html_start + tag_end + 1;
        let mut result = String::with_capacity(html.len() + CSP_META.len() + 13);
        result.push_str(&html[..insert_at]);
        result.push_str("<head>");
        result.push_str(CSP_META);
        result.push_str("</head>");
        result.push_str(&html[insert_at..]);
        return result;
    }

    format!("<!doctype html><html><head>{CSP_META}</head><body>{html}</body></html>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_declared_and_obvious_html() {
        assert!(can_preview(Some("text/html; charset=utf-8"), b"hello"));
        assert!(can_preview(None, b"  <!doctype html><title>Hello</title>"));
        assert!(!can_preview(Some("application/json"), br#"{"ok":true}"#));
    }

    #[test]
    fn injects_the_policy_at_the_start_of_an_existing_head() {
        let document =
            safe_html_document("<html><head><title>Hi</title></head><body>Body</body></html>");
        let csp = document.find("Content-Security-Policy").unwrap();
        let title = document.find("<title>").unwrap();
        assert!(csp < title);
    }

    #[test]
    fn wraps_html_fragments() {
        let document = safe_html_document("<h1>Hello</h1>");
        assert!(document.starts_with("<!doctype html><html><head>"));
        assert!(document.contains("<body><h1>Hello</h1></body>"));
    }
}
