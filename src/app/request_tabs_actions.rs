use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RequestTabsPersistenceAction {
    SucceedWithoutWrite,
    VetoReadOnly,
    Write,
}

struct RequestTabsDurableBaseline<'a> {
    persisted: &'a mut RequestTabs,
    writable: bool,
}

impl<'a> RequestTabsDurableBaseline<'a> {
    fn new(persisted: &'a mut RequestTabs, writable: bool) -> Self {
        Self {
            persisted,
            writable,
        }
    }

    fn action(&self, current: &RequestTabs) -> RequestTabsPersistenceAction {
        if current == self.persisted {
            RequestTabsPersistenceAction::SucceedWithoutWrite
        } else if self.writable {
            RequestTabsPersistenceAction::Write
        } else {
            RequestTabsPersistenceAction::VetoReadOnly
        }
    }

    fn record_write_result<E>(&mut self, current: &RequestTabs, result: &Result<(), E>) {
        if result.is_ok() {
            self.persisted.clone_from(current);
        }
    }
}

impl ApiTester {
    pub(super) fn update_active_request_tab_title(&mut self, cx: &mut Context<Self>) {
        let input_title = self.saved_request_name.read(cx).value().to_string();
        let title = {
            let active = self.request_tabs.active();
            if input_title.trim().is_empty()
                && active.association().saved_request_id().is_none()
                && active.baseline_title() == DEFAULT_REQUEST_TAB_TITLE
            {
                DEFAULT_REQUEST_TAB_TITLE.to_owned()
            } else {
                input_title
            }
        };
        self.request_tabs.active_mut().set_title(title);
        self.schedule_request_tabs_persist(cx);
        cx.notify();
    }

    pub(super) fn snapshot_active_request_tab(&mut self, cx: &App) {
        if self.workspace_tabs.welcome_request_tab_id() == Some(self.request_tabs.active_tab_id()) {
            if self.workspace_tabs.welcome_is_open()
                || self.workspace_tabs.active() != ActiveWorkspaceTab::Request
                || !self.request_is_dirty()
            {
                return;
            }
            self.workspace_tabs.take_welcome_request_tab_id();
            self.request_tabs.dismiss_welcome();
        }
        let tab_id = self.request_tabs.active_tab_id().as_str().to_owned();
        let template = self.request_template(cx);
        let input_title = self.saved_request_name.read(cx).value().to_string();
        let title = {
            let active = self.request_tabs.active();
            if input_title.trim().is_empty()
                && active.association().saved_request_id().is_none()
                && active.baseline_title() == DEFAULT_REQUEST_TAB_TITLE
            {
                DEFAULT_REQUEST_TAB_TITLE.to_owned()
            } else {
                input_title
            }
        };

        let active = self.request_tabs.active_mut();
        active.set_template(template);
        active.set_title(title);
        self.request_tab_runtime.insert(
            tab_id,
            RequestTabRuntime {
                request_pane: self.request_pane,
                response_tab: self.response_tab,
                pretty_body: self.pretty_body,
                response: self.response.clone(),
                request_error: self.request_error.clone(),
                script_diagnostic: self.script_diagnostic.clone(),
                pre_script_report: self.pre_script_report.clone(),
                post_script_report: self.post_script_report.clone(),
                preview_error: self.preview_error.clone(),
                copied: self.copied,
                request_notice: self.request_notice.clone(),
            },
        );
    }

    pub(super) fn schedule_request_tabs_persist(&mut self, cx: &mut Context<Self>) {
        if self.request_dirty.is_hydrating() {
            return;
        }
        self.request_tabs_persist_task = Some(cx.spawn(async move |this, cx| {
            Timer::after(REQUEST_TABS_PERSIST_DEBOUNCE).await;
            let Some(this) = this.upgrade() else {
                return;
            };
            this.update(cx, |this, cx| {
                this.snapshot_active_request_tab(cx);
                let _ = this.save_request_tabs_state();
                cx.notify();
            })
            .ok();
        }));
    }

