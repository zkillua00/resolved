use super::request_tab_reconciliation::reconcile_restored_request_tabs;
use super::*;

enum UpstreamWorkspaceMutation {
    RenameWorkspace {
        name: String,
    },
    RenameCollection {
        collection_id: String,
        name: String,
    },
    MoveCollection {
        collection_id: String,
        parent_collection_id: Option<String>,
    },
    DeleteCollection {
        collection_id: String,
        folder: bool,
    },
    DuplicateRequest {
        collection_id: String,
        name: String,
        definition: RequestTemplate,
    },
    RenameRequest {
        collection_id: String,
        request_id: String,
        name: String,
        definition: RequestTemplate,
    },
    MoveRequest {
        collection_id: String,
        request_id: String,
        target_collection_id: String,
    },
    DeleteRequest {
        collection_id: String,
        request_id: String,
    },
}

impl UpstreamWorkspaceMutation {
    fn required_permission(&self) -> &'static str {
        match self {
            Self::RenameWorkspace { .. } => WORKSPACES_UPDATE,
            Self::RenameCollection { .. } | Self::MoveCollection { .. } => COLLECTIONS_UPDATE,
            Self::DeleteCollection { .. } => COLLECTIONS_DELETE,
            Self::DuplicateRequest { .. } => REQUESTS_CREATE,
            Self::RenameRequest { .. } | Self::MoveRequest { .. } => REQUESTS_UPDATE,
            Self::DeleteRequest { .. } => REQUESTS_DELETE,
        }
    }

    fn success_notice(&self) -> String {
        match self {
            Self::RenameWorkspace { name } => format!("Renamed workspace to “{name}”."),
            Self::RenameCollection { name, .. } => format!("Renamed to “{name}”."),
            Self::MoveCollection { .. } => "Folder location updated.".to_owned(),
            Self::DeleteCollection { folder, .. } => {
                if *folder {
                    "Folder deleted.".to_owned()
                } else {
                    "Collection deleted.".to_owned()
                }
            }
            Self::DuplicateRequest { name, .. } => {
                format!("Duplicated request as “{name}”.")
            }
            Self::RenameRequest { name, .. } => format!("Renamed request to “{name}”."),
            Self::MoveRequest { .. } => "Request location updated.".to_owned(),
            Self::DeleteRequest { .. } => "Request deleted.".to_owned(),
        }
    }

    fn failure_subject(&self) -> &'static str {
        match self {
            Self::RenameWorkspace { .. } => "workspace",
            Self::RenameCollection { .. } => "collection",
            Self::MoveCollection { .. } => "folder",
            Self::DeleteCollection { folder, .. } => {
                if *folder {
                    "folder"
                } else {
                    "collection"
                }
            }
            Self::DuplicateRequest { .. } => "request copy",
            Self::RenameRequest { .. } => "request",
            Self::MoveRequest { .. } => "request",
            Self::DeleteRequest { .. } => "request",
        }
    }

    fn renamed_request(&self) -> Option<(&str, &str)> {
        match self {
            Self::RenameRequest {
                request_id, name, ..
            } => Some((request_id, name)),
            _ => None,
        }
    }

    async fn execute(
        &self,
        client: &Client,
        target: &ActiveUpstreamWorkspace,
        bearer_token: &str,
    ) -> Result<(), UpstreamWorkspaceError> {
        match self {
            Self::RenameWorkspace { name } => update_upstream_workspace(
                client,
                &target.base_url,
                bearer_token,
                &target.workspace_id,
                name,
            )
            .await
            .map(|_| ()),
            Self::RenameCollection {
                collection_id,
                name,
            } => update_upstream_collection(
                client,
                &target.base_url,
                bearer_token,
                &target.workspace_id,
                collection_id,
                name,
            )
            .await
            .map(|_| ()),
            Self::MoveCollection {
                collection_id,
                parent_collection_id,
            } => move_upstream_collection(
                client,
                &target.base_url,
                bearer_token,
                &target.workspace_id,
                collection_id,
                parent_collection_id.as_deref(),
            )
            .await
            .map(|_| ()),
            Self::DeleteCollection { collection_id, .. } => {
                delete_upstream_collection(
                    client,
                    &target.base_url,
                    bearer_token,
                    &target.workspace_id,
                    collection_id,
                )
                .await
            }
            Self::DuplicateRequest {
                collection_id,
                name,
                definition,
            } => create_upstream_saved_request(
                client,
                &target.base_url,
                bearer_token,
                &target.workspace_id,
                collection_id,
                name,
                definition,
            )
            .await
            .map(|_| ()),
            Self::RenameRequest {
                collection_id,
                request_id,
                name,
                definition,
            } => update_upstream_saved_request(
                client,
                &target.base_url,
                bearer_token,
                &target.workspace_id,
                collection_id,
                request_id,
                name,
                definition,
            )
            .await
            .map(|_| ()),
            Self::MoveRequest {
                collection_id,
                request_id,
                target_collection_id,
            } => move_upstream_saved_request(
                client,
                &target.base_url,
                bearer_token,
                &target.workspace_id,
                collection_id,
                request_id,
                target_collection_id,
            )
            .await
            .map(|_| ()),
            Self::DeleteRequest {
                collection_id,
                request_id,
            } => {
                delete_upstream_saved_request(
                    client,
                    &target.base_url,
                    bearer_token,
                    &target.workspace_id,
                    collection_id,
                    request_id,
                )
                .await
            }
        }
    }
}

