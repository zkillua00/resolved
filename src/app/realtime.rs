use super::request_tab_reconciliation::reconcile_restored_request_tabs;
use super::*;

const REALTIME_REFRESH_DEBOUNCE: Duration = Duration::from_millis(120);
const REALTIME_REFRESH_RETRY_DELAY: Duration = Duration::from_millis(300);
const REALTIME_REFRESH_ATTEMPTS: usize = 3;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct RealtimeRefreshOutcome {
    permissions_changed: bool,
    switched_workspace: bool,
}

impl ApiTester {
    pub(super) fn stop_realtime(&mut self) {
        if let Some(abort_handle) = self.realtime_abort_handle.take() {
            abort_handle.abort();
        }
        if let Some(abort_handle) = self.realtime_refresh_abort_handle.take() {
            abort_handle.abort();
        }
        self.realtime_generation = self.realtime_generation.wrapping_add(1);
        self.realtime_refresh_generation = self.realtime_refresh_generation.wrapping_add(1);
        self.realtime_status = RealtimeConnectionStatus::Inactive;
    }

    pub(super) fn start_realtime_for_active_upstream(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.stop_realtime();

        let Ok(target) = self.active_upstream_workspace() else {
            return;
        };
        let Some(_) = self.settings.upstreams.server(&target.upstream_id) else {
            return;
        };
        self.realtime_status = RealtimeConnectionStatus::Connecting;
        let generation = self.realtime_generation;
        let upstream_id = target.upstream_id.clone();
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let vault = self.credential_vault.clone();
        let runtime = Arc::clone(&self.runtime);
        let task_runtime = Arc::clone(&runtime);
        let task_upstream_id = upstream_id.clone();
        let base_url = target.base_url;
        let task = self.runtime.spawn(async move {
            let credential = match task_runtime
                .spawn_blocking(move || vault.load_upstream(&task_upstream_id))
                .await
            {
                Ok(Ok(Some(credential))) if credential.expires_at > Utc::now() => credential,
                Ok(Ok(Some(_))) | Ok(Ok(None)) => {
                    let _ = sender.send(RealtimeSignal::AuthenticationRequired);
                    return;
                }
                Ok(Err(error)) => {
                    tracing::warn!(%error, "could not open the saved real-time session");
                    let _ = sender.send(RealtimeSignal::Unavailable);
                    return;
                }
                Err(error) => {
                    tracing::warn!(%error, "could not load the saved real-time session");
                    let _ = sender.send(RealtimeSignal::Unavailable);
                    return;
                }
            };
            if let Err(error) = watch_upstream_changes(
                &base_url,
                credential.bearer_token(),
                credential.expires_at,
                sender.clone(),
            )
            .await
            {
                tracing::warn!(%error, "the real-time endpoint is invalid");
                let _ = sender.send(RealtimeSignal::Unavailable);
            }
        });
        self.realtime_abort_handle = Some(task.abort_handle());

        cx.spawn_in(window, async move |this, cx| {
            while let Some(signal) = receiver.recv().await {
                let _ = this.update_in(cx, |this, window, cx| {
                    if this.realtime_generation != generation {
                        return;
                    }
                    match signal {
                        RealtimeSignal::Connected => {
                            this.realtime_status = RealtimeConnectionStatus::Connected;
                            this.sync_activity_log_realtime(
                                server_management::activity_views::ActivityLogKind::Change,
                                window,
                                cx,
                            );
                            this.sync_activity_log_realtime(
                                server_management::activity_views::ActivityLogKind::Audit,
                                window,
                                cx,
                            );
                            this.queue_realtime_refresh(&upstream_id, None, window, cx);
                            cx.notify();
                        }
                        RealtimeSignal::ConnectionLost => {
                            this.realtime_status = RealtimeConnectionStatus::Reconnecting;
                            cx.notify();
                        }
                        RealtimeSignal::Change(change) => {
                            this.realtime_status = RealtimeConnectionStatus::Connected;
                            this.handle_realtime_activity_change(&upstream_id, &change, window, cx);
                            if change.is_shared_history_change() {
                                this.handle_realtime_shared_history_change(
                                    &upstream_id,
                                    &change,
                                    window,
                                    cx,
                                );
                            } else {
                                this.queue_realtime_refresh(
                                    &upstream_id,
                                    Some(&change),
                                    window,
                                    cx,
                                );
                            }
                            cx.notify();
                        }
                        RealtimeSignal::AuthenticationRequired => {
                            this.mark_realtime_session_expired(&upstream_id, window, cx);
                        }
                        RealtimeSignal::Unavailable => {
                            this.realtime_status = RealtimeConnectionStatus::Unavailable;
                            cx.notify();
                        }
                    }
                });
            }
        })
        .detach();
    }