    fn save_request_tabs_state(&mut self) -> Result<(), String> {
        let mut durable = RequestTabsDurableBaseline::new(
            &mut self.last_persisted_request_tabs,
            self.request_tabs_writable,
        );
        match durable.action(&self.request_tabs) {
            RequestTabsPersistenceAction::SucceedWithoutWrite => {
                if self.request_tabs_writable {
                    self.request_tabs_warning = None;
                }
                Ok(())
            }
            RequestTabsPersistenceAction::VetoReadOnly => {
                let message = "Request tabs are read-only because their stored state could not be loaded safely; the latest tab changes cannot be saved.".to_owned();
                self.request_tabs_warning = Some(message.clone());
                Err(message)
            }
            RequestTabsPersistenceAction::Write => {
                let result = self.database_store.save_request_tabs(&self.request_tabs);
                durable.record_write_result(&self.request_tabs, &result);
                match result {
                    Ok(()) => {
                        self.request_tabs_warning = None;
                        Ok(())
                    }
                    Err(error) => {
                        let message = format!("Request tabs could not be saved: {error}");
                        self.request_tabs_warning = Some(message.clone());
                        Err(message)
                    }
                }
            }
        }
    }

    pub(super) fn persist_request_tabs_now(&mut self, cx: &mut Context<Self>) -> bool {
        self.request_tabs_persist_task = None;
        self.snapshot_active_request_tab(cx);
        self.save_request_tabs_state().is_ok()
    }

    pub(crate) fn flush_request_tabs(&mut self, cx: &mut Context<Self>) -> bool {
        let saved = self.persist_request_tabs_now(cx);
        if !saved {
            self.request_notice = Some(
                "The latest request-tab changes could not be saved, so the window was kept open."
                    .to_owned(),
            );
            cx.notify();
        }
        saved
    }

    pub(super) fn update_active_unsaved_request_tab_location(
        &mut self,
        collection_id: Option<String>,
        folder_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let tab_id = self.request_tabs.active_tab_id().clone();
        if self
            .request_tabs
            .active()
            .association()
            .saved_request_id()
            .is_some()
        {
            return;
        }
        let association = RequestTabAssociation::new(folder_id, collection_id, None);
        if self
            .request_tabs
            .repair_association(&tab_id, association, false)
        {
            self.persist_request_tabs_now(cx);
        }
    }

    pub(super) fn restore_active_request_tab(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let record = self.request_tabs.active().clone();
        let runtime = self
            .request_tab_runtime
            .get(record.id().as_str())
            .cloned()
            .unwrap_or_default();
        let association = record.association().clone();
        let preserved_sidebar_tab = self.sidebar_tab;
        let preserved_collection_id = self.selected_collection_id.clone();
        let preserved_folder_id = self.selected_folder_id.clone();
        let restored_collection_id = association
            .collection_id()
            .filter(|id| self.workspace.collection(id).is_some())
            .map(ToOwned::to_owned);
        let restored_folder_id = restored_collection_id.as_deref().and_then(|collection_id| {
            association.folder_id().and_then(|folder_id| {
                self.workspace
                    .collection(collection_id)
                    .is_some_and(|collection| collection.folder(folder_id).is_some())
                    .then(|| folder_id.to_owned())
            })
        });

        self.detached_request_dirty = false;
        self.request_dirty.clear();
        self.request_dirty.begin_hydration();
        self.load_template_unchecked(
            record.template().clone(),
            restored_collection_id.clone(),
            association.saved_request_id().map(ToOwned::to_owned),
            window,
            cx,
        );

        self.sidebar_tab = preserved_sidebar_tab;
        if restored_collection_id.is_some() {
            self.selected_collection_id = restored_collection_id;
            self.selected_folder_id = restored_folder_id;
        } else {
            self.selected_collection_id = preserved_collection_id;
            self.selected_folder_id = preserved_folder_id;
        }
        if let Some(collection_id) = self.selected_collection_id.clone() {
            self.expanded_collection_ids.insert(collection_id.clone());
            if let Some(folder_id) = self.selected_folder_id.as_deref()
                && let Some(collection) = self.workspace.collection(&collection_id)
                && let Ok(path) = collection.folder_path_ids(folder_id)
            {
                self.expanded_folder_ids.extend(path);
            }
        }
        let preserved_collection_name = self
            .selected_collection_id
            .as_deref()
            .and_then(|id| self.workspace.collection(id))
            .map(|collection| collection.name.clone())
            .unwrap_or_default();
        self.collection_name.update(cx, |input, cx| {
            input.set_value(preserved_collection_name, window, cx)
        });
        self.saved_request_name.update(cx, |input, cx| {
            let title = if association.saved_request_id().is_none()
                && record.title() == DEFAULT_REQUEST_TAB_TITLE
            {
                String::new()
            } else {
                record.title().to_owned()
            };
            input.set_value(title, window, cx);
        });
        self.active_saved_request_id = association.saved_request_id().map(ToOwned::to_owned);
        self.loaded_request_baseline = record.baseline_template().clone();
        self.detached_request_dirty = record.is_detached();
        self.request_dirty.clear();
        self.request_dirty.end_hydration();
        self.refresh_all_request_dirty_parts(cx);

        self.request_pane = runtime.request_pane;
        self.response_tab = runtime.response_tab;
        self.pretty_body = runtime.pretty_body;
        self.response = runtime.response;
        self.request_error = runtime.request_error;
        self.script_diagnostic = runtime.script_diagnostic;
        self.pre_script_report = runtime.pre_script_report;
        self.post_script_report = runtime.post_script_report;
        self.preview_error = runtime.preview_error;
        self.copied = runtime.copied;
        self.request_notice = runtime.request_notice;

        if let Some(response) = self.response.clone() {
            self.update_response_editor(&response, window, cx);
            if self.response_tab == ResponseTab::Preview
                && self.workspace_tabs.active() == ActiveWorkspaceTab::Request
                && self.sidebar_tab != SidebarTab::Environments
            {
                self.show_preview(window, cx);
            } else {
                self.hide_preview(cx);
            }
        } else {
            self.response_editor.update(cx, |editor, cx| {
                editor.set_value(String::new(), window, cx);
            });
            self.hide_preview(cx);
        }
        self.refresh_variable_intelligence(cx);
        cx.notify();
    }

