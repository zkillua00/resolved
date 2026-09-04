use super::*;

impl ScriptConsoleModel {
    pub(super) fn row_count(&self) -> usize {
        self.sections.iter().map(|section| section.rows.len()).sum()
    }

    pub(super) fn copy_all_text(&self) -> String {
        if self.sections.is_empty() {
            return "No script has run yet.".to_owned();
        }

        self.sections
            .iter()
            .map(|section| {
                let mut lines = vec![match section.duration {
                    Some(duration) => {
                        format!("{} · {}", section.title, format_script_duration(duration))
                    }
                    None => section.title.clone(),
                }];
                lines.extend(section.rows.iter().map(|row| {
                    let mut text = if row.tone == ScriptConsoleTone::Command {
                        format!("> {}", row.message)
                    } else {
                        format!("[{}] {}", row.label, row.message)
                    };
                    if let Some(detail) = row.detail.as_deref() {
                        for line in detail.lines() {
                            text.push_str("\n    ");
                            text.push_str(line);
                        }
                    }
                    text
                }));
                lines.join("\n")
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

pub(super) fn script_console_model(
    pre: Option<&ScriptReport>,
    post: Option<&ScriptReport>,
    diagnostic: Option<&ScriptDiagnostic>,
    request_error: Option<&str>,
) -> ScriptConsoleModel {
    let mut sections = Vec::new();
    let mut diagnostic_rendered = false;

    for report in [pre, post].into_iter().flatten() {
        let mut rows = Vec::new();

        for log in &report.logs {
            let (label, tone, message) = match log.level {
                ScriptLogLevel::Log => ("LOG", ScriptConsoleTone::Neutral, log.message.clone()),
                ScriptLogLevel::Info => ("INFO", ScriptConsoleTone::Info, log.message.clone()),
                ScriptLogLevel::Warn => ("WARN", ScriptConsoleTone::Warning, log.message.clone()),
                ScriptLogLevel::Error => ("ERROR", ScriptConsoleTone::Danger, log.message.clone()),
                ScriptLogLevel::Debug if log.message.starts_with("> ") => (
                    "",
                    ScriptConsoleTone::Command,
                    log.message.trim_start_matches("> ").to_owned(),
                ),
                ScriptLogLevel::Debug => ("DEBUG", ScriptConsoleTone::Debug, log.message.clone()),
            };
            let values = log
                .values
                .iter()
                .map(|value| ScriptConsoleValue {
                    kind: value.kind.clone(),
                    preview: value.preview.clone(),
                })
                .collect::<Vec<_>>();
            let detail = script_console_values_detail(&values);
            rows.push(ScriptConsoleRow {
                label: label.to_owned(),
                message,
                detail,
                copy_value: log.message.clone(),
                tone,
                values,
            });
        }

        for test in &report.tests {
            let label = if test.passed { "PASS" } else { "FAIL" };
            let mut copy_value = format!("[{label}] {}", test.name);
            if let Some(message) = test.message.as_deref() {
                copy_value.push('\n');
                copy_value.push_str(message);
            }
            rows.push(ScriptConsoleRow {
                label: label.to_owned(),
                message: test.name.clone(),
                detail: test.message.clone(),
                copy_value,
                tone: if test.passed {
                    ScriptConsoleTone::Success
                } else {
                    ScriptConsoleTone::Danger
                },
                values: Vec::new(),
            });
        }

        if report.response_body_truncated {
            let message = "Response body was truncated for the script runtime.".to_owned();
            rows.push(ScriptConsoleRow {
                label: "NOTICE".to_owned(),
                message: message.clone(),
                detail: None,
                copy_value: message,
                tone: ScriptConsoleTone::Warning,
                values: Vec::new(),
            });
        }

        if let Some(diagnostic) = diagnostic.filter(|item| item.phase == report.phase) {
            rows.push(script_diagnostic_console_row(diagnostic));
            diagnostic_rendered = true;
        }

        if rows.is_empty() {
            let message = "No console output or tests.".to_owned();
            rows.push(ScriptConsoleRow {
                label: "EMPTY".to_owned(),
                message: message.clone(),
                detail: None,
                copy_value: message,
                tone: ScriptConsoleTone::Neutral,
                values: Vec::new(),
            });
        }

        sections.push(ScriptConsoleSection {
            key: report.phase.to_string(),
            title: script_phase_title(report.phase).to_owned(),
            duration: Some(report.duration),
            rows,
        });
    }

    if let Some(diagnostic) = diagnostic.filter(|_| !diagnostic_rendered) {
        sections.push(ScriptConsoleSection {
            key: diagnostic.phase.to_string(),
            title: script_phase_title(diagnostic.phase).to_owned(),
            duration: None,
            rows: vec![script_diagnostic_console_row(diagnostic)],
        });
    }

    // Script failures already carry a structured diagnostic. Avoid presenting
    // the same error a second time through the request-level fallback string.
    if diagnostic.is_none()
        && let Some(error) = request_error.filter(|error| !error.trim().is_empty())
    {
        sections.push(ScriptConsoleSection {
            key: "request".to_owned(),
            title: "Request".to_owned(),
            duration: None,
            rows: vec![ScriptConsoleRow {
                label: "ERROR".to_owned(),
                message: error.to_owned(),
                detail: None,
                copy_value: error.to_owned(),
                tone: ScriptConsoleTone::Danger,
                values: Vec::new(),
            }],
        });
    }

    ScriptConsoleModel { sections }
}

pub(super) fn script_diagnostic_console_row(diagnostic: &ScriptDiagnostic) -> ScriptConsoleRow {
    let label = script_error_kind_label(diagnostic.kind);
    let message = format!("{}: {}", diagnostic.filename, diagnostic.message);
    let mut copy_value = format!("[{label}] {message}");
    if let Some(stack) = diagnostic.stack.as_deref() {
        copy_value.push('\n');
        copy_value.push_str(stack);
    }
    ScriptConsoleRow {
        label: label.to_owned(),
        message,
        detail: diagnostic.stack.clone(),
        copy_value,
        tone: ScriptConsoleTone::Danger,
        values: Vec::new(),
    }
}

fn script_console_values_detail(values: &[ScriptConsoleValue]) -> Option<String> {
    let expandable = values.iter().filter_map(|value| {
        if !matches!(value.kind.as_str(), "array" | "object") {
            return None;
        }
        serde_json::from_str::<serde_json::Value>(&value.preview)
            .ok()
            .and_then(|parsed| serde_json::to_string_pretty(&parsed).ok())
    });
    let detail = expandable.collect::<Vec<_>>().join("\n");
    (!detail.is_empty()).then_some(detail)
}

pub(super) fn script_phase_title(phase: ScriptPhase) -> &'static str {
    match phase {
        ScriptPhase::PreRequest => "Pre-request",
        ScriptPhase::PostResponse => "Post-response",
    }
}

pub(super) fn script_error_kind_label(kind: ScriptErrorKind) -> &'static str {
    match kind {
        ScriptErrorKind::SourceLimit => "SOURCE",
        ScriptErrorKind::BodyLimit => "BODY",
        ScriptErrorKind::Cancelled => "CANCEL",
        ScriptErrorKind::TimedOut => "TIMEOUT",
        ScriptErrorKind::MemoryLimit => "MEMORY",
        ScriptErrorKind::Syntax => "SYNTAX",
        ScriptErrorKind::Runtime => "RUNTIME",
        ScriptErrorKind::OutputLimit => "OUTPUT",
        ScriptErrorKind::Engine => "ENGINE",
    }
}

pub(super) fn format_script_duration(duration: Duration) -> String {
    format!("{:.2} ms", duration.as_secs_f64() * 1_000.0)
}

pub(super) fn script_console_tone_color(tone: ScriptConsoleTone, cx: &App) -> Hsla {
    match tone {
        ScriptConsoleTone::Neutral | ScriptConsoleTone::Debug | ScriptConsoleTone::Command => {
            cx.theme().muted_foreground
        }
        ScriptConsoleTone::Info => cx.theme().info,
        ScriptConsoleTone::Warning => cx.theme().warning,
        ScriptConsoleTone::Danger => cx.theme().danger,
        ScriptConsoleTone::Success => cx.theme().success,
    }
}

pub(super) fn script_console_tone_background(tone: ScriptConsoleTone, cx: &App) -> Hsla {
    match tone {
        ScriptConsoleTone::Info => cx.theme().info.opacity(0.025),
        ScriptConsoleTone::Warning => cx.theme().warning.opacity(0.045),
        ScriptConsoleTone::Danger => cx.theme().danger.opacity(0.055),
        ScriptConsoleTone::Success => cx.theme().success.opacity(0.025),
        ScriptConsoleTone::Neutral | ScriptConsoleTone::Debug | ScriptConsoleTone::Command => {
            cx.api_surface_lowest()
        }
    }
}

pub(super) fn script_console_tone_icon(tone: ScriptConsoleTone) -> IconName {
    match tone {
        ScriptConsoleTone::Neutral => IconName::SquareTerminal,
        ScriptConsoleTone::Info => IconName::Info,
        ScriptConsoleTone::Warning => IconName::TriangleAlert,
        ScriptConsoleTone::Danger => IconName::CircleX,
        ScriptConsoleTone::Success => IconName::CircleCheck,
        ScriptConsoleTone::Debug => IconName::Inspector,
        ScriptConsoleTone::Command => IconName::ChevronRight,
    }
}

pub(super) fn script_console_value_color(kind: &str, cx: &App) -> Hsla {
    match kind {
        "string" => cx.theme().success,
        "number" | "bigint" => cx.theme().warning,
        "boolean" => cx.theme().info,
        "error" => cx.theme().danger,
        "null" | "undefined" => cx.theme().muted_foreground,
        _ => cx.theme().foreground,
    }
}

impl ApiTester {
    fn append_script_console_report(&mut self, mut report: ScriptReport) {
        let current = self.post_script_report.get_or_insert_with(|| ScriptReport {
            phase: ScriptPhase::PostResponse,
            duration: Duration::ZERO,
            logs: Vec::new(),
            tests: Vec::new(),
            response_body_truncated: false,
        });
        current.duration += report.duration;
        current.logs.append(&mut report.logs);
        current.tests.append(&mut report.tests);
        current.response_body_truncated |= report.response_body_truncated;
        self.script_console_cleared_key = None;
        self.script_console_scroll.scroll_to_bottom();
    }

    pub(super) fn evaluate_script_console(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let source = self.script_console_input.read(cx).value(cx).to_string();
        if source.trim().is_empty() {
            return;
        }
        self.script_console_input
            .update(cx, |editor, cx| editor.set_value("", window, cx));
        if let Err(error) = self.start_script_console(source, false, window, cx) {
            window.push_notification(Notification::warning(error), cx);
        }
    }

    pub(super) fn start_control_script_console(
        &mut self,
        source: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<u64, String> {
        self.start_script_console(source, true, window, cx)
    }

    #[allow(clippy::result_large_err)]
    fn start_script_console(
        &mut self,
        source: String,
        mcp_owned: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<u64, String> {
        if self.script_console_running || self.sending {
            return Err(
                "Another request or script-console evaluation is already running.".to_owned(),
            );
        }
        if source.trim().is_empty() {
            return Err("Script console source cannot be empty.".to_owned());
        }
        let (Some(request), Some(response)) =
            (self.response_request.clone(), self.response.clone())
        else {
            return Err("Run an HTTP request before using the script console.".to_owned());
        };

        if self.script_console_history.last() != Some(&source) {
            self.script_console_history.push(source.clone());
            if self.script_console_history.len() > 100 {
                self.script_console_history.remove(0);
            }
        }
        self.script_console_history_cursor = None;
        self.script_console_history_draft.clear();
        self.append_script_console_report(ScriptReport {
            phase: ScriptPhase::PostResponse,
            duration: Duration::ZERO,
            logs: vec![ScriptLog {
                level: ScriptLogLevel::Debug,
                message: format!("> {source}"),
                values: Vec::new(),
            }],
            tests: Vec::new(),
            response_body_truncated: false,
        });
        self.script_console_running = true;
        self.mcp_script_console_generation =
            self.mcp_script_console_generation.wrapping_add(1).max(1);
        let operation_id = self.mcp_script_console_generation;
        self.mcp_script_console_operation_id = mcp_owned.then_some(operation_id);
        self.mcp_script_console_request_id = mcp_owned
            .then(|| self.active_saved_request_id.clone())
            .flatten();
        self.mcp_script_console_owned = mcp_owned;
        if mcp_owned {
            self.response_tab = ResponseTab::Scripts;
        }

        let generation = self.request_generation;
        let tab_id = self.request_tabs.active_tab_id().clone();
        let environment_id = self.workspace.active_environment_id.clone();
        let mut scope = Self::script_scope(
            environment_id
                .as_deref()
                .and_then(|id| self.workspace.environment(id)),
        );
        scope.script_timeout = self.settings.script.timeout();
        let namespace = self.request_namespace.clone();
        let cancellation = ScriptCancellation::new();
        self.script_cancellation = Some(cancellation.clone());
        let chainer = self.build_inline_chainer(&environment_id);
        let task = self.runtime.spawn_blocking(move || {
            crate::core::execute_post_response_console_with_chain(
                &source,
                &request,
                &response,
                &scope,
                &namespace,
                &cancellation,
                Some(&chainer),
            )
        });

        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.script_console_running = false;
                this.mcp_script_console_owned = false;
                if generation != this.request_generation
                    || &tab_id != this.request_tabs.active_tab_id()
                {
                    this.script_cancellation = None;
                    cx.notify();
                    return;
                }
                match result {
                    Ok(Ok(result)) => {
                        let chained = result.chained_requests.clone();
                        if let Err(message) = this.apply_environment_mutations(
                            environment_id.as_deref(),
                            &result.environment_mutations,
                            window,
                            cx,
                        ) {
                            this.append_script_console_error(message);
                        }
                        this.append_script_console_report(result.report);
                        if !chained.is_empty() {
                            this.run_post_chain(
                                generation,
                                environment_id.clone(),
                                chained,
                                window,
                                cx,
                            );
                        } else {
                            this.script_cancellation = None;
                        }
                    }
                    Ok(Err(mut error)) => {
                        this.script_cancellation = None;
                        let message = error.diagnostic.message.clone();
                        if let Some(stack) = error.diagnostic.stack.as_deref()
                            && !stack.trim().is_empty()
                        {
                            error.report.logs.push(ScriptLog {
                                level: ScriptLogLevel::Error,
                                message: format!("{message}\n{stack}"),
                                values: Vec::new(),
                            });
                        } else {
                            error.report.logs.push(ScriptLog {
                                level: ScriptLogLevel::Error,
                                message,
                                values: Vec::new(),
                            });
                        }
                        this.append_script_console_report(error.report);
                    }
                    Err(error) => {
                        this.script_cancellation = None;
                        this.append_script_console_error(format!(
                            "Console evaluation task failed: {error}"
                        ));
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
        Ok(operation_id)
    }

    pub(super) fn navigate_script_console_history(
        &mut self,
        direction: i32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.script_console_history.is_empty() || self.script_console_running {
            return;
        }

        let next = if direction < 0 {
            match self.script_console_history_cursor {
                Some(index) => index.saturating_sub(1),
                None => {
                    self.script_console_history_draft =
                        self.script_console_input.read(cx).value(cx).to_string();
                    self.script_console_history.len() - 1
                }
            }
        } else {
            let Some(index) = self.script_console_history_cursor else {
                return;
            };
            if index + 1 >= self.script_console_history.len() {
                self.script_console_history_cursor = None;
                let draft = self.script_console_history_draft.clone();
                self.script_console_input
                    .update(cx, |editor, cx| editor.set_value(draft, window, cx));
                return;
            }
            index + 1
        };

        self.script_console_history_cursor = Some(next);
        let source = self.script_console_history[next].clone();
        self.script_console_input
            .update(cx, |editor, cx| editor.set_value(source, window, cx));
    }

    fn append_script_console_error(&mut self, message: String) {
        self.append_script_console_report(ScriptReport {
            phase: ScriptPhase::PostResponse,
            duration: Duration::ZERO,
            logs: vec![ScriptLog {
                level: ScriptLogLevel::Error,
                message,
                values: Vec::new(),
            }],
            tests: Vec::new(),
            response_body_truncated: false,
        });
    }

    pub(super) fn copy_script_results(&mut self, cx: &mut Context<Self>) {
        let mut model = script_console_model(
            self.pre_script_report.as_ref(),
            self.post_script_report.as_ref(),
            self.script_diagnostic.as_ref(),
            self.request_error.as_deref(),
        );
        if self.script_console_cleared_key
            == Some(script_console_content_key(self.request_generation, &model))
        {
            model = ScriptConsoleModel::default();
        }
        cx.write_to_clipboard(ClipboardItem::new_string(model.copy_all_text()));
        self.copied = true;
        cx.notify();
    }

    pub(super) fn render_script_results(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut model = script_console_model(
            self.pre_script_report.as_ref(),
            self.post_script_report.as_ref(),
            self.script_diagnostic.as_ref(),
            self.request_error.as_deref(),
        );
        let console_content_key = script_console_content_key(self.request_generation, &model);
        if self.script_console_cleared_key == Some(console_content_key) {
            model = ScriptConsoleModel::default();
        }
        let row_count = model.row_count();
        let prompt_lines = self
            .script_console_input
            .read(cx)
            .value(cx)
            .split('\n')
            .count()
            .clamp(1, 5);
        let prompt_height = px(24. + (prompt_lines.saturating_sub(1) as f32 * 19.));
        let copy_all_text = model.copy_all_text();
        let context_owner = cx.entity().downgrade();
        let copy_label = if self.copied { "Copied" } else { "Copy all" };
        let generation = self.request_generation;
        let sections = model
            .sections
            .iter()
            .enumerate()
            .map(|(section_index, section)| {
                let section_key = section.key.clone();
                let rows = section.rows.iter().enumerate().map(|(row_index, row)| {
                    let expansion_key =
                        format!("{generation}-{section_key}-{section_index}-{row_index}");
                    let expandable = !row.values.is_empty() && row.detail.is_some();
                    let expanded = self.script_console_expanded_rows.contains(&expansion_key);
                    let row_id: SharedString = format!(
                        "script-console-row-{generation}-{section_key}-{section_index}-{row_index}"
                    )
                    .into();
                    let tone_color = script_console_tone_color(row.tone, cx);
                    let tone_background = script_console_tone_background(row.tone, cx);
                    let is_command = row.tone == ScriptConsoleTone::Command;
                    let row_owner = context_owner.clone();
                    let row_copy_value = row.copy_value.clone();
                    let row_copy_all = copy_all_text.clone();
                    let value_elements = row
                        .values
                        .iter()
                        .map(|value| {
                            div()
                                .text_color(script_console_value_color(&value.kind, cx))
                                .child(value.preview.clone())
                        })
                        .collect::<Vec<_>>();
                    let toggle_key = expansion_key.clone();

                    h_flex()
                        .id(row_id)
                        .w_full()
                        .min_h(px(24.))
                        .items_start()
                        .px_1()
                        .py(px(2.))
                        .gap_1()
                        .border_b_1()
                        .border_color(cx.api_outline_variant().opacity(0.55))
                        .bg(tone_background)
                        .hover(|style| style.bg(cx.theme().muted.opacity(0.18)))
                        .when(expandable, |this| {
                            this.cursor_pointer().on_click(cx.listener(
                                move |this, _: &ClickEvent, _, cx| {
                                    if !this.script_console_expanded_rows.insert(toggle_key.clone())
                                    {
                                        this.script_console_expanded_rows.remove(&toggle_key);
                                    }
                                    cx.notify();
                                },
                            ))
                        })
                        .child(if is_command {
                            div()
                                .w(px(16.))
                                .h(px(19.))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_size(cx.theme().mono_font_size)
                                .text_color(cx.theme().muted_foreground)
                                .child(">")
                                .into_any_element()
                        } else {
                            div()
                                .w(px(16.))
                                .h(px(19.))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    gpui_component::Icon::new(if expandable {
                                        if expanded {
                                            IconName::ChevronDown
                                        } else {
                                            IconName::ChevronRight
                                        }
                                    } else {
                                        script_console_tone_icon(row.tone)
                                    })
                                    .with_size(px(12.))
                                    .text_color(tone_color),
                                )
                                .into_any_element()
                        })
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_size(cx.theme().mono_font_size)
                                .line_height(px(19.))
                                .child(if value_elements.is_empty() {
                                    div()
                                        .min_w_0()
                                        .whitespace_normal()
                                        .child(row.message.clone())
                                        .into_any_element()
                                } else {
                                    h_flex()
                                        .min_w_0()
                                        .flex_wrap()
                                        .gap_x_2()
                                        .children(value_elements)
                                        .into_any_element()
                                })
                                .when_some(
                                    row.detail.clone().filter(|_| !expandable || expanded),
                                    |this, detail| {
                                        this.child(
                                            div()
                                                .mt_1()
                                                .pl_2()
                                                .border_l_1()
                                                .border_color(tone_color.opacity(0.5))
                                                .whitespace_normal()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(detail),
                                        )
                                    },
                                ),
                        )
                        .context_menu(move |menu, _, _| {
                            script_console_context_menu(
                                menu,
                                row_owner.clone(),
                                Some(row_copy_value.clone()),
                                row_copy_all.clone(),
                                console_content_key,
                            )
                        })
                        .into_any_element()
                });

                v_flex().w_full().children(rows).into_any_element()
            })
            .collect::<Vec<_>>();
        let prompt = h_flex()
            .id("script-console-prompt")
            .h(prompt_height)
            .flex_shrink_0()
            .items_start()
            .bg(cx.api_surface_lowest())
            .px_1()
            .py(px(2.))
            .child(
                div()
                    .w(px(16.))
                    .flex_shrink_0()
                    .h(px(19.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_size(cx.theme().mono_font_size)
                    .text_color(cx.theme().muted_foreground)
                    .child(">"),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .when(
                        self.response.is_none() || self.script_console_running || self.sending,
                        |this| this.opacity(0.55),
                    )
                    .child(self.script_console_input.clone()),
            );

        v_flex()
            .size_full()
            .overflow_hidden()
            .bg(cx.api_surface_lowest())
            .child(
                h_flex()
                    .h(px(30.))
                    .flex_shrink_0()
                    .px_2()
                    .gap_2()
                    .border_b_1()
                    .border_color(cx.api_outline_variant())
                    .bg(cx.api_surface_low())
                    .child(
                        gpui_component::Icon::new(IconName::SquareTerminal)
                            .with_size(px(15.))
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(div().text_sm().font_semibold().child("Script console"))
                    .when(
                        self.active_saved_request_id.as_deref()
                            == self.mcp_script_console_request_id.as_deref()
                            && self.mcp_script_console_operation_id.is_some(),
                        |this| {
                            this.child(
                                div()
                                    .px_2()
                                    .py(px(2.))
                                    .rounded_full()
                                    .bg(cx.theme().info.opacity(0.12))
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(cx.theme().info)
                                    .child(if self.script_console_running {
                                        "MCP running"
                                    } else {
                                        "MCP"
                                    }),
                            )
                        },
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!(
                                "{row_count} {}",
                                if row_count == 1 { "entry" } else { "entries" }
                            )),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new(("copy-all-script-output", generation))
                            .icon(if self.copied {
                                IconName::Check
                            } else {
                                IconName::Copy
                            })
                            .label(copy_label)
                            .small()
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.copy_script_results(cx);
                            })),
                    ),
            )
            .child(
                v_flex()
                    .id(("script-results-scroll", generation))
                    .flex_1()
                    .min_h_0()
                    .track_scroll(&self.script_console_scroll)
                    .children(sections)
                    .overflow_y_scrollbar()
                    .child(prompt)
                    .child(div().flex_1().context_menu(move |menu, _, _| {
                        script_console_context_menu(
                            menu,
                            context_owner.clone(),
                            None,
                            copy_all_text.clone(),
                            console_content_key,
                        )
                    })),
            )
            .into_any_element()
    }
}

fn script_console_context_menu(
    mut menu: PopupMenu,
    owner: WeakEntity<ApiTester>,
    row: Option<String>,
    all: String,
    console_content_key: u64,
) -> PopupMenu {
    if let Some(row) = row {
        menu = menu.item(
            PopupMenuItem::new("Copy message").on_click(move |_, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(row.clone()));
            }),
        );
    }
    let clear_owner = owner.clone();
    menu.item(PopupMenuItem::new("Copy all").on_click(move |_, _, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string(all.clone()));
        if let Some(owner) = owner.upgrade() {
            owner.update(cx, |this, cx| {
                this.copied = true;
                cx.notify();
            });
        }
    }))
    .separator()
    .item(
        PopupMenuItem::new("Clear console").on_click(move |_, _, cx| {
            if let Some(owner) = clear_owner.upgrade() {
                owner.update(cx, |this, cx| {
                    this.script_console_cleared_key = Some(console_content_key);
                    this.script_console_expanded_rows.clear();
                    this.copied = false;
                    cx.notify();
                });
            }
        }),
    )
}

