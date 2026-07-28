use super::*;

impl ApiTester {
    pub(super) fn create_collection_folder(
        &mut self,
        collection_id: String,
        parent_folder_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.workspace_writable || self.sending {
            return;
        }
        let Some(collection) = self.workspace.collection(&collection_id) else {
            return;
        };
        let name = unique_name(
            "New folder",
            collection
                .folders
                .iter()
                .filter(|folder| folder.parent_folder_id == parent_folder_id)
                .map(|folder| folder.name.as_str()),
        );
        let mut candidate = self.workspace.clone();
        match candidate.create_collection_folder(
            &collection_id,
            parent_folder_id.as_deref(),
            name.clone(),
        ) {
            Ok(folder_id) => {
                if self.commit_workspace(candidate).is_ok() {
                    self.selected_collection_id = Some(collection_id.clone());
                    self.selected_folder_id = Some(folder_id.clone());
                    self.expanded_collection_ids.insert(collection_id);
                    if let Some(parent_id) = parent_folder_id {
                        self.expanded_folder_ids.insert(parent_id);
                    }
                    self.renaming_folder_id = Some(folder_id);
                    self.folder_name
                        .update(cx, |input, cx| input.set_value(name, window, cx));
                    self.folder_name.read(cx).focus_handle(cx).focus(window);
                    self.update_active_unsaved_request_tab_location(
                        self.selected_collection_id.clone(),
                        self.selected_folder_id.clone(),
                        cx,
                    );
                    cx.notify();
                }
            }
            Err(error) => {
                self.workspace_warning = Some(error.to_string());
                cx.notify();
            }
        }
    }

    pub(super) fn select_collection_folder(
        &mut self,
        collection_id: String,
        folder_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((collection_name, folder_name)) = self
            .workspace
            .collection(&collection_id)
            .and_then(|collection| {
                collection
                    .folder(&folder_id)
                    .map(|folder| (collection.name.clone(), folder.name.clone()))
            })
        else {
            return;
        };
        self.selected_collection_id = Some(collection_id);
        self.selected_folder_id = Some(folder_id);
        self.collection_name
            .update(cx, |input, cx| input.set_value(collection_name, window, cx));
        self.folder_name
            .update(cx, |input, cx| input.set_value(folder_name, window, cx));
        self.sidebar_tab = SidebarTab::Collections;
        self.update_active_unsaved_request_tab_location(
            self.selected_collection_id.clone(),
            self.selected_folder_id.clone(),
            cx,
        );
        cx.notify();
    }

    pub(super) fn toggle_collection_folder(
        &mut self,
        collection_id: String,
        folder_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.expanded_folder_ids.remove(&folder_id) {
            self.expanded_folder_ids.insert(folder_id.clone());
        }
        self.select_collection_folder(collection_id, folder_id, window, cx);
    }

    pub(super) fn begin_collection_folder_rename(
        &mut self,
        collection_id: String,
        folder_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_collection_folder(collection_id, folder_id.clone(), window, cx);
        self.renaming_folder_id = Some(folder_id);
        self.folder_name.read(cx).focus_handle(cx).focus(window);
        cx.notify();
    }

    pub(super) fn finish_collection_folder_rename(&mut self, cx: &mut Context<Self>) {
        if !self.workspace_writable {
            return;
        }
        let (Some(collection_id), Some(folder_id)) = (
            self.selected_collection_id.clone(),
            self.renaming_folder_id.clone(),
        ) else {
            return;
        };
        let name = self.folder_name.read(cx).value().to_string();
        let mut candidate = self.workspace.clone();
        match candidate.rename_collection_folder(&collection_id, &folder_id, name) {
            Ok(()) => {
                if self.commit_workspace(candidate).is_ok() {
                    self.renaming_folder_id = None;
                }
            }
            Err(error) => self.workspace_warning = Some(error.to_string()),
        }
        cx.notify();
    }

