use super::*;

impl ApiTester {
    pub(super) fn render_request_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        documentation::refresh_targets(
            &self.documentation_intelligence,
            &self.documentation,
            &self.query_params,
            &self.headers,
            &self.workspace,
            cx,
        );
        let header_count = self.request_header_count(cx);
        let query_param_count = self.request_query_param_count(cx);
        let status = if let Some(notice) = self.request_notice.clone() {
            Some(
                div()
                    .w_full()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .border_1()
                    .border_color(cx.theme().warning.opacity(0.65))
                    .bg(cx.theme().warning.opacity(0.08))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_xs()
                    .text_color(cx.theme().warning)
                    .child(notice)
                    .into_any_element(),
            )
        } else {
            self.execution_stage.map(|stage| {
                div()
                    .w_full()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(cx.theme().primary.opacity(0.08))
                    .text_xs()
                    .font_semibold()
                    .text_color(cx.theme().primary)
                    .child(stage.label())
                    .into_any_element()
            })
        };

        v_flex()
            .size_full()
            .min_h_0()
            .bg(cx.api_surface())
            .child(
                v_flex()
                    .flex_shrink_0()
                    .gap_3()
                    .pt_4()
                    .child(div().px_4().child(self.render_url_row(cx)))
                    .when_some(status, |this, status| {
                        this.child(div().px_4().child(status))
                    })
                    .child(
                        TabBar::new("request-tabs")
                            .underline()
                            .px_4()
                            .children([
                                format!("Params ({query_param_count})"),
                                format!("Headers ({header_count})"),
                                "Body".to_owned(),
                                "Pre-request".to_owned(),
                                "Post-response".to_owned(),
                                self.cookie_tab_label(),
                                "Documentation".to_owned(),
                            ])
                            .selected_index(self.request_pane.index())
                            .on_click(cx.listener(|this, index: &usize, _, cx| {
                                this.request_pane = RequestPane::from_index(*index);
                                cx.notify();
                            })),
                    ),
            )
            .child(request_workspace::request_content_container(
                div()
                    .size_full()
                    .pt_2()
                    .bg(
                        if matches!(
                            self.request_pane,
                            RequestPane::Params | RequestPane::Headers | RequestPane::Cookies
                        ) {
                            cx.api_surface_low()
                        } else {
                            cx.api_surface()
                        },
                    )
                    .when(self.request_pane == RequestPane::Documentation, |this| {
                        this.child(
                            div()
                                .debug_selector(|| "request-documentation-editor".to_owned())
                                .size_full()
                                .child(self.documentation.clone()),
                        )
                    })
                    .when(self.request_pane == RequestPane::Params, |this| {
                        this.child(self.render_query_params_editor(cx))
                    })
                    .when(self.request_pane == RequestPane::Headers, |this| {
                        this.child(self.render_headers_editor(cx))
                    })
                    .when(self.request_pane == RequestPane::Body, |this| {
                        this.child(self.render_body_editor(cx))
                    })
                    .when(self.request_pane == RequestPane::PreRequest, |this| {
                        this.child(self.pre_request_script.clone())
                    })
                    .when(self.request_pane == RequestPane::Cookies, |this| {
                        this.child(self.render_cookie_manager("request-cookies".into(), cx))
                    })
                    .when(self.request_pane == RequestPane::PostResponse, |this| {
                        this.child(self.post_response_script.clone())
                    }),
            ))
            .into_any_element()
    }
}

/// Give percentage-height tables and editors the remaining panel bounds without
/// letting their intrinsic content size expand the flex item.
pub(in crate::app) fn request_content_container(content: impl IntoElement) -> impl IntoElement {
    div()
        .relative()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .overflow_hidden()
        .child(div().absolute().inset_0().child(content))
}
