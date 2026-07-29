use super::*;

// Script errors intentionally carry a full structured report and diagnostic.
// Keeping the unboxed error preserves that context across the background task.
#[allow(clippy::result_large_err)]
impl ApiTester {
    fn script_scope(environment: Option<&Environment>) -> ScriptScope {
        let mut script_environment = ScriptEnvironment::default();
        if let Some(environment) = environment {
            for variable in environment
                .variables
                .iter()
                .filter(|variable| variable.enabled)
            {
                if variable.secret {
                    script_environment.insert_secret(variable.key.clone(), variable.value.clone());
                } else {
                    script_environment.insert(variable.key.clone(), variable.value.clone());
                }
            }
        }
        ScriptScope {
            environment: script_environment,
            ..ScriptScope::default()
        }
    }

    pub(super) fn start_request(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sending {
            return;
        }

        let validation_error = if input_text_is_blank(&self.method, cx) {
            Some("HTTP method cannot be empty.".to_owned())
        } else if self.active_environment_editor_is_dirty(cx) {
            Some(
                "The active environment has unsaved changes. Save or Revert them before sending."
                    .to_owned(),
            )
        } else {
            None
        };
        if let Some(error) = validation_error {
            self.response = None;
            self.request_error = Some(error);
            self.script_diagnostic = None;
            self.execution_stage = None;
            self.hide_preview(cx);
            cx.notify();
            return;
        }

        self.dismiss_template_variable_popover();
        self.request_notice = None;
        let template = self.request_template(cx);
        let environment_id = self.workspace.active_environment_id.clone();
        let scope = Self::script_scope(
            environment_id
                .as_deref()
                .and_then(|id| self.workspace.environment(id)),
        );
        self.request_generation = self.request_generation.wrapping_add(1);
        let generation = self.request_generation;
        self.sending = true;
        self.execution_stage = Some(ExecutionStage::PreRequest);
        self.response = None;
        self.request_error = None;
        self.script_diagnostic = None;
        self.pre_script_report = None;
        self.post_script_report = None;
        self.preview_error = None;
        self.copied = false;
        self.hide_preview(cx);

        let source = template.scripts.pre_request.clone();
        let request = template.request.clone();
        let cancellation = ScriptCancellation::new();
        self.script_cancellation = Some(cancellation.clone());
        let task = self
            .runtime
            .spawn_blocking(move || execute_pre_request(&source, &request, &scope, &cancellation));
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.finish_pre_request(generation, template, environment_id, result, window, cx);
            });
        })
        .detach();
    }

    pub(super) fn finish_pre_request(
        &mut self,
        generation: u64,
        template: RequestTemplate,
        environment_id: Option<String>,
        result: Result<Result<PreRequestResult, ScriptError>, tokio::task::JoinError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if generation != self.request_generation {
            return;
        }

        self.script_cancellation = None;
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                self.fail_request(
                    &template.request,
                    format!("pre-request script task failed: {error}"),
                    cx,
                );
                return;
            }
        };
        let pre_result = match result {
            Ok(result) => result,
            Err(error) => {
                if matches!(
                    error.diagnostic.kind,
                    crate::core::ScriptErrorKind::Cancelled
                ) {
                    self.finish_cancelled(cx);
                    return;
                }
                self.pre_script_report = Some(error.report.clone());
                self.script_diagnostic = Some(error.diagnostic.clone());
                self.response_tab = ResponseTab::Scripts;
                self.fail_request(&template.request, error.to_string(), cx);
                return;
            }
        };

        self.pre_script_report = Some(pre_result.report.clone());
        if let Err(error) = self.apply_environment_mutations(
            environment_id.as_deref(),
            &pre_result.environment_mutations,
            window,
            cx,
        ) {
            self.fail_request(&template.request, error, cx);
            return;
        }

        let environment = environment_id
            .as_deref()
            .and_then(|id| self.workspace.environment(id));
        let mut resolved = match resolve_request(&pre_result.request, environment) {
            Ok(resolved) => resolved,
            Err(error) => {
                self.fail_request(&template.request, error.to_string(), cx);
                return;
            }
        };
        if let Some(environment) = environment {
            resolved.sensitive_values.extend(
                environment
                    .variables
                    .iter()
                    .filter(|variable| variable.enabled && variable.secret)
                    .map(|variable| variable.value.clone()),
            );
        }
        self.begin_network_request(
            generation,
            template,
            environment_id,
            resolved,
            pre_result.report,
            window,
            cx,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn begin_network_request(
        &mut self,
        generation: u64,
        template: RequestTemplate,
        environment_id: Option<String>,
        resolved: crate::core::ResolvedRequest,
        pre_report: ScriptReport,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execution_stage = Some(ExecutionStage::Request);
        let task: RequestTask = spawn_request(
            self.runtime.handle(),
            self.client.clone(),
            resolved.request.clone(),
        );
        self.abort_handle = Some(task.abort_handle());
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let result = task.wait().await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.finish_network_request(
                    generation,
                    template,
                    environment_id,
                    resolved,
                    pre_report,
                    result,
                    window,
                    cx,
                );
            });
        })
        .detach();
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn finish_network_request(
        &mut self,
        generation: u64,
        template: RequestTemplate,
        environment_id: Option<String>,
        resolved: crate::core::ResolvedRequest,
        pre_report: ScriptReport,
        result: Result<ResponseData, RequestError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if generation != self.request_generation {
            return;
        }
        self.abort_handle = None;

        let response = match result {
            Ok(response) => response,
            Err(RequestError::Cancelled) => {
                self.finish_cancelled(cx);
                return;
            }
            Err(error) => {
                self.pre_script_report = Some(pre_report);
                self.fail_request_with_secrets(
                    &resolved.request,
                    resolved.redact_secrets(&error.to_string()),
                    &resolved.sensitive_values,
                    cx,
                );
                return;
            }
        };

        let mut display_response = response.clone();
        display_response.final_url = resolved.redact_secrets(&response.final_url);
        self.response = Some(display_response.clone());
        self.update_response_editor(&display_response, window, cx);

        self.execution_stage = Some(ExecutionStage::PostResponse);
        let scope = Self::script_scope(
            environment_id
                .as_deref()
                .and_then(|id| self.workspace.environment(id)),
        );
        let source = template.scripts.post_response.clone();
        let request = resolved.request.clone();
        let history_request = resolved.request.clone();
        let history_sensitive_values = resolved.sensitive_values.clone();
        let cancellation = ScriptCancellation::new();
        self.script_cancellation = Some(cancellation.clone());
        let task = self.runtime.spawn_blocking(move || {
            execute_post_response(&source, &request, &response, &scope, &cancellation)
        });
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.finish_post_response(
                    generation,
                    environment_id,
                    pre_report,
                    display_response,
                    history_request,
                    history_sensitive_values,
                    result,
                    window,
                    cx,
                );
            });
        })
        .detach();
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn finish_post_response(
        &mut self,
        generation: u64,
        environment_id: Option<String>,
        pre_report: ScriptReport,
        response: ResponseData,
        history_request: RequestDraft,
        history_sensitive_values: Vec<String>,
        result: Result<Result<PostResponseResult, ScriptError>, tokio::task::JoinError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if generation != self.request_generation {
            return;
        }

        self.script_cancellation = None;
        self.sending = false;
        self.execution_stage = None;
        self.pre_script_report = Some(pre_report);
        self.response = Some(response.clone());

        match result {
            Ok(Ok(post_result)) => {
                let mutation_result = self.apply_environment_mutations(
                    environment_id.as_deref(),
                    &post_result.environment_mutations,
                    window,
                    cx,
                );
                self.post_script_report = Some(post_result.report);
                match mutation_result {
                    Ok(()) => {
                        self.script_diagnostic = None;
                        self.request_error = None;
                    }
                    Err(error) => {
                        self.request_error = Some(error);
                        self.response_tab = ResponseTab::Scripts;
                        self.hide_preview(cx);
                    }
                }
            }
            Ok(Err(error)) => {
                if matches!(
                    error.diagnostic.kind,
                    crate::core::ScriptErrorKind::Cancelled
                ) {
                    self.request_error =
                        Some("Request completed; post-response script cancelled".to_owned());
                } else {
                    self.request_error = Some(error.to_string());
                }
                self.post_script_report = Some(error.report);
                self.script_diagnostic = Some(error.diagnostic);
                self.response_tab = ResponseTab::Scripts;
                self.hide_preview(cx);
            }
            Err(error) => {
                self.request_error = Some(format!("post-response script task failed: {error}"));
                self.response_tab = ResponseTab::Scripts;
                self.hide_preview(cx);
            }
        }

        self.history.push(HistoryEntry::completed_with_secrets(
            &history_request,
            &response,
            &history_sensitive_values,
        ));
        self.persist_history();
        if self.response_tab == ResponseTab::Preview
            && self.workspace_tabs.active() == ActiveWorkspaceTab::Request
            && self.sidebar_tab != SidebarTab::Environments
        {
            self.show_preview(window, cx);
        } else {
            self.hide_preview(cx);
        }
        cx.notify();
    }

    pub(super) fn cancel_request(&mut self, cx: &mut Context<Self>) {
        if let Some(cancellation) = self.script_cancellation.take() {
            cancellation.cancel();
        }
        if let Some(abort_handle) = self.abort_handle.take() {
            abort_handle.abort();
        }
        self.request_generation = self.request_generation.wrapping_add(1);
        self.finish_cancelled(cx);
    }

    pub(super) fn finish_cancelled(&mut self, cx: &mut Context<Self>) {
        self.sending = false;
        self.execution_stage = None;
        self.abort_handle = None;
        self.script_cancellation = None;
        self.request_error = Some("Request cancelled".to_owned());
        self.preview_error = None;
        self.hide_preview(cx);
        cx.notify();
    }

    pub(super) fn fail_request(
        &mut self,
        request: &RequestDraft,
        message: String,
        cx: &mut Context<Self>,
    ) {
        self.fail_request_with_secrets(request, message, &[], cx);
    }

    pub(super) fn fail_request_with_secrets(
        &mut self,
        request: &RequestDraft,
        message: String,
        sensitive_values: &[String],
        cx: &mut Context<Self>,
    ) {
        self.sending = false;
        self.execution_stage = None;
        self.abort_handle = None;
        self.script_cancellation = None;
        self.history.push(HistoryEntry::failed_with_secrets(
            request,
            message.clone(),
            sensitive_values,
        ));
        self.request_error = Some(message);
        self.persist_history();
        cx.notify();
    }

    pub(super) fn apply_environment_mutations(
        &mut self,
        environment_id: Option<&str>,
        mutations: &[EnvironmentMutation],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if mutations.is_empty() {
            return Ok(());
        }

        let Some(environment_id) = environment_id else {
            self.workspace_warning = Some(
                "Script environment changes were transient because no environment is active."
                    .to_owned(),
            );
            return Ok(());
        };
        if !self.workspace_writable {
            return Err(
                "Script environment changes could not be saved because storage is read-only."
                    .to_owned(),
            );
        }

        let mut candidate = self.workspace.clone();
        let original_metadata = candidate
            .environment(environment_id)
            .ok_or_else(|| format!("active environment '{environment_id}' no longer exists"))?
            .variables
            .iter()
            .map(|variable| {
                (
                    variable.key.clone(),
                    (variable.id.clone(), variable.enabled, variable.secret),
                )
            })
            .collect::<BTreeMap<_, _>>();
        for mutation in mutations {
            let result = match mutation {
                EnvironmentMutation::Set { key, value } => {
                    let existing = candidate
                        .environment(environment_id)
                        .and_then(|environment| {
                            environment
                                .variables
                                .iter()
                                .find(|variable| variable.key == *key)
                                .map(|variable| {
                                    (variable.id.clone(), variable.enabled, variable.secret)
                                })
                        });
                    if let Some((id, enabled, secret)) = existing {
                        candidate.update_environment_variable(
                            environment_id,
                            &id,
                            key.clone(),
                            value.clone(),
                            enabled,
                            secret,
                        )
                    } else if let Some((original_id, enabled, secret)) =
                        original_metadata.get(key).cloned()
                    {
                        candidate
                            .add_environment_variable(
                                environment_id,
                                key.clone(),
                                value.clone(),
                                enabled,
                                secret,
                            )
                            .map(|temporary_id| {
                                if let Some(variable) = candidate
                                    .environments
                                    .iter_mut()
                                    .find(|environment| environment.id == environment_id)
                                    .and_then(|environment| {
                                        environment
                                            .variables
                                            .iter_mut()
                                            .find(|variable| variable.id == temporary_id)
                                    })
                                {
                                    variable.id = original_id;
                                }
                            })
                    } else {
                        candidate
                            .add_environment_variable(
                                environment_id,
                                key.clone(),
                                value.clone(),
                                true,
                                false,
                            )
                            .map(|_| ())
                    }
                }
                EnvironmentMutation::Unset { key } => {
                    let variable_id =
                        candidate
                            .environment(environment_id)
                            .and_then(|environment| {
                                environment
                                    .variables
                                    .iter()
                                    .find(|variable| variable.key == *key)
                                    .map(|variable| variable.id.clone())
                            });
                    variable_id.map_or(Ok(()), |id| {
                        candidate
                            .remove_environment_variable(environment_id, &id)
                            .map(|_| ())
                    })
                }
            };
            if let Err(error) = result {
                let message = format!("Script environment update was not saved: {error}");
                self.workspace_warning = Some(message.clone());
                return Err(message);
            }
        }

        self.commit_workspace(candidate)
            .map_err(|error| format!("Script environment update was not saved: {error}"))?;
        self.refresh_variable_intelligence(cx);
        if self.selected_environment_id.as_deref() == Some(environment_id) {
            self.reload_environment_editor(window, cx);
        }
        Ok(())
    }
}