    fn mark_realtime_session_expired(
        &mut self,
        upstream_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut settings = self.settings.clone();
        let Some(profile) = settings
            .upstreams
            .servers
            .iter_mut()
            .find(|profile| profile.id == upstream_id)
        else {
            return;
        };
        let label = profile.display_label();
        profile.session_expires_at = Utc::now();
        if self.settings_writable {
            if let Err(error) = self.database_store.save_app_settings(&settings) {
                self.settings_notice =
                    Some(format!("The server session could not be updated: {error}"));
            } else if let Err(error) = self.apply_persisted_settings(settings, false, cx) {
                self.settings_notice = Some(error);
            }
        }
        self.stop_realtime();
        self.settings_notice = Some(format!("Log in to {label} again."));
        if matches!(
            self.workspace_tabs.active(),
            ActiveWorkspaceTab::RequestProxy | ActiveWorkspaceTab::Settings
        ) {
            self.refresh_server_management(window, cx);
        }
        cx.notify();
    }

    fn handle_realtime_shared_history_change(
        &mut self,
        upstream_id: &str,
        change: &RealtimeResourceChange,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let WorkspaceProviderId::Upstream {
            upstream_id: active_upstream_id,
            workspace_id,
        } = self.workspace_providers.active_id()
        else {
            return;
        };
        if active_upstream_id != upstream_id
            || !change.affects_workspace(workspace_id)
            || !self
                .server_management
                .is_history_visible_for(&change.resource_id)
        {
            return;
        }
        tracing::debug!(
            event_id = %change.event_id,
            action = %change.action,
            workspace_id,
            history_owner_id = %change.resource_id,
            "refreshing visible shared history after a real-time change"
        );
        self.refresh_profile_history_realtime(change.resource_id.clone(), window, cx);
    }

