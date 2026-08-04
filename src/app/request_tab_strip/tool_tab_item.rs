use super::super::*;
use super::tab_control::WorkspaceTabControl;

pub(super) fn render_workspace_tool_tab(
    app: &ApiTester,
    tool: WorkspaceToolTab,
    cx: &mut Context<ApiTester>,
) -> AnyElement {
    let (row_id, drag_selector, title, icon, dirty) = match &tool {
        WorkspaceToolTab::Snippets => (
            "workspace-tool-tab-snippets".to_owned(),
            "workspace-snippets-tab-drag-handle",
            "Snippets".to_owned(),
            IconName::CaseSensitive,
            app.snippet_editor_is_dirty(cx),
        ),
        WorkspaceToolTab::Settings => (
            "workspace-tool-tab-settings".to_owned(),
            "workspace-settings-tab-drag-handle",
            "Settings".to_owned(),
            IconName::Settings2,
            false,
        ),
        WorkspaceToolTab::ThemeCss(editor_id) => (
            format!("workspace-tool-tab-theme-css-{editor_id}"),
            "workspace-theme-css-tab-drag-handle",
            app.theme_editor_title(editor_id)
                .unwrap_or_else(|| "Theme CSS".to_owned()),
            IconName::Palette,
            app.theme_editor(editor_id)
                .is_some_and(|session| session.dirty),
        ),
    };
    let row_debug_selector =
        matches!(&tool, WorkspaceToolTab::Settings).then_some("workspace-settings-tab");

    WorkspaceTabControl::new(WorkspaceTab::Tool(tool), row_id, title)
        .icon(icon)
        .dirty(dirty)
        .debug_selectors(row_debug_selector, Some(drag_selector))
        .render(app, cx)
}
