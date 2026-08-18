use std::collections::HashSet;

use super::super::*;

#[derive(Clone)]
struct OpenTabMenuEntry {
    tab: WorkspaceTab,
    title: String,
    icon: Option<IconName>,
    active: bool,
    group_label: Option<String>,
}

pub(super) fn render_open_tabs_menu(app: &ApiTester, cx: &mut Context<ApiTester>) -> AnyElement {
    let active_tab = app.workspace_tabs.active_tab(&app.request_tabs);
    let mut rendered_group_ids = HashSet::new();
    let entries = app
        .workspace_tabs
        .visible_tabs(&app.request_tabs)
        .into_iter()
        .filter_map(|tab| {
            let (title, icon, group_label) = match &tab {
                WorkspaceTab::Welcome => (
                    "Welcome".to_owned(),
                    Some(IconName::GalleryVerticalEnd),
                    None,
                ),
                WorkspaceTab::Request(tab_id) => {
                    let request = app.request_tabs.get(tab_id)?;
                    let group_label = request.group_id().and_then(|group_id| {
                        rendered_group_ids.insert(group_id.clone()).then(|| {
                            app.request_tabs
                                .group(group_id)
                                .map(|group| format!("Group · {}", group.display_title()))
                                .unwrap_or_else(|| "Group".to_owned())
                        })
                    });
                    (request.display_title().to_owned(), None, group_label)
                }
                WorkspaceTab::Tool(WorkspaceToolTab::Snippets) => {
                    ("Snippets".to_owned(), Some(IconName::CaseSensitive), None)
                }
                WorkspaceTab::Tool(WorkspaceToolTab::RequestProxy) => {
                    ("Request proxy".to_owned(), Some(IconName::Globe), None)
                }
                WorkspaceTab::Tool(WorkspaceToolTab::Settings) => {
                    ("Settings".to_owned(), Some(IconName::Settings2), None)
                }
                WorkspaceTab::Tool(WorkspaceToolTab::ThemeCss(editor_id)) => (
                    app.theme_editor_title(editor_id)
                        .unwrap_or_else(|| "Theme CSS".to_owned()),
                    Some(IconName::Palette),
                    None,
                ),
            };
            let active = tab == active_tab;
            Some(OpenTabMenuEntry {
                tab,
                title,
                icon,
                active,
                group_label,
            })
        })
        .collect::<Vec<_>>();
    let owner = cx.entity().downgrade();

    Button::new("all-tabs")
        .icon(IconName::ChevronDown)
        .small()
        .ghost()
        .rounded_full()
        .tooltip("All tabs")
        .dropdown_menu(move |mut menu, _, _| {
            menu = menu.max_h(px(520.)).scrollable(true);
            for entry in &entries {
                if let Some(group_label) = &entry.group_label {
                    menu = menu.label(group_label.clone());
                }
                let owner = owner.clone();
                let tab = entry.tab.clone();
                let mut item = PopupMenuItem::new(compact_label(&entry.title, 44))
                    .checked(entry.active)
                    .on_click(move |_, window, cx| {
                        if let Some(owner) = owner.upgrade() {
                            owner.update(cx, |this, cx| {
                                this.activate_workspace_tab(tab.clone(), window, cx);
                            });
                        }
                    });
                if let Some(icon) = entry.icon.clone() {
                    item = item.icon(icon);
                }
                menu = menu.item(item);
            }
            menu
        })
        .anchor(Corner::TopRight)
        .into_any_element()
}
