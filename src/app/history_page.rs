use super::*;

impl ApiTester {
    pub(super) fn confirm_delete(&mut self, target: PendingDelete, cx: &mut Context<Self>) -> bool {
        if self.pending_delete.as_ref() == Some(&target) {
            self.pending_delete = None;
            true
        } else {
            self.pending_delete = Some(target);
            cx.notify();
            false
        }
    }

    pub(super) fn clear_history(&mut self, cx: &mut Context<Self>) {
        if !self.history_writable {
            self.history_warning = Some(
                "History was not cleared because the database is read-only for this session."
                    .to_owned(),
            );
            cx.notify();
            return;
        }
        if !self.confirm_delete(PendingDelete::History, cx) {
            return;
        }

        let mut candidate = self.history.clone();
        candidate.clear();
        match self.database_store.save_history(&candidate) {
            Ok(()) => {
                self.history = candidate;
                self.history_warning = None;
                self.delete_own_shared_history(cx);
            }
            Err(error) => {
                self.history_warning = Some(format!("History could not be cleared: {error}"));
            }
        }
        cx.notify();
    }

    pub(super) fn upload_shared_history_entry(
        &mut self,
        upload: SharedHistoryUpload,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.request_history_target.take() else {
            return;
        };
        let vault = self.credential_vault.clone();
        let client = self.upstream_client.clone();
        let runtime = Arc::clone(&self.runtime);
        let upstream_id = target.upstream_id.clone();
        let task = self.runtime.spawn(async move {
            let credential = runtime
                .spawn_blocking(move || vault.load_upstream(&upstream_id))
                .await
                .map_err(|error| format!("Could not open the saved session: {error}"))?
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "Log in to this server again.".to_owned())?;
            if credential.expires_at <= Utc::now() {
                return Err("Log in to this server again.".to_owned());
            }
            upload_shared_history(
                &client,
                &target.base_url,
                credential.bearer_token(),
                &target.workspace_id,
                &upload,
            )
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let Some(this) = this.upgrade() else {
                return;
            };
            this.update(cx, |this, cx| {
                let error = match result {
                    Ok(Ok(())) => None,
                    Ok(Err(error)) => Some(error),
                    Err(error) => Some(format!("the background task failed: {error}")),
                };
                if let Some(error) = error {
                    this.history_warning = Some(format!(
                        "History was saved locally but could not be shared: {error}"
                    ));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn delete_own_shared_history(&mut self, cx: &mut Context<Self>) {
        let Ok(target) = self.active_upstream_workspace() else {
            return;
        };
        let vault = self.credential_vault.clone();
        let client = self.upstream_client.clone();
        let runtime = Arc::clone(&self.runtime);
        let upstream_id = target.upstream_id.clone();
        let task = self.runtime.spawn(async move {
            let credential = runtime
                .spawn_blocking(move || vault.load_upstream(&upstream_id))
                .await
                .map_err(|error| format!("Could not open the saved session: {error}"))?
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "Log in to this server again.".to_owned())?;
            delete_shared_history(
                &client,
                &target.base_url,
                credential.bearer_token(),
                &target.workspace_id,
            )
            .await
            .map_err(|error| error.to_string())
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let Some(this) = this.upgrade() else {
                return;
            };
            this.update(cx, |this, cx| {
                let error = match result {
                    Ok(Ok(())) => None,
                    Ok(Err(error)) => Some(error),
                    Err(error) => Some(format!("the background task failed: {error}")),
                };
                if let Some(error) = error {
                    this.history_warning = Some(format!(
                        "Local history was cleared, but shared history could not be cleared: {error}"
                    ));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn load_history(
        &mut self,
        history_id: String,
        request: RequestDraft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_history_request_tab(history_id, request, window, cx);
    }

    pub(super) fn render_history(&self, cx: &mut Context<Self>) -> AnyElement {
        let rows = self
            .history
            .entries()
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let history_id = entry.id.clone();
                let status = entry.response.as_ref().map(|response| response.status);
                let status_text: SharedString = status
                    .map(|status| status.to_string())
                    .unwrap_or_else(|| "ERR".to_owned())
                    .into();
                let method: SharedString = entry.request.method.clone().into();
                let url: SharedString = compact_url(&entry.request.url).into();
                let time: SharedString = entry
                    .created_at
                    .with_timezone(&Local)
                    .format("%b %d · %H:%M")
                    .to_string()
                    .into();
                let color = method_color(&entry.request.method, cx);

                div()
                    .id(("history-entry", index))
                    .w_full()
                    .mb_1()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|style| style.bg(cx.theme().sidebar_accent))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let Some(request) = this
                            .history
                            .entries()
                            .iter()
                            .find(|entry| entry.id == history_id)
                            .map(|entry| entry.request.clone())
                        else {
                            return;
                        };
                        this.load_history(history_id.clone(), request, window, cx);
                    }))
                    .child(
                        h_flex()
                            .justify_between()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_semibold()
                                            .text_color(color)
                                            .child(method),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(status_text),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(time),
                            ),
                    )
                    .child(
                        div()
                            .mt_1()
                            .w_full()
                            .overflow_hidden()
                            .text_sm()
                            .whitespace_nowrap()
                            .child(url),
                    )
                    .into_any_element()
            })
            .collect::<Vec<_>>();

        v_flex()
            .size_full()
            .min_w_0()
            .h_full()
            .flex_shrink_0()
            .border_r_1()
            .border_color(cx.theme().sidebar_border)
            .bg(cx.api_surface())
            .child(
                h_flex()
                    .h(px(56.))
                    .px_4()
                    .flex_shrink_0()
                    .justify_between()
                    .border_b_1()
                    .border_color(cx.theme().sidebar_border)
                    .child(
                        div()
                            .text_base()
                            .font_semibold()
                            .child(format!("History ({})", self.history.len())),
                    )
                    .child(
                        Button::new("clear-history")
                            .label(if self.pending_delete == Some(PendingDelete::History) {
                                "Confirm"
                            } else {
                                "Clear"
                            })
                            .xsmall()
                            .ghost()
                            .danger()
                            .disabled(self.history.is_empty() || !self.history_writable)
                            .tooltip("Click twice to permanently clear request history")
                            .on_click(cx.listener(|this, _, _, cx| this.clear_history(cx))),
                    ),
            )
            .child(
                div()
                    .id("history-scroll")
                    .flex_1()
                    .min_h_0()
                    .p_2()
                    .overflow_y_scroll()
                    .when(rows.is_empty(), |this| {
                        this.child(
                            v_flex()
                                .items_center()
                                .gap_1()
                                .px_4()
                                .py_8()
                                .text_center()
                                .text_color(cx.theme().muted_foreground)
                                .child(div().text_sm().child("No requests yet"))
                                .child(div().text_xs().child("Completed requests appear here.")),
                        )
                    })
                    .children(rows),
            )
            .when_some(self.history_warning.clone(), |this, warning| {
                this.child(
                    div()
                        .px_3()
                        .py_2()
                        .border_t_1()
                        .border_color(cx.theme().warning)
                        .text_xs()
                        .text_color(cx.theme().warning)
                        .child(warning),
                )
            })
            .into_any_element()
    }
}
