use super::super::drag_drop::{DropPlacement, WorkspaceTabDrag};
use super::super::*;

fn render_workspace_tab_drop_zone(
    target: WorkspaceTab,
    placement: DropPlacement,
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
                cx,
            );
        }))
        .into_any_element()
}

pub(super) fn render_workspace_tab_drop_overlay(
    target: WorkspaceTab,
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
            cx,
        ))
        .child(render_workspace_tab_drop_zone(
            target,
            DropPlacement::After,
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
    fn reorder_workspace_tab_from_drop(
        &mut self,
        dragged: WorkspaceTab,
        target: WorkspaceTab,
        placement: DropPlacement,
        cx: &mut Context<Self>,
    ) {
        if self.sending || dragged == target || placement == DropPlacement::Inside {
            return;
        }

        if !self.workspace_tabs.reorder_tab(
            &self.request_tabs,
            &dragged,
            &target,
            placement == DropPlacement::After,
        ) {
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
}
