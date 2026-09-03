use super::drag_drop::{DropPlacement, WorkspaceTabDrag};
use super::*;

impl ApiTester {
    /// Render the workspace pane tree below the title bar / navigation rail.
    ///
    /// A single-leaf tree renders exactly the legacy strip + content layout so
    /// the common case is unchanged. A split tree renders resizable panes, each
    /// with its own tab strip; the pane hosting the currently active workspace
    /// tab renders the primary surface, while secondary panes render their own
    /// real request editor (see `render_secondary_pane_content`).
    pub(super) fn render_workspace_panes(&self, cx: &mut Context<Self>) -> AnyElement {
        let pane_area = self.render_workspace_pane_area(cx);
        let show_navigation_sidebar = self.workspace_tabs.active() == ActiveWorkspaceTab::Request
            && self.navigation_sidebar_open
            && self.sidebar_tab != SidebarTab::Environments;
        if !show_navigation_sidebar {
            return pane_area;
        }

        h_resizable("workspace-split")
            .child(
                resizable_panel()
                    .size(px(360.))
                    .size_range(px(320.)..px(480.))
                    .child(
                        div()
                            .debug_selector(|| "workspace-navigation-sidebar".to_owned())
                            .size_full()
                            .child(self.render_sidebar(cx)),
                    ),
            )
            .child(resizable_panel().child(pane_area))
            .into_any_element()
    }

    fn render_workspace_pane_area(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.panes.is_single_leaf() {
            return self.render_single_pane_legacy(cx);
        }

        let active_tab = self.workspace_tabs.active_tab(&self.request_tabs);
        let primary_pane_id = self.panes.pane_for_tab(&active_tab);
        v_flex()
            .relative()
            .h_full()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .debug_selector(|| "workspace-pane-area".to_owned())
            .child(self.render_pane_root(&self.panes, primary_pane_id, cx))
            .into_any_element()
    }

    fn render_single_pane_legacy(&self, cx: &mut Context<Self>) -> AnyElement {
        let active_workspace_tab = self.workspace_tabs.active();
        let show_workspace_tab_strip = active_workspace_tab != ActiveWorkspaceTab::Request
            || self.sidebar_tab != SidebarTab::Environments;
        let workspace = self.render_active_workspace_surface(cx);
        let snippets_workspace = self.workspace_tabs.snippets_open().then(|| {
            let active = active_workspace_tab;
            div()
                .size_full()
                .min_h_0()
                .when(active != ActiveWorkspaceTab::Snippets, |this| this.hidden())
                .child(self.render_snippets_workspace(cx))
        });
        let request_proxy_workspace = self.workspace_tabs.request_proxy_open().then(|| {
            let active = active_workspace_tab;
            div()
                .size_full()
                .min_h_0()
                .when(active != ActiveWorkspaceTab::RequestProxy, |this| {
                    this.hidden()
                })
                .child(self.render_request_proxy_workspace(cx))
        });
        let server_tools_workspace = self.workspace_tabs.server_tools_open().then(|| {
            let active = active_workspace_tab;
            div()
                .size_full()
                .min_h_0()
                .when(active != ActiveWorkspaceTab::ServerTools, |this| {
                    this.hidden()
                })
                .child(self.render_server_tools_workspace(cx))
        });
        let settings_workspace = self.workspace_tabs.settings_open().then(|| {
            let active = active_workspace_tab;
            div()
                .size_full()
                .min_h_0()
                .when(active != ActiveWorkspaceTab::Settings, |this| this.hidden())
                .child(self.render_settings_workspace(cx))
        });
        // A single pane still exposes edge drop zones so the first split can
        // be created by dragging a tab to the pane's edge.
        let pane_id = self.panes.panes().first().map(|pane| pane.id());
        let split_overlay = pane_id
            .map(|pane_id| render_pane_dock_overlays(pane_id, cx))
            .unwrap_or_default();

        v_flex()
            .relative()
            .h_full()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .children(show_workspace_tab_strip.then(|| self.render_request_tab_strip(cx)))
            .child(
                div()
                    .when_some(pane_id, |this, pane_id| {
                        this.group(pane_dock_group(pane_id))
                    })
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(workspace)
                    .children(snippets_workspace)
                    .children(request_proxy_workspace)
                    .children(server_tools_workspace)
                    .children(settings_workspace)
                    .children(split_overlay),
            )
            .into_any_element()
    }

