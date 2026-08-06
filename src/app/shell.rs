use super::*;

impl Render for ApiTester {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let visible_tabs = self.workspace_tabs.visible_tabs(&self.request_tabs);
        let active_tab = self.workspace_tabs.active_tab(&self.request_tabs);
        if self.panes.reconcile_open_tabs(&visible_tabs, &active_tab) {
            self.reconcile_pane_editors(window, cx);
        }
        self.debug_overlay.read(cx).record_ui_frame();
        self.sync_request_export_preview(window, cx);
        let app_style = cx.api_theme().classes.app.clone();
        let workspace_surface = v_flex()
            .relative()
            .h_full()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .child(self.render_workspace_panes(cx))
            .child(self.debug_overlay.clone());
        let workspace_surface =
            if let Some(interchange_panel) = self.render_request_interchange_panel(cx) {
                h_resizable("request-interchange-layout")
                    .child(resizable_panel().child(workspace_surface))
                    .child(
                        resizable_panel()
                            .size(px(request_interchange::REQUEST_INTERCHANGE_PANEL_WIDTH))
                            .size_range(px(400.)..px(720.))
                            .child(interchange_panel),
                    )
                    .into_any_element()
            } else {
                workspace_surface.into_any_element()
            };

        v_flex()
            .size_full()
            .relative()
            .overflow_hidden()
            .mt(app_style.margin.top)
            .mr(app_style.margin.right)
            .mb(app_style.margin.bottom)
            .ml(app_style.margin.left)
            .pt(app_style.padding.top)
            .pr(app_style.padding.right)
            .pb(app_style.padding.bottom)
            .pl(app_style.padding.left)
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
                    .child(workspace_surface),
            )
            .children(self.render_template_variable_popover(cx))
            .children(Root::render_dialog_layer(window, cx))
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use gpui::{Bounds, Modifiers, TestAppContext, point, size};
    use gpui_component::setting::{SettingGroup, SettingItem, SettingPage, Settings};

    use super::*;

    struct RetainedSettingsHarness {
        settings_visible: bool,
        background_clicks: Rc<Cell<usize>>,
        rendered_page: Rc<Cell<usize>>,
    }

    impl Render for RetainedSettingsHarness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let background_clicks = Rc::clone(&self.background_clicks);
            self.rendered_page.set(0);
            let first_page_rendered = Rc::clone(&self.rendered_page);
            let second_page_rendered = Rc::clone(&self.rendered_page);
            let settings = Settings::new("retained-settings-test")
                .sidebar_width(px(180.))
                .pages([
                    SettingPage::new("First")
                        .resettable(false)
                        .group(SettingGroup::new().item(SettingItem::render_searchable(
                            "first",
                            move |_, _, _| {
                                first_page_rendered.set(1);
                                div().h(px(80.))
                            },
                        ))),
                    SettingPage::new("Second")
                        .resettable(false)
                        .group(SettingGroup::new().item(SettingItem::render_searchable(
                            "second",
                            move |_, _, _| {
                                second_page_rendered.set(2);
                                div()
                                    .debug_selector(|| "retained-second-page".to_owned())
                                    .h(px(80.))
                            },
                        ))),
                ]);

            div()
                .relative()
                .size_full()
                .debug_selector(|| "retained-workspace".to_owned())
                .child(
                    div()
                        .id("background")
                        .size_full()
                        .when(self.settings_visible, |this| this.hidden())
                        .on_click(move |_, _, _| {
                            background_clicks.set(background_clicks.get() + 1)
                        }),
                )
                .child(
                    div()
                        .debug_selector(|| "retained-settings-surface".to_owned())
                        .size_full()
                        .when(!self.settings_visible, |this| this.hidden())
                        .child(settings),
                )
        }
    }

    #[gpui::test]
    fn mounted_settings_preserve_state_while_hidden(cx: &mut TestAppContext) {
        let background_clicks = Rc::new(Cell::new(0));
        let rendered_page = Rc::new(Cell::new(0));
        let mut harness = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            gpui_component::init(cx);
            crate::theme::configure(cx);
            let view = cx.new(|_| RetainedSettingsHarness {
                settings_visible: true,
                background_clicks: Rc::clone(&background_clicks),
                rendered_page: Rc::clone(&rendered_page),
            });
            harness = Some(view.clone());
            Root::new(view, window, cx)
        });
        cx.simulate_resize(size(px(900.), px(700.)));
        cx.run_until_parked();

        assert_eq!(rendered_page.get(), 1);
        let visible_bounds = cx
            .debug_bounds("retained-settings-surface")
            .expect("the visible Settings surface must be laid out");
        let workspace_bounds = cx
            .debug_bounds("retained-workspace")
            .expect("the test workspace must be laid out");
        let viewport_bounds =
            cx.update(|window, _| Bounds::new(point(px(0.), px(0.)), window.viewport_size()));
        assert_eq!(visible_bounds, workspace_bounds);
        assert!(visible_bounds.size.width > px(0.));
        assert!(visible_bounds.size.height > px(0.));
        assert!(visible_bounds.is_contained_within(&viewport_bounds));

        cx.update(|window, _| window.focus_next());
        cx.run_until_parked();
        let search_input = cx
            .update(|window, cx| window.focused_input(cx))
            .expect("the settings search input must be the first tab stop");
        cx.update(|window, cx| {
            search_input.update(cx, |input, cx| input.set_value("second", window, cx));
        });
        cx.run_until_parked();
        assert_eq!(rendered_page.get(), 2);

        let harness = harness.expect("the window builder installs the settings harness");
        cx.update(|_, cx| {
            harness.update(cx, |harness, cx| {
                harness.settings_visible = false;
                cx.notify();
            });
        });
        cx.run_until_parked();

        cx.simulate_click(point(px(90.), px(140.)), Modifiers::none());
        assert_eq!(background_clicks.get(), 1);

        cx.update(|_, cx| {
            harness.update(cx, |harness, cx| {
                harness.settings_visible = true;
                cx.notify();
            });
        });
        cx.run_until_parked();

        assert_eq!(rendered_page.get(), 2);
        let restored_bounds = cx
            .debug_bounds("retained-settings-surface")
            .expect("the restored Settings surface must be laid out");
        let restored_page_bounds = cx
            .debug_bounds("retained-second-page")
            .expect("the restored Settings page must be painted");
        assert_eq!(restored_bounds, workspace_bounds);
        assert!(restored_bounds.is_contained_within(&viewport_bounds));
        assert!(restored_page_bounds.is_contained_within(&viewport_bounds));
    }
}
