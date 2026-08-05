use super::drag_drop::{DropPlacement, WorkspaceTabDrag};
use super::*;

impl ApiTester {
    /// Render the workspace pane tree below the title bar / navigation rail.
    ///
    /// A single-leaf tree renders exactly the legacy strip + content layout so
    /// the common case is unchanged. A split tree renders resizable panes, each
    /// with its own tab strip; only the pane hosting the currently active
    /// workspace tab renders real content, while other panes show a placeholder
    /// (per-pane request editor state is a follow-up).
    pub(super) fn render_workspace_panes(&self, cx: &mut Context<Self>) -> AnyElement {
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
        let settings_workspace = self.workspace_tabs.settings_open().then(|| {
            let active = active_workspace_tab;
            div()
                .size_full()
                .min_h_0()
                .when(active != ActiveWorkspaceTab::Settings, |this| this.hidden())
                .child(self.render_settings_workspace(cx))
        });

        v_flex()
            .relative()
            .h_full()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .children(
                show_workspace_tab_strip.then(|| self.render_request_tab_strip(cx)),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(workspace)
                    .children(snippets_workspace)
                    .children(settings_workspace),
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
            ActiveWorkspaceTab::Settings => div().hidden().into_any_element(),
            ActiveWorkspaceTab::ThemeCss => div()
                .size_full()
                .min_h_0()
                .child(self.render_theme_css_workspace(cx))
                .into_any_element(),
            ActiveWorkspaceTab::Request if self.sidebar_tab == SidebarTab::Environments => {
                self.render_environment_workspace(cx)
            }
            ActiveWorkspaceTab::Request => div()
                .size_full()
                .min_h_0()
                .child(
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
            PaneRoot::Leaf(pane) => {
                self.render_pane(pane, primary_pane_id == Some(pane.id()), cx)
            }
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
        let split_overlay = render_pane_split_drop_zones(pane_id, cx);
        v_flex()
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .child(strip)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(content)
                    .children(split_overlay),
            )
            .into_any_element()
    }

    fn render_pane_content(&self, pane: &Pane, is_primary: bool, cx: &mut Context<Self>) -> AnyElement {
        if !is_primary {
            let pane_id = pane.id();
            let insert_index = pane.tabs().len();
            return v_flex()
                .size_full()
                .flex_1()
                .min_h_0()
                .items_center()
                .justify_center()
                .gap_2()
                .can_drop(move |value, _, _| {
                    value
                        .downcast_ref::<WorkspaceTabDrag>()
                        .is_some_and(|drag| {
                            drag.tab != WorkspaceTab::Welcome
                        })
                })
                .drag_over::<WorkspaceTabDrag>(move |style, _, _, cx| {
                    style.bg(cx.theme().drop_target.opacity(0.35))
                })
                .on_drop(cx.listener(move |this, drag: &WorkspaceTabDrag, _, cx| {
                    this.on_workspace_tab_move(drag, pane_id, insert_index, cx);
                }))
                .child(
                    v_flex()
                        .items_center()
                        .gap_2()
                        .text_color(cx.theme().muted_foreground)
                        .child(
                            div()
                                .size(px(40.))
                                .rounded_full()
                                .bg(cx.api_surface_low())
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    div()
                                        .size_full()
                                        .rounded_full()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_xs()
                                        .child("+"),
                                ),
                        )
                        .child(
                            div().text_sm().child(
                                if pane.tabs().is_empty() {
                                    "No tabs in this pane"
                                } else {
                                    "Secondary pane"
                                }
                                .to_owned(),
                            ),
                        )
                        .child(
                            div().text_xs().child(
                                "Per-pane request editors are a follow-up; drop a tab here or use the strip above to move tabs.",
                            ),
                        ),
                )
                .into_any_element();
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
            .children(settings_workspace)
            .into_any_element()
    }
}

/// Render left/right/top/bottom edge drop zones around a pane's content area
/// that create a new split containing the dragged tab when dropped on.
fn render_pane_split_drop_zones(
    pane_id: PaneId,
    cx: &mut Context<ApiTester>,
) -> Vec<AnyElement> {
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
    ]
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
        .when(vertical && leading, |this| this.top_0().left_0().right_0().h(px(16.)))
        .when(vertical && after, |this| {
            this.bottom_0().left_0().right_0().h(px(16.))
        })
        .when(!vertical && leading, |this| {
            this.left_0().top_0().bottom_0().w(px(16.))
        })
        .when(!vertical && after, |this| {
            this.right_0().top_0().bottom_0().w(px(16.))
        })
        .can_drop(move |value, _, _| {
            value.downcast_ref::<WorkspaceTabDrag>().is_some()
        })
        .drag_over::<WorkspaceTabDrag>(move |style, _, _, cx| {
            style.bg(cx.theme().drop_target.opacity(0.5))
        })
        .on_drop(cx.listener(move |this, drag: &WorkspaceTabDrag, _, cx| {
            this.on_workspace_tab_split(drag, pane_id, direction, after, cx);
        }))
        .into_any_element()
}