    /// The content surface shown for the currently active workspace tab, as
    /// rendered before panes were introduced.
    fn render_active_workspace_surface(&self, cx: &mut Context<Self>) -> AnyElement {
        match self.workspace_tabs.active() {
            ActiveWorkspaceTab::Welcome => div()
                .size_full()
                .min_h_0()
                .child(self.render_welcome_page(cx))
                .into_any_element(),
            ActiveWorkspaceTab::Snippets => div().hidden().into_any_element(),
            ActiveWorkspaceTab::RequestProxy => div().hidden().into_any_element(),
            ActiveWorkspaceTab::ServerTools => div().hidden().into_any_element(),
            ActiveWorkspaceTab::Settings => div().hidden().into_any_element(),
            ActiveWorkspaceTab::ThemeCss => div()
                .size_full()
                .min_h_0()
                .child(self.render_theme_css_workspace(cx))
                .into_any_element(),
            ActiveWorkspaceTab::Request if self.sidebar_tab == SidebarTab::Environments => {
                self.render_environment_workspace(cx)
            }
            ActiveWorkspaceTab::Request if self.request_tabs.active().template().is_websocket() => {
                self.render_websocket_workspace(cx)
            }
            ActiveWorkspaceTab::Request => v_flex()
                .size_full()
                .min_h_0()
                .child(
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
                )
                .into_any_element(),
        }
    }

    fn render_pane_root(
        &self,
        root: &PaneRoot,
        primary_pane_id: Option<PaneId>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match root {
            PaneRoot::Leaf(pane) => self.render_pane(pane, primary_pane_id == Some(pane.id()), cx),
            PaneRoot::Split(split) => {
                let anchor_id = split
                    .children
                    .first()
                    .and_then(|child| match child {
                        PaneRoot::Leaf(pane) => Some(pane.id()),
                        PaneRoot::Split(_) => None,
                    })
                    .map(|id| id.0)
                    .unwrap_or(0);
                let mut resizer = match split.direction {
                    SplitDirection::Horizontal => h_resizable(SharedString::from(format!(
                        "workspace-pane-split-h-{anchor_id}"
                    ))),
                    SplitDirection::Vertical => v_resizable(SharedString::from(format!(
                        "workspace-pane-split-v-{anchor_id}"
                    ))),
                };
                for child in &split.children {
                    resizer = resizer.child(
                        resizable_panel()
                            .size_range(px(180.)..px(2_000.))
                            .child(self.render_pane_root(child, primary_pane_id, cx)),
                    );
                }
                resizer.into_any_element()
            }
        }
    }

    fn render_pane(&self, pane: &Pane, is_primary: bool, cx: &mut Context<Self>) -> AnyElement {
        let tabs = pane.tabs();
        let pane_id = pane.id();
        let strip = self.render_request_tab_strip_with(&tabs, Some(pane_id), cx);
        let content = self.render_pane_content(pane, is_primary, cx);
        let split_overlay = render_pane_dock_overlays(pane_id, cx);
        v_flex()
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .child(strip)
            .child(
                div()
                    .group(pane_dock_group(pane_id))
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(content)
                    .children(split_overlay),
            )
            .into_any_element()
    }

    fn render_pane_content(
        &self,
        pane: &Pane,
        is_primary: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // A non-primary pane either hosts a real per-pane request editor for
        // its active request tab, or a lightweight surface for tool/welcome
        // tabs. Sessions are kept coherent by `reconcile_pane_editors`.
        if !is_primary {
            return self.render_secondary_pane_content(pane, cx);
        }

        // The primary pane's content is the existing single surface plus the
        // retained open-tool surfaces (Snippets/Settings) so tools keep their
        // GPUI state while another tab is active.
        let active_workspace_tab = self.workspace_tabs.active();
        let workspace = self.render_active_workspace_surface(cx);
        let snippets_workspace = self.workspace_tabs.snippets_open().then(|| {
            let active = active_workspace_tab;
            div()
                .size_full()
                .min_h_0()
                .when(active != ActiveWorkspaceTab::Snippets, |this| this.hidden())
                .child(self.render_snippets_workspace(cx))
        });
        let request_proxy_workspace = self.workspace_tabs.request_proxy_open().then(|| {
            let active = active_workspace_tab;
            div()
                .size_full()
                .min_h_0()
                .when(active != ActiveWorkspaceTab::RequestProxy, |this| {
                    this.hidden()
                })
                .child(self.render_request_proxy_workspace(cx))
        });
        let server_tools_workspace = self.workspace_tabs.server_tools_open().then(|| {
            let active = active_workspace_tab;
            div()
                .size_full()
                .min_h_0()
                .when(active != ActiveWorkspaceTab::ServerTools, |this| {
                    this.hidden()
                })
                .child(self.render_server_tools_workspace(cx))
        });
        let settings_workspace = self.workspace_tabs.settings_open().then(|| {
            let active = active_workspace_tab;
            div()
                .size_full()
                .min_h_0()
                .when(active != ActiveWorkspaceTab::Settings, |this| this.hidden())
                .child(self.render_settings_workspace(cx))
        });

        div()
            .size_full()
            .min_h_0()
            .child(workspace)
            .children(snippets_workspace)
            .children(request_proxy_workspace)
            .children(server_tools_workspace)
            .children(settings_workspace)
            .into_any_element()
    }
}

