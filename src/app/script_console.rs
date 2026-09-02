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
                    let mut text = format!("[{}] {}", row.label, row.message);
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
            let (label, tone) = match log.level {
                ScriptLogLevel::Log => ("LOG", ScriptConsoleTone::Neutral),
                ScriptLogLevel::Info => ("INFO", ScriptConsoleTone::Info),
                ScriptLogLevel::Warn => ("WARN", ScriptConsoleTone::Warning),
                ScriptLogLevel::Error => ("ERROR", ScriptConsoleTone::Danger),
                ScriptLogLevel::Debug => ("DEBUG", ScriptConsoleTone::Debug),
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
                message: log.message.clone(),
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
        ScriptConsoleTone::Neutral | ScriptConsoleTone::Debug => cx.theme().muted_foreground,
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
        ScriptConsoleTone::Neutral | ScriptConsoleTone::Debug => cx.api_surface_lowest(),
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
    pub(super) fn copy_script_results(&mut self, cx: &mut Context<Self>) {
        let mut model = script_console_model(
            self.pre_script_report.as_ref(),
            self.post_script_report.as_ref(),
            self.script_diagnostic.as_ref(),
            self.request_error.as_deref(),
        );
        if self.script_console_cleared_key
            == Some(script_console_content_key(
                self.request_generation,
                &model,
            ))
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
                let duration = section
                    .duration
                    .map(format_script_duration)
                    .unwrap_or_default();
                let section_meta = if duration.is_empty() {
                    format!(
                        "{} {}",
                        section.rows.len(),
                        if section.rows.len() == 1 {
                            "entry"
                        } else {
                            "entries"
                        }
                    )
                } else {
                    format!(
                        "{duration} · {} {}",
                        section.rows.len(),
                        if section.rows.len() == 1 {
                            "entry"
                        } else {
                            "entries"
                        }
                    )
                };
                let rows = section.rows.iter().enumerate().map(|(row_index, row)| {
                    let expansion_key = format!(
                        "{generation}-{section_key}-{section_index}-{row_index}"
                    );
                    let expandable = !row.values.is_empty() && row.detail.is_some();
                    let expanded = self
                        .script_console_expanded_rows
                        .contains(&expansion_key);
                    let group_id: SharedString = format!(
                        "script-console-row-group-{generation}-{section_key}-{section_index}-{row_index}"
                    )
                    .into();
                    let row_id: SharedString = format!(
                        "script-console-row-{generation}-{section_key}-{section_index}-{row_index}"
                    )
                    .into();
                    let copy_id: SharedString = format!(
                        "copy-script-console-row-{generation}-{section_key}-{section_index}-{row_index}"
                    )
                    .into();
                    let copy_hint_id: SharedString = format!(
                        "copy-script-console-row-hint-{generation}-{section_key}-{section_index}-{row_index}"
                    )
                    .into();
                    let tone_color = script_console_tone_color(row.tone, cx);
                    let tone_background = script_console_tone_background(row.tone, cx);
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
                        .group(group_id.clone())
                        .w_full()
                        .min_h(px(30.))
                        .items_start()
                        .px_2()
                        .py_1()
                        .gap_2()
                        .border_b_1()
                        .border_color(cx.api_outline_variant())
                        .bg(tone_background)
                        .hover(|style| style.bg(cx.api_surface_low()))
                        .when(expandable, |this| {
                            this.cursor_pointer().on_click(cx.listener(
                                move |this, _: &ClickEvent, _, cx| {
                                    if !this
                                        .script_console_expanded_rows
                                        .insert(toggle_key.clone())
                                    {
                                        this.script_console_expanded_rows.remove(&toggle_key);
                                    }
                                    cx.notify();
                                },
                            ))
                        })
                        .child(
                            div()
                                .w(px(20.))
                                .h(px(20.))
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
                                        .with_size(px(14.))
                                        .text_color(tone_color),
                                ),
                        )
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
                                }),
                        )
                        .child(
                            div()
                                .h(px(20.))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(row.label.clone()),
                        )
                        .child(
                            div()
                                .id(copy_hint_id)
                                .w(px(28.))
                                .h(px(24.))
                                .flex_shrink_0()
                                .opacity(0.55)
                                .group_hover(group_id, |style| style.opacity(1.))
                                .tooltip(|window, cx| {
                                    Tooltip::new("Copy message").build(window, cx)
                                })
                                .child(
                                    Clipboard::new(copy_id).value(row.copy_value.clone()),
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

                v_flex()
                    .w_full()
                    .child(
                        h_flex()
                            .h(px(28.))
                            .flex_shrink_0()
                            .px_3()
                            .gap_2()
                            .border_b_1()
                            .border_color(cx.api_outline_variant())
                            .bg(cx.theme().muted.opacity(0.34))
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child(section.title.clone()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(section_meta),
                            ),
                    )
                    .children(rows)
                    .into_any_element()
            })
            .collect::<Vec<_>>();

        v_flex()
            .size_full()
            .rounded_md()
            .border_1()
            .border_color(cx.api_outline_variant())
            .overflow_hidden()
            .bg(cx.api_surface_lowest())
            .child(
                h_flex()
                    .h(px(40.))
                    .flex_shrink_0()
                    .px_3()
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
                div()
                    .id(("script-results-scroll", generation))
                    .flex_1()
                    .min_h_0()
                    .when(model.sections.is_empty(), |this| {
                        this.flex()
                            .items_center()
                            .justify_center()
                            .p_4()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("No script has run yet.")
                    })
                    .children(sections)
                    .overflow_y_scrollbar()
                    .context_menu(move |menu, _, _| {
                        script_console_context_menu(
                            menu,
                            context_owner.clone(),
                            None,
                            copy_all_text.clone(),
                            console_content_key,
                        )
                    }),
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
    .item(PopupMenuItem::new("Clear console").on_click(move |_, _, cx| {
        if let Some(owner) = clear_owner.upgrade() {
            owner.update(cx, |this, cx| {
                this.script_console_cleared_key = Some(console_content_key);
                this.script_console_expanded_rows.clear();
                this.copied = false;
                cx.notify();
            });
        }
    }))
}

fn script_console_content_key(generation: u64, model: &ScriptConsoleModel) -> u64 {
    use std::hash::{DefaultHasher, Hash as _, Hasher as _};

    let mut hasher = DefaultHasher::new();
    generation.hash(&mut hasher);
    model.copy_all_text().hash(&mut hasher);
    hasher.finish()
}
