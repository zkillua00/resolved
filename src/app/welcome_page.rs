use super::*;

impl ApiTester {
    pub(super) fn render_welcome_title_bar(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        h_flex()
            .h(APP_TITLE_BAR_HEIGHT)
            .flex_shrink_0()
            .pl(window_chrome::leading_inset())
            .pr(window_chrome::trailing_inset())
            .border_b_1()
            .border_color(cx.theme().title_bar_border)
            .bg(cx.theme().title_bar)
            .child(
                h_flex()
                    .min_w_0()
                    .gap_6()
                    .child(resolved_brand_lockup(cx))
                    .child(self.render_title_workspace_control(cx))
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("Welcome"),
                    ),
            )
            .child(window_chrome::caption_drag_region())
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("welcome-title-import-request")
                            .label("Import")
                            .large()
                            .h(px(38.))
                            .outline()
                            .rounded(px(20.))
                            .disabled(self.sending)
                            .tooltip("Open the request import workspace")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_request_import_panel(window, cx);
                            })),
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
                    .child(window_chrome::window_controls(window, cx)),
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
                    .child(img(ICON_ASSET_PATH).size(px(72.)))
                    .child(
                        v_flex()
                            .items_center()
                            .gap_2()
                            .child(div().text_2xl().font_semibold().child(PRODUCT_NAME))
                            .child(div().text_base().font_semibold().child(BRAND_HEADLINE))
                            .child(
                                div()
                                    .max_w(px(500.))
                                    .text_center()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(BRAND_EXPLANATION),
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
                            )
                            .child(
                                Button::new("welcome-import-request")
                                    .label("Import request")
                                    .large()
                                    .outline()
                                    .disabled(self.sending)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_request_import_panel(window, cx);
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }
}