#[derive(Clone, Copy)]
enum PaneDockTarget {
    Center,
    Split {
        direction: SplitDirection,
        after: bool,
    },
}

fn pane_dock_group(pane_id: PaneId) -> SharedString {
    format!("workspace-pane-dock-group-{}", pane_id.0).into()
}

/// Keep the broad edge targets for forgiving drops, and add a five-way dock
/// compass like a traditional IDE. The compass is only revealed while a
/// workspace tab is over this pane's group.
fn render_pane_dock_overlays(pane_id: PaneId, cx: &mut Context<ApiTester>) -> Vec<AnyElement> {
    vec![
        render_workspace_pane_split_edge(
            pane_id,
            DropPlacement::Before,
            SplitDirection::Horizontal,
            "left",
            cx,
        ),
        render_workspace_pane_split_edge(
            pane_id,
            DropPlacement::After,
            SplitDirection::Horizontal,
            "right",
            cx,
        ),
        render_workspace_pane_split_edge(
            pane_id,
            DropPlacement::Before,
            SplitDirection::Vertical,
            "top",
            cx,
        ),
        render_workspace_pane_split_edge(
            pane_id,
            DropPlacement::After,
            SplitDirection::Vertical,
            "bottom",
            cx,
        ),
        render_pane_dock_compass(pane_id, cx),
    ]
}

fn render_pane_dock_compass(pane_id: PaneId, cx: &mut Context<ApiTester>) -> AnyElement {
    div()
        .absolute()
        .left(gpui::relative(0.5))
        .top(gpui::relative(0.5))
        .ml(-px(78.))
        .mt(-px(78.))
        .size(px(156.))
        .child(render_pane_dock_target(
            pane_id,
            PaneDockTarget::Split {
                direction: SplitDirection::Vertical,
                after: false,
            },
            "top",
            px(54.),
            px(0.),
            cx,
        ))
        .child(render_pane_dock_target(
            pane_id,
            PaneDockTarget::Split {
                direction: SplitDirection::Horizontal,
                after: false,
            },
            "left",
            px(0.),
            px(54.),
            cx,
        ))
        .child(render_pane_dock_target(
            pane_id,
            PaneDockTarget::Center,
            "center",
            px(54.),
            px(54.),
            cx,
        ))
        .child(render_pane_dock_target(
            pane_id,
            PaneDockTarget::Split {
                direction: SplitDirection::Horizontal,
                after: true,
            },
            "right",
            px(108.),
            px(54.),
            cx,
        ))
        .child(render_pane_dock_target(
            pane_id,
            PaneDockTarget::Split {
                direction: SplitDirection::Vertical,
                after: true,
            },
            "bottom",
            px(54.),
            px(108.),
            cx,
        ))
        .into_any_element()
}

