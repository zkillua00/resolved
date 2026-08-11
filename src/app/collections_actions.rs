use super::*;

impl ApiTester {
    pub(super) fn create_collection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.workspace_writable {
            if self.can_create_collection_content() {
                self.open_create_upstream_collection_dialog(None, None, window, cx);
            }
            return;
        }
        let name = unique_name(
            "Collection",
            self.workspace
                .collections
                .iter()
                .map(|collection| collection.name.as_str()),
        );
        let mut candidate = self.workspace.clone();
        match candidate.create_collection(name) {
            Ok(id) => {
                if self.commit_workspace(candidate).is_ok() {
                    self.expanded_collection_ids.insert(id.clone());
                    self.select_collection(id.clone(), window, cx);
                    self.renaming_collection_id = Some(id);
                    self.collection_name.read(cx).focus_handle(cx).focus(window);
                } else {
                    cx.notify();
                }
            }
            Err(error) => {
                self.workspace_warning = Some(error.to_string());
                cx.notify();
            }
        }
    }

    pub(super) fn select_collection(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_collection_id.as_deref() == Some(&id) {
            self.selected_folder_id = None;
            self.sidebar_tab = SidebarTab::Collections;
            self.update_active_unsaved_request_tab_location(Some(id), None, cx);
            cx.notify();
            return;
        }
        let Some(collection) = self.workspace.collection(&id) else {
            return;
        };
        self.selected_collection_id = Some(id);
        self.selected_folder_id = None;
        self.collection_name.update(cx, |input, cx| {
            input.set_value(collection.name.clone(), window, cx)
        });
        self.sidebar_tab = SidebarTab::Collections;
        self.update_active_unsaved_request_tab_location(
            self.selected_collection_id.clone(),
            None,
            cx,
        );
        cx.notify();
    }

    pub(super) fn toggle_collection(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.expanded_collection_ids.remove(&id) {
            self.expanded_collection_ids.insert(id.clone());
        }
        self.select_collection(id, window, cx);
    }

    pub(super) fn begin_collection_rename(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_collection(id.clone(), window, cx);
        self.renaming_collection_id = Some(id);
        self.collection_name.read(cx).focus_handle(cx).focus(window);
        cx.notify();
    }

    pub(super) fn finish_collection_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.rename_collection(window, cx);
        self.renaming_collection_id = None;
        cx.notify();
    }

    pub(super) fn rename_collection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_update_collection_content() {
            return;
        }
        let Some(id) = self.selected_collection_id.clone() else {
            return;
        };
        let name = self.collection_name.read(cx).value().to_string();
        let mut candidate = self.workspace.clone();
        match candidate.rename_collection(&id, name) {
            Ok(()) => {
                if self.workspace_writable {
                    let _ = self.commit_workspace(candidate);
                } else if let Some(collection) = candidate.collection(&id) {
                    self.rename_collection_on_upstream(id, collection.name.clone(), window, cx);
                    return;
                }
            }
            Err(error) => self.workspace_warning = Some(error.to_string()),
        }
        cx.notify();
    }

    pub(super) fn open_collection_delete_dialog(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_delete_collection_content() {
            return;
        }
        let Some(collection) = self.workspace.collection(&id) else {
            return;
        };
        let collection_name = collection.name.clone();
        self.collection_delete_confirmation
            .update(cx, |input, cx| input.set_value("", window, cx));

        let this = cx.entity().downgrade();
        let confirmation_input = self.collection_delete_confirmation.clone();
        window.open_dialog(cx, move |dialog, _, cx| {
            let id_for_ok = id.clone();
            let name_for_ok = collection_name.clone();
            let this_for_ok = this.clone();
            let input_for_ok = confirmation_input.clone();
            let input_for_footer = confirmation_input.clone();
            let name_for_footer = collection_name.clone();

            dialog
                .title("Delete collection?")
                .w(px(480.))
                .confirm()
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("Delete collection")
                        .ok_variant(ButtonVariant::Danger),
                )
                .footer(move |ok, cancel, window, cx| {
                    let confirmed =
                        input_for_footer.read(cx).value().as_ref() == name_for_footer.as_str();
                    vec![
                        cancel(window, cx),
                        if confirmed {
                            ok(window, cx)
                        } else {
                            Button::new("delete-collection-disabled")
                                .label("Delete collection")
                                .danger()
                                .disabled(true)
                                .into_any_element()
                        },
                    ]
                })
                .on_ok(move |_, window, cx| {
                    if input_for_ok.read(cx).value().as_ref() != name_for_ok.as_str() {
                        return false;
                    }
                    let Some(this) = this_for_ok.upgrade() else {
                        return true;
                    };
                    this.update(cx, |this, cx| {
                        this.delete_collection(id_for_ok.clone(), window, cx);
                    });
                    true
                })
                .child(
                    v_flex()
                        .gap_3()
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!(
                                    "This permanently deletes “{}” and every request inside it.",
                                    collection_name
                                )),
                        )
                        .child(
                            v_flex()
                                .gap_2()
                                .child(
                                    div()
                                        .text_sm()
                                        .child("Type the collection name to confirm:"),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .font_semibold()
                                        .text_color(cx.theme().danger)
                                        .child(collection_name.clone()),
                                )
                                .child(Input::new(&confirmation_input)),
                        ),
                )
        });
        self.collection_delete_confirmation
            .read(cx)
            .focus_handle(cx)
            .focus(window);
    }

    pub(super) fn delete_collection(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.can_delete_collection_content() {
            return false;
        }
        if !self.workspace_writable {
            self.delete_collection_on_upstream(id, false, window, cx);
            return true;
        }
        let deleting_selected = self.selected_collection_id.as_deref() == Some(id.as_str());
        let mut candidate = self.workspace.clone();
        match candidate.remove_collection(&id) {
            Ok(_) => {
                self.snapshot_active_request_tab(cx);
                let mut candidate_request_tabs = self.request_tabs.clone();
                candidate_request_tabs.detach_collection(&id);
                if self
                    .commit_workspace_and_request_tabs(candidate, candidate_request_tabs)
                    .is_ok()
                {
                    self.expanded_collection_ids.remove(&id);
                    if self.renaming_collection_id.as_deref() == Some(id.as_str()) {
                        self.renaming_collection_id = None;
                    }
                    self.sync_active_request_tab_identity();
                    if deleting_selected {
                        self.selected_collection_id = self
                            .workspace
                            .collections
                            .first()
                            .map(|collection| collection.id.clone());
                        let name = self
                            .selected_collection_id
                            .as_deref()
                            .and_then(|id| self.workspace.collection(id))
                            .map(|collection| collection.name.clone())
                            .unwrap_or_default();
                        self.collection_name
                            .update(cx, |input, cx| input.set_value(name, window, cx));
                        self.selected_folder_id = None;
                    }
                    self.request_notice = Some("Collection deleted.".to_owned());
                    cx.notify();
                    return true;
                }
            }
            Err(error) => self.workspace_warning = Some(error.to_string()),
        }
        cx.notify();
        false
    }

    pub(super) fn save_current_request(
        &mut self,
        save_as: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(save_route) = self.request_save_route(save_as) else {
            return;
        };
        let active_association = self.request_tabs.active().association().clone();
        let update_id = (!save_as)
            .then(|| active_association.saved_request_id().map(ToOwned::to_owned))
            .flatten();
        let collection_id = if update_id.is_some() {
            active_association.collection_id().map(ToOwned::to_owned)
        } else {
            self.selected_collection_id
                .clone()
                .or_else(|| active_association.collection_id().map(ToOwned::to_owned))
        };
        let Some(collection_id) = collection_id else {
            self.workspace_warning =
                Some("Create or select a collection before saving this request.".to_owned());
            cx.notify();
            return;
        };
        let folder_id = if update_id.is_some() {
            active_association.folder_id().map(ToOwned::to_owned)
        } else if self.selected_collection_id.as_deref() == Some(collection_id.as_str()) {
            self.selected_folder_id.clone()
        } else {
            active_association.folder_id().map(ToOwned::to_owned)
        };
        self.snapshot_active_request_tab(cx);
        let definition = self.request_template(cx);
        let entered_name = self.saved_request_name.read(cx).value().trim().to_owned();
        let name = if entered_name.is_empty() {
            default_request_name(&definition.request)
        } else {
            entered_name
        };

        if save_route == RequestSaveRoute::Upstream {
            self.save_request_on_upstream(
                collection_id,
                folder_id,
                update_id,
                name,
                definition,
                window,
                cx,
            );
            return;
        }

        let mut candidate = self.workspace.clone();
        let result = if let Some(id) = update_id {
            candidate
                .update_saved_request(&collection_id, &id, definition)
                .and_then(|()| candidate.rename_saved_request(&collection_id, &id, name))
                .map(|()| id)
        } else {
            candidate.create_saved_request_in_folder(
                &collection_id,
                folder_id.as_deref(),
                name,
                definition,
            )
        };

        match result {
            Ok(id) => {
                let saved = candidate
                    .saved_request(&id)
                    .map(|(_, request)| (request.name.clone(), request.folder_id.clone()));
                let Some((saved_name, saved_folder_id)) = saved else {
                    self.workspace_warning =
                        Some("Saved request disappeared before it could be persisted.".to_owned());
                    cx.notify();
                    return;
                };
                let mut candidate_request_tabs = self.request_tabs.clone();
                candidate_request_tabs.active_mut().mark_saved(
                    saved_name.clone(),
                    RequestTabAssociation::new(
                        saved_folder_id,
                        Some(collection_id.clone()),
                        Some(id.clone()),
                    ),
                );
                if self
                    .commit_workspace_and_request_tabs(candidate, candidate_request_tabs)
                    .is_ok()
                {
                    self.request_dirty.begin_hydration();
                    self.saved_request_name
                        .update(cx, |input, cx| input.set_value(saved_name, window, cx));
                    self.request_dirty.end_hydration();
                    self.loaded_request_baseline =
                        self.request_tabs.active().baseline_template().clone();
                    self.request_dirty.clear();
                    self.sync_active_request_tab_identity();
                    debug_assert_eq!(
                        self.workspace
                            .saved_request(&id)
                            .map(|(_, request)| request.definition.clone()),
                        Some(self.loaded_request_baseline.clone()),
                    );
                    self.request_notice = None;
                }
            }
            Err(error) => self.workspace_warning = Some(error.to_string()),
        }
        cx.notify();
    }

    pub(super) fn open_saved_request_rename_dialog(
        &mut self,
        collection_id: String,
        request_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_update_request_content() || self.sending {
            return;
        }
        let Some(request_name) = self
            .workspace
            .collection(&collection_id)
            .and_then(|collection| {
                collection
                    .requests
                    .iter()
                    .find(|request| request.id == request_id)
                    .map(|request| request.name.clone())
            })
        else {
            return;
        };
        let rename_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Request name")
                .default_value(request_name)
        });
        let this = cx.entity().downgrade();
        let input_for_dialog = rename_input.clone();
        window.open_dialog(cx, move |dialog, _, cx| {
            let this_for_ok = this.clone();
            let input_for_ok = input_for_dialog.clone();
            let collection_id_for_ok = collection_id.clone();
            let request_id_for_ok = request_id.clone();
            dialog
                .title("Rename request")
                .w(px(440.))
                .confirm()
                .button_props(DialogButtonProps::default().ok_text("Rename"))
                .on_ok(move |_, window, cx| {
                    let name = input_for_ok.read(cx).value().trim().to_owned();
                    if name.is_empty() {
                        return false;
                    }
                    let Some(this) = this_for_ok.upgrade() else {
                        return true;
                    };
                    this.update(cx, |this, cx| {
                        this.rename_saved_request(
                            collection_id_for_ok.clone(),
                            request_id_for_ok.clone(),
                            name,
                            window,
                            cx,
                        );
                    });
                    true
                })
                .child(
                    v_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("Enter a new display name for this saved request."),
                        )
                        .child(Input::new(&input_for_dialog)),
                )
        });
        rename_input.read(cx).focus_handle(cx).focus(window);
    }

    pub(super) fn rename_saved_request(
        &mut self,
        collection_id: String,
        request_id: String,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.can_update_request_content() || self.sending {
            return false;
        }
        let mut candidate = self.workspace.clone();
        match candidate.rename_saved_request(&collection_id, &request_id, name.clone()) {
            Ok(()) => {
                if !self.workspace_writable {
                    let Some(request) =
                        candidate.collection(&collection_id).and_then(|collection| {
                            collection
                                .requests
                                .iter()
                                .find(|request| request.id == request_id)
                        })
                    else {
                        return false;
                    };
                    let remote_collection_id = request
                        .folder_id
                        .clone()
                        .unwrap_or_else(|| collection_id.clone());
                    self.rename_request_on_upstream(
                        remote_collection_id,
                        request_id,
                        name,
                        request.definition.clone(),
                        window,
                        cx,
                    );
                    return true;
                }
                self.snapshot_active_request_tab(cx);
                let mut candidate_request_tabs = self.request_tabs.clone();
                candidate_request_tabs.rename_saved_request(&request_id, &name);
                if self
                    .commit_workspace_and_request_tabs(candidate, candidate_request_tabs)
                    .is_ok()
                {
                    if self.active_saved_request_id.as_deref() == Some(request_id.as_str()) {
                        let active_title = self.request_tabs.active().title().to_owned();
                        self.saved_request_name
                            .update(cx, |input, cx| input.set_value(active_title, window, cx));
                    }
                    self.request_notice = Some(format!("Renamed request to “{name}”."));
                    cx.notify();
                    return true;
                }
            }
            Err(error) => self.workspace_warning = Some(error.to_string()),
        }
        cx.notify();
        false
    }

    pub(super) fn duplicate_saved_request(
        &mut self,
        collection_id: String,
        request_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_create_request_content() || self.sending {
            return;
        }
        let Some(collection) = self.workspace.collection(&collection_id) else {
            return;
        };
        let Some(source) = collection
            .requests
            .iter()
            .find(|request| request.id == request_id)
        else {
            return;
        };
        let base_name = format!("{} copy", source.name);
        let duplicate_name = unique_name(
            &base_name,
            collection
                .requests
                .iter()
                .map(|request| request.name.as_str()),
        );
        if !self.workspace_writable {
            let remote_collection_id = source
                .folder_id
                .clone()
                .unwrap_or_else(|| collection_id.clone());
            self.duplicate_request_on_upstream(
                remote_collection_id,
                duplicate_name,
                source.definition.clone(),
                window,
                cx,
            );
            return;
        }
        let mut candidate = self.workspace.clone();
        match candidate.duplicate_saved_request(&collection_id, &request_id, duplicate_name.clone())
        {
            Ok(_) => {
                if self.commit_workspace(candidate).is_ok() {
                    self.expanded_collection_ids.insert(collection_id);
                    self.request_notice =
                        Some(format!("Duplicated request as “{duplicate_name}”."));
                }
            }
            Err(error) => self.workspace_warning = Some(error.to_string()),
        }
        cx.notify();
    }

    pub(super) fn copy_saved_request_link(
        &mut self,
        collection_id: String,
        request_id: String,
        cx: &mut Context<Self>,
    ) {
        let Some(url) = self
            .workspace
            .collection(&collection_id)
            .and_then(|collection| {
                collection
                    .requests
                    .iter()
                    .find(|request| request.id == request_id)
                    .map(|request| request.definition.request.url.clone())
            })
        else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(url));
        self.request_notice = Some("Copied request link.".to_owned());
        cx.notify();
    }

    pub(super) fn open_saved_request_delete_dialog(
        &mut self,
        collection_id: String,
        request_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_delete_request_content() || self.sending {
            return;
        }
        let Some(request_name) = self
            .workspace
            .collection(&collection_id)
            .and_then(|collection| {
                collection
                    .requests
                    .iter()
                    .find(|request| request.id == request_id)
                    .map(|request| request.name.clone())
            })
        else {
            return;
        };
        let this = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let this_for_ok = this.clone();
            let collection_id_for_ok = collection_id.clone();
            let request_id_for_ok = request_id.clone();
            dialog
                .title("Delete request?")
                .w(px(440.))
                .confirm()
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("Delete request")
                        .ok_variant(ButtonVariant::Danger),
                )
                .on_ok(move |_, window, cx| {
                    let Some(this) = this_for_ok.upgrade() else {
                        return true;
                    };
                    this.update(cx, |this, cx| {
                        this.delete_saved_request(
                            collection_id_for_ok.clone(),
                            request_id_for_ok.clone(),
                            window,
                            cx,
                        );
                    });
                    true
                })
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!(
                            "“{request_name}” will be permanently removed from this collection."
                        )),
                )
        });
    }

    pub(super) fn delete_saved_request(
        &mut self,
        collection_id: String,
        request_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.can_delete_request_content() || self.sending {
            return false;
        }
        if self.workspace.collection(&collection_id).is_none() {
            return false;
        }
        if !self.workspace_writable {
            let remote_collection_id = self
                .workspace
                .collection(&collection_id)
                .and_then(|collection| {
                    collection
                        .requests
                        .iter()
                        .find(|request| request.id == request_id)
                })
                .and_then(|request| request.folder_id.clone())
                .unwrap_or_else(|| collection_id.clone());
            self.delete_request_on_upstream(remote_collection_id, request_id, window, cx);
            return true;
        }
        let mut candidate = self.workspace.clone();
        match candidate.remove_saved_request(&collection_id, &request_id) {
            Ok(_) => {
                self.snapshot_active_request_tab(cx);
                let mut candidate_request_tabs = self.request_tabs.clone();
                candidate_request_tabs.detach_saved_request(&request_id);
                if self
                    .commit_workspace_and_request_tabs(candidate, candidate_request_tabs)
                    .is_ok()
                {
                    if self.active_saved_request_id.as_deref() == Some(request_id.as_str()) {
                        self.sync_active_request_tab_identity();
                    }
                    self.request_notice = Some("Request deleted.".to_owned());
                    cx.notify();
                    return true;
                }
            }
            Err(error) => self.workspace_warning = Some(error.to_string()),
        }
        cx.notify();
        false
    }
}
