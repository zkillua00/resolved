use super::super::*;
use super::tab_control::WorkspaceTabControl;

pub(super) fn render_welcome_tab(
    app: &ApiTester,
    pane_id: Option<PaneId>,
    cx: &mut Context<ApiTester>,
) -> AnyElement {
    WorkspaceTabControl::new(WorkspaceTab::Welcome, "workspace-welcome-tab", "Welcome")
        .pane_opt(pane_id)
        .icon(IconName::GalleryVerticalEnd)
        .debug_selectors(
            Some("workspace-welcome-tab"),
            Some("workspace-welcome-tab-drag-handle"),
        )
        .render(app, cx)
}
