use super::super::*;
use super::tab_control::WorkspaceTabControl;

pub(super) fn render_request_tab(
    app: &ApiTester,
    tab: &RequestTabRecord,
    pane_id: Option<PaneId>,
    cx: &mut Context<ApiTester>,
) -> AnyElement {
    let tab_id = tab.id().clone();
    let active = app.workspace_tabs.active() == ActiveWorkspaceTab::Request
        && app.request_tabs.active_tab_id() == &tab_id;
    let current_request = app.request_tabs.active_tab_id() == &tab_id;
    let dirty = if active {
        app.request_is_dirty()
    } else {
        tab.is_dirty()
    };
    let accent = tab
        .group_id()
        .and_then(|group_id| app.request_tabs.group(group_id))
        .map(|group| request_tab_group_color(group.color(), cx));
    let row_id = format!("open-request-tab-{}", tab_id.as_str());

    WorkspaceTabControl::new(
        WorkspaceTab::Request(tab_id),
        row_id,
        request_tab_label(app, tab, cx),
    )
    .pane_opt(pane_id)
    .dirty(dirty)
    .accent(accent)
    .debug_selectors(
        current_request.then_some("current-workspace-request-tab"),
        current_request.then_some("current-workspace-request-tab-drag-handle"),
    )
    .render(app, cx)
}

pub(super) fn request_tab_group_color(color: &RequestTabGroupColor, cx: &App) -> Hsla {
    match color {
        RequestTabGroupColor::Gray => cx.theme().muted_foreground,
        RequestTabGroupColor::Blue => cx.theme().blue,
        RequestTabGroupColor::Cyan => cx.theme().cyan,
        RequestTabGroupColor::Green => cx.theme().green,
        RequestTabGroupColor::Yellow => cx.theme().yellow,
        RequestTabGroupColor::Orange => cx.theme().warning,
        RequestTabGroupColor::Red => cx.theme().red,
        RequestTabGroupColor::Pink => cx.theme().magenta,
        RequestTabGroupColor::Purple => cx.theme().primary,
        RequestTabGroupColor::Custom(_) => cx.theme().primary,
    }
}

/// Presentation only: keep persisted names and rename/export behavior unchanged.
pub(super) fn request_tab_label(app: &ApiTester, tab: &RequestTabRecord, cx: &App) -> String {
    let method = app.request_tab_method(tab, cx);
    let title = tab.display_title();
    if method.is_empty()
        || title
            .strip_prefix(&method)
            .is_some_and(|rest| rest.starts_with(' '))
    {
        title.to_owned()
    } else {
        format!("{method} {title}")
    }
}
