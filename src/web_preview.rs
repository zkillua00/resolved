use gpui::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _, Window,
    div,
};
use gpui_wry::WebView as GpuiWebView;
use wry::{NewWindowResponse, WebViewBuilder, WebViewBuilderExtDarwin};

const SAFE_EMPTY: &str = r#"<!doctype html>
<html>
<head>
  <meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'none'; img-src data: blob:; style-src 'unsafe-inline'; font-src data:; media-src data: blob:; connect-src 'none'; frame-src 'none'; object-src 'none'; form-action 'none'; base-uri 'none'">
  <meta name="color-scheme" content="light dark">
</head>
<body></body>
</html>"#;

const CSP_META: &str = r#"<meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'none'; img-src data: blob:; style-src 'unsafe-inline'; font-src data:; media-src data: blob:; connect-src 'none'; frame-src 'none'; object-src 'none'; form-action 'none'; base-uri 'none'"><meta name="color-scheme" content="light dark">"#;

pub struct HtmlPreview {
    webview: Entity<GpuiWebView>,
}

impl HtmlPreview {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let webview = cx.new(|cx| {
            let raw = WebViewBuilder::new()
                .with_html(SAFE_EMPTY)
                .with_incognito(true)
                .with_javascript_disabled()
                .with_devtools(false)
                .with_autoplay(false)
                .with_allow_link_preview(false)
                .with_drag_drop_handler(|_| true)
                .with_navigation_handler(|url| url.starts_with("about:blank"))
                .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
                .with_download_started_handler(|_, _| false)
                .build_as_child(window)
                .expect("failed to create the WKWebView response preview");

            let mut view = GpuiWebView::new(raw, window, cx);
            view.hide();
            view
        });

        Self { webview }
    }

    pub fn load_html(&mut self, html: &str, cx: &mut Context<Self>) -> wry::Result<()> {
        let document = safe_html_document(html);
        let result = self.webview.update(cx, |view, _| {
            view.raw().load_html(&document)?;
            view.show();
            Ok(())
        });
        cx.notify();
        result
    }

    pub fn hide(&mut self, cx: &mut Context<Self>) {
        self.webview.update(cx, |view, _| view.hide());
        cx.notify();
    }
}

impl Render for HtmlPreview {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.webview.clone())
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
