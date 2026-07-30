use super::*;

impl ApiTester {
    pub(super) fn render_welcome_title_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .h(px(APP_TITLE_BAR_HEIGHT))
            .flex_shrink_0()
            .pl(px(92.))
            .pr_6()
            .border_b_1()
            .border_color(cx.theme().title_bar_border)
            .bg(cx.theme().title_bar)
            .justify_between()
            .child(
                h_flex()
                    .min_w_0()
                    .gap_6()
                    .child(
                        div()
                            .text_xl()
                            .font_semibold()
                            .text_color(cx.api_primary_bright())
                            .child("API Tester"),
                    )
                    .child(
                        h_flex()
                            .h_full()
                            .items_center()
                            .border_b_2()
                            .border_color(cx.theme().primary)
                            .px_1()
                            .text_sm()
                            .font_semibold()
                            .child("Workspace"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("Welcome"),
                    ),
            )
            .child(
                Button::new("welcome-title-new-request")
                    .icon(IconName::Plus)
                    .label("New request")
                    .large()
                    .h(px(38.))
                    .primary()
                    .rounded(px(20.))
                    .disabled(self.sending)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_blank_request_tab(window, cx);
                    })),
            )
            .into_any_element()
    }

    pub(super) fn render_welcome_page(&self, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .size_full()
            .min_h_0()
            .items_center()
            .justify_center()
            .px_8()
            .pb_16()
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(620.))
                    .items_center()
                    .gap_5()
                    .child(
                        div()
                            .size(px(56.))
                            .rounded_xl()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(cx.theme().primary.opacity(0.14))
                            .text_color(cx.api_primary_bright())
                            .child(Icon::new(IconName::GalleryVerticalEnd).with_size(px(26.))),
                    )
                    .child(
                        v_flex()
                            .items_center()
                            .gap_2()
                            .child(div().text_2xl().font_semibold().child("Welcome"))
                            .child(
                                div()
                                    .max_w(px(500.))
                                    .text_center()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(
                                        "Start a request, or browse your collections to reopen saved work.",
                                    ),
                            ),
                    )
                    .child(
                        h_flex()
                            .flex_wrap()
                            .justify_center()
                            .gap_3()
                            .child(
                                Button::new("welcome-new-request")
                                    .icon(IconName::Plus)
                                    .label("New request")
                                    .large()
                                    .primary()
                                    .disabled(self.sending)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_blank_request_tab(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("welcome-browse-collections")
                                    .icon(IconName::FolderOpen)
                                    .label("Browse collections")
                                    .large()
                                    .outline()
                                    .disabled(self.sending)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.activate_request_workspace(
                                            SidebarTab::Collections,
                                            window,
                                            cx,
                                        );
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }
}
