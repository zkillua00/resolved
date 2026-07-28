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
                active: tab.id() == app.request_tabs.active_tab_id(),
                group_label,
            }
        })
        .collect::<Vec<_>>();
    let owner = cx.entity().downgrade();

    Button::new("all-request-tabs")
        .icon(IconName::ChevronDown)
        .small()
        .ghost()
        .rounded_full()
        .tooltip("All request tabs")
        .dropdown_menu(move |mut menu, _, _| {
            menu = menu.max_h(px(520.)).scrollable(true);
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
            menu
        })
        .anchor(Corner::TopRight)
        .into_any_element()
}
