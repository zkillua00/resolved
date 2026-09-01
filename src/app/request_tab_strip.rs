use super::*;

mod context_menu;
mod group_item;
mod overflow_menu;
mod tab_control;
mod tab_drag;
mod tab_item;
mod tool_tab_item;
mod welcome_item;

use context_menu::build_request_tab_context_menu;
use group_item::render_request_tab_group;
use overflow_menu::render_open_tabs_menu;
use std::collections::HashSet;
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
        let tabs = self.workspace_tabs.visible_tabs(&self.request_tabs);
        self.render_request_tab_strip_with(&tabs, None, cx)
    }

    pub(super) fn render_request_tab_strip_with(
        &self,
        tabs: &[WorkspaceTab],
        pane_id: Option<PaneId>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut elements = Vec::new();
        let mut rendered_group_ids = HashSet::new();

        for workspace_tab in tabs {
            match workspace_tab {
                WorkspaceTab::Welcome => elements.push(render_welcome_tab(self, pane_id, cx)),
                WorkspaceTab::Request(tab_id) => {
                    let Some(tab) = self.request_tabs.get(tab_id) else {
                        continue;
                    };
                    let group = tab
                        .group_id()
                        .and_then(|group_id| self.request_tabs.group(group_id));
                    if let Some(group) = group
                        && rendered_group_ids.insert(group.id().clone())
                    {
                        elements.push(render_request_tab_group(self, group, cx));
                    }
                    if !group.is_some_and(RequestTabGroup::is_collapsed) {
                        elements.push(render_request_tab(self, tab, pane_id, cx));
                    }
                }
                WorkspaceTab::Tool(tool) => {
                    elements.push(render_workspace_tool_tab(self, tool.clone(), pane_id, cx));
                }
            }
        }

        let context_owner = cx.entity().downgrade();
        let empty_pane_drop_zone = pane_id
            .filter(|_| tabs.is_empty())
            .map(|pane_id| tab_drag::render_empty_pane_tab_drop_zone(pane_id, cx));
        let tab_scroller = h_flex()
            .id("request-tabs-scroll")
            .h_full()
            .flex_1()
            .min_w_0()
            .overflow_x_scroll()
            .children(elements)
            .children(empty_pane_drop_zone)
            .capture_any_mouse_down(cx.listener(|this, _: &MouseDownEvent, _, _| {
                this.request_tab_context_target = None;
            }))
            .context_menu(move |menu, window, cx| {
                build_request_tab_context_menu(menu, context_owner.clone(), window, cx)
            });

        h_flex()
            .h(rems(2.625))
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

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px, size};

    fn drag_between(
        cx: &mut VisualTestContext,
        source: gpui::Bounds<Pixels>,
        target: Point<Pixels>,
    ) {
        let source = source.center();
        cx.simulate_mouse_down(source, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_move(
            point(source.x + px(8.), source.y),
            MouseButton::Left,
            Modifiers::none(),
        );
        cx.simulate_mouse_move(target, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(target, MouseButton::Left, Modifiers::none());
        cx.run_until_parked();
    }

    #[gpui::test]
    fn request_and_tool_tabs_drag_through_the_same_strip_controls(cx: &mut TestAppContext) {
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
                app.open_blank_request_tab(window, cx);
                app.open_workspace_tool_tab(WorkspaceToolTab::Settings, window, cx);
            });
        });
        cx.run_until_parked();

        let (first, second) = cx.update(|_, cx| {
            let app = app.read(cx);
            (
                app.request_tabs.tabs()[0].id().clone(),
                app.request_tabs.tabs()[1].id().clone(),
            )
        });
        let settings = WorkspaceTab::Tool(WorkspaceToolTab::Settings);
        assert_eq!(
            cx.update(|_, cx| app
                .read(cx)
                .workspace_tabs
                .visible_tabs(&app.read(cx).request_tabs)),
            vec![
                WorkspaceTab::Request(first.clone()),
                WorkspaceTab::Request(second.clone()),
                settings.clone(),
            ]
        );

        let settings_handle = cx
            .debug_bounds("workspace-settings-tab-drag-handle")
            .expect("Settings must expose the shared tab drag handle");
        let request_target = cx
            .debug_bounds("current-workspace-request-tab")
            .expect("the current request must expose the shared tab drop target");
        drag_between(
            cx,
            settings_handle,
            point(request_target.origin.x + px(8.), request_target.center().y),
        );
        assert_eq!(
            cx.update(|_, cx| app
                .read(cx)
                .workspace_tabs
                .visible_tabs(&app.read(cx).request_tabs)),
            vec![
                WorkspaceTab::Request(first.clone()),
                settings.clone(),
                WorkspaceTab::Request(second.clone()),
            ],
            "a tool tab must be droppable beside a request tab",
        );

        let request_handle = cx
            .debug_bounds("current-workspace-request-tab-drag-handle")
            .expect("requests must use the same shared tab drag handle");
        let settings_target = cx
            .debug_bounds("workspace-settings-tab")
            .expect("Settings must expose the shared tab drop target");
        drag_between(
            cx,
            request_handle,
            point(
                settings_target.origin.x + px(8.),
                settings_target.center().y,
            ),
        );
        assert_eq!(
            cx.update(|_, cx| app
                .read(cx)
                .workspace_tabs
                .visible_tabs(&app.read(cx).request_tabs)),
            vec![
                WorkspaceTab::Request(first),
                WorkspaceTab::Request(second.clone()),
                settings,
            ],
            "a request tab must be droppable beside a tool tab through the same controls",
        );

        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.activate_request_tab(second.clone(), window, cx);
                app.request_close_request_tab(second.clone(), window, cx);
            });
        });
        cx.run_until_parked();
        assert_eq!(
            cx.update(|_, cx| app
                .read(cx)
                .workspace_tabs
                .active_tab(&app.read(cx).request_tabs)),
            WorkspaceTab::Tool(WorkspaceToolTab::Settings),
            "closing a moved request must select its actual right-hand tab regardless of content",
        );
    }
}