    pub(super) fn activate_request_tab(
        &mut self,
        tab_id: RequestTabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.request_tabs.get(&tab_id).is_none() {
            return;
        }
        let dismissing_welcome_backing =
            self.workspace_tabs.welcome_request_tab_id() == Some(&tab_id);
        let leaving_visible_welcome =
            dismissing_welcome_backing && self.workspace_tabs.welcome_is_open();
        let leaving_tool = self.workspace_tabs.active() != ActiveWorkspaceTab::Request;
        let activating_current_request = self.request_tabs.active_tab_id() == &tab_id;
        if self.sending && !(leaving_tool && activating_current_request) {
            self.request_notice =
                Some("Finish or cancel the active request before switching tabs.".to_owned());
            cx.notify();
            return;
        }
        if self.workspace_tabs.active() == ActiveWorkspaceTab::Settings {
            self.cancel_shortcut_recording(cx);
        }
        self.workspace_tabs.activate_request();
        if dismissing_welcome_backing {
            self.request_tabs.dismiss_welcome();
        }
        self.sidebar_tab = SidebarTab::Collections;
        if activating_current_request {
            if leaving_visible_welcome {
                self.request_tab_runtime
                    .entry(tab_id.as_str().to_owned())
                    .or_default();
                self.hide_preview(cx);
                self.restore_active_request_tab(window, cx);
                self.persist_request_tabs_now(cx);
                return;
            }
            if dismissing_welcome_backing {
                self.snapshot_active_request_tab(cx);
                self.persist_request_tabs_now(cx);
                return;
            }
            if self.expand_request_tab_group_for(&tab_id) {
                self.persist_request_tabs_now(cx);
            }
            if leaving_tool && self.response_tab == ResponseTab::Preview {
                self.show_preview(window, cx);
            }
            cx.notify();
            return;
        }
        self.snapshot_active_request_tab(cx);
        self.hide_preview(cx);
        if self.request_tabs.activate(&tab_id) {
            self.expand_request_tab_group_for(&tab_id);
            self.restore_active_request_tab(window, cx);
            self.persist_request_tabs_now(cx);
        }
    }