    pub(super) fn move_collection_folder(
        &mut self,
        collection_id: String,
        folder_id: String,
        parent_folder_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if !self.workspace_writable || self.sending {
            return;
        }
        let mut candidate = self.workspace.clone();
        match candidate.move_collection_folder(
            &collection_id,
            &folder_id,
            parent_folder_id.as_deref(),
        ) {
            Ok(()) => {
                if self.commit_workspace(candidate).is_ok() {
                    self.selected_collection_id = Some(collection_id.clone());
                    self.selected_folder_id = Some(folder_id);
                    self.expanded_collection_ids.insert(collection_id.clone());
                    if let Some(parent_folder_id) = parent_folder_id {
                        self.expanded_folder_ids.insert(parent_folder_id.clone());
                        let parent_name = self
                            .workspace
                            .collection(&collection_id)
                            .and_then(|collection| collection.folder(&parent_folder_id))
                            .map(|folder| folder.name.as_str())
                            .unwrap_or("selected folder");
                        self.request_notice = Some(format!("Moved folder under “{parent_name}”."));
                    } else {
                        self.request_notice =
                            Some("Moved folder to the collection root.".to_owned());
                    }
                    self.update_active_unsaved_request_tab_location(
                        self.selected_collection_id.clone(),
                        self.selected_folder_id.clone(),
                        cx,
                    );
                }
            }
            Err(error) => self.workspace_warning = Some(error.to_string()),
        }
        cx.notify();
    }