fn script_console_content_key(generation: u64, model: &ScriptConsoleModel) -> u64 {
    use std::hash::{DefaultHasher, Hash as _, Hasher as _};

    let mut hasher = DefaultHasher::new();
    generation.hash(&mut hasher);
    model.copy_all_text().hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod interactive_tests {
    use gpui::{TestAppContext, VisualTestContext, px, size};

    use super::*;

    fn mount_app(
        cx: &mut TestAppContext,
    ) -> (Entity<ApiTester>, &mut VisualTestContext, tempfile::TempDir) {
        let directory = tempfile::tempdir().expect("create temporary database directory");
        let store = DatabaseStore::new(directory.path().join("api-tester.sqlite3"));
        store.initialize().expect("initialize test database");
        let mut app = None;
        let (_, visual) = cx.add_window_view(|window, cx| {
            gpui_component::init(cx);
            let base_key_bindings = shortcuts::capture_base_key_bindings(cx);
            crate::theme::configure(cx);
            let view = cx
                .new(|cx| ApiTester::new_with_database_store(base_key_bindings, store, window, cx));
            crate::register_app_action_handlers(&view, cx);
            app = Some(view.clone());
            gpui_component::Root::new(view, window, cx)
        });
        (app.expect("capture app entity"), visual, directory)
    }

    #[gpui::test]
    fn console_prompt_is_an_editor_and_arrow_keys_restore_history(cx: &mut TestAppContext) {
        let (app, cx, _directory) = mount_app(cx);
        cx.simulate_resize(size(px(1_200.), px(800.)));
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.post_script_report = Some(ScriptReport {
                    phase: ScriptPhase::PostResponse,
                    duration: Duration::ZERO,
                    logs: Vec::new(),
                    tests: Vec::new(),
                    response_body_truncated: false,
                });
                app.response_tab = ResponseTab::Scripts;
                app.script_console_history =
                    vec!["api.response.status".to_owned(), "api".to_owned()];
                app.script_console_input
                    .update(cx, |editor, cx| editor.set_value("draft", window, cx));
                app.script_console_input
                    .read(cx)
                    .focus_handle(cx)
                    .focus(window);
                cx.notify();
            });
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("up up down down");
        assert_eq!(
            cx.update(|_, cx| app
                .read(cx)
                .script_console_input
                .read(cx)
                .value(cx)
                .to_string()),
            "draft"
        );
    }
}
