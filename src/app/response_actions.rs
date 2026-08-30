use super::*;

impl ApiTester {
    pub(super) fn update_response_editor(
        &mut self,
        response: &ResponseData,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let language = response_language(response);
        let content = if is_probably_text(&response.body) {
            format_body(&response.body, self.pretty_body, &self.settings.formatter)
        } else {
            format!(
                "Binary response ({}). The post-response script receives a bounded Base64 view.",
                format_bytes(response.size_bytes())
            )
        };
        self.response_editor.update(cx, |editor, cx| {
            editor.set_language(language, cx);
            editor.set_value(content, window, cx);
        });
    }

    pub(super) fn select_response_tab(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.response_tab = ResponseTab::from_index(index);
        self.preview_error = None;
        self.copied = false;

        if self.response_tab == ResponseTab::Preview {
            self.show_preview(window, cx);
        } else {
            self.hide_preview(cx);
        }
        cx.notify();
    }

    pub(super) fn show_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(response) = &self.response else {
            self.hide_preview(cx);
            return;
        };
        if !can_preview(response.content_type.as_deref(), &response.body) {
            self.hide_preview(cx);
            return;
        }

        let html = response.body_text_lossy();
        let is_new = self.preview.is_none();
        let preview = self
            .preview
            .get_or_insert_with(|| cx.new(|cx| HtmlPreview::new(&html, window, cx)))
            .clone();
        if !preview.read(cx).is_available() {
            self.preview_error = Some("The response preview could not be created.".to_owned());
            self.hide_preview(cx);
            return;
        }
        // A newly-created WebView already has this response as its initial
        // document. Only navigate an existing preview.
        let result = if is_new {
            preview.update(cx, |preview, cx| preview.show(cx));
            Ok(())
        } else {
            preview.update(cx, |preview, cx| preview.load_html(&html, cx))
        };
        match result {
            Ok(()) => self.preview_error = None,
            Err(error) => {
                self.preview_error = Some(error.to_string());
                self.hide_preview(cx);
            }
        }
    }

    pub(super) fn hide_preview(&mut self, cx: &mut Context<Self>) {
        if let Some(preview) = self.preview.take() {
            preview.update(cx, |preview, cx| preview.hide(cx));
        }
    }

    pub(super) fn copy_response(&mut self, cx: &mut Context<Self>) {
        if self.response_tab == ResponseTab::Scripts {
            self.copy_script_results(cx);
            return;
        }

        let Some(response) = &self.response else {
            return;
        };

        let value = match self.response_tab {
            ResponseTab::Headers => response
                .headers
                .iter()
                .map(|header| format!("{}: {}", header.name, header.value))
                .collect::<Vec<_>>()
                .join("\n"),
            ResponseTab::Body => self.response_editor.read(cx).value(cx).to_string(),
            ResponseTab::Preview => {
                format_body(&response.body, self.pretty_body, &self.settings.formatter)
            }
            ResponseTab::Scripts => unreachable!("script copying is handled without a response"),
        };
        cx.write_to_clipboard(ClipboardItem::new_string(value));
        self.copied = true;
        cx.notify();
    }
}