    pub(super) fn open_blank_request_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sending {
            return;
        }
        if self.workspace_tabs.active() == ActiveWorkspaceTab::Settings {
            self.cancel_shortcut_recording(cx);
        }
        let welcome_backing = self.workspace_tabs.take_welcome_request_tab_id();
        let reuse_welcome_backing = welcome_backing.is_some() && !self.request_is_dirty();
        if welcome_backing.is_some() {
            self.request_tabs.dismiss_welcome();
        }
        if let Some(tab_id) = welcome_backing.filter(|_| reuse_welcome_backing) {
            self.workspace_tabs.activate_request();
            self.sidebar_tab = SidebarTab::Collections;
            let association = RequestTabAssociation::new(
                self.selected_folder_id.clone(),
                self.selected_collection_id.clone(),
                None,
            );
            let _ = self
                .request_tabs
                .repair_association(&tab_id, association, false);
            self.request_tab_runtime
                .insert(tab_id.as_str().to_owned(), RequestTabRuntime::default());
            self.hide_preview(cx);
            self.restore_active_request_tab(window, cx);
            self.persist_request_tabs_now(cx);
            return;
        }
        self.workspace_tabs.activate_request();
        self.sidebar_tab = SidebarTab::Collections;
        self.snapshot_active_request_tab(cx);
        let tab_id = if self.selected_collection_id.is_none() {
            self.request_tabs.open_new()
        } else {
            self.request_tabs.open_unsaved(
                DEFAULT_REQUEST_TAB_TITLE,
                RequestTemplate::default(),
                RequestTabAssociation::new(
                    self.selected_folder_id.clone(),
                    self.selected_collection_id.clone(),
                    None,
                ),
            )
        };
        self.request_tab_runtime
            .insert(tab_id.as_str().to_owned(), RequestTabRuntime::default());
        self.hide_preview(cx);
        self.restore_active_request_tab(window, cx);
        self.persist_request_tabs_now(cx);
    }

    pub(super) fn open_saved_request_tab(
        &mut self,
        collection_id: String,
        request_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.sending {
            return;
        }
        if self.workspace_tabs.active() == ActiveWorkspaceTab::Settings {
            self.cancel_shortcut_recording(cx);
        }
        let Some((title, definition, folder_id)) = self
            .workspace
            .collection(&collection_id)
            .and_then(|collection| {
                collection
                    .requests
                    .iter()
                    .find(|request| request.id == request_id)
                    .map(|request| {
                        (
                            request.name.clone(),
                            request.definition.clone(),
                            request.folder_id.clone(),
                        )
                    })
            })
        else {
            return;
        };

        let welcome_backing = self.workspace_tabs.take_welcome_request_tab_id();
        let discard_welcome_backing = welcome_backing.clone().filter(|_| !self.request_is_dirty());
        if welcome_backing.is_some() {
            self.request_tabs.dismiss_welcome();
        }
        self.workspace_tabs.activate_request();
        self.sidebar_tab = SidebarTab::Collections;
        if discard_welcome_backing.is_none() {
            self.snapshot_active_request_tab(cx);
        }
        let selected_folder_id = folder_id.clone();
        let opened = self.request_tabs.open_saved(
            title,
            definition,
            RequestTabAssociation::new(folder_id, Some(collection_id.clone()), Some(request_id)),
        );
        if opened.opened {
            self.request_tab_runtime.insert(
                opened.tab_id.as_str().to_owned(),
                RequestTabRuntime::default(),
            );
        }
        if let Some(backing_id) = discard_welcome_backing {
            self.request_tabs
                .close_tabs(std::slice::from_ref(&backing_id));
            self.request_tab_runtime.remove(backing_id.as_str());
        }
        self.expand_request_tab_group_for(&opened.tab_id);
        self.selected_collection_id = Some(collection_id.clone());
        self.selected_folder_id = selected_folder_id.clone();
        self.expanded_collection_ids.insert(collection_id);
        if let Some(folder_id) = selected_folder_id
            && let Some(collection) = self
                .selected_collection_id
                .as_deref()
                .and_then(|id| self.workspace.collection(id))
            && let Ok(path) = collection.folder_path_ids(&folder_id)
        {
            self.expanded_folder_ids.extend(path);
        }
        self.hide_preview(cx);
        self.restore_active_request_tab(window, cx);
        self.persist_request_tabs_now(cx);
    }

    pub(super) fn open_history_request_tab(
        &mut self,
        history_id: String,
        request: RequestDraft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.sending {
            return;
        }
        if self.workspace_tabs.active() == ActiveWorkspaceTab::Settings {
            self.cancel_shortcut_recording(cx);
        }
        let welcome_backing = self.workspace_tabs.take_welcome_request_tab_id();
        let discard_welcome_backing = welcome_backing.clone().filter(|_| !self.request_is_dirty());
        if welcome_backing.is_some() {
            self.request_tabs.dismiss_welcome();
        }
        self.workspace_tabs.activate_request();
        self.sidebar_tab = SidebarTab::Collections;
        if discard_welcome_backing.is_none() {
            self.snapshot_active_request_tab(cx);
        }
        let title = format!("{} {}", request.method, compact_url(&request.url));
        let tab_id = self.request_tabs.open_unsaved(
            title,
            RequestTemplate::new(request),
            RequestTabAssociation::new(
                self.selected_folder_id.clone(),
                self.selected_collection_id.clone(),
                None,
            ),
        );
        self.request_tab_runtime
            .insert(tab_id.as_str().to_owned(), RequestTabRuntime::default());
        if let Some(backing_id) = discard_welcome_backing {
            self.request_tabs
                .close_tabs(std::slice::from_ref(&backing_id));
            self.request_tab_runtime.remove(backing_id.as_str());
        }
        self.request_notice = Some(format!("Opened history entry {history_id}."));
        self.hide_preview(cx);
        self.restore_active_request_tab(window, cx);
        self.persist_request_tabs_now(cx);
    }

    pub(super) fn request_close_request_tab(
        &mut self,
        tab_id: RequestTabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.request_close_request_tabs(tab_id, RequestTabCloseScope::Current, window, cx);
    }

    pub(super) fn request_close_request_tabs(
        &mut self,
        anchor_id: RequestTabId,
        scope: RequestTabCloseScope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.sending {
            self.request_notice =
                Some("Finish or cancel the active request before closing tabs.".to_owned());
            cx.notify();
            return;
        }
        self.snapshot_active_request_tab(cx);
        let close_ids = self.request_tabs.close_target_ids(&anchor_id, scope);
        if close_ids.is_empty() {
            return;
        }
        let dirty_titles = close_ids
            .iter()
            .filter_map(|id| self.request_tabs.get(id))
            .filter(|tab| tab.is_dirty())
            .map(|tab| tab.display_title().to_owned())
            .collect::<Vec<_>>();
        if dirty_titles.is_empty() {
            self.close_request_tabs_now(close_ids, anchor_id, window, cx);
            return;
        }

        let this = cx.entity().downgrade();
        let close_count = close_ids.len();
        let dirty_count = dirty_titles.len();
        let dialog_title = if close_count == 1 {
            "Discard request changes?".to_owned()
        } else {
            format!("Close {close_count} request tabs?")
        };
        let detail = close_tabs_confirmation_detail(&dirty_titles, close_count);
        window.open_dialog(cx, move |dialog, _, cx| {
            let close_this = this.clone();
            let close_ids = close_ids.clone();
            let anchor_id = anchor_id.clone();
            dialog
                .title(dialog_title.clone())
                .w(px(440.))
                .confirm()
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(if close_count == 1 {
                            "Discard".to_owned()
                        } else {
                            format!("Close {close_count} tabs")
                        })
                        .ok_variant(ButtonVariant::Danger),
                )
                .on_ok(move |_, window, cx| {
                    if let Some(this) = close_this.upgrade() {
                        this.update(cx, |this, cx| {
                            this.close_request_tabs_now(
                                close_ids.clone(),
                                anchor_id.clone(),
                                window,
                                cx,
                            );
                        });
                    }
                    true
                })
                .child(
                    v_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(detail.clone()),
                        )
                        .when(dirty_count > 1, |this| {
                            this.child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!(
                                        "{dirty_count} tabs contain unsaved request changes."
                                    )),
                            )
                        }),
                )
        });
    }

    pub(super) fn close_request_tabs_now(
        &mut self,
        close_ids: Vec<RequestTabId>,
        anchor_id: RequestTabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let active_before = self.request_tabs.active_tab_id().clone();
        let request_surface_was_active =
            self.workspace_tabs.active() == ActiveWorkspaceTab::Request;
        let closes_every_request = self
            .request_tabs
            .tabs()
            .iter()
            .all(|tab| close_ids.contains(tab.id()));
        let anchor_survives =
            !close_ids.contains(&anchor_id) && self.request_tabs.get(&anchor_id).is_some();
        let removed = self.request_tabs.close_tabs(&close_ids);
        if removed.is_empty() {
            return;
        }
        for tab in &removed {
            self.request_tab_runtime.remove(tab.id().as_str());
        }
        if closes_every_request {
            let welcome_id = self.request_tabs.active_tab_id().clone();
            self.workspace_tabs
                .open_welcome(welcome_id, request_surface_was_active);
            self.hide_preview(cx);
            self.persist_request_tabs_now(cx);
            cx.notify();
            return;
        }
        if self.request_tabs.get(&active_before).is_none() && anchor_survives {
            let _ = self.request_tabs.activate(&anchor_id);
        }
        let active_after = self.request_tabs.active_tab_id().clone();
        if active_after != active_before {
            if let Some(group_id) = self.request_tabs.active().group_id().cloned() {
                let _ = self.request_tabs.set_group_collapsed(&group_id, false);
            }
            self.hide_preview(cx);
            self.restore_active_request_tab(window, cx);
        } else {
            cx.notify();
        }
        self.persist_request_tabs_now(cx);
    }

    pub(super) fn sync_active_request_tab_identity(&mut self) {
        let active = self.request_tabs.active();
        self.active_saved_request_id = active
            .association()
            .saved_request_id()
            .map(ToOwned::to_owned);
        self.detached_request_dirty = active.is_detached();
    }
}

