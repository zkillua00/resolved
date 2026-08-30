use super::super::*;
use super::tab_control::WorkspaceTabControl;

pub(super) fn render_workspace_tool_tab(
    app: &ApiTester,
    tool: WorkspaceToolTab,
    pane_id: Option<PaneId>,
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
        WorkspaceToolTab::RequestProxy => (
            "workspace-tool-tab-request-proxy".to_owned(),
            "workspace-request-proxy-tab-drag-handle",
            "Request proxy".to_owned(),
            IconName::Replace,
            false,
        ),
        WorkspaceToolTab::ServerTools => (
            "workspace-tool-tab-server-tools".to_owned(),
            "workspace-server-tools-tab-drag-handle",
            "Server Tools".to_owned(),
            IconName::Inspector,
            false,
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
    let row_debug_selector = match &tool {
        WorkspaceToolTab::RequestProxy => Some("workspace-request-proxy-tab"),
        WorkspaceToolTab::ServerTools => Some("workspace-server-tools-tab"),
        WorkspaceToolTab::Settings => Some("workspace-settings-tab"),
        WorkspaceToolTab::Snippets | WorkspaceToolTab::ThemeCss(_) => None,
    };

    WorkspaceTabControl::new(WorkspaceTab::Tool(tool), row_id, title)
        .pane_opt(pane_id)
        .icon(icon)
        .dirty(dirty)
        .debug_selectors(row_debug_selector, Some(drag_selector))
        .render(app, cx)
}