impl ApiTester {
    pub(super) fn rename_collection_on_upstream(
        &mut self,
        collection_id: String,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.mutate_upstream_workspace(
            UpstreamWorkspaceMutation::RenameCollection {
                collection_id,
                name,
            },
            window,
            cx,
        );
    }

    pub(super) fn move_collection_on_upstream(
        &mut self,
        collection_id: String,
        parent_collection_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.mutate_upstream_workspace(
            UpstreamWorkspaceMutation::MoveCollection {
                collection_id,
                parent_collection_id,
            },
            window,
            cx,
        );
    }

    pub(super) fn delete_collection_on_upstream(
        &mut self,
        collection_id: String,
        folder: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.mutate_upstream_workspace(
            UpstreamWorkspaceMutation::DeleteCollection {
                collection_id,
                folder,
            },
            window,
            cx,
        );
    }

    pub(super) fn duplicate_request_on_upstream(
        &mut self,
        collection_id: String,
        name: String,
        definition: RequestTemplate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.mutate_upstream_workspace(
            UpstreamWorkspaceMutation::DuplicateRequest {
                collection_id,
                name,
                definition,
            },
            window,
            cx,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn rename_request_on_upstream(
        &mut self,
        collection_id: String,
        request_id: String,
        name: String,
        definition: RequestTemplate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.mutate_upstream_workspace(
            UpstreamWorkspaceMutation::RenameRequest {
                collection_id,
                request_id,
                name,
                definition,
            },
            window,
            cx,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn move_request_on_upstream(
        &mut self,
        collection_id: String,
        request_id: String,
        target_collection_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.mutate_upstream_workspace(
            UpstreamWorkspaceMutation::MoveRequest {
                collection_id,
                request_id,
                target_collection_id,
            },
            window,
            cx,
        );
    }

    pub(super) fn delete_request_on_upstream(
        &mut self,
        collection_id: String,
        request_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.mutate_upstream_workspace(
            UpstreamWorkspaceMutation::DeleteRequest {
                collection_id,
                request_id,
            },
            window,
            cx,
        );
    }

    fn mutate_upstream_workspace(
        &mut self,
        mutation: UpstreamWorkspaceMutation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.workspace_writable || self.sending || self.workspace_switch_status.busy() {
            return;
        }
        if !self.active_upstream_has_permission(mutation.required_permission()) {
            return;
        }
        let target = match self.active_upstream_workspace() {
            Ok(target) => target,
            Err(error) => {
                self.workspace_warning = Some(error);
                cx.notify();
                return;
            }
        };

        self.snapshot_active_request_tab(cx);
        self.workspace_switch_generation = self.workspace_switch_generation.wrapping_add(1);
        let generation = self.workspace_switch_generation;
        self.workspace_switch_status = WorkspaceSwitchStatus::Loading;
        let vault = self.credential_vault.clone();
        let client = self.upstream_client.clone();
        let runtime = Arc::clone(&self.runtime);
        let credential_upstream_id = target.upstream_id.clone();
        let task_target = target.clone();
        let task = self.runtime.spawn(async move {
            let credential = runtime
                .spawn_blocking(move || vault.load_upstream(&credential_upstream_id))
                .await
                .map_err(|error| format!("Could not open the saved session: {error}"))?
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "Log in to this server again.".to_owned())?;
            if credential.expires_at <= Utc::now() {
                return Err("Log in to this server again.".to_owned());
            }
            mutation
                .execute(&client, &task_target, credential.bearer_token())
                .await
                .map_err(|error| {
                    format!(
                        "The {} could not be changed: {error}",
                        mutation.failure_subject()
                    )
                })?;
            let workspace = get_upstream_workspace(
                &client,
                &task_target.base_url,
                credential.bearer_token(),
                &task_target.workspace_id,
            )
            .await
            .map_err(|error| format!("The workspace could not be reloaded: {error}"))?;
            Ok::<_, String>((mutation, workspace))
        });
        self.workspace_switch_abort_handle = Some(task.abort_handle());
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.workspace_switch_generation != generation {
                    return;
                }
                this.workspace_switch_abort_handle = None;
                match result {
                    Ok(Ok((mutation, workspace))) => this.finish_upstream_workspace_mutation(
                        target, mutation, workspace, window, cx,
                    ),
                    Ok(Err(error)) => this.fail_remote_workspace_write(error, cx),
                    Err(error) if error.is_cancelled() => {}
                    Err(error) => this.fail_remote_workspace_write(
                        format!("The server change could not be completed: {error}"),
                        cx,
                    ),
                }
            });
        })
        .detach();
    }

    fn finish_upstream_workspace_mutation(
        &mut self,
        target: ActiveUpstreamWorkspace,
        mutation: UpstreamWorkspaceMutation,
        remote: UpstreamWorkspaceView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let expected_provider = WorkspaceProviderId::Upstream {
            upstream_id: target.upstream_id.clone(),
            workspace_id: target.workspace_id.clone(),
        };
        if self.workspace_providers.active_id() != &expected_provider {
            self.workspace_switch_status = WorkspaceSwitchStatus::Idle;
            return;
        }
        if remote.id != target.workspace_id {
            self.fail_remote_workspace_write(
                "The server returned a different workspace.".to_owned(),
                cx,
            );
            return;
        }

        let summary = remote.summary();
        let mut workspace = remote.into_local_workspace();
        workspace.environments = self.workspace.environments.clone();
        workspace.active_environment_id = self.workspace.active_environment_id.clone();
        if let Err(error) = workspace.validate() {
            self.fail_remote_workspace_write(
                format!("The server returned an invalid workspace: {error}"),
                cx,
            );
            return;
        }

        let mut request_tabs = self.request_tabs.clone();
        if let Some((request_id, name)) = mutation.renamed_request() {
            request_tabs.rename_saved_request(request_id, name);
        }
        reconcile_restored_request_tabs(&mut request_tabs, &workspace, true);
        let tabs_warning = self
            .workspace_providers
            .active()
            .save_request_tabs(&request_tabs)
            .err()
            .map(|error| format!("The request tabs could not be remembered: {error}"));

        let previous_collection_id = self.selected_collection_id.clone();
        let previous_folder_id = self.selected_folder_id.clone();
        self.workspace = workspace.clone();
        self.request_tabs = request_tabs;
        self.last_persisted_request_tabs = self.request_tabs.clone();
        self.workspace_providers
            .register(Arc::new(RemoteWorkspaceProvider::new(
                self.database_store.clone(),
                target.upstream_id.clone(),
                target.workspace_id.clone(),
                workspace,
            )));
        self.selected_collection_id = previous_collection_id
            .filter(|id| self.workspace.collection(id).is_some())
            .or_else(|| {
                self.workspace
                    .collections
                    .first()
                    .map(|collection| collection.id.clone())
            });
        self.selected_folder_id = previous_folder_id.filter(|folder_id| {
            self.selected_collection_id
                .as_deref()
                .and_then(|collection_id| self.workspace.collection(collection_id))
                .is_some_and(|collection| collection.folder(folder_id).is_some())
        });
        let collection_name = self
            .selected_collection_id
            .as_deref()
            .and_then(|id| self.workspace.collection(id))
            .map(|collection| collection.name.clone())
            .unwrap_or_default();
        let folder_name = self
            .selected_collection_id
            .as_deref()
            .and_then(|collection_id| self.workspace.collection(collection_id))
            .and_then(|collection| {
                self.selected_folder_id
                    .as_deref()
                    .and_then(|folder_id| collection.folder(folder_id))
            })
            .map(|folder| folder.name.clone())
            .unwrap_or_default();
        self.collection_name
            .update(cx, |input, cx| input.set_value(collection_name, window, cx));
        self.folder_name
            .update(cx, |input, cx| input.set_value(folder_name, window, cx));
        if let Some((request_id, _)) = mutation.renamed_request()
            && self.active_saved_request_id.as_deref() == Some(request_id)
        {
            let title = self.request_tabs.active().title().to_owned();
            self.saved_request_name
                .update(cx, |input, cx| input.set_value(title, window, cx));
        }
        self.sync_active_request_tab_identity();

        let mut settings = self.settings.clone();
        let summary_updated = settings
            .upstreams
            .servers
            .iter_mut()
            .find(|profile| profile.id == target.upstream_id)
            .and_then(|profile| {
                profile
                    .workspaces
                    .iter_mut()
                    .find(|workspace| workspace.id == summary.id)
            })
            .map(|stored| stored.name = summary.name)
            .is_some();
        let settings_warning = if summary_updated {
            self.commit_settings(settings, false, cx).err()
        } else {
            Some("The workspace is no longer listed for this server.".to_owned())
        };

        self.workspace_switch_status = WorkspaceSwitchStatus::Idle;
        self.workspace_warning = settings_warning;
        self.request_tabs_warning = tabs_warning;
        self.request_notice = Some(mutation.success_notice());
        cx.notify();
    }

    pub(super) fn open_rename_upstream_workspace_dialog(
        &mut self,
        upstream_id: String,
        workspace_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(profile) = self.settings.upstreams.server(&upstream_id) else {
            return;
        };
        if !profile.has_permission(WORKSPACES_UPDATE) || self.workspace_switch_status.busy() {
            return;
        }
        let Some(workspace) = profile
            .workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)
        else {
            return;
        };
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Workspace name")
                .default_value(workspace.name.clone())
        });
        let this = cx.entity().downgrade();
        let dialog_input = input.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            let rename_this = this.clone();
            let rename_input = dialog_input.clone();
            dialog
                .title("Rename workspace")
                .w(px(440.))
                .confirm()
                .button_props(DialogButtonProps::default().ok_text("Rename"))
                .on_ok(move |_, window, cx| {
                    let name = rename_input.read(cx).value().trim().to_owned();
                    if name.is_empty() {
                        return false;
                    }
                    if let Some(this) = rename_this.upgrade() {
                        this.update(cx, |this, cx| {
                            this.mutate_upstream_workspace(
                                UpstreamWorkspaceMutation::RenameWorkspace { name: name.clone() },
                                window,
                                cx,
                            );
                        });
                    }
                    true
                })
                .child(Input::new(&dialog_input))
        });
        input.read(cx).focus_handle(cx).focus(window);
    }

    pub(super) fn open_delete_upstream_workspace_dialog(
        &mut self,
        upstream_id: String,
        workspace_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(profile) = self.settings.upstreams.server(&upstream_id) else {
            return;
        };
        if !profile.has_permission(WORKSPACES_DELETE) || self.workspace_switch_status.busy() {
            return;
        }
        let Some(workspace) = profile
            .workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)
        else {
            return;
        };
        let workspace_name = workspace.name.clone();
        let confirmation =
            cx.new(|cx| InputState::new(window, cx).placeholder("Type the workspace name"));
        let this = cx.entity().downgrade();
        let dialog_input = confirmation.clone();
        window.open_dialog(cx, move |dialog, _, cx| {
            let delete_this = this.clone();
            let input_for_ok = dialog_input.clone();
            let input_for_footer = dialog_input.clone();
            let expected_name = workspace_name.clone();
            let footer_name = workspace_name.clone();
            let delete_upstream_id = upstream_id.clone();
            let delete_workspace_id = workspace_id.clone();
            dialog
                .title("Delete workspace?")
                .w(px(480.))
                .confirm()
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("Delete workspace")
                        .ok_variant(ButtonVariant::Danger),
                )
                .footer(move |ok, cancel, window, cx| {
                    let confirmed = input_for_footer.read(cx).value().as_ref()
                        == footer_name.as_str();
                    vec![
                        cancel(window, cx),
                        if confirmed {
                            ok(window, cx)
                        } else {
                            Button::new("delete-upstream-workspace-disabled")
                                .label("Delete workspace")
                                .danger()
                                .disabled(true)
                                .into_any_element()
                        },
                    ]
                })
                .on_ok(move |_, window, cx| {
                    if input_for_ok.read(cx).value().as_ref() != expected_name.as_str() {
                        return false;
                    }
                    if let Some(this) = delete_this.upgrade() {
                        this.update(cx, |this, cx| {
                            this.delete_upstream_workspace(
                                delete_upstream_id.clone(),
                                delete_workspace_id.clone(),
                                window,
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
                                    "This permanently deletes “{workspace_name}” and all of its collections, requests, and environments."
                                )),
                        )
                        .child(Input::new(&dialog_input)),
                )
        });
        confirmation.read(cx).focus_handle(cx).focus(window);
    }

    fn delete_upstream_workspace(
        &mut self,
        upstream_id: String,
        workspace_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(profile) = self.settings.upstreams.server(&upstream_id).cloned() else {
            return;
        };
        if !profile.has_permission(WORKSPACES_DELETE)
            || self.sending
            || self.workspace_switch_status.busy()
        {
            return;
        }
        let Some(base_url) = profile.parsed_base_url() else {
            return;
        };
        self.workspace_switch_generation = self.workspace_switch_generation.wrapping_add(1);
        let generation = self.workspace_switch_generation;
        self.workspace_switch_status = WorkspaceSwitchStatus::Loading;
        let vault = self.credential_vault.clone();
        let client = self.upstream_client.clone();
        let runtime = Arc::clone(&self.runtime);
        let task_upstream_id = upstream_id.clone();
        let task_workspace_id = workspace_id.clone();
        let task = self.runtime.spawn(async move {
            let credential = runtime
                .spawn_blocking(move || vault.load_upstream(&task_upstream_id))
                .await
                .map_err(|error| format!("Could not open the saved session: {error}"))?
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "Log in to this server again.".to_owned())?;
            delete_upstream_workspace(
                &client,
                &base_url,
                credential.bearer_token(),
                &task_workspace_id,
            )
            .await
            .map_err(|error| format!("The workspace could not be deleted: {error}"))?;
            list_upstream_workspaces(&client, &base_url, credential.bearer_token())
                .await
                .map_err(|error| format!("The workspace list could not be reloaded: {error}"))
        });
        self.workspace_switch_abort_handle = Some(task.abort_handle());
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.workspace_switch_generation != generation {
                    return;
                }
                this.workspace_switch_abort_handle = None;
                match result {
                    Ok(Ok(workspaces)) => {
                        let preferred = workspaces.first().map(|workspace| workspace.id.clone());
                        let summaries = workspaces
                            .iter()
                            .map(UpstreamWorkspaceView::summary)
                            .collect::<Vec<_>>();
                        let mut settings = this.settings.clone();
                        let Some(profile) = settings
                            .upstreams
                            .servers
                            .iter_mut()
                            .find(|profile| profile.id == upstream_id)
                        else {
                            this.fail_remote_workspace_write(
                                "That server is no longer configured.".to_owned(),
                                cx,
                            );
                            return;
                        };
                        profile.replace_workspaces(summaries, preferred.clone());
                        if let Err(error) = this.commit_settings(settings, false, cx) {
                            this.fail_remote_workspace_write(error, cx);
                            return;
                        }
                        this.workspace_switch_status = WorkspaceSwitchStatus::Idle;
                        this.request_notice = Some("Workspace deleted.".to_owned());
                        if let Some(workspace_id) = preferred {
                            this.switch_to_upstream(
                                upstream_id.clone(),
                                Some(workspace_id),
                                window,
                                cx,
                            );
                        } else if let Some(local) = this.local_workspaces.first() {
                            this.switch_to_local_workspace(local.id.clone(), window, cx);
                        }
                    }
                    Ok(Err(error)) => this.fail_remote_workspace_write(error, cx),
                    Err(error) if error.is_cancelled() => {}
                    Err(error) => this.fail_remote_workspace_write(
                        format!("The workspace could not be deleted: {error}"),
                        cx,
                    ),
                }
            });
        })
        .detach();
    }
}