fn close_tabs_confirmation_detail(dirty_titles: &[String], close_count: usize) -> String {
    if dirty_titles.len() == 1 {
        return if close_count == 1 {
            format!(
                "“{}” has unsaved changes. Closing it will discard the draft.",
                dirty_titles[0]
            )
        } else {
            format!(
                "“{}” has unsaved changes. Closing these tabs will discard that draft.",
                dirty_titles[0]
            )
        };
    }
    let preview = dirty_titles
        .iter()
        .take(3)
        .map(|title| format!("“{title}”"))
        .collect::<Vec<_>>()
        .join(", ");
    if dirty_titles.len() > 3 {
        format!(
            "{preview}, and {} more have unsaved changes. Closing these tabs will discard those drafts.",
            dirty_titles.len() - 3
        )
    } else {
        format!("{preview} have unsaved changes. Closing these tabs will discard those drafts.")
    }
}

#[cfg(test)]
mod persistence_tests {
    use super::*;

    fn changed_tabs() -> RequestTabs {
        let mut tabs = RequestTabs::new();
        tabs.open_new();
        tabs
    }

    #[test]
    fn unchanged_state_is_a_noop_regardless_of_writability() {
        for writable in [true, false] {
            let current = RequestTabs::new();
            let mut persisted = current.clone();
            let durable = RequestTabsDurableBaseline::new(&mut persisted, writable);

            assert_eq!(
                durable.action(&current),
                RequestTabsPersistenceAction::SucceedWithoutWrite
            );
        }
    }

