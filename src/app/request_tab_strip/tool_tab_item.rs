use super::super::*;

pub(super) fn render_workspace_tool_tab(
    app: &ApiTester,
    tool: WorkspaceToolTab,
    cx: &mut Context<ApiTester>,
) -> AnyElement {
    let (tab_id, title, icon) = match &tool {
        WorkspaceToolTab::Snippets => (
            SharedString::from("workspace-tool-tab-snippets"),
            "Snippets".to_owned(),
            IconName::CaseSensitive,
        ),
        WorkspaceToolTab::Settings => (
            SharedString::from("workspace-tool-tab-settings"),
            "Settings".to_owned(),
            IconName::Settings2,
        ),
        WorkspaceToolTab::ThemeCss(editor_id) => {
            let title = app
                .theme_editor_title(editor_id)
                .unwrap_or_else(|| "Theme CSS".to_owned());
            (
                SharedString::from(format!("workspace-tool-tab-theme-css-{editor_id}")),
                title,
                IconName::Palette,
            )
        }
    };
    let row_group: SharedString = format!("{tab_id}-group").into();
    let close_button_id: SharedString = format!("{tab_id}-close").into();
    let active = app.workspace_tabs.tool_is_active(&tool);
    let dirty = match &tool {
        WorkspaceToolTab::Snippets => app.snippet_editor_is_dirty(cx),
        WorkspaceToolTab::Settings => false,
        WorkspaceToolTab::ThemeCss(editor_id) => app
            .theme_editor(editor_id)
            .is_some_and(|session| session.dirty),
    };
    let activate_tool = tool.clone();
    let close_tool = tool.clone();
    let context_tool = tool;

    h_flex()
        .id(tab_id.clone())
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
        .when(active, |this| {
            this.bg(cx.api_surface())
                .border_b_2()
                .border_color(cx.theme().primary)
        })
        .when(!active, |this| {
            this.bg(cx.api_surface_low())
                .hover(|style| style.bg(cx.api_surface_container()))
        })
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, _, _, _| {
                this.request_tab_context_target =
                    Some(super::RequestTabContextTarget::Tool(context_tool.clone()));
            }),
        )
        .on_click(cx.listener(move |this, _, window, cx| {
            this.activate_workspace_tab(WorkspaceTab::Tool(activate_tool.clone()), window, cx);
        }))
        .child(Icon::new(icon).xsmall())
        .child(
            div()
                .min_w_0()
                .flex_1()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_sm()
                .font_medium()
                .child(title),
        )
        .when(dirty, |this| {
            this.child(
                div()
                    .size(px(7.))
                    .flex_shrink_0()
                    .rounded_full()
                    .bg(cx.theme().warning),
            )
        })
        .child(
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
                    this.close_workspace_tool_tab(close_tool.clone(), window, cx);
                })),
        )
        .into_any_element()
}
