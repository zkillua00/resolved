//! Selected history details retain their editors across paints and identical
//! realtime snapshots. Only the selected entry is retained, not an old history.
use super::*;
use crate::code_editor::{CodeEditor, CodeEditorConfig, CodeLanguage};
use crate::core::SharedHistoryHeader;
use std::ops::Range;
use std::rc::Weak;

/// InputState virtualizes rows, but still shapes a whole visible logical line.
/// Bound pathological buffers too, so a minified response cannot put 1 MiB on
/// one row. Ordinary multiline text scrolls continuously. Pages are exact
/// UTF-8 slices; the UI explicitly identifies byte ranges.
const PAGE_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, Default)]
pub(super) struct ProfileDetailState {
    generation: Weak<Vec<SharedHistoryEntry>>,
    selected_id: String,
    entry: Option<Rc<SharedHistoryEntry>>,
    detail: Option<Entity<HistoryDetail>>,
}

impl ProfileDetailState {
    pub(super) fn retain_entry(
        &mut self,
        entries: &[SharedHistoryEntry],
        selected_id: Option<&str>,
    ) {
        let current = entries
            .iter()
            .find(|entry| Some(entry.id.as_str()) == selected_id);
        if self.entry.as_deref() != current {
            // Clear private decoded text immediately, even when an empty/error
            // result or a hidden page means the detail renderer will not run.
            *self = Self::default();
        }
    }
}

pub(super) fn render_history_entry(
    history: &Rc<Vec<SharedHistoryEntry>>,
    selected_id: &str,
    state: &mut ProfileDetailState,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let generation = Rc::downgrade(history);
    if state.selected_id != selected_id || !state.generation.ptr_eq(&generation) {
        let entry = history.iter().find(|entry| entry.id == selected_id);
        let unchanged = entry.is_some() && state.entry.as_deref() == entry;
        if !unchanged {
            state.entry = entry.map(|entry| Rc::new(entry.clone()));
            state.detail = state
                .entry
                .as_ref()
                .map(|entry| cx.new(|cx| HistoryDetail::new(entry.clone(), window, cx)));
        }
        state.selected_id = selected_id.to_owned();
        state.generation = generation;
    }
    state.detail.as_ref().map_or_else(
        || div().child("Select a history entry.").into_any_element(),
        |detail| detail.clone().into_any_element(),
    )
}

struct HistoryDetail {
    entry: Rc<SharedHistoryEntry>,
    sections: Vec<Entity<PagedText>>,
}