    #[test]
    fn changed_read_only_state_vetoes_persistence() {
        let current = changed_tabs();
        let mut persisted = RequestTabs::new();
        let durable = RequestTabsDurableBaseline::new(&mut persisted, false);

        assert_eq!(
            durable.action(&current),
            RequestTabsPersistenceAction::VetoReadOnly
        );
    }

    #[test]
    fn changed_writable_state_requests_one_write_and_success_advances_baseline() {
        let current = changed_tabs();
        let mut persisted = RequestTabs::new();
        let mut writes = 0;
        let mut durable = RequestTabsDurableBaseline::new(&mut persisted, true);

        if durable.action(&current) == RequestTabsPersistenceAction::Write {
            writes += 1;
            let result: Result<(), &str> = Ok(());
            durable.record_write_result(&current, &result);
        }

        assert_eq!(writes, 1);
        assert_eq!(
            durable.action(&current),
            RequestTabsPersistenceAction::SucceedWithoutWrite,
            "the shutdown retry must not write the same state again"
        );
    }

    #[test]
    fn failed_write_retains_baseline_for_retry_or_read_only_veto() {
        let current = changed_tabs();
        let mut persisted = RequestTabs::new();
        {
            let mut durable = RequestTabsDurableBaseline::new(&mut persisted, true);
            assert_eq!(
                durable.action(&current),
                RequestTabsPersistenceAction::Write
            );
            let result = Err("disk full");
            durable.record_write_result(&current, &result);
            assert_eq!(
                durable.action(&current),
                RequestTabsPersistenceAction::Write,
                "a failed write must remain pending for the next flush"
            );
        }

        let durable = RequestTabsDurableBaseline::new(&mut persisted, false);
        assert_eq!(
            durable.action(&current),
            RequestTabsPersistenceAction::VetoReadOnly
        );
    }

    #[test]
    fn aggregate_close_confirmation_bounds_the_title_preview() {
        let detail = close_tabs_confirmation_detail(
            &[
                "One".to_owned(),
                "Two".to_owned(),
                "Three".to_owned(),
                "Four".to_owned(),
            ],
            5,
        );

        assert!(detail.contains("“One”, “Two”, “Three”, and 1 more"));
        assert!(!detail.contains("“Four”"));
    }
}