    pub(super) fn open_collection_folder_delete_dialog(
        &mut self,
        collection_id: String,
        folder_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.workspace_writable || self.sending {
            return;
        }
        let Some(folder_name) = self
            .workspace
            .collection(&collection_id)
            .and_then(|collection| collection.folder(&folder_id))
            .map(|folder| folder.name.clone())
        else {
            return;
        };
        let confirmation =
            cx.new(|cx| InputState::new(window, cx).placeholder("Type the folder name"));
        let this = cx.entity().downgrade();
        let input_for_dialog = confirmation.clone();
        window.open_dialog(cx, move |dialog, _, cx| {
            let close_this = this.clone();
            let delete_collection_id = collection_id.clone();
            let delete_folder_id = folder_id.clone();
            let expected_name = folder_name.clone();
            let input_for_ok = input_for_dialog.clone();
            let input_for_footer = input_for_dialog.clone();
            let footer_name = folder_name.clone();
            dialog
                .title("Delete folder?")
                .w(px(480.))
                .confirm()
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("Delete folder")
                        .ok_variant(ButtonVariant::Danger),
                )
                .footer(move |ok, cancel, window, cx| {
                    let confirmed =
                        input_for_footer.read(cx).value().as_ref() == footer_name.as_str();
                    vec![
                        cancel(window, cx),
                        if confirmed {
                            ok(window, cx)
                        } else {
                            Button::new("delete-folder-disabled")
                                .label("Delete folder")
                                .danger()
                                .disabled(true)
                                .into_any_element()
                        },
                    ]
                })
                .on_ok(move |_, _, cx| {
                    if input_for_ok.read(cx).value().as_ref() != expected_name.as_str() {
                        return false;
                    }
                    if let Some(this) = close_this.upgrade() {
                        this.update(cx, |this, cx| {
                            this.delete_collection_folder(
                                delete_collection_id.clone(),
                                delete_folder_id.clone(),
                                cx,
                            );
                        });
                    }
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
                                    "This permanently deletes “{folder_name}”, its nested folders, and every request inside them."
                                )),
                        )
                        .child(Input::new(&input_for_dialog)),
                )
        });
        confirmation.read(cx).focus_handle(cx).focus(window);
    }

    pub(super) fn delete_collection_folder(
        &mut self,
        collection_id: String,
        folder_id: String,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.workspace_writable || self.sending {
            return false;
        }
        let Some(collection) = self.workspace.collection(&collection_id) else {
            return false;
        };
        let mut removed_folder_ids = BTreeSet::from([folder_id.clone()]);
        loop {
            let previous_len = removed_folder_ids.len();
            for folder in &collection.folders {
                if folder
                    .parent_folder_id
                    .as_deref()
                    .is_some_and(|parent| removed_folder_ids.contains(parent))
                {
                    removed_folder_ids.insert(folder.id.clone());
                }
            }
            if removed_folder_ids.len() == previous_len {
                break;
            }
        }
        let removed_request_ids = collection
            .requests
            .iter()
            .filter(|request| {
                request
                    .folder_id
                    .as_deref()
                    .is_some_and(|id| removed_folder_ids.contains(id))
            })
            .map(|request| request.id.clone())
            .collect::<Vec<_>>();

        let mut candidate = self.workspace.clone();
        match candidate.remove_collection_folder(&collection_id, &folder_id) {
            Ok(_) => {
                self.snapshot_active_request_tab(cx);
                let mut candidate_request_tabs = self.request_tabs.clone();
                for request_id in &removed_request_ids {
                    candidate_request_tabs.detach_saved_request(request_id);
                }
                for removed_folder_id in &removed_folder_ids {
                    candidate_request_tabs.detach_folder(&collection_id, removed_folder_id);
                }
                if self
                    .commit_workspace_and_request_tabs(candidate, candidate_request_tabs)
                    .is_ok()
                {
                    for removed_folder_id in &removed_folder_ids {
                        self.expanded_folder_ids.remove(removed_folder_id);
                    }
                    if self
                        .selected_folder_id
                        .as_deref()
                        .is_some_and(|id| removed_folder_ids.contains(id))
                    {
                        self.selected_folder_id = None;
                    }
                    if self
                        .renaming_folder_id
                        .as_deref()
                        .is_some_and(|id| removed_folder_ids.contains(id))
                    {
                        self.renaming_folder_id = None;
                    }
                    self.sync_active_request_tab_identity();
                    self.request_notice = Some("Folder deleted.".to_owned());
                    cx.notify();
                    return true;
                }
            }
            Err(error) => self.workspace_warning = Some(error.to_string()),
        }
        cx.notify();
        false
    }

    pub(super) fn move_saved_request_to_selected_folder(
        &mut self,
        collection_id: String,
        request_id: String,
        cx: &mut Context<Self>,
    ) {
        if !self.workspace_writable || self.sending {
            return;
        }
        let target_folder_id =
            if self.selected_collection_id.as_deref() == Some(collection_id.as_str()) {
                self.selected_folder_id.clone()
            } else {
                None
            };
        let mut candidate = self.workspace.clone();
        match candidate.move_saved_request(&collection_id, &request_id, target_folder_id.as_deref())
        {
            Ok(()) => {
                self.snapshot_active_request_tab(cx);
                let mut candidate_request_tabs = self.request_tabs.clone();
                candidate_request_tabs.move_saved_request(
                    &request_id,
                    &collection_id,
                    target_folder_id.as_deref(),
                );
                if self
                    .commit_workspace_and_request_tabs(candidate, candidate_request_tabs)
                    .is_ok()
                {
                    if let Some(folder_id) = target_folder_id.as_ref() {
                        self.expanded_folder_ids.insert(folder_id.clone());
                    }
                    self.sync_active_request_tab_identity();
                    self.request_notice = Some(match target_folder_id {
                        Some(folder_id) => {
                            let name = self
                                .workspace
                                .collection(&collection_id)
                                .and_then(|collection| collection.folder(&folder_id))
                                .map(|folder| folder.name.as_str())
                                .unwrap_or("selected folder");
                            format!("Moved request to “{name}”.")
                        }
                        None => "Moved request to the collection root.".to_owned(),
                    });
                }
            }
            Err(error) => self.workspace_warning = Some(error.to_string()),
        }
        cx.notify();
    }
}
