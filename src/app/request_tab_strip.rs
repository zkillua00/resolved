use super::*;

mod context_menu;
mod group_item;
mod overflow_menu;
mod tab_item;
mod tool_tab_item;
mod welcome_item;

use context_menu::build_request_tab_context_menu;
use group_item::render_request_tab_group;
use overflow_menu::render_open_tabs_menu;
use tab_item::render_request_tab;
use tool_tab_item::render_workspace_tool_tab;
use welcome_item::render_welcome_tab;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) enum RequestTabContextTarget {
    Tab(RequestTabId),
    Group(RequestTabGroupId),
    Tool(WorkspaceToolTab),
}

impl ApiTester {
    pub(super) fn render_request_tab_strip(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut elements = Vec::new();
        let mut previous_group_id: Option<RequestTabGroupId> = None;

        if self.workspace_tabs.welcome_is_open() {
            elements.push(render_welcome_tab(self, cx));
        }
        for tab in self.request_tabs.tabs() {
            if self.workspace_tabs.welcome_is_open()
                && self.workspace_tabs.welcome_request_tab_id() == Some(tab.id())
            {
                continue;
            }
            let group = tab
                .group_id()
                .and_then(|group_id| self.request_tabs.group(group_id));
            let starts_group =
                group.is_some_and(|group| previous_group_id.as_ref() != Some(group.id()));

            if let Some(group) = group
                && starts_group
            {
                elements.push(render_request_tab_group(self, group, cx));
            }
            if !group.is_some_and(RequestTabGroup::is_collapsed) {
                elements.push(render_request_tab(self, tab, cx));
            }
            previous_group_id = tab.group_id().cloned();
        }
        if self.workspace_tabs.settings_open() {
            elements.push(render_workspace_tool_tab(
                self,
                WorkspaceToolTab::Settings,
                cx,
            ));
        }
        if self.theme_editor.is_some() {
            elements.push(render_workspace_tool_tab(
                self,
                WorkspaceToolTab::ThemeCss,
                cx,
            ));
        }

        let context_owner = cx.entity().downgrade();
        let tab_scroller = h_flex()
            .id("request-tabs-scroll")
            .h_full()
            .flex_1()
            .min_w_0()
            .overflow_x_scroll()
            .children(elements)
            .capture_any_mouse_down(cx.listener(|this, _: &MouseDownEvent, _, _| {
                this.request_tab_context_target = None;
            }))
            .context_menu(move |menu, window, cx| {
                build_request_tab_context_menu(menu, context_owner.clone(), window, cx)
            });

        h_flex()
            .h(px(42.))
            .w_full()
            .flex_shrink_0()
            .border_b_1()
            .border_color(cx.api_outline_variant())
            .bg(cx.api_surface_low())
            .child(tab_scroller)
            .when_some(self.request_tabs_warning.clone(), |this, warning| {
                this.child(
                    Button::new("request-tabs-warning")
                        .icon(IconName::Info)
                        .xsmall()
                        .ghost()
                        .text_color(cx.theme().warning)
                        .tooltip(warning),
                )
            })
            .child(
                h_flex()
                    .h_full()
                    .px_1()
                    .gap_1()
                    .border_l_1()
                    .border_color(cx.api_outline_variant())
                    .child(render_open_tabs_menu(self, cx))
                    .child(
                        Button::new("new-request-tab")
                            .icon(IconName::Plus)
                            .small()
                            .ghost()
                            .rounded_full()
                            .tooltip("New request tab")
                            .disabled(self.sending)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_blank_request_tab(window, cx);
                            })),
                    ),
            )
            .into_any_element()
    }
}