fn render_pane_dock_target(
    pane_id: PaneId,
    target: PaneDockTarget,
    tag: &'static str,
    left: Pixels,
    top: Pixels,
    cx: &mut Context<ApiTester>,
) -> AnyElement {
    let id: SharedString = format!("workspace-pane-dock-{tag}-{}", pane_id.0).into();
    div()
        .id(id)
        .absolute()
        .left(left)
        .top(top)
        .size(px(48.))
        .rounded_md()
        .border_1()
        .border_color(cx.api_outline_variant())
        .bg(cx.api_surface().opacity(0.96))
        .shadow_md()
        .invisible()
        .group_drag_over::<WorkspaceTabDrag>(pane_dock_group(pane_id), |style| style.visible())
        .debug_selector(move || format!("workspace-pane-dock-{tag}-{}", pane_id.0))
        .flex()
        .items_center()
        .justify_center()
        .can_drop(|value, _, _| value.downcast_ref::<WorkspaceTabDrag>().is_some())
        .drag_over::<WorkspaceTabDrag>(|style, _, _, cx| {
            style
                .border_2()
                .border_color(cx.theme().drag_border)
                .bg(cx.theme().drop_target)
        })
        .on_drop(cx.listener(
            move |this, drag: &WorkspaceTabDrag, window, cx| match target {
                PaneDockTarget::Center => {
                    this.on_workspace_tab_move(drag, pane_id, usize::MAX, window, cx);
                }
                PaneDockTarget::Split { direction, after } => {
                    this.on_workspace_tab_split(drag, pane_id, direction, after, window, cx);
                }
            },
        ))
        .child(render_pane_dock_glyph(tag, cx))
        .into_any_element()
}

fn render_pane_dock_glyph(tag: &'static str, cx: &mut Context<ApiTester>) -> AnyElement {
    let accent = cx.theme().drag_border.opacity(0.55);
    div()
        .relative()
        .w(px(26.))
        .h(px(22.))
        .rounded_sm()
        .border_1()
        .border_color(cx.api_outline_variant())
        .bg(cx.theme().background)
        .child(
            div()
                .absolute()
                .when(tag == "center", |this| this.inset_1())
                .when(tag == "left", |this| {
                    this.left_0().top_0().bottom_0().w(gpui::relative(0.45))
                })
                .when(tag == "right", |this| {
                    this.right_0().top_0().bottom_0().w(gpui::relative(0.45))
                })
                .when(tag == "top", |this| {
                    this.top_0().left_0().right_0().h(gpui::relative(0.45))
                })
                .when(tag == "bottom", |this| {
                    this.bottom_0().left_0().right_0().h(gpui::relative(0.45))
                })
                .rounded_sm()
                .bg(accent),
        )
        .into_any_element()
}

