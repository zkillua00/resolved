use super::*;

impl Render for ApiTester {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.debug_overlay.read(cx).record_ui_frame();

        let workspace = match self.workspace_tabs.active() {
            ActiveWorkspaceTab::Welcome => v_flex()
                .size_full()
                .min_h_0()
                .child(self.render_request_tab_strip(cx))
                .child(div().flex_1().min_h_0().child(self.render_welcome_page(cx)))
                .into_any_element(),
            ActiveWorkspaceTab::Settings => v_flex()
                .size_full()
                .min_h_0()
                .child(self.render_request_tab_strip(cx))
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .child(self.render_settings_workspace(cx)),
                )
                .into_any_element(),
            ActiveWorkspaceTab::ThemeCss => v_flex()
                .size_full()
                .min_h_0()
                .child(self.render_request_tab_strip(cx))
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .child(self.render_theme_css_workspace(cx)),
                )
                .into_any_element(),
            ActiveWorkspaceTab::Request if self.sidebar_tab == SidebarTab::Environments => {
                self.render_environment_workspace(cx)
            }
            ActiveWorkspaceTab::Request => v_flex()
                .size_full()
                .min_h_0()
                .child(self.render_request_tab_strip(cx))
                .child(
                    div().flex_1().min_h_0().child(
                        h_resizable("workspace-split")
                            .child(
                                resizable_panel()
                                    .size(px(360.))
                                    .size_range(px(320.)..px(480.))
                                    .child(self.render_sidebar(cx)),
                            )
                            .child(
                                resizable_panel().child(
                                    v_flex().size_full().min_h_0().child(
                                        v_resizable("request-response-split")
                                            .child(
                                                resizable_panel()
                                                    .size(px(480.))
                                                    .size_range(px(360.)..px(900.))
                                                    .child(self.render_request_panel(cx)),
                                            )
                                            .child(
                                                resizable_panel()
                                                    .size_range(px(240.)..px(1_400.))
                                                    .child(self.render_response_panel(cx)),
                                            ),
                                    ),
                                ),
                            ),
                    ),
                )
                .into_any_element(),
        };

        v_flex()
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(cx.api_surface())
            .text_color(cx.theme().foreground)
            .key_context(shortcuts::APP_KEY_CONTEXT)
            .capture_any_mouse_down(cx.listener(Self::cancel_shortcut_recording_on_pointer))
            .capture_key_down(cx.listener(Self::capture_template_key_down))
            .child(self.render_title_bar(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_navigation_rail(cx))
                    .child(
                        div()
                            .relative()
                            .h_full()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .child(workspace)
                            .child(self.debug_overlay.clone()),
                    ),
            )
            .children(self.render_template_variable_popover(cx))
            .children(Root::render_dialog_layer(window, cx))
    }
}
