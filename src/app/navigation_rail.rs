use super::*;

impl ApiTester {
    pub(super) fn render_navigation_rail(&self, cx: &mut Context<Self>) -> AnyElement {
        let request_workspace_active = self.workspace_tabs.active() == ActiveWorkspaceTab::Request;
        let settings_workspace_active = matches!(
            self.workspace_tabs.active(),
            ActiveWorkspaceTab::Settings | ActiveWorkspaceTab::ThemeCss
        );
        let snippets_workspace_active =
            self.workspace_tabs.active() == ActiveWorkspaceTab::Snippets;
        let compact = self.navigation_compact;
        let rail_width = if compact { px(56.) } else { px(116.) };
        let item_width = if compact { px(44.) } else { px(100.) };
        let item_height = if compact { px(44.) } else { px(56.) };

        v_flex()
            .w(rail_width)
            .h_full()
            .flex_shrink_0()
            .items_center()
            .gap_2()
            .py_3()
            .border_r_1()
            .border_color(cx.theme().sidebar_border)
            .bg(cx.api_surface_low())
            .child(
                v_flex()
                    .id("rail-collections")
                    .w(item_width)
                    .h(item_height)
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .rounded_lg()
                    .cursor_pointer()
                    .text_color(cx.theme().muted_foreground)
                    .when(
                        request_workspace_active && self.sidebar_tab == SidebarTab::Collections,
                        |this| {
                            this.bg(cx.theme().sidebar_accent)
                                .text_color(cx.theme().foreground)
                        },
                    )
                    .hover(|style| style.bg(cx.theme().sidebar_accent))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.activate_request_workspace(SidebarTab::Collections, window, cx);
                    }))
                    .when(compact, |this| {
                        this.tooltip(|window, cx| Tooltip::new("Collections").build(window, cx))
                    })
                    .child(gpui_component::Icon::new(IconName::FolderOpen).with_size(px(18.)))
                    .when(!compact, |this| {
                        this.child(
                            div()
                                .text_size(px(10.5))
                                .font_semibold()
                                .child("Collections"),
                        )
                    }),
            )
            .child(
                v_flex()
                    .id("rail-environments")
                    .w(item_width)
                    .h(item_height)
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .rounded_lg()
                    .cursor_pointer()
                    .text_color(cx.theme().muted_foreground)
                    .when(
                        request_workspace_active && self.sidebar_tab == SidebarTab::Environments,
                        |this| {
                            this.bg(cx.theme().sidebar_accent)
                                .text_color(cx.theme().foreground)
                        },
                    )
                    .hover(|style| style.bg(cx.theme().sidebar_accent))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.activate_request_workspace(SidebarTab::Environments, window, cx);
                    }))
                    .when(compact, |this| {
                        this.tooltip(|window, cx| Tooltip::new("Environments").build(window, cx))
                    })
                    .child(gpui_component::Icon::new(IconName::Globe).with_size(px(18.)))
                    .when(!compact, |this| {
                        this.child(
                            div()
                                .text_size(px(10.5))
                                .font_semibold()
                                .child("Environments"),
                        )
                    }),
            )
            .child(
                v_flex()
                    .id("rail-history")
                    .w(item_width)
                    .h(item_height)
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .rounded_lg()
                    .cursor_pointer()
                    .text_color(cx.theme().muted_foreground)
                    .when(
                        request_workspace_active && self.sidebar_tab == SidebarTab::History,
                        |this| {
                            this.bg(cx.theme().sidebar_accent)
                                .text_color(cx.theme().foreground)
                        },
                    )
                    .hover(|style| style.bg(cx.theme().sidebar_accent))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.activate_request_workspace(SidebarTab::History, window, cx);
                    }))
                    .when(compact, |this| {
                        this.tooltip(|window, cx| Tooltip::new("History").build(window, cx))
                    })
                    .child(
                        gpui_component::Icon::new(IconName::GalleryVerticalEnd).with_size(px(18.)),
                    )
                    .when(!compact, |this| {
                        this.child(div().text_size(px(10.5)).font_semibold().child("History"))
                    }),
            )
            .child(
                v_flex()
                    .id("rail-snippets")
                    .w(item_width)
                    .h(item_height)
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .rounded_lg()
                    .cursor_pointer()
                    .text_color(cx.theme().muted_foreground)
                    .when(snippets_workspace_active, |this| {
                        this.bg(cx.theme().sidebar_accent)
                            .text_color(cx.theme().foreground)
                    })
                    .hover(|style| style.bg(cx.theme().sidebar_accent))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_workspace_tool_tab(WorkspaceToolTab::Snippets, window, cx);
                    }))
                    .when(compact, |this| {
                        this.tooltip(|window, cx| Tooltip::new("Snippets").build(window, cx))
                    })
                    .child(gpui_component::Icon::new(IconName::CaseSensitive).with_size(px(18.)))
                    .when(!compact, |this| {
                        this.child(div().text_size(px(10.5)).font_semibold().child("Snippets"))
                    }),
            )
            .child(
                v_flex()
                    .id("rail-settings")
                    .w(item_width)
                    .h(item_height)
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .rounded_lg()
                    .cursor_pointer()
                    .text_color(cx.theme().muted_foreground)
                    .when(settings_workspace_active, |this| {
                        this.bg(cx.theme().sidebar_accent)
                            .text_color(cx.theme().foreground)
                    })
                    .hover(|style| style.bg(cx.theme().sidebar_accent))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_workspace_tool_tab(WorkspaceToolTab::Settings, window, cx);
                    }))
                    .when(compact, |this| {
                        this.tooltip(|window, cx| Tooltip::new("Settings").build(window, cx))
                    })
                    .child(gpui_component::Icon::new(IconName::Settings2).with_size(px(18.)))
                    .when(!compact, |this| {
                        this.child(div().text_size(px(10.5)).font_semibold().child("Settings"))
                    }),
            )
            .child(div().flex_1())
            .child(self.render_upstream_navigation_control(item_width, item_height, compact, cx))
            .child(
                h_flex()
                    .id("rail-compact-toggle")
                    .w(item_width)
                    .h(px(32.))
                    .justify_center()
                    .rounded_lg()
                    .cursor_pointer()
                    .text_color(cx.theme().muted_foreground)
                    .hover(|style| style.bg(cx.theme().sidebar_accent))
                    .tooltip(move |window, cx| {
                        Tooltip::new(if compact {
                            "Expand navigation"
                        } else {
                            "Collapse navigation"
                        })
                        .build(window, cx)
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.navigation_compact = !this.navigation_compact;
                        this.persist_navigation_preference(cx);
                        cx.notify();
                    }))
                    .child(
                        gpui_component::Icon::new(if compact {
                            IconName::ChevronRight
                        } else {
                            IconName::ChevronLeft
                        })
                        .with_size(px(16.)),
                    ),
            )
            .into_any_element()
    }
}