fn render_workspace_pane_split_edge(
    pane_id: PaneId,
    placement: DropPlacement,
    direction: SplitDirection,
    tag: &'static str,
    cx: &mut Context<ApiTester>,
) -> AnyElement {
    let zone_id: SharedString =
        SharedString::from(format!("workspace-pane-split-{tag}-{}", pane_id.0));
    let after = placement == DropPlacement::After;
    let vertical = direction == SplitDirection::Vertical;
    let leading = placement == DropPlacement::Before;

    div()
        .id(zone_id)
        .absolute()
        .debug_selector(move || format!("workspace-pane-split-{tag}").to_owned())
        .when(vertical && leading, |this| {
            this.top_0().left_0().right_0().h(px(64.))
        })
        .when(vertical && after, |this| {
            this.bottom_0().left_0().right_0().h(px(64.))
        })
        .when(!vertical && leading, |this| {
            this.left_0().top_0().bottom_0().w(px(64.))
        })
        .when(!vertical && after, |this| {
            this.right_0().top_0().bottom_0().w(px(64.))
        })
        .can_drop(move |value, _, _| value.downcast_ref::<WorkspaceTabDrag>().is_some())
        .drag_over::<WorkspaceTabDrag>(move |style, _, _, cx| {
            style.bg(cx.theme().drop_target.opacity(0.5))
        })
        .on_drop(
            cx.listener(move |this, drag: &WorkspaceTabDrag, window, cx| {
                this.on_workspace_tab_split(drag, pane_id, direction, after, window, cx);
            }),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Modifiers, MouseButton, TestAppContext, point, px, size};

    #[gpui::test]
    fn dock_compass_splits_and_merges_workspace_panes(cx: &mut TestAppContext) {
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
        cx.run_until_parked();

        let original_pane_id = cx.update(|_, cx| {
            let app = app.read(cx);
            assert!(
                app.panes.is_single_leaf(),
                "a fresh app must start as a single pane"
            );
            app.panes.panes()[0].id()
        });

        let drag_handle = cx
            .debug_bounds("current-workspace-request-tab-drag-handle")
            .expect("the current request must expose the shared tab drag handle");
        let right_edge = cx
            .debug_bounds("workspace-pane-split-right")
            .expect("a single pane must retain its broad right-edge drop zone");
        let source = drag_handle.center();
        cx.simulate_mouse_down(source, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_move(
            point(source.x + px(8.), source.y),
            MouseButton::Left,
            Modifiers::none(),
        );
        cx.simulate_mouse_move(right_edge.center(), MouseButton::Left, Modifiers::none());
        cx.run_until_parked();
        let right_selector: &'static str =
            Box::leak(format!("workspace-pane-dock-right-{}", original_pane_id.0).into_boxed_str());
        let right_target = cx
            .debug_bounds(right_selector)
            .expect("a single pane must expose the right dock-compass target");
        assert_eq!(right_target.size, size(px(48.), px(48.)));
        cx.simulate_mouse_move(right_target.center(), MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(right_target.center(), MouseButton::Left, Modifiers::none());
        cx.run_until_parked();

        cx.update(|_, cx| {
            let app = app.read(cx);
            assert!(
                !app.panes.is_single_leaf(),
                "dropping on the dock compass must split the single pane"
            );
            assert_eq!(app.panes.len(), 2);
        });
        let sidebar = cx
            .debug_bounds("workspace-navigation-sidebar")
            .expect("Collections must remain a shell-level sidebar while panes are split");
        let pane_area = cx
            .debug_bounds("workspace-pane-area")
            .expect("the split pane area must be independently addressable");
        assert!(
            sidebar.right() <= pane_area.left(),
            "the Collections sidebar must sit outside request pane contents",
        );

        let split_drag_handle = cx
            .debug_bounds("current-workspace-request-tab-drag-handle")
            .expect("the split request must remain draggable");
        let original_right_edge = cx
            .debug_bounds("workspace-pane-split-right")
            .expect("the original pane must retain its broad right-edge drop zone");
        let source = split_drag_handle.center();
        cx.simulate_mouse_down(source, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_move(
            point(source.x + px(8.), source.y),
            MouseButton::Left,
            Modifiers::none(),
        );
        cx.simulate_mouse_move(
            original_right_edge.center(),
            MouseButton::Left,
            Modifiers::none(),
        );
        cx.run_until_parked();
        let center_selector: &'static str = Box::leak(
            format!("workspace-pane-dock-center-{}", original_pane_id.0).into_boxed_str(),
        );
        let center_target = cx
            .debug_bounds(center_selector)
            .expect("the empty original pane must expose a center dock-compass target");
        cx.simulate_mouse_move(center_target.center(), MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(center_target.center(), MouseButton::Left, Modifiers::none());
        cx.run_until_parked();

        cx.update(|_, cx| {
            let app = app.read(cx);
            assert!(
                app.panes.is_single_leaf(),
                "dropping the tab on the center target must merge and unsplit the workspace",
            );
        });

        let active_request = cx.update(|_, cx| app.read(cx).request_tabs.active_tab_id().clone());
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.request_close_request_tab(active_request.clone(), window, cx);
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.activate_request_workspace(SidebarTab::Collections, window, cx);
            });
        });
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("workspace-navigation-sidebar").is_some(),
            "Collections must be reopenable after the request tab that was visible beside it closes",
        );
    }

    #[gpui::test]
    fn dropping_a_split_tab_beside_an_original_tab_unsplits_the_workspace(cx: &mut TestAppContext) {
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
            app.update(cx, |app, cx| app.open_blank_request_tab(window, cx));
        });
        cx.run_until_parked();

        let (first, second, original_pane) = cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                let first = app.request_tabs.tabs()[0].id().clone();
                let second = app.request_tabs.tabs()[1].id().clone();
                let original_pane = app.panes.panes()[0].id();
                let split_pane = app
                    .panes
                    .split_off_pane(original_pane, SplitDirection::Vertical, true)
                    .expect("split original pane");
                assert!(app.panes.move_tab_between_panes(
                    &WorkspaceTab::Request(second.clone()),
                    original_pane,
                    split_pane,
                    0,
                ));
                app.reconcile_pane_editors(window, cx);
                (first, second, original_pane)
            })
        });
        assert_eq!(cx.update(|_, cx| app.read(cx).panes.len()), 2);

        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.reorder_workspace_tab_from_drop(
                    WorkspaceTab::Request(second.clone()),
                    WorkspaceTab::Request(first.clone()),
                    DropPlacement::After,
                    Some(original_pane),
                    window,
                    cx,
                );
            });
        });

        cx.update(|_, cx| {
            let app = app.read(cx);
            assert!(app.panes.is_single_leaf());
            assert_eq!(
                app.panes.active_tabs(),
                vec![WorkspaceTab::Request(first), WorkspaceTab::Request(second)],
            );
        });
    }
}