    fn queue_realtime_refresh(
        &mut self,
        upstream_id: &str,
        change: Option<&RealtimeResourceChange>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let WorkspaceProviderId::Upstream {
            upstream_id: active_upstream_id,
            workspace_id,
        } = self.workspace_providers.active_id().clone()
        else {
            return;
        };
        if active_upstream_id != upstream_id || self.workspace_switch_status.busy() {
            return;
        }
        if let Some(change) = change {
            tracing::debug!(
                event_id = %change.event_id,
                resource = %change.resource,
                action = %change.action,
                active_workspace_changed = change.affects_workspace(&workspace_id),
                identity_changed = change.is_identity_change(),
                "received a server resource change"
            );
        }
        let Some(profile) = self.settings.upstreams.server(upstream_id).cloned() else {
            return;
        };
        let Some(base_url) = profile.parsed_base_url() else {
            return;
        };

        if let Some(abort_handle) = self.realtime_refresh_abort_handle.take() {
            abort_handle.abort();
        }
        self.realtime_refresh_generation = self.realtime_refresh_generation.wrapping_add(1);
        let refresh_generation = self.realtime_refresh_generation;
        let realtime_generation = self.realtime_generation;
        let task_upstream_id = upstream_id.to_owned();
        let result_upstream_id = task_upstream_id.clone();
        let preferred_workspace_id = workspace_id.clone();
        let task_workspace_id = preferred_workspace_id.clone();
        let vault = self.credential_vault.clone();
        let client = self.upstream_client.clone();
        let runtime = Arc::clone(&self.runtime);
        let task_runtime = Arc::clone(&runtime);
        let task = self.runtime.spawn(async move {
            tokio::time::sleep(REALTIME_REFRESH_DEBOUNCE).await;
            let credential = task_runtime
                .spawn_blocking(move || vault.load_upstream(&task_upstream_id))
                .await
                .map_err(|error| format!("Could not open the saved session: {error}"))?
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "Log in to this server again.".to_owned())?;
            if credential.expires_at <= Utc::now() {
                return Err("Log in to this server again.".to_owned());
            }
            let mut retry_delay = REALTIME_REFRESH_RETRY_DELAY;
            for attempt in 0..REALTIME_REFRESH_ATTEMPTS {
                match load_upstream_workspace(
                    &client,
                    &base_url,
                    credential.bearer_token(),
                    Some(&task_workspace_id),
                )
                .await
                {
                    Ok(loaded) => return Ok(loaded),
                    Err(error) if attempt + 1 < REALTIME_REFRESH_ATTEMPTS => {
                        tracing::debug!(%error, attempt = attempt + 1, "retrying server refresh");
                        tokio::time::sleep(retry_delay).await;
                        retry_delay = retry_delay.saturating_mul(2);
                    }
                    Err(error) => return Err(error),
                }
            }
            unreachable!("the real-time refresh loop always returns")
        });
        self.realtime_refresh_abort_handle = Some(task.abort_handle());

        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.realtime_generation != realtime_generation
                    || this.realtime_refresh_generation != refresh_generation
                {
                    return;
                }
                this.realtime_refresh_abort_handle = None;
                match result {
                    Ok(Ok(loaded)) => {
                        let outcome = this.apply_realtime_refresh(
                            &result_upstream_id,
                            &preferred_workspace_id,
                            loaded,
                            window,
                            cx,
                        );
                        if !outcome.switched_workspace
                            && matches!(
                                this.workspace_tabs.active(),
                                ActiveWorkspaceTab::RequestProxy | ActiveWorkspaceTab::Settings
                            )
                        {
                            this.refresh_server_management(window, cx);
                        }
                        if outcome.permissions_changed && !outcome.switched_workspace {
                            this.start_realtime_for_active_upstream(window, cx);
                        }
                    }
                    Ok(Err(error)) => {
                        this.workspace_warning = Some(format!(
                            "The latest server changes could not be loaded: {error}"
                        ));
                        cx.notify();
                    }
                    Err(error) if error.is_cancelled() => {}
                    Err(error) => {
                        this.workspace_warning = Some(format!(
                            "The latest server changes could not be loaded: {error}"
                        ));
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    fn apply_realtime_refresh(
        &mut self,
        upstream_id: &str,
        workspace_id: &str,
        loaded: LoadedUpstreamWorkspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> RealtimeRefreshOutcome {
        let expected_provider = WorkspaceProviderId::Upstream {
            upstream_id: upstream_id.to_owned(),
            workspace_id: workspace_id.to_owned(),
        };
        if self.workspace_providers.active_id() != &expected_provider {
            return RealtimeRefreshOutcome::default();
        }

        let selected_workspace_id = loaded
            .selected
            .as_ref()
            .map(|selected| selected.workspace.id.as_str());
        if selected_workspace_id != Some(workspace_id) {
            let can_leave_workspace =
                !self.sending && !self.request_is_dirty() && !self.environment_editor_is_dirty(cx);
            if loaded.selected.is_some() && can_leave_workspace {
                self.finish_upstream_switch(upstream_id.to_owned(), loaded, window, cx);
                return RealtimeRefreshOutcome {
                    switched_workspace: true,
                    ..RealtimeRefreshOutcome::default()
                };
            }
            let profile_updated =
                self.update_realtime_profile(upstream_id, workspace_id, &loaded, cx);
            if loaded.selected.is_none()
                && can_leave_workspace
                && profile_updated
                && let Some(local_workspace_id) = self
                    .database_store
                    .active_local_workspace_id()
                    .ok()
                    .or_else(|| {
                        self.local_workspaces
                            .first()
                            .map(|workspace| workspace.id.clone())
                    })
            {
                self.switch_to_local_workspace(local_workspace_id, window, cx);
                if matches!(
                    self.workspace_providers.active_id(),
                    WorkspaceProviderId::Local(_)
                ) {
                    self.settings_notice =
                        Some("You no longer have access to that server workspace.".to_owned());
                    cx.notify();
                    return RealtimeRefreshOutcome {
                        switched_workspace: true,
                        ..RealtimeRefreshOutcome::default()
                    };
                }
            }
            self.workspace_warning = Some(
                "This workspace is no longer available. Your open edits are still here.".to_owned(),
            );
            cx.notify();
            return RealtimeRefreshOutcome::default();
        }

        let previous_permissions = self
            .settings
            .upstreams
            .server(upstream_id)
            .map(|profile| profile.permission_keys.clone())
            .unwrap_or_default();
        let Some(selected) = loaded.selected.as_ref() else {
            return RealtimeRefreshOutcome::default();
        };
        let active_environment_id = self
            .settings
            .upstreams
            .server(upstream_id)
            .and_then(|profile| profile.active_environment_id(workspace_id))
            .map(str::to_owned)
            .filter(|environment_id| {
                selected
                    .environments
                    .iter()
                    .any(|environment| environment.id == *environment_id)
            });
        let mut workspace = selected.workspace.clone().into_local_workspace();
        workspace.environments = selected
            .environments
            .clone()
            .into_iter()
            .map(UpstreamEnvironmentView::into_local)
            .collect();
        workspace.active_environment_id = active_environment_id;
        if let Err(error) = workspace.validate() {
            self.workspace_warning =
                Some(format!("The server returned an invalid workspace: {error}"));
            cx.notify();
            return RealtimeRefreshOutcome::default();
        }

        let environment_editor_dirty = self.environment_editor_is_dirty(cx);
        let previous_selected_environment_id = self.selected_environment_id.clone();
        self.snapshot_active_request_tab(cx);
        self.snapshot_secondary_pane_request_tabs(cx);
        let active_request_id = self.request_tabs.active_tab_id().clone();
        let active_request_was_dirty = self.request_tabs.active().is_dirty();
        let request_conflicts = refresh_open_request_tabs(&mut self.request_tabs, &workspace);

        if !self.update_realtime_profile(upstream_id, workspace_id, &loaded, cx) {
            return RealtimeRefreshOutcome::default();
        }
        let provider = RemoteWorkspaceProvider::new(
            self.database_store.clone(),
            upstream_id.to_owned(),
            workspace_id.to_owned(),
            workspace.clone(),
        );
        self.workspace_providers.register(Arc::new(provider));
        self.workspace = workspace;
        self.workspace_warning = None;

        self.selected_collection_id = self
            .selected_collection_id
            .take()
            .filter(|id| self.workspace.collection(id).is_some())
            .or_else(|| {
                self.workspace
                    .collections
                    .first()
                    .map(|collection| collection.id.clone())
            });
        self.selected_folder_id = self.selected_folder_id.take().filter(|folder_id| {
            self.selected_collection_id
                .as_deref()
                .and_then(|collection_id| self.workspace.collection(collection_id))
                .is_some_and(|collection| collection.folder(folder_id).is_some())
        });
        self.expanded_collection_ids
            .retain(|id| self.workspace.collection(id).is_some());
        self.expanded_folder_ids.retain(|folder_id| {
            self.workspace
                .collections
                .iter()
                .any(|collection| collection.folder(folder_id).is_some())
        });
        if let Some(collection_id) = self.selected_collection_id.clone() {
            self.expanded_collection_ids.insert(collection_id);
        }

        self.selected_environment_id = if environment_editor_dirty {
            previous_selected_environment_id
        } else {
            previous_selected_environment_id
                .filter(|id| self.workspace.environment(id).is_some())
                .or_else(|| self.workspace.active_environment_id.clone())
                .or_else(|| {
                    self.workspace
                        .environments
                        .first()
                        .map(|environment| environment.id.clone())
                })
        };
        if self.renaming_collection_id.is_none() {
            let name = self
                .selected_collection_id
                .as_deref()
                .and_then(|id| self.workspace.collection(id))
                .map(|collection| collection.name.clone())
                .unwrap_or_default();
            self.collection_name
                .update(cx, |input, cx| input.set_value(name, window, cx));
        }
        if !environment_editor_dirty {
            self.reload_environment_editor(window, cx);
        }

        update_script_variable_catalog(&self.script_variable_catalog, &self.workspace);
        self.template_variable_catalog
            .borrow_mut()
            .replace_environment(self.workspace.active_environment());

        if active_request_was_dirty {
            self.loaded_request_baseline = self.request_tabs.active().baseline_template().clone();
            self.refresh_all_request_dirty_parts(cx);
        } else {
            self.restore_active_request_tab(window, cx);
        }
        self.refresh_secondary_pane_request_tabs(&request_conflicts, window, cx);
        self.persist_request_tabs_now(cx);
        if request_conflicts.contains(&active_request_id) {
            self.request_notice =
                Some("This request changed on the server. Your edits are still here.".to_owned());
        }
        if environment_editor_dirty {
            self.workspace_warning = Some(
                "This environment changed on the server. Your edits are still here.".to_owned(),
            );
        }
        cx.notify();

        let permissions_changed = self
            .settings
            .upstreams
            .server(upstream_id)
            .is_some_and(|profile| profile.permission_keys != previous_permissions);
        RealtimeRefreshOutcome {
            permissions_changed,
            switched_workspace: false,
        }
    }

    fn update_realtime_profile(
        &mut self,
        upstream_id: &str,
        workspace_id: &str,
        loaded: &LoadedUpstreamWorkspace,
        cx: &mut Context<Self>,
    ) -> bool {
        let mut settings = self.settings.clone();
        let Some(profile) = settings
            .upstreams
            .servers
            .iter_mut()
            .find(|profile| profile.id == upstream_id)
        else {
            return false;
        };
        profile.user_id.clone_from(&loaded.current_user.id);
        profile.email.clone_from(&loaded.current_user.email);
        profile
            .display_name
            .clone_from(&loaded.current_user.display_name);
        profile.replace_permissions(loaded.permission_keys.clone());
        let active_workspace_id = loaded
            .summaries
            .iter()
            .any(|workspace| workspace.id == workspace_id)
            .then(|| workspace_id.to_owned());
        profile.replace_workspaces(loaded.summaries.clone(), active_workspace_id);
        if let Some(selected) = loaded.selected.as_ref()
            && selected.workspace.id == workspace_id
        {
            let active_environment_id = profile
                .active_environment_id(workspace_id)
                .map(str::to_owned)
                .filter(|environment_id| {
                    selected
                        .environments
                        .iter()
                        .any(|environment| environment.id == *environment_id)
                });
            profile.set_active_environment_id(workspace_id, active_environment_id.as_deref());
        }
        if let Err(error) = self.database_store.save_app_settings(&settings) {
            self.workspace_warning =
                Some(format!("The server changes could not be saved: {error}"));
            cx.notify();
            return false;
        }
        if let Err(error) = self.apply_persisted_settings(settings, false, cx) {
            self.workspace_warning = Some(error);
            cx.notify();
            return false;
        }
        true
    }
}

fn refresh_open_request_tabs(
    request_tabs: &mut RequestTabs,
    workspace: &Workspace,
) -> HashSet<RequestTabId> {
    let snapshots = request_tabs
        .tabs()
        .iter()
        .filter_map(|tab| {
            Some((
                tab.id().clone(),
                tab.association().saved_request_id()?.to_owned(),
                tab.baseline_title().to_owned(),
                tab.baseline_template().clone(),
                tab.is_dirty(),
            ))
        })
        .collect::<Vec<_>>();
    let mut conflicts = HashSet::new();

    for (tab_id, request_id, previous_title, previous_template, dirty) in snapshots {
        let Some((collection, saved_request)) = workspace.saved_request(&request_id) else {
            conflicts.insert(tab_id);
            continue;
        };
        if dirty
            && (previous_title != saved_request.name
                || previous_template != saved_request.definition)
        {
            conflicts.insert(tab_id.clone());
        }
        if !dirty && let Some(tab) = request_tabs.get_mut(&tab_id) {
            tab.set_template(saved_request.definition.clone());
        }
        request_tabs.mark_tab_saved(
            &tab_id,
            &previous_title,
            saved_request.name.clone(),
            RequestTabAssociation::new(
                saved_request.folder_id.clone(),
                Some(collection.id.clone()),
                Some(saved_request.id.clone()),
            ),
            saved_request.definition.clone(),
        );
    }
    reconcile_restored_request_tabs(request_tabs, workspace, true);
    conflicts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn template(url: &str) -> RequestTemplate {
        RequestTemplate::new(RequestDraft::new("GET", url))
    }

    #[test]
    fn refreshes_clean_tabs_and_preserves_dirty_drafts() {
        let mut workspace = Workspace::default();
        let collection_id = workspace.create_collection("API").unwrap();
        let request_id = workspace
            .create_saved_request(
                &collection_id,
                "Users",
                template("https://example.test/users"),
            )
            .unwrap();
        let (_, saved_request) = workspace.saved_request(&request_id).unwrap();
        let association =
            RequestTabAssociation::new(None, Some(collection_id.clone()), Some(request_id.clone()));
        let mut clean_tabs = RequestTabs::default();
        clean_tabs.open_saved(
            saved_request.name.clone(),
            saved_request.definition.clone(),
            association.clone(),
        );
        let mut dirty_tabs = clean_tabs.clone();
        dirty_tabs
            .active_mut()
            .set_template(template("https://example.test/local-draft"));

        workspace
            .update_saved_request(
                &collection_id,
                &request_id,
                template("https://example.test/people"),
            )
            .unwrap();
        workspace
            .rename_saved_request(&collection_id, &request_id, "People")
            .unwrap();

        assert!(refresh_open_request_tabs(&mut clean_tabs, &workspace).is_empty());
        assert_eq!(
            clean_tabs.active().template(),
            &template("https://example.test/people")
        );
        assert!(!clean_tabs.active().is_dirty());

        let conflicts = refresh_open_request_tabs(&mut dirty_tabs, &workspace);
        assert!(conflicts.contains(dirty_tabs.active_tab_id()));
        assert_eq!(
            dirty_tabs.active().template(),
            &template("https://example.test/local-draft")
        );
        assert_eq!(
            dirty_tabs.active().baseline_template(),
            &template("https://example.test/people")
        );
        assert!(dirty_tabs.active().is_dirty());
    }

    #[test]
    fn detaches_deleted_open_requests_and_reports_the_conflict() {
        let mut workspace = Workspace::default();
        let collection_id = workspace.create_collection("API").unwrap();
        let request_id = workspace
            .create_saved_request(
                &collection_id,
                "Users",
                template("https://example.test/users"),
            )
            .unwrap();
        let (_, saved_request) = workspace.saved_request(&request_id).unwrap();
        let mut tabs = RequestTabs::default();
        tabs.open_saved(
            saved_request.name.clone(),
            saved_request.definition.clone(),
            RequestTabAssociation::new(None, Some(collection_id.clone()), Some(request_id.clone())),
        );

        workspace
            .remove_saved_request(&collection_id, &request_id)
            .unwrap();
        let conflicts = refresh_open_request_tabs(&mut tabs, &workspace);

        assert!(conflicts.contains(tabs.active_tab_id()));
        assert!(tabs.active().is_detached());
        assert_eq!(
            tabs.active().template(),
            &template("https://example.test/users")
        );
    }
}
