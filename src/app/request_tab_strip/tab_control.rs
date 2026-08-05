use super::super::*;
use super::tab_drag::{render_workspace_tab_drop_overlay, workspace_tab_drag};

pub(super) struct WorkspaceTabControl {
    tab: WorkspaceTab,
    row_id: SharedString,
    title: String,
    icon: Option<IconName>,
    dirty: bool,
    accent: Option<Hsla>,
    closable: bool,
    pane_id: Option<PaneId>,
    row_debug_selector: Option<&'static str>,
    drag_debug_selector: Option<&'static str>,
}

impl WorkspaceTabControl {
    pub(super) fn new(
        tab: WorkspaceTab,
        row_id: impl Into<SharedString>,
        title: impl Into<String>,
    ) -> Self {
        Self {
            closable: !matches!(tab, WorkspaceTab::Welcome),
            tab,
            row_id: row_id.into(),
            title: title.into(),
            icon: None,
            dirty: false,
            accent: None,
            pane_id: None,
            row_debug_selector: None,
            drag_debug_selector: None,
        }
    }

    /// Associate this tab control with the pane whose strip hosts it so that
    /// reordering is scoped to that pane.
    pub(super) fn pane_opt(mut self, pane_id: Option<PaneId>) -> Self {
        self.pane_id = pane_id;
        self
    }

    pub(super) fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    pub(super) fn dirty(mut self, dirty: bool) -> Self {
        self.dirty = dirty;
        self
    }

    pub(super) fn accent(mut self, accent: Option<Hsla>) -> Self {
        self.accent = accent;
        self
    }

    pub(super) fn debug_selectors(
        mut self,
        row: Option<&'static str>,
        drag: Option<&'static str>,
    ) -> Self {
        self.row_debug_selector = row;
        self.drag_debug_selector = drag;
        self
    }

    pub(super) fn render(self, app: &ApiTester, cx: &mut Context<ApiTester>) -> AnyElement {
        let active = app.workspace_tabs.active_tab(&app.request_tabs) == self.tab;
        let can_reorder = !app.sending;
        let row_group: SharedString = format!("{}-group", self.row_id).into();
        let title_id: SharedString = format!("{}-title", self.row_id).into();
        let close_button_id: SharedString = format!("{}-close", self.row_id).into();
        let context_target = match &self.tab {
            WorkspaceTab::Request(tab_id) => {
                Some(super::RequestTabContextTarget::Tab(tab_id.clone()))
            }
            WorkspaceTab::Tool(tool) => Some(super::RequestTabContextTarget::Tool(tool.clone())),
            WorkspaceTab::Welcome => None,
        };
        let activate_tab = self.tab.clone();
        let close_tab = self.tab.clone();
        let drop_target = self.tab.clone();
        let drag = workspace_tab_drag(self.tab, self.title.clone());
        let display_title = compact_label(&self.title, 28);
        let tooltip = self.title;

        h_flex()
            .id(self.row_id)
            .group(row_group.clone())
            .relative()
            .h_full()
            .flex_shrink_0()
            .min_w(px(148.))
            .max_w(px(240.))
            .px_3()
            .gap_2()
            .border_r_1()
            .border_color(cx.api_outline_variant())
            .cursor_pointer()
            .when_some(self.row_debug_selector, |this, selector| {
                this.debug_selector(move || selector.to_owned())
            })
            .when(active, |this| {
                this.bg(cx.api_surface())
                    .border_b_2()
                    .border_color(cx.theme().primary)
            })
            .when(!active, |this| {
                this.bg(cx.api_surface_low())
                    .hover(|style| style.bg(cx.api_surface_container()))
            })
            .when_some(self.accent, |this, color| {
                this.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(2.))
                        .bg(color),
                )
            })
            .when_some(context_target, |this, context_target| {
                this.on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, _, _, _| {
                        this.request_tab_context_target = Some(context_target.clone());
                    }),
                )
            })
            .when(self.closable, |this| {
                let middle_close = close_tab.clone();
                this.on_mouse_down(
                    MouseButton::Middle,
                    cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        match middle_close.clone() {
                            WorkspaceTab::Request(tab_id) => {
                                this.request_close_request_tab(tab_id, window, cx);
                            }
                            WorkspaceTab::Tool(tool) => {
                                this.close_workspace_tool_tab(tool, window, cx);
                            }
                            WorkspaceTab::Welcome => {}
                        }
                    }),
                )
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                this.activate_workspace_tab(activate_tab.clone(), window, cx);
            }))
            .child(
                h_flex()
                    .id(title_id)
                    .min_w_0()
                    .flex_1()
                    .gap_2()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_sm()
                    .font_medium()
                    .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
                    .when_some(self.drag_debug_selector, |this, selector| {
                        this.debug_selector(move || selector.to_owned())
                    })
                    .when(can_reorder, |this| {
                        this.cursor_move().on_drag(drag, |drag, position, _, cx| {
                            cx.stop_propagation();
                            drag.preview(position, cx)
                        })
                    })
                    .when_some(self.icon, |this, icon| this.child(Icon::new(icon).xsmall()))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(display_title),
                    ),
            )
            .when(self.dirty, |this| {
                this.child(
                    div()
                        .size(px(7.))
                        .flex_shrink_0()
                        .rounded_full()
                        .bg(cx.theme().warning),
                )
            })
            .when(self.closable, |this| {
                this.child(
                    Button::new(close_button_id)
                        .icon(IconName::Close)
                        .xsmall()
                        .ghost()
                        .rounded_full()
                        .tooltip("Close tab")
                        .when(!active, |this| {
                            this.invisible()
                                .group_hover(row_group, |style| style.visible())
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            match close_tab.clone() {
                                WorkspaceTab::Request(tab_id) => {
                                    this.request_close_request_tab(tab_id, window, cx);
                                }
                                WorkspaceTab::Tool(tool) => {
                                    this.close_workspace_tool_tab(tool, window, cx);
                                }
                                WorkspaceTab::Welcome => {}
                            }
                        })),
                )
            })
            .when(can_reorder, |this| {
                this.child(render_workspace_tab_drop_overlay(drop_target, self.pane_id, cx))
            })
            .into_any_element()
    }
}
