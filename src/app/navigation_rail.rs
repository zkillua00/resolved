use super::*;

impl ApiTester {
    pub(super) fn render_navigation_rail(&self, cx: &mut Context<Self>) -> AnyElement {
        let request_workspace_active = self.workspace_tabs.active() == ActiveWorkspaceTab::Request;
        let settings_workspace_active = matches!(
            self.workspace_tabs.active(),
            ActiveWorkspaceTab::Snippets
                | ActiveWorkspaceTab::Settings
                | ActiveWorkspaceTab::ThemeCss
        );
        let request_proxy_workspace_active =
            self.workspace_tabs.active() == ActiveWorkspaceTab::RequestProxy;
        let server_tools_workspace_active =
            self.workspace_tabs.active() == ActiveWorkspaceTab::ServerTools;
        let using_server = matches!(
            self.workspace_providers.active_id(),
            WorkspaceProviderId::Upstream { .. }
        );
        let compact = self.navigation_compact;
        let rail_width = if compact { rems(3.5) } else { rems(7.25) };
        let item_width = if compact { rems(2.75) } else { rems(6.25) };
        let item_height = if compact { rems(2.75) } else { rems(3.5) };

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
                    .debug_selector(|| "rail-collections".to_owned())
                    .w(item_width)
                    .h(item_height)
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .rounded_lg()
                    .cursor_pointer()
                    .text_color(cx.theme().muted_foreground)
                    .when(
                        request_workspace_active
                            && self.navigation_sidebar_open
                            && self.sidebar_tab == SidebarTab::Collections,
                        |this| {
                            this.bg(cx.theme().sidebar_accent)
                                .text_color(cx.theme().foreground)
                        },
                    )
                    .hover(|style| style.bg(cx.theme().sidebar_accent))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_navigation_sidebar(SidebarTab::Collections, window, cx);
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
                        request_workspace_active
                            && self.navigation_sidebar_open
                            && self.sidebar_tab == SidebarTab::History,
                        |this| {
                            this.bg(cx.theme().sidebar_accent)
                                .text_color(cx.theme().foreground)
                        },
                    )
                    .hover(|style| style.bg(cx.theme().sidebar_accent))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_navigation_sidebar(SidebarTab::History, window, cx);
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
            .children(using_server.then(|| {
                v_flex()
                    .id("rail-request-proxy")
                    .debug_selector(|| "rail-request-proxy".to_owned())
                    .w(item_width)
                    .h(item_height)
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .rounded_lg()
                    .cursor_pointer()
                    .text_color(cx.theme().muted_foreground)
                    .when(request_proxy_workspace_active, |this| {
                        this.bg(cx.theme().sidebar_accent)
                            .text_color(cx.theme().foreground)
                    })
                    .hover(|style| style.bg(cx.theme().sidebar_accent))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_workspace_tool_tab(WorkspaceToolTab::RequestProxy, window, cx);
                    }))
                    .when(compact, |this| {
                        this.tooltip(|window, cx| Tooltip::new("Request proxy").build(window, cx))
                    })
                    .child(gpui_component::Icon::new(IconName::Replace).with_size(px(18.)))
                    .when(!compact, |this| {
                        this.child(div().text_size(px(10.5)).font_semibold().child("Proxy"))
                    })
                    .into_any_element()
            }))
            .children(using_server.then(|| {
                v_flex()
                    .id("rail-server-tools")
                    .debug_selector(|| "rail-server-tools".to_owned())
                    .w(item_width)
                    .h(item_height)
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .rounded_lg()
                    .cursor_pointer()
                    .text_color(cx.theme().muted_foreground)
                    .when(server_tools_workspace_active, |this| {
                        this.bg(cx.theme().sidebar_accent)
                            .text_color(cx.theme().foreground)
                    })
                    .hover(|style| style.bg(cx.theme().sidebar_accent))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_workspace_tool_tab(WorkspaceToolTab::ServerTools, window, cx);
                    }))
                    .when(compact, |this| {
                        this.tooltip(|window, cx| Tooltip::new("Server Tools").build(window, cx))
                    })
                    .child(gpui_component::Icon::new(IconName::Inspector).with_size(px(18.)))
                    .when(!compact, |this| {
                        this.child(
                            div()
                                .text_size(px(10.5))
                                .font_semibold()
                                .child("Server Tools"),
                        )
                    })
                    .into_any_element()
            }))
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
            .child(self.render_upstream_navigation_control(
                item_width.to_pixels(cx.theme().font_size),
                item_height.to_pixels(cx.theme().font_size),
                compact,
                cx,
            ))
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

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, px, size};

    #[gpui::test]
    fn sidebar_rail_buttons_toggle_their_active_sidebar(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().expect("create temporary database directory");
        let store = DatabaseStore::new(directory.path().join("api-tester.sqlite3"));
        store.initialize().expect("initialize test database");

        let mut app = None;
        let store_for_app = store.clone();
        let (_, cx) = cx.add_window_view(|window, cx| {
            gpui_component::init(cx);
            let base_key_bindings = shortcuts::capture_base_key_bindings(cx);
            crate::theme::configure(cx);
            let view = cx.new(|cx| {
                ApiTester::new_with_database_store(base_key_bindings, store_for_app, window, cx)
            });
            crate::register_app_action_handlers(&view, cx);
            app = Some(view.clone());
            gpui_component::Root::new(view, window, cx)
        });
        let app = app.expect("capture app entity");
        cx.update(|window, _| window.activate_window());
        cx.simulate_resize(size(px(1_200.), px(800.)));
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.activate_request_workspace(SidebarTab::Collections, window, cx);
            });
        });
        cx.run_until_parked();

        assert!(cx.debug_bounds("workspace-navigation-sidebar").is_some());
        assert!(cx.debug_bounds("rail-collections").is_some());
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.toggle_navigation_sidebar(SidebarTab::Collections, window, cx)
            });
        });
        cx.run_until_parked();
        assert!(
            !cx.update(|_, cx| app.read(cx).navigation_sidebar_open),
            "clicking the active Collections button must close the sidebar",
        );

        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.toggle_navigation_sidebar(SidebarTab::Collections, window, cx)
            });
        });
        cx.run_until_parked();
        assert!(
            cx.update(|_, cx| app.read(cx).navigation_sidebar_open),
            "clicking Collections again must reopen the sidebar",
        );

        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.activate_request_workspace(SidebarTab::History, window, cx);
                app.toggle_navigation_sidebar(SidebarTab::History, window, cx);
            });
        });
        cx.run_until_parked();
        assert!(
            !cx.update(|_, cx| app.read(cx).navigation_sidebar_open),
            "clicking the active History button must close the sidebar",
        );

        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.toggle_navigation_sidebar(SidebarTab::History, window, cx)
            });
        });
        cx.run_until_parked();
        assert!(
            cx.update(|_, cx| {
                let app = app.read(cx);
                app.navigation_sidebar_open && app.sidebar_tab == SidebarTab::History
            }),
            "clicking History again must reopen the History sidebar",
        );
    }
}
