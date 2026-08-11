use super::*;

impl ApiTester {
    pub(super) fn render_title_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        match self.workspace_tabs.active() {
            ActiveWorkspaceTab::Welcome => return self.render_welcome_title_bar(cx),
            ActiveWorkspaceTab::Snippets => return self.render_snippets_title_bar(cx),
            ActiveWorkspaceTab::Settings => return self.render_settings_title_bar(cx),
            ActiveWorkspaceTab::ThemeCss => return self.render_theme_css_title_bar(cx),
            ActiveWorkspaceTab::Request => {}
        }
        if self.sidebar_tab == SidebarTab::Environments {
            return self.render_environment_title_bar(cx);
        }

        let active_environment_full = self
            .workspace
            .active_environment()
            .map(|environment| environment.name.clone())
            .unwrap_or_else(|| "No environment".to_owned());
        let active_environment = compact_label(&active_environment_full, 30);
        let active_environment_id = self.workspace.active_environment_id.clone();
        let environments = self
            .workspace
            .environments
            .iter()
            .map(|environment| (environment.id.clone(), environment.name.clone()))
            .collect::<Vec<_>>();
        let this = cx.entity().downgrade();
        let active_request_tab = self.request_tabs.active();
        let request_name = active_request_tab.display_title();
        let request_name_width =
            ((request_name.chars().count() as f32 * 8.) + 20.).clamp(96., 240.);
        let collection_name = active_request_tab
            .association()
            .collection_id()
            .and_then(|id| self.workspace.collection(id))
            .map(|collection| collection.name.as_str())
            .or_else(|| {
                self.selected_collection_id
                    .as_deref()
                    .and_then(|id| self.workspace.collection(id))
                    .map(|collection| collection.name.as_str())
            })
            .unwrap_or("No collection");
        let folder_path = active_request_tab
            .association()
            .collection_id()
            .and_then(|collection_id| self.workspace.collection(collection_id))
            .and_then(|collection| {
                active_request_tab
                    .association()
                    .folder_id()
                    .and_then(|folder_id| collection.folder_path_ids(folder_id).ok())
                    .map(|path| {
                        path.into_iter()
                            .filter_map(|folder_id| {
                                collection
                                    .folder(&folder_id)
                                    .map(|folder| folder.name.clone())
                            })
                            .collect::<Vec<_>>()
                    })
            })
            .unwrap_or_default();
        let request_path = std::iter::once(collection_name.to_owned())
            .chain(folder_path)
            .collect::<Vec<_>>()
            .join(" / ");
        let has_saved_request = active_request_tab
            .association()
            .saved_request_id()
            .is_some();
        let can_save = !self.sending
            && self.can_save_request_content()
            && (active_request_tab.association().collection_id().is_some()
                || self.selected_collection_id.is_some());
        let can_switch_environment = self.can_select_environment() && !self.sending;
        let dirty = self.request_is_dirty();
        let request_actions_this = this.clone();
        let request_actions_disabled = self.sending;

