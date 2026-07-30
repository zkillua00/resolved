use super::super::*;

#[derive(Clone)]
struct OpenTabMenuEntry {
    tab_id: RequestTabId,
    title: String,
    active: bool,
    group_label: Option<String>,
}

pub(super) fn render_open_tabs_menu(app: &ApiTester, cx: &mut Context<ApiTester>) -> AnyElement {
    let mut previous_group_id: Option<RequestTabGroupId> = None;
    let entries = app
        .request_tabs
        .tabs()
        .iter()
        .filter(|tab| {
            !app.workspace_tabs.welcome_is_open()
                || app.workspace_tabs.welcome_request_tab_id() != Some(tab.id())
        })
        .map(|tab| {
            let group_label = tab.group_id().and_then(|group_id| {
                let starts_group = previous_group_id.as_ref() != Some(group_id);
                previous_group_id = Some(group_id.clone());
                starts_group.then(|| {
                    app.request_tabs
                        .group(group_id)
                        .map(|group| format!("Group · {}", group.display_title()))
                        .unwrap_or_else(|| "Group".to_owned())
                })
            });
            if tab.group_id().is_none() {
                previous_group_id = None;
            }
            OpenTabMenuEntry {
                tab_id: tab.id().clone(),
                title: tab.display_title().to_owned(),
                active: app.workspace_tabs.active() == ActiveWorkspaceTab::Request
                    && tab.id() == app.request_tabs.active_tab_id(),
                group_label,
            }
        })
        .collect::<Vec<_>>();
    let welcome_open = app.workspace_tabs.welcome_is_open();
    let welcome_active = app.workspace_tabs.active() == ActiveWorkspaceTab::Welcome;
    let settings_open = app.workspace_tabs.settings_open();
    let settings_active = app
        .workspace_tabs
        .tool_is_active(WorkspaceToolTab::Settings);
    let theme_css_open = app.theme_editor.is_some();
    let theme_css_active = app
        .workspace_tabs
        .tool_is_active(WorkspaceToolTab::ThemeCss);
    let owner = cx.entity().downgrade();

    Button::new("all-tabs")
        .icon(IconName::ChevronDown)
        .small()
        .ghost()
        .rounded_full()
        .tooltip("All tabs")
        .dropdown_menu(move |mut menu, _, _| {
            menu = menu.max_h(px(520.)).scrollable(true);
            if welcome_open {
                let welcome_owner = owner.clone();
                menu = menu.item(
                    PopupMenuItem::new("Welcome")
                        .icon(IconName::GalleryVerticalEnd)
                        .checked(welcome_active)
                        .on_click(move |_, window, cx| {
                            if let Some(owner) = welcome_owner.upgrade() {
                                owner.update(cx, |this, cx| {
                                    this.activate_workspace_tab(WorkspaceTab::Welcome, window, cx);
                                });
                            }
                        }),
                );
                if !entries.is_empty() {
                    menu = menu.separator().label("Requests");
                }
            }
            for entry in &entries {
                if let Some(group_label) = &entry.group_label {
                    menu = menu.label(group_label.clone());
                }
                let owner = owner.clone();
                let tab_id = entry.tab_id.clone();
                menu = menu.item(
                    PopupMenuItem::new(compact_label(&entry.title, 44))
                        .checked(entry.active)
                        .on_click(move |_, window, cx| {
                            if let Some(owner) = owner.upgrade() {
                                owner.update(cx, |this, cx| {
                                    this.sidebar_tab = SidebarTab::Collections;
                                    this.activate_request_tab(tab_id.clone(), window, cx);
                                });
                            }
                        }),
                );
            }
            if settings_open || theme_css_open {
                menu = menu.separator().label("Tools");
            }
            if settings_open {
                let owner = owner.clone();
                menu = menu.item(
                    PopupMenuItem::new("Settings")
                        .icon(IconName::Settings2)
                        .checked(settings_active)
                        .on_click(move |_, window, cx| {
                            if let Some(owner) = owner.upgrade() {
                                owner.update(cx, |this, cx| {
                                    this.activate_workspace_tab(
                                        WorkspaceTab::Tool(WorkspaceToolTab::Settings),
                                        window,
                                        cx,
                                    );
                                });
                            }
                        }),
                );
            }
            if theme_css_open {
                let owner = owner.clone();
                menu = menu.item(
                    PopupMenuItem::new("Theme CSS")
                        .icon(IconName::Palette)
                        .checked(theme_css_active)
                        .on_click(move |_, window, cx| {
                            if let Some(owner) = owner.upgrade() {
                                owner.update(cx, |this, cx| {
                                    this.activate_workspace_tab(
                                        WorkspaceTab::Tool(WorkspaceToolTab::ThemeCss),
                                        window,
                                        cx,
                                    );
                                });
                            }
                        }),
                );
            }
            menu
        })
        .anchor(Corner::TopRight)
        .into_any_element()
}