impl HistoryDetail {
    fn new(entry: Rc<SharedHistoryEntry>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut sections = Vec::new();
        let mut section = |title, text, note, truncated| {
            sections.push(cx.new(|cx| PagedText::new(title, text, note, truncated, window, cx)));
        };
        section("URL", entry.request.url.clone(), "", false);
        section(
            "REQUEST HEADERS",
            headers_text(&entry.request.headers),
            "Headers marked not to share are omitted.",
            false,
        );
        let request = &entry.request;
        let body = if request.body_mode == "raw" {
            request.body.clone()
        } else if request.body_fields.is_empty() {
            "No shared request body".to_owned()
        } else {
            request
                .body_fields
                .iter()
                .map(|field| {
                    if field.kind == "file" {
                        format!("{} = [file contents and path omitted]", field.name)
                    } else {
                        format!("{} = {}", field.name, field.value)
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        section("REQUEST BODY", body, "", request.body_truncated);
        if let Some(response) = &entry.response {
            section(
                "RESPONSE HEADERS",
                headers_text(&response.headers),
                "Sensitive response headers are redacted before sharing.",
                false,
            );
            section(
                "RESPONSE BODY",
                shared_history_response_body(response),
                "",
                response.body_truncated,
            );
        }
        if !entry.error.is_empty() {
            section("ERROR", entry.error.clone(), "", false);
        }
        Self { entry, sections }
    }
}

fn headers_text(headers: &[SharedHistoryHeader]) -> String {
    if headers.is_empty() {
        return "No shared headers".to_owned();
    }
    headers
        .iter()
        .map(|header| format!("{}: {}", header.name, header.value))
        .collect::<Vec<_>>()
        .join("\n")
}

impl Render for HistoryDetail {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("profile-history-entry-scroll")
            .size_full()
            .min_w_0()
            .overflow_y_scroll()
            .p_5()
            .gap_5()
            .child(
                h_flex()
                    .gap_3()
                    .text_sm()
                    .font_semibold()
                    .child(
                        div()
                            .text_color(method_color(&self.entry.request.method, cx))
                            .child(self.entry.request.method.clone()),
                    )
                    .children(self.entry.response.as_ref().map(|response| {
                        div()
                            .text_color(status_color(response.status, cx))
                            .child(format!("{} {}", response.status, response.status_text))
                    })),
            )
            .children(self.sections.iter().cloned())
    }
}

fn page_ranges(text: &str) -> Vec<Range<usize>> {
    // Ordinary multiline documents keep native continuous viewport scrolling.
    // Only pathological logical lines need explicitly paged buffers.
    if text.split('\n').all(|line| line.len() <= PAGE_BYTES) {
        return vec![0..text.len()];
    }
    let mut ranges = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let mut end = (start + PAGE_BYTES).min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        ranges.push(start..end);
        start = end;
    }
    if ranges.is_empty() {
        ranges.push(0..0);
    }
    ranges
}

struct PagedText {
    title: &'static str,
    note: &'static str,
    truncated: bool,
    text: String,
    pages: Vec<Range<usize>>,
    page: usize,
    height: Pixels,
    editor: Entity<CodeEditor>,
}

impl PagedText {
    fn new(
        title: &'static str,
        text: String,
        note: &'static str,
        truncated: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let pages = page_ranges(&text);
        let height = px((32. + text.lines().take(12).count().max(1) as f32 * 22.).min(280.));
        let editor = cx.new(|cx| {
            CodeEditor::new(
                CodeEditorConfig::default()
                    .language(CodeLanguage::Plain)
                    .initial_value(text[pages[0].clone()].to_owned())
                    .placeholder("Empty")
                    .read_only(true)
                    .soft_wrap(false)
                    .line_numbers(false)
                    .active_line(false)
                    .rows(12),
                window,
                cx,
            )
        });
        Self {
            title,
            note,
            truncated,
            text,
            pages,
            page: 0,
            height,
            editor,
        }
    }

    fn select_page(&mut self, page: usize, window: &mut Window, cx: &mut Context<Self>) {
        if page >= self.pages.len() || page == self.page {
            return;
        }
        self.page = page;
        let value = self.text[self.pages[page].clone()].to_owned();
        self.editor
            .update(cx, |editor, cx| editor.set_value(value, window, cx));
        cx.notify();
    }
}

impl Render for PagedText {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let range = &self.pages[self.page];
        let title = self.title;
        v_flex()
            .gap_2()
            .flex_shrink_0()
            .child(
                h_flex()
                    .gap_2()
                    .text_xs()
                    .font_semibold()
                    .child(self.title)
                    .when(self.truncated, |view| view.child("Truncated"))
                    .child(
                        Button::new("copy-full-text")
                            .label("Copy all")
                            .small()
                            .ghost()
                            .tooltip("Copy the complete shared text, not only the current page")
                            .on_click(cx.listener(|this, _, _, cx| {
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                    this.text.clone(),
                                ));
                            })),
                    ),
            )
            .when(!self.note.is_empty(), |view| {
                view.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(self.note),
                )
            })
            .when(self.pages.len() > 1, |view| {
                view.child(
                    h_flex()
                        .flex_wrap()
                        .gap_3()
                        .text_xs()
                        .child(format!(
                            "Raw text · bytes {}–{} of {} · page {}/{} (pages may split lines)",
                            range.start + 1,
                            range.end,
                            self.text.len(),
                            self.page + 1,
                            self.pages.len(),
                        ))
                        .when(self.page > 0, |view| {
                            view.child(
                                Button::new("previous-page")
                                    .label("Previous")
                                    .small()
                                    .outline()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.select_page(this.page.saturating_sub(1), window, cx);
                                    })),
                            )
                        })
                        .when(self.page + 1 < self.pages.len(), |view| {
                            view.child(
                                Button::new("next-page")
                                    .label("Next")
                                    .small()
                                    .outline()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.select_page(this.page + 1, window, cx);
                                    })),
                            )
                        }),
                )
            })
            .child(
                div()
                    .debug_selector(move || format!("profile-history-body-{title}"))
                    .w_full()
                    .h(self.height)
                    .child(self.editor.clone()),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Modifiers, ScrollDelta, ScrollWheelEvent, TestAppContext, point, size};

    #[gpui::test]
    fn maximum_multiline_body_scrolls_without_editing(cx: &mut TestAppContext) {
        let text = "line\n".repeat(1024 * 1024 / 5);
        let mut viewer = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            gpui_component::init(cx);
            crate::theme::configure(cx);
            let view = cx.new(|cx| PagedText::new("BODY", text.clone(), "", false, window, cx));
            viewer = Some(view.clone());
            gpui_component::Root::new(view, window, cx)
        });
        cx.simulate_resize(size(px(800.), px(400.)));
        cx.run_until_parked();
        let viewer = viewer.unwrap();
        let editor = cx.read(|cx| viewer.read(cx).editor.clone());
        let input = cx.read(|cx| editor.read(cx).input_state());
        assert_eq!(cx.read(|cx| viewer.read(cx).pages.len()), 1);
        cx.simulate_click(point(px(80.), px(100.)), Modifiers::none());
        let before = cx.read(|cx| input.read(cx).cursor());
        cx.simulate_event(ScrollWheelEvent {
            position: point(px(80.), px(100.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-160.))),
            ..Default::default()
        });
        cx.simulate_click(point(px(80.), px(100.)), Modifiers::none());
        assert!(cx.read(|cx| input.read(cx).cursor()) > before);
        cx.simulate_input("must not edit");
        assert!(cx.read(|cx| editor.read(cx).value(cx).as_ref() == text));
        // Controlled native-harness evidence, not an application FPS estimate.
        // Construction/initial text indexing is intentionally outside this span.
        let started = std::time::Instant::now();
        for _ in 0..20 {
            cx.simulate_event(ScrollWheelEvent {
                position: point(px(80.), px(100.)),
                delta: ScrollDelta::Pixels(point(px(0.), px(-40.))),
                ..Default::default()
            });
            cx.run_until_parked();
        }
        eprintln!(
            "1MiB multiline viewer: 20 warmed native wheel updates in {:?}",
            started.elapsed()
        );
    }

    #[gpui::test]
    fn selected_entry_reuses_editors_and_page_across_identical_snapshots(cx: &mut TestAppContext) {
        let mut state = ProfileDetailState::default();
        let entry: SharedHistoryEntry = serde_json::from_value(serde_json::json!({
            "id": "first", "created_at": "2026-01-01T00:00:00Z",
            "request": {
                "method": "POST", "url": "https://example.test", "headers": [],
                "body": "x".repeat(1024 * 1024), "body_mode": "raw",
                "raw_body_language": "json", "body_fields": [], "body_truncated": false
            },
            "response": null, "error": ""
        }))
        .unwrap();
        let mut history = Rc::new(vec![entry.clone()]);
        let (_, cx) = cx.add_window_view(|window, cx| {
            gpui_component::init(cx);
            crate::theme::configure(cx);
            let _ = render_history_entry(&history, "first", &mut state, window, cx);
            gpui_component::Root::new(state.detail.clone().unwrap(), window, cx)
        });
        let first = state.detail.clone().unwrap();
        let body = cx.read(|cx| first.read(cx).sections[2].clone());
        let editor = cx.read(|cx| body.read(cx).editor.clone());
        cx.update(|window, cx| {
            body.update(cx, |body, cx| body.select_page(12, window, cx));
            editor.read(cx).input_state().update(cx, |input, cx| {
                input.set_cursor_position(lsp_types::Position::new(0, 17), window, cx);
            });
        });
        assert!(cx.read(|cx| editor.read(cx).value(cx).len()) <= PAGE_BYTES);
        history = Rc::new(vec![entry.clone()]);
        cx.update(|window, cx| {
            let _ = render_history_entry(&history, "first", &mut state, window, cx);
        });
        assert_eq!(
            state.detail.as_ref().unwrap().entity_id(),
            first.entity_id()
        );
        assert_eq!(cx.read(|cx| body.read(cx).page), 12);
        assert_eq!(
            cx.read(|cx| editor.read(cx).input_state().read(cx).cursor_position()),
            lsp_types::Position::new(0, 17),
        );
        let mut second = entry;
        second.id = "second".to_owned();
        history = Rc::new(vec![second]);
        cx.update(|window, cx| {
            let _ = render_history_entry(&history, "second", &mut state, window, cx);
        });
        assert_ne!(
            state.detail.as_ref().unwrap().entity_id(),
            first.entity_id()
        );
        let next_body = cx.read(|cx| state.detail.as_ref().unwrap().read(cx).sections[2].clone());
        assert_eq!(cx.read(|cx| next_body.read(cx).page), 0);
        assert_eq!(
            Rc::strong_count(&history),
            1,
            "cache must not retain a history generation"
        );

        let mut management = ServerManagementState::default();
        management.profile_detail = state;
        management.selected_profile_history_id = Some("second".to_owned());
        let previous = management.profile_detail.detail.clone().unwrap();
        management.set_profile_history((*history).clone());
        assert_eq!(
            management
                .profile_detail
                .detail
                .as_ref()
                .unwrap()
                .entity_id(),
            previous.entity_id()
        );
        let mut changed = (*history).clone();
        changed[0].error = "Updated shared result".into();
        management.set_profile_history(changed);
        assert!(management.profile_detail.detail.is_none());
        assert!(management.profile_detail.entry.is_none());
        cx.update(|window, cx| {
            let _ = render_history_entry(
                &management.profile_history,
                "second",
                &mut management.profile_detail,
                window,
                cx,
            );
        });
        assert!(management.profile_detail.detail.is_some());
        // Both an empty success and a denied/failed fetch use this same
        // boundary, without relying on another visible detail render.
        management.set_profile_history(Vec::new());
        assert!(management.profile_detail.detail.is_none());
        assert!(management.profile_detail.entry.is_none());
    }

    #[test]
    fn maximum_bodies_have_bounded_lossless_pages() {
        for text in ["x".repeat(1024 * 1024), "🦀".repeat(1024 * 1024 / 4)] {
            let pages = page_ranges(&text);
            assert!(pages.len() >= 64);
            assert!(pages.iter().all(|range| range.len() <= PAGE_BYTES));
            let reconstructed = pages
                .iter()
                .map(|range| &text[range.clone()])
                .collect::<String>();
            assert_eq!(reconstructed, text);
        }
        let multiline = "line\n".repeat(1024 * 1024 / 5);
        assert_eq!(page_ranges(&multiline), vec![0..multiline.len()]);
        assert_eq!(page_ranges(""), vec![0..0]);
    }
}
