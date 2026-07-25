use std::sync::Arc;

use chrono::Local;
use gpui::{
    AnyElement, App, AppContext as _, ClickEvent, ClipboardItem, Context, Entity, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
    prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, IndexPath, Selectable as _, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    select::{Select, SelectState},
    tab::TabBar,
    v_flex,
};
use reqwest::Client;
use tokio::{runtime::Runtime, task::AbortHandle};

use crate::{
    core::{
        HeaderEntry, HistoryEntry, HistoryStore, REDACTED_VALUE, RequestDraft, RequestError,
        RequestHistory, RequestTask, ResponseData, build_client, format_body, is_probably_text,
        spawn_request,
    },
    web_preview::{HtmlPreview, can_preview},
};

const METHODS: [&str; 7] = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];
type MethodSelect = SelectState<Vec<&'static str>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RequestTab {
    Headers,
    Body,
}

impl RequestTab {
    fn index(self) -> usize {
        match self {
            Self::Headers => 0,
            Self::Body => 1,
        }
    }

    fn from_index(index: usize) -> Self {
        if index == 1 {
            Self::Body
        } else {
            Self::Headers
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResponseTab {
    Body,
    Headers,
    Preview,
}

impl ResponseTab {
    fn index(self) -> usize {
        match self {
            Self::Body => 0,
            Self::Headers => 1,
            Self::Preview => 2,
        }
    }

    fn from_index(index: usize) -> Self {
        match index {
            1 => Self::Headers,
            2 => Self::Preview,
            _ => Self::Body,
        }
    }
}

struct HeaderRow {
    id: usize,
    name: Entity<InputState>,
    value: Entity<InputState>,
    enabled: bool,
}

pub struct ApiTester {
    method: Entity<MethodSelect>,
    url: Entity<InputState>,
    body: Entity<InputState>,
    headers: Vec<HeaderRow>,
    next_header_id: usize,
    request_tab: RequestTab,
    response_tab: ResponseTab,
    pretty_body: bool,
    sending: bool,
    request_generation: u64,
    abort_handle: Option<AbortHandle>,
    response: Option<ResponseData>,
    request_error: Option<String>,
    preview_error: Option<String>,
    copied: bool,
    client: Client,
    runtime: Arc<Runtime>,
    history: RequestHistory,
    history_store: HistoryStore,
    history_warning: Option<String>,
    preview: Entity<HtmlPreview>,
    _subscriptions: Vec<Subscription>,
}

impl ApiTester {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let method =
            cx.new(|cx| MethodSelect::new(METHODS.to_vec(), Some(IndexPath::new(0)), window, cx));
        let url = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("https://api.example.com/users")
                .default_value("https://httpbin.org/get")
        });
        let body = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .rows(10)
                .soft_wrap(false)
                .placeholder("Raw request body")
        });
        let preview = cx.new(|cx| HtmlPreview::new(window, cx));

        let history_store = HistoryStore::default();
        let (history, history_warning) = match history_store.load() {
            Ok(history) => (history, None),
            Err(error) => (
                RequestHistory::default(),
                Some(format!("History could not be loaded: {error}")),
            ),
        };

        let client = build_client().expect("failed to create the HTTP client");
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("api-tester-network")
                .enable_all()
                .build()
                .expect("failed to create the network runtime"),
        );

        let url_subscription = cx.subscribe(&url, |this, _, event, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.start_request(cx);
            }
        });

        let mut this = Self {
            method,
            url,
            body,
            headers: Vec::new(),
            next_header_id: 0,
            request_tab: RequestTab::Headers,
            response_tab: ResponseTab::Body,
            pretty_body: true,
            sending: false,
            request_generation: 0,
            abort_handle: None,
            response: None,
            request_error: None,
            preview_error: None,
            copied: false,
            client,
            runtime,
            history,
            history_store,
            history_warning,
            preview,
            _subscriptions: vec![url_subscription],
        };
        this.push_header_row("", "", true, window, cx);
        this
    }

    fn push_header_row(
        &mut self,
        name: impl Into<SharedString>,
        value: impl Into<SharedString>,
        enabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = name.into();
        let value = value.into();
        let name_state = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Header")
                .default_value(name)
        });
        let value_state = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Value")
                .default_value(value)
        });
        let id = self.next_header_id;
        self.next_header_id += 1;
        self.headers.push(HeaderRow {
            id,
            name: name_state,
            value: value_state,
            enabled,
        });
    }

    fn draft(&self, cx: &App) -> RequestDraft {
        let method = self
            .method
            .read(cx)
            .selected_value()
            .copied()
            .unwrap_or("GET")
            .to_owned();
        let headers = self
            .headers
            .iter()
            .map(|row| {
                let mut header = HeaderEntry::new(
                    row.name.read(cx).value().to_string(),
                    row.value.read(cx).value().to_string(),
                );
                header.enabled = row.enabled;
                header
            })
            .collect();

        let mut draft = RequestDraft::new(method, self.url.read(cx).value().to_string());
        draft.headers = headers;
        draft.body = self.body.read(cx).value().to_string();
        draft
    }

    fn start_request(&mut self, cx: &mut Context<Self>) {
        if self.sending {
            return;
        }

        let request = self.draft(cx);
        self.request_generation = self.request_generation.wrapping_add(1);
        let generation = self.request_generation;
        self.sending = true;
        self.response = None;
        self.request_error = None;
        self.preview_error = None;
        self.copied = false;
        self.hide_preview(cx);

        let task: RequestTask =
            spawn_request(self.runtime.handle(), self.client.clone(), request.clone());
        self.abort_handle = Some(task.abort_handle());
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = task.wait().await;
            let _ = this.update(cx, |this, cx| {
                this.finish_request(generation, request, result, cx);
            });
        })
        .detach();
    }

    fn finish_request(
        &mut self,
        generation: u64,
        request: RequestDraft,
        result: Result<ResponseData, RequestError>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.request_generation {
            return;
        }

        self.sending = false;
        self.abort_handle = None;

        match result {
            Ok(response) => {
                self.history
                    .push(HistoryEntry::completed(&request, &response));
                self.response = Some(response);
                self.request_error = None;
                if self.response_tab == ResponseTab::Preview {
                    self.show_preview(cx);
                }
            }
            Err(RequestError::Cancelled) => {
                self.request_error = Some("Request cancelled".to_owned());
            }
            Err(error) => {
                let message = error.to_string();
                self.history
                    .push(HistoryEntry::failed(&request, message.clone()));
                self.request_error = Some(message);
            }
        }

        self.persist_history();
        cx.notify();
    }

    fn cancel_request(&mut self, cx: &mut Context<Self>) {
        let Some(abort_handle) = self.abort_handle.take() else {
            return;
        };

        abort_handle.abort();
        self.request_generation = self.request_generation.wrapping_add(1);
        self.sending = false;
        self.request_error = Some("Request cancelled".to_owned());
        self.preview_error = None;
        self.hide_preview(cx);
        cx.notify();
    }

    fn persist_history(&mut self) {
        self.history_warning = self
            .history_store
            .save(&self.history)
            .err()
            .map(|error| format!("History could not be saved: {error}"));
    }

    fn clear_history(&mut self, cx: &mut Context<Self>) {
        self.history.clear();
        self.persist_history();
        cx.notify();
    }

    fn load_history(&mut self, request: RequestDraft, window: &mut Window, cx: &mut Context<Self>) {
        if self.sending {
            return;
        }

        let selected_method = METHODS
            .iter()
            .position(|method| *method == request.method)
            .unwrap_or(0);
        self.method.update(cx, |state, cx| {
            state.set_selected_index(Some(IndexPath::new(selected_method)), window, cx);
        });
        self.url.update(cx, |state, cx| {
            state.set_value(request.url, window, cx);
        });
        self.body.update(cx, |state, cx| {
            state.set_value(request.body, window, cx);
        });

        self.headers.clear();
        for header in request.headers {
            let was_redacted = header.value == REDACTED_VALUE;
            self.push_header_row(
                header.name,
                if was_redacted {
                    String::new()
                } else {
                    header.value
                },
                header.enabled && !was_redacted,
                window,
                cx,
            );
        }
        if self.headers.is_empty() {
            self.push_header_row("", "", true, window, cx);
        }

        self.response = None;
        self.request_error = None;
        self.preview_error = None;
        self.copied = false;
        self.hide_preview(cx);
        cx.notify();
    }

    fn select_response_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        self.response_tab = ResponseTab::from_index(index);
        self.preview_error = None;

        if self.response_tab == ResponseTab::Preview {
            self.show_preview(cx);
        } else {
            self.hide_preview(cx);
        }
        cx.notify();
    }

    fn show_preview(&mut self, cx: &mut Context<Self>) {
        let Some(response) = &self.response else {
            self.hide_preview(cx);
            return;
        };
        if !can_preview(response.content_type.as_deref(), &response.body) {
            self.hide_preview(cx);
            return;
        }

        let html = response.body_text_lossy();
        let result = self
            .preview
            .update(cx, |preview, cx| preview.load_html(&html, cx));
        self.preview_error = result.err().map(|error| error.to_string());
    }

    fn hide_preview(&mut self, cx: &mut Context<Self>) {
        self.preview.update(cx, |preview, cx| preview.hide(cx));
    }

    fn copy_response(&mut self, cx: &mut Context<Self>) {
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
            ResponseTab::Body | ResponseTab::Preview => {
                format_body(&response.body, self.pretty_body)
            }
        };
        cx.write_to_clipboard(ClipboardItem::new_string(value));
        self.copied = true;
        cx.notify();
    }

    fn request_header_count(&self, cx: &App) -> usize {
        self.headers
            .iter()
            .filter(|row| row.enabled && !row.name.read(cx).value().trim().is_empty())
            .count()
    }

    fn render_title_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .h(px(42.))
            .flex_shrink_0()
            .pl(px(82.))
            .pr_4()
            .border_b_1()
            .border_color(cx.theme().title_bar_border)
            .bg(cx.theme().title_bar)
            .justify_between()
            .child(
                h_flex()
                    .gap_2()
                    .child(div().size_2().rounded_full().bg(cx.theme().primary))
                    .child(div().text_sm().font_semibold().child("API Tester")),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Native GPUI · restricted WKWebView preview"),
            )
            .into_any_element()
    }

    fn render_history(&self, cx: &mut Context<Self>) -> AnyElement {
        let rows = self
            .history
            .entries()
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let request = entry.request.clone();
                let status = entry.response.as_ref().map(|response| response.status);
                let status_text: SharedString = status
                    .map(|status| status.to_string())
                    .unwrap_or_else(|| "ERR".to_owned())
                    .into();
                let method: SharedString = entry.request.method.clone().into();
                let url: SharedString = compact_url(&entry.request.url).into();
                let time: SharedString = entry
                    .created_at
                    .with_timezone(&Local)
                    .format("%b %d · %H:%M")
                    .to_string()
                    .into();
                let color = method_color(&entry.request.method, cx);

                div()
                    .id(("history-entry", index))
                    .w_full()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(cx.theme().sidebar_border)
                    .cursor_pointer()
                    .hover(|style| style.bg(cx.theme().sidebar_accent))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.load_history(request.clone(), window, cx);
                    }))
                    .child(
                        h_flex()
                            .justify_between()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_semibold()
                                            .text_color(color)
                                            .child(method),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(status_text),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(time),
                            ),
                    )
                    .child(
                        div()
                            .mt_1()
                            .w_full()
                            .overflow_hidden()
                            .text_sm()
                            .whitespace_nowrap()
                            .child(url),
                    )
                    .into_any_element()
            })
            .collect::<Vec<_>>();

        v_flex()
            .w(px(250.))
            .h_full()
            .flex_shrink_0()
            .border_r_1()
            .border_color(cx.theme().sidebar_border)
            .bg(cx.theme().sidebar)
            .child(
                h_flex()
                    .h_11()
                    .px_3()
                    .flex_shrink_0()
                    .justify_between()
                    .border_b_1()
                    .border_color(cx.theme().sidebar_border)
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .child(format!("History ({})", self.history.len())),
                    )
                    .child(
                        Button::new("clear-history")
                            .label("Clear")
                            .xsmall()
                            .ghost()
                            .disabled(self.history.is_empty())
                            .on_click(cx.listener(|this, _, _, cx| this.clear_history(cx))),
                    ),
            )
            .child(
                div()
                    .id("history-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .when(rows.is_empty(), |this| {
                        this.child(
                            v_flex()
                                .items_center()
                                .gap_1()
                                .px_4()
                                .py_8()
                                .text_center()
                                .text_color(cx.theme().muted_foreground)
                                .child(div().text_sm().child("No requests yet"))
                                .child(div().text_xs().child("Completed requests appear here.")),
                        )
                    })
                    .children(rows),
            )
            .when_some(self.history_warning.clone(), |this, warning| {
                this.child(
                    div()
                        .px_3()
                        .py_2()
                        .border_t_1()
                        .border_color(cx.theme().warning)
                        .text_xs()
                        .text_color(cx.theme().warning)
                        .child(warning),
                )
            })
            .into_any_element()
    }

    fn render_url_row(&self, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .w_full()
            .h_8()
            .gap_2()
            .child(Select::new(&self.method).w(px(112.)))
            .child(
                div()
                    .w_full()
                    .min_w(px(220.))
                    .flex_shrink()
                    .child(Input::new(&self.url).small()),
            )
            .child(
                Button::new("send-request")
                    .label(if self.sending { "Sending…" } else { "Send" })
                    .primary()
                    .loading(self.sending)
                    .disabled(self.sending)
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.start_request(cx))),
            )
            .when(self.sending, |this| {
                this.child(
                    Button::new("cancel-request")
                        .label("Cancel")
                        .danger()
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_request(cx))),
                )
            })
            .into_any_element()
    }

    fn render_headers_editor(&self, cx: &mut Context<Self>) -> AnyElement {
        let rows = self
            .headers
            .iter()
            .map(|row| {
                let id = row.id;
                h_flex()
                    .w_full()
                    .gap_2()
                    .child(
                        Button::new(("toggle-header", id))
                            .label(if row.enabled { "✓" } else { "○" })
                            .xsmall()
                            .ghost()
                            .selected(row.enabled)
                            .tooltip(if row.enabled {
                                "Disable header"
                            } else {
                                "Enable header"
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(row) = this.headers.iter_mut().find(|row| row.id == id)
                                {
                                    row.enabled = !row.enabled;
                                    cx.notify();
                                }
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&row.name).small()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&row.value).small()),
                    )
                    .child(
                        Button::new(("remove-header", id))
                            .label("−")
                            .xsmall()
                            .ghost()
                            .danger()
                            .tooltip("Remove header")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.headers.retain(|row| row.id != id);
                                cx.notify();
                            })),
                    )
                    .into_any_element()
            })
            .collect::<Vec<_>>();

        v_flex()
            .size_full()
            .gap_2()
            .child(
                h_flex()
                    .px_1()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(div().w(px(34.)).child(""))
                    .child(div().flex_1().child("NAME"))
                    .child(div().flex_1().child("VALUE"))
                    .child(div().w(px(30.)).child("")),
            )
            .child(
                v_flex()
                    .id("header-rows")
                    .flex_1()
                    .min_h_0()
                    .gap_2()
                    .overflow_y_scroll()
                    .children(rows),
            )
            .child(
                Button::new("add-header")
                    .label("+ Add header")
                    .small()
                    .ghost()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.push_header_row("", "", true, window, cx);
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    fn render_request_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let header_count = self.request_header_count(cx);
        v_flex()
            .h(px(310.))
            .flex_shrink_0()
            .gap_3()
            .p_4()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(self.render_url_row(cx))
            .child(
                TabBar::new("request-tabs")
                    .underline()
                    .small()
                    .children([format!("Headers ({header_count})"), "Body".to_owned()])
                    .selected_index(self.request_tab.index())
                    .on_click(cx.listener(|this, index: &usize, _, cx| {
                        this.request_tab = RequestTab::from_index(*index);
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .when(self.request_tab == RequestTab::Headers, |this| {
                        this.child(self.render_headers_editor(cx))
                    })
                    .when(self.request_tab == RequestTab::Body, |this| {
                        this.child(Input::new(&self.body).h_full())
                    }),
            )
            .into_any_element()
    }

    fn render_response_summary(
        &self,
        response: &ResponseData,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let status_color = status_color(response.status, cx);
        h_flex()
            .gap_3()
            .text_xs()
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(status_color.opacity(0.14))
                    .text_color(status_color)
                    .font_semibold()
                    .child(format!("{} {}", response.status, response.status_text)),
            )
            .child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .child(format_duration(response.duration)),
            )
            .child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .child(format_bytes(response.size_bytes())),
            )
            .child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .child(response.http_version.clone()),
            )
            .into_any_element()
    }

    fn render_response_body(&self, response: &ResponseData, cx: &mut Context<Self>) -> AnyElement {
        let content: SharedString = if is_probably_text(&response.body) {
            format_body(&response.body, self.pretty_body).into()
        } else {
            format!(
                "Binary response ({}). Use Copy to place the raw lossy representation on the clipboard.",
                format_bytes(response.size_bytes())
            )
            .into()
        };

        div()
            .id("response-body-scroll")
            .size_full()
            .overflow_scroll()
            .p_3()
            .rounded_md()
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().muted.opacity(0.32))
            .font_family(cx.theme().mono_font_family.clone())
            .text_size(cx.theme().mono_font_size)
            .line_height(px(19.))
            .child(content)
            .into_any_element()
    }

    fn render_response_headers(
        &self,
        response: &ResponseData,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id("response-headers-scroll")
            .size_full()
            .overflow_y_scroll()
            .children(response.headers.iter().enumerate().map(|(index, header)| {
                h_flex()
                    .px_3()
                    .py_2()
                    .gap_4()
                    .when(index > 0, |this| {
                        this.border_t_1().border_color(cx.theme().border)
                    })
                    .child(
                        div()
                            .w(px(220.))
                            .flex_shrink_0()
                            .text_sm()
                            .font_semibold()
                            .child(header.name.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .font_family(cx.theme().mono_font_family.clone())
                            .child(header.value.clone()),
                    )
            }))
            .into_any_element()
    }

    fn render_preview(&self, response: &ResponseData, cx: &mut Context<Self>) -> AnyElement {
        if !can_preview(response.content_type.as_deref(), &response.body) {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_2()
                .text_color(cx.theme().muted_foreground)
                .child(div().text_sm().font_semibold().child("No HTML preview"))
                .child(div().text_xs().child(
                    "The response is not declared as HTML and has no HTML document markers.",
                ))
                .into_any_element();
        }

        v_flex()
            .size_full()
            .min_h_0()
            .child(
                h_flex()
                    .h_8()
                    .flex_shrink_0()
                    .px_3()
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().muted.opacity(0.45))
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Captured response · JavaScript, network access, navigation, and downloads blocked"),
            )
            .when_some(self.preview_error.clone(), |this, error| {
                this.child(
                    div()
                        .px_3()
                        .py_2()
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(error),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .border_1()
                    .border_t_0()
                    .border_color(cx.theme().border)
                    .child(self.preview.clone()),
            )
            .into_any_element()
    }

    fn render_response_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(response) = &self.response else {
            return v_flex()
                .flex_1()
                .min_h_0()
                .items_center()
                .justify_center()
                .gap_2()
                .text_color(cx.theme().muted_foreground)
                .when_some(self.request_error.clone(), |this, error| {
                    this.child(
                        div()
                            .max_w(px(640.))
                            .px_4()
                            .py_3()
                            .rounded_md()
                            .border_1()
                            .border_color(cx.theme().danger)
                            .text_color(cx.theme().danger)
                            .text_sm()
                            .child(error),
                    )
                })
                .when(self.request_error.is_none() && !self.sending, |this| {
                    this.child(div().text_sm().font_semibold().child("Ready to send"))
                        .child(
                            div()
                                .text_xs()
                                .child("Choose a method, enter a URL, then press Send or Return."),
                        )
                })
                .when(self.sending, |this| {
                    this.child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .child("Waiting for response…"),
                    )
                })
                .into_any_element();
        };

        v_flex()
            .flex_1()
            .min_h_0()
            .gap_3()
            .p_4()
            .child(
                h_flex()
                    .justify_between()
                    .child(self.render_response_summary(response, cx))
                    .child(
                        h_flex()
                            .gap_2()
                            .when(self.response_tab == ResponseTab::Body, |this| {
                                this.child(
                                    Button::new("toggle-pretty")
                                        .label(if self.pretty_body { "Pretty" } else { "Raw" })
                                        .xsmall()
                                        .selected(self.pretty_body)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.pretty_body = !this.pretty_body;
                                            this.copied = false;
                                            cx.notify();
                                        })),
                                )
                            })
                            .child(
                                Button::new("copy-response")
                                    .label(if self.copied { "Copied" } else { "Copy" })
                                    .xsmall()
                                    .ghost()
                                    .on_click(cx.listener(|this, _, _, cx| this.copy_response(cx))),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .justify_between()
                    .child(
                        TabBar::new("response-tabs")
                            .underline()
                            .small()
                            .children(["Body", "Headers", "Preview"])
                            .selected_index(self.response_tab.index())
                            .on_click(cx.listener(|this, index: &usize, _, cx| {
                                this.select_response_tab(*index, cx);
                            })),
                    )
                    .child(
                        div()
                            .max_w(px(460.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(compact_url(&response.final_url)),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .when(self.response_tab == ResponseTab::Body, |this| {
                        this.child(self.render_response_body(response, cx))
                    })
                    .when(self.response_tab == ResponseTab::Headers, |this| {
                        this.child(self.render_response_headers(response, cx))
                    })
                    .when(self.response_tab == ResponseTab::Preview, |this| {
                        this.child(self.render_preview(response, cx))
                    }),
            )
            .into_any_element()
    }
}

impl Render for ApiTester {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .overflow_hidden()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.render_title_bar(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_start()
                    .child(self.render_history(cx))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(self.render_request_panel(cx))
                            .child(self.render_response_panel(cx)),
                    ),
            )
    }
}

fn compact_url(url: &str) -> String {
    const MAX_CHARS: usize = 64;
    if url.chars().count() <= MAX_CHARS {
        return url.to_owned();
    }
    let prefix: String = url.chars().take(MAX_CHARS - 1).collect();
    format!("{prefix}…")
}

fn format_duration(duration: std::time::Duration) -> String {
    if duration.as_secs() >= 1 {
        format!("{:.2} s", duration.as_secs_f64())
    } else {
        format!("{} ms", duration.as_millis())
    }
}

fn format_bytes(bytes: usize) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    if bytes as f64 >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB)
    } else if bytes as f64 >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB)
    } else {
        format!("{bytes} B")
    }
}

fn method_color(method: &str, cx: &App) -> Hsla {
    match method {
        "GET" | "HEAD" => cx.theme().green,
        "POST" => cx.theme().yellow,
        "PUT" | "PATCH" => cx.theme().blue,
        "DELETE" => cx.theme().red,
        _ => cx.theme().cyan,
    }
}

fn status_color(status: u16, cx: &App) -> Hsla {
    match status {
        200..=299 => cx.theme().success,
        300..=399 => cx.theme().info,
        400..=499 => cx.theme().warning,
        _ => cx.theme().danger,
    }
}
