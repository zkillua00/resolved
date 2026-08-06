use super::super::drag_drop::{DropPlacement, WorkspaceTabDrag};
use super::super::*;

fn render_workspace_tab_drop_zone(
    target: WorkspaceTab,
    placement: DropPlacement,
    pane_id: Option<PaneId>,
    cx: &mut Context<ApiTester>,
) -> AnyElement {
    let placement_label = match placement {
        DropPlacement::Before => "before",
        DropPlacement::After => "after",
        DropPlacement::Inside => unreachable!("workspace tabs only support before/after drops"),
    };
    let target_key = workspace_tab_dom_key(&target);
    let zone_id: SharedString = format!("workspace-tab-drop-{placement_label}-{target_key}").into();
    let predicate_target = target.clone();
    let drop_target = target;

    div()
        .id(zone_id)
        .h_full()
        .flex_1()
        .can_drop(move |value, _, _| {
            value
                .downcast_ref::<WorkspaceTabDrag>()
                .is_some_and(|drag| drag.tab != predicate_target)
        })
        .drag_over::<WorkspaceTabDrag>(move |style, _, _, cx| {
            let style = style.bg(cx.theme().drop_target);
            match placement {
                DropPlacement::Before => style.border_l_2().border_color(cx.theme().drag_border),
                DropPlacement::After => style.border_r_2().border_color(cx.theme().drag_border),
                DropPlacement::Inside => style,
            }
        })
        .on_drop(cx.listener(move |this, drag: &WorkspaceTabDrag, _, cx| {
            this.reorder_workspace_tab_from_drop(
                drag.tab.clone(),
                drop_target.clone(),
                placement,
                pane_id,
                cx,
            );
        }))
        .into_any_element()
}

pub(super) fn render_workspace_tab_drop_overlay(
    target: WorkspaceTab,
    pane_id: Option<PaneId>,
    cx: &mut Context<ApiTester>,
) -> AnyElement {
    let before_target = target.clone();
    h_flex()
        .absolute()
        .top_0()
        .right_0()
        .bottom_0()
        .left_0()
        .child(render_workspace_tab_drop_zone(
            before_target,
            DropPlacement::Before,
            pane_id,
            cx,
        ))
        .child(render_workspace_tab_drop_zone(
            target,
            DropPlacement::After,
            pane_id,
            cx,
        ))
        .into_any_element()
}

pub(super) fn workspace_tab_drag(
    tab: WorkspaceTab,
    label: impl Into<SharedString>,
) -> WorkspaceTabDrag {
    WorkspaceTabDrag::new(tab, label)
}

fn workspace_tab_dom_key(tab: &WorkspaceTab) -> String {
    match tab {
        WorkspaceTab::Welcome => "welcome".to_owned(),
        WorkspaceTab::Request(tab_id) => format!("request-{}", tab_id.as_str()),
        WorkspaceTab::Tool(WorkspaceToolTab::Snippets) => "snippets".to_owned(),
        WorkspaceTab::Tool(WorkspaceToolTab::Settings) => "settings".to_owned(),
        WorkspaceTab::Tool(WorkspaceToolTab::ThemeCss(editor_id)) => {
            format!("theme-css-{editor_id}")
        }
    }
}

impl ApiTester {
    pub(super) fn reorder_workspace_tab_from_drop(
        &mut self,
        dragged: WorkspaceTab,
        target: WorkspaceTab,
        placement: DropPlacement,
        pane_id: Option<PaneId>,
        cx: &mut Context<Self>,
    ) {
        if self.sending || dragged == target || placement == DropPlacement::Inside {
            return;
        }

        // When the drop happens in a specific pane, reorder within that pane's
        // tab list. The shared (single-pane) strip still mirrors
        // `workspace_tabs` for backward compatibility.
        let within_pane = pane_id.is_some_and(|pane_id| {
            self.panes
                .reorder_within_pane(pane_id, &dragged, &target, placement == DropPlacement::After)
        });
        if !self.workspace_tabs.reorder_tab(
            &self.request_tabs,
            &dragged,
            &target,
            placement == DropPlacement::After,
        ) && !within_pane
        {
            return;
        }

        let request_order_changed = match dragged {
            WorkspaceTab::Request(tab_id) if self.request_tabs_writable => self
                .workspace_tabs
                .align_request_tab_order(&mut self.request_tabs, &tab_id),
            WorkspaceTab::Welcome | WorkspaceTab::Request(_) | WorkspaceTab::Tool(_) => false,
        };
        if request_order_changed {
            self.persist_request_tabs_now(cx);
        }
        cx.notify();
    }