        h_flex()
            .h(px(APP_TITLE_BAR_HEIGHT))
            .flex_shrink_0()
            .pl(px(92.))
            .pr_2()
            .border_b_1()
            .border_color(cx.theme().title_bar_border)
            .bg(cx.theme().title_bar)
            .justify_between()
            .child(
                h_flex()
                    .min_w_0()
                    .flex_1()
                    .gap_6()
                    .child(resolved_brand_lockup(cx))
                    .child(self.render_title_workspace_control(cx))
                    .child(
                        h_flex()
                            .min_w_0()
                            .flex_1()
                            .overflow_hidden()
                            .gap_2()
                            .child(
                                h_flex()
                                    .min_w_0()
                                    .max_w(px(520.))
                                    .gap_1()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .max_w(px(300.))
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_sm()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(format!("{request_path} /")),
                                    )
                                    .child(
                                        Input::new(&self.saved_request_name)
                                            .small()
                                            .appearance(false)
                                            .focus_bordered(false)
                                            .w(px(request_name_width))
                                            .px_0(),
                                    ),
                            )
                            .when(dirty, |this| {
                                this.child(
                                    div()
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .bg(cx.theme().warning.opacity(0.12))
                                        .text_xs()
                                        .font_semibold()
                                        .text_color(cx.theme().warning)
                                        .child("Modified"),
                                )
                            }),
                    ),
            )
            .child(
                h_flex()
                    .flex_shrink_0()
                    .gap_1()
                    .child(
                        Button::new("active-environment")
                            .icon(IconName::Settings2)
                            .label(active_environment)
                            .large()
                            .h(px(38.))
                            .outline()
                            .rounded(px(20.))
                            .tooltip(active_environment_full)
                            .dropdown_menu(move |menu, _, _| {
                                let no_environment_this = this.clone();
                                let menu = menu.min_w(px(220.)).item(
                                    PopupMenuItem::new("No environment")
                                        .checked(active_environment_id.is_none())
                                        .disabled(!can_switch_environment)
                                        .on_click(move |_, _, cx| {
                                            if let Some(this) = no_environment_this.upgrade() {
                                                this.update(cx, |this, cx| {
                                                    this.activate_environment(None, cx);
                                                });
                                            }
                                        }),
                                );
                                let menu = environments.iter().fold(menu, |menu, (id, name)| {
                                    let environment_id = id.clone();
                                    let checked =
                                        active_environment_id.as_deref() == Some(id.as_str());
                                    let environment_this = this.clone();
                                    menu.item(
                                        PopupMenuItem::new(name.clone())
                                            .checked(checked)
                                            .disabled(!can_switch_environment)
                                            .on_click(move |_, _, cx| {
                                                if let Some(this) = environment_this.upgrade() {
                                                    this.update(cx, |this, cx| {
                                                        this.activate_environment(
                                                            Some(environment_id.clone()),
                                                            cx,
                                                        );
                                                    });
                                                }
                                            }),
                                    )
                                });
                                let manage_this = this.clone();
                                menu.separator().item(
                                    PopupMenuItem::new("Manage environments…").on_click(
                                        move |_, window, cx| {
                                            if let Some(this) = manage_this.upgrade() {
                                                this.update(cx, |this, cx| {
                                                    this.activate_request_workspace(
                                                        SidebarTab::Environments,
                                                        window,
                                                        cx,
                                                    );
                                                });
                                            }
                                        },
                                    ),
                                )
                            }),
                    )
                    .child(
                        Button::new("title-save-request")
                            .label(if has_saved_request { "Update" } else { "Save" })
                            .large()
                            .h(px(38.))
                            .outline()
                            .rounded(px(20.))
                            .disabled(!can_save)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save_current_request(false, window, cx);
                            })),
                    )
                    .child(
                        Button::new("title-request-actions")
                            .icon(IconName::EllipsisVertical)
                            .small()
                            .h(px(38.))
                            .w(px(38.))
                            .ghost()
                            .rounded_full()
                            .tooltip("Request actions")
                            .dropdown_menu(move |menu, _, _| {
                                let import_this = request_actions_this.clone();
                                let export_this = request_actions_this.clone();
                                let mut menu = menu
                                    .min_w(px(220.))
                                    .item(
                                        PopupMenuItem::new("Import requests…")
                                            .icon(IconName::FolderOpen)
                                            .disabled(request_actions_disabled)
                                            .on_click(move |_, window, cx| {
                                                if let Some(this) = import_this.upgrade() {
                                                    this.update(cx, |this, cx| {
                                                        this.open_request_import_panel(window, cx);
                                                    });
                                                }
                                            }),
                                    )
                                    .item(
                                        PopupMenuItem::new("Export request…")
                                            .icon(IconName::SquareTerminal)
                                            .disabled(request_actions_disabled)
                                            .on_click(move |_, window, cx| {
                                                if let Some(this) = export_this.upgrade() {
                                                    this.update(cx, |this, cx| {
                                                        this.open_request_export_panel(window, cx);
                                                    });
                                                }
                                            }),
                                    );
                                if has_saved_request {
                                    let save_copy_this = request_actions_this.clone();
                                    menu = menu.separator().item(
                                        PopupMenuItem::new("Save as new request…")
                                            .icon(IconName::Copy)
                                            .disabled(!can_save)
                                            .on_click(move |_, window, cx| {
                                                if let Some(this) = save_copy_this.upgrade() {
                                                    this.update(cx, |this, cx| {
                                                        this.save_current_request(true, window, cx);
                                                    });
                                                }
                                            }),
                                    );
                                }
                                menu
                            })
                            .anchor(Corner::TopRight),
                    ),
            )
            .into_any_element()
    }
}