    /// Create a new split adjacent to `target_pane_id` and place the dragged
    /// tab into the freshly created pane.
    pub(in crate::app) fn on_workspace_tab_split(
        &mut self,
        drag: &WorkspaceTabDrag,
        target_pane_id: PaneId,
        direction: SplitDirection,
        after: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.sending {
            return;
        }
        if let Some(source_pane_id) = self.panes.pane_for_tab(&drag.tab)
            && source_pane_id == target_pane_id
            && !self.panes.is_single_leaf()
        {
            return;
        }
        let Ok(new_pane_id) = self.panes.split_off_pane(target_pane_id, direction, after) else {
            return;
        };
        if let Some(source_pane_id) = self.panes.pane_for_tab(&drag.tab) {
            self.panes
                .move_tab_between_panes(&drag.tab, source_pane_id, new_pane_id, 0);
        } else if let Some(pane) = self.panes.pane_mut(new_pane_id) {
            pane.insert_or_activate(drag.tab.clone());
        }
        self.sync_workspace_tabs_from_panes(cx);
        self.reconcile_pane_editors(window, cx);
        cx.notify();
    }

    /// Move the dragged tab into `target_pane_id` at `index`.
    pub(in crate::app) fn on_workspace_tab_move(
        &mut self,
        drag: &WorkspaceTabDrag,
        target_pane_id: PaneId,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.sending {
            return;
        }
        let Some(source_pane_id) = self.panes.pane_for_tab(&drag.tab) else {
            return;
        };
        if source_pane_id == target_pane_id {
            return;
        }
        if self.panes.move_tab_between_panes(&drag.tab, source_pane_id, target_pane_id, index) {
            self.sync_workspace_tabs_from_panes(cx);
            self.reconcile_pane_editors(window, cx);
        }
        cx.notify();
    }

    /// Reconcile the singleton `workspace_tabs` view so the active surface and
    /// tool open/close flags track the pane tree after a move or split.
    fn sync_workspace_tabs_from_panes(&mut self, cx: &mut Context<Self>) {
        let tabs = self.panes.active_tabs();
        let open_now = |tool: &WorkspaceToolTab| -> bool {
            tabs.iter().any(|tab| tab == &WorkspaceTab::Tool(tool.clone()))
        };
        if !open_now(&WorkspaceToolTab::Snippets) {
            let _ = self.workspace_tabs.close_tool(&WorkspaceToolTab::Snippets);
        }
        if !open_now(&WorkspaceToolTab::Settings) {
            let _ = self.workspace_tabs.close_tool(&WorkspaceToolTab::Settings);
        }
        let active = self.workspace_tabs.active_tab(&self.request_tabs);
        if self.panes.pane_for_tab(&active).is_none() {
            let fallback = tabs.first().cloned();
            match fallback {
                Some(WorkspaceTab::Request(tab_id)) => {
                    self.workspace_tabs.activate_request();
                    let _ = self.request_tabs.activate(&tab_id);
                }
                Some(WorkspaceTab::Welcome) => {
                    let _ = self.workspace_tabs.activate_welcome();
                }
                Some(WorkspaceTab::Tool(tool)) => self.workspace_tabs.open_tool(tool),
                None => {}
            }
        }
        cx.notify();
    }
}
