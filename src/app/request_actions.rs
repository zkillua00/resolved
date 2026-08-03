use super::*;

impl ApiTester {
    pub(super) fn request_header_count(&self, cx: &App) -> usize {
        self.headers
            .iter()
            .filter(|row| row.enabled && !input_text_is_blank(&row.name, cx))
            .count()
    }

    pub(super) fn push_header_row(
        &mut self,
        name: impl Into<SharedString>,
        value: impl Into<SharedString>,
        enabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = name.into();
        let value = value.into();
        let id = self.next_header_id;
        self.next_header_id = self.next_header_id.wrapping_add(1);
        let name_catalog = Rc::clone(&self.template_variable_catalog);
        let name_state = cx.new(|cx| template_input_state(window, cx, name_catalog, "Key", name));
        let value_catalog = Rc::clone(&self.template_variable_catalog);
        let value_state =
            cx.new(|cx| template_input_state(window, cx, value_catalog, "Value", value));
        let name_template_input = name_state.clone();
        let name_subscription = cx.subscribe_in(
            &name_state,
            window,
            move |this, input, event, window, cx| {
                this.track_template_input_focus(input, event);
                if matches!(event, InputEvent::Change) {
                    this.schedule_template_input_refresh(&name_template_input, cx);
                    this.refresh_request_dirty_part(RequestDirtyPart::Headers, cx);
                }
                if matches!(event, InputEvent::PressEnter { .. })
                    && let Some(row) = this.headers.iter().find(|row| row.id == id)
                {
                    row.value.read(cx).focus_handle(cx).focus(window);
                }
            },
        );
        let value_template_input = value_state.clone();
        let value_subscription = cx.subscribe_in(
            &value_state,
            window,
            move |this, input, event, window, cx| {
                this.track_template_input_focus(input, event);
                if matches!(event, InputEvent::Change) {
                    this.schedule_template_input_refresh(&value_template_input, cx);
                    this.refresh_request_dirty_part(RequestDirtyPart::Headers, cx);
                }
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.focus_next_header_row(id, window, cx);
                }
            },
        );
        self.refresh_template_input(&name_state, cx);
        self.refresh_template_input(&value_state, cx);
        self.headers.push(HeaderRow {
            id,
            name: name_state,
            value: value_state,
            enabled,
            _subscriptions: vec![name_subscription, value_subscription],
        });
        self.refresh_request_dirty_part(RequestDirtyPart::Headers, cx);
        if !self.request_dirty.is_hydrating() {
            cx.notify();
        }
    }

    pub(super) fn focus_next_header_row(
        &mut self,
        row_id: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.headers.iter().position(|row| row.id == row_id) else {
            return;
        };
        if index + 1 == self.headers.len() {
            self.push_header_row("", "", true, window, cx);
        }
        if let Some(input) = self.headers.get(index + 1).map(|row| row.name.clone()) {
            input.read(cx).focus_handle(cx).focus(window);
        }
    }

    pub(super) fn duplicate_header_row(
        &mut self,
        row_id: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.headers.iter().position(|row| row.id == row_id) else {
            return;
        };
        let name = self.headers[index].name.read(cx).value();
        let value = self.headers[index].value.read(cx).value();
        let enabled = self.headers[index].enabled;
        self.push_header_row(name, value, enabled, window, cx);
        if let Some(duplicate) = self.headers.pop() {
            self.headers.insert(index + 1, duplicate);
        }
        cx.notify();
    }

    pub(super) fn copy_header_row(&mut self, row_id: usize, cx: &mut Context<Self>) {
        let Some(row) = self.headers.iter().find(|row| row.id == row_id) else {
            return;
        };
        let name = row.name.read(cx).value();
        let value = row.value.read(cx).value();
        let header = if name.trim().is_empty() {
            value.to_string()
        } else {
            format!("{name}: {value}")
        };
        cx.write_to_clipboard(ClipboardItem::new_string(header));
        self.request_notice = Some("Copied header.".to_owned());
        cx.notify();
    }

    pub(super) fn remove_header_row(
        &mut self,
        row_id: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.headers.retain(|row| row.id != row_id);
        if self.headers.is_empty() {
            self.push_header_row("", "", true, window, cx);
        }
        self.refresh_request_dirty_part(RequestDirtyPart::Headers, cx);
        cx.notify();
    }

    pub(super) fn push_body_field_row(
        &mut self,
        name: impl Into<SharedString>,
        value: impl Into<SharedString>,
        enabled: bool,
        kind: BodyFieldKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = name.into();
        let value = value.into();
        let id = self.next_body_field_id;
        self.next_body_field_id = self.next_body_field_id.wrapping_add(1);
        let name_catalog = Rc::clone(&self.template_variable_catalog);
        let name_state = cx.new(|cx| template_input_state(window, cx, name_catalog, "Key", name));
        let value_catalog = Rc::clone(&self.template_variable_catalog);
        let value_placeholder = if kind == BodyFieldKind::File {
            "Choose or enter a file path"
        } else {
            "Value"
        };
        let value_state =
            cx.new(|cx| template_input_state(window, cx, value_catalog, value_placeholder, value));
        let name_template_input = name_state.clone();
        let name_subscription = cx.subscribe_in(
            &name_state,
            window,
            move |this, input, event, window, cx| {
                this.track_template_input_focus(input, event);
                if matches!(event, InputEvent::Change) {
                    this.schedule_template_input_refresh(&name_template_input, cx);
                    this.refresh_request_dirty_part(RequestDirtyPart::BodyFields, cx);
                }
                if matches!(event, InputEvent::PressEnter { .. })
                    && let Some(row) = this.body_fields.iter().find(|row| row.id == id)
                {
                    row.value.read(cx).focus_handle(cx).focus(window);
                }
            },
        );
        let value_template_input = value_state.clone();
        let value_subscription = cx.subscribe_in(
            &value_state,
            window,
            move |this, input, event, window, cx| {
                this.track_template_input_focus(input, event);
                if matches!(event, InputEvent::Change) {
                    this.schedule_template_input_refresh(&value_template_input, cx);
                    this.refresh_request_dirty_part(RequestDirtyPart::BodyFields, cx);
                }
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.focus_next_body_field_row(id, window, cx);
                }
            },
        );
        self.refresh_template_input(&name_state, cx);
        self.refresh_template_input(&value_state, cx);
        self.body_fields.push(request_body_editor::BodyFieldRow {
            id,
            name: name_state,
            value: value_state,
            enabled,
            kind,
            _subscriptions: vec![name_subscription, value_subscription],
        });
        self.refresh_request_dirty_part(RequestDirtyPart::BodyFields, cx);
        if !self.request_dirty.is_hydrating() {
            cx.notify();
        }
    }

    pub(super) fn focus_next_body_field_row(
        &mut self,
        row_id: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.body_fields.iter().position(|row| row.id == row_id) else {
            return;
        };
        if index + 1 == self.body_fields.len() {
            self.push_body_field_row("", "", true, BodyFieldKind::Text, window, cx);
        }
        if let Some(input) = self.body_fields.get(index + 1).map(|row| row.name.clone()) {
            input.read(cx).focus_handle(cx).focus(window);
        }
    }

    pub(super) fn duplicate_body_field_row(
        &mut self,
        row_id: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.body_fields.iter().position(|row| row.id == row_id) else {
            return;
        };
        let name = self.body_fields[index].name.read(cx).value();
        let value = self.body_fields[index].value.read(cx).value();
        let enabled = self.body_fields[index].enabled;
        let kind = self.body_fields[index].kind;
        self.push_body_field_row(name, value, enabled, kind, window, cx);
        if let Some(duplicate) = self.body_fields.pop() {
            self.body_fields.insert(index + 1, duplicate);
        }
        cx.notify();
    }

    pub(super) fn remove_body_field_row(
        &mut self,
        row_id: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.body_fields.retain(|row| row.id != row_id);
        if self.body_fields.is_empty() {
            self.push_body_field_row("", "", true, BodyFieldKind::Text, window, cx);
        }
        self.refresh_request_dirty_part(RequestDirtyPart::BodyFields, cx);
        cx.notify();
    }

    pub(super) fn set_body_field_kind(
        &mut self,
        row_id: usize,
        kind: BodyFieldKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(row) = self.body_fields.iter_mut().find(|row| row.id == row_id) else {
            return;
        };
        row.kind = kind;
        row.value.update(cx, |state, cx| {
            state.set_placeholder(
                if kind == BodyFieldKind::File {
                    "Choose or enter a file path"
                } else {
                    "Value"
                },
                window,
                cx,
            );
        });
        self.refresh_request_dirty_part(RequestDirtyPart::BodyFields, cx);
        cx.notify();
    }

    fn normalized_header_entries(&self, cx: &App) -> Vec<HeaderEntry> {
        self.headers
            .iter()
            .filter_map(|row| {
                let name = row.name.read(cx).value().to_string();
                let value = row.value.read(cx).value().to_string();
                if name.trim().is_empty() && value.trim().is_empty() {
                    return None;
                }
                let mut header = HeaderEntry::new(name, value);
                header.enabled = row.enabled;
                Some(header)
            })
            .collect()
    }

    fn normalized_body_fields(&self, cx: &App) -> Vec<BodyField> {
        self.body_fields
            .iter()
            .filter_map(|row| {
                let name = row.name.read(cx).value().to_string();
                let value = row.value.read(cx).value().to_string();
                if name.trim().is_empty() && value.trim().is_empty() {
                    return None;
                }
                Some(BodyField {
                    enabled: row.enabled,
                    name,
                    value,
                    kind: row.kind,
                })
            })
            .collect()
    }

    pub(super) fn draft(&self, cx: &App) -> RequestDraft {
        let method = self.method.read(cx).value().trim().to_ascii_uppercase();

        let mut draft = RequestDraft::new(method, self.url.read(cx).value().to_string());
        draft.headers = self.normalized_header_entries(cx);
        draft.body = self.body.read(cx).value(cx).to_string();
        draft.body_mode = self.body_mode;
        draft.raw_body_language = self.raw_body_language;
        draft.body_fields = self.normalized_body_fields(cx);
        draft
    }

    pub(super) fn request_template(&self, cx: &App) -> RequestTemplate {
        RequestTemplate {
            request: self.draft(cx),
            scripts: RequestScripts {
                pre_request: self.pre_request_script.read(cx).value(cx).to_string(),
                post_response: self.post_response_script.read(cx).value(cx).to_string(),
            },
        }
    }

    pub(super) fn request_is_dirty(&self) -> bool {
        self.detached_request_dirty
            || self.request_dirty.any()
            || self.request_tabs.active().is_dirty()
    }

    pub(super) fn refresh_request_dirty_part(
        &mut self,
        part: RequestDirtyPart,
        cx: &mut Context<Self>,
    ) {
        if self.request_dirty.is_hydrating() {
            return;
        }

        let part_is_dirty = self.request_part_is_dirty(part, cx);
        self.request_dirty.set(part, part_is_dirty);
        self.schedule_request_tabs_persist(cx);
    }

    pub(super) fn refresh_all_request_dirty_parts(&mut self, cx: &mut Context<Self>) {
        for part in [
            RequestDirtyPart::Method,
            RequestDirtyPart::Url,
            RequestDirtyPart::Headers,
            RequestDirtyPart::RawBody,
            RequestDirtyPart::BodyMode,
            RequestDirtyPart::RawBodyLanguage,
            RequestDirtyPart::BodyFields,
            RequestDirtyPart::PreScript,
            RequestDirtyPart::PostScript,
        ] {
            let dirty = self.request_part_is_dirty(part, cx);
            self.request_dirty.set(part, dirty);
        }
    }

    pub(super) fn request_part_is_dirty(&self, part: RequestDirtyPart, cx: &App) -> bool {
        let baseline = &self.loaded_request_baseline;
        match part {
            RequestDirtyPart::Method => {
                let current = self.method.read(cx).value();
                !current
                    .trim()
                    .eq_ignore_ascii_case(baseline.request.method.as_str())
            }
            RequestDirtyPart::Url => {
                !input_text_equals(&self.url, baseline.request.url.as_str(), cx)
            }
            RequestDirtyPart::Headers => {
                self.normalized_header_entries(cx) != baseline.request.headers
            }
            RequestDirtyPart::RawBody => {
                let input = self.body.read(cx).input_state();
                !input_text_equals(&input, baseline.request.body.as_str(), cx)
            }
            RequestDirtyPart::BodyMode => self.body_mode != baseline.request.body_mode,
            RequestDirtyPart::RawBodyLanguage => {
                self.raw_body_language != baseline.request.raw_body_language
            }
            RequestDirtyPart::BodyFields => {
                self.normalized_body_fields(cx) != baseline.request.body_fields
            }
            RequestDirtyPart::PreScript => {
                let input = self.pre_request_script.read(cx).input_state();
                !input_text_equals(&input, baseline.scripts.pre_request.as_str(), cx)
            }
            RequestDirtyPart::PostScript => {
                let input = self.post_response_script.read(cx).input_state();
                !input_text_equals(&input, baseline.scripts.post_response.as_str(), cx)
            }
        }
    }

    pub(super) fn update_body_language(&mut self, cx: &mut Context<Self>) {
        let language = code_language_for_raw_body(self.raw_body_language);
        self.body
            .update(cx, |editor, cx| editor.set_language(language, cx));
    }

    pub(super) fn format_raw_body(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.body_mode != BodyMode::Raw {
            self.request_notice =
                Some("Formatting is available when the request body uses Raw mode.".to_owned());
            cx.notify();
            return;
        }
        let source = self.body.read(cx).value(cx).to_string();
        if source.trim().is_empty() {
            self.request_notice = Some("The raw body buffer is empty.".to_owned());
            cx.notify();
            return;
        }

        let formatted =
            format_raw_body_source(self.raw_body_language, &source, &self.settings.formatter);
        let formatted = match formatted {
            Ok(formatted) => formatted,
            Err(message) => {
                self.request_notice = Some(message);
                cx.notify();
                return;
            }
        };
        if formatted == source {
            self.request_notice = Some("The raw body is already formatted.".to_owned());
            cx.notify();
            return;
        }

        replace_editor_source(&self.body, &source, &formatted, window, cx);
        self.request_notice = Some(format!("Formatted raw {} body.", self.raw_body_language));
        cx.notify();
    }

    pub(super) fn format_script_editor(
        &mut self,
        editor: Entity<CodeEditor>,
        label: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let source = editor.read(cx).value(cx).to_string();
        if source.trim().is_empty() {
            self.request_notice = Some(format!("The {label} script buffer is empty."));
            cx.notify();
            return;
        }

        let formatted = match crate::core::format_script_source(&source, &self.settings.formatter) {
            Ok(formatted) => formatted,
            Err(message) => {
                self.request_notice = Some(message);
                cx.notify();
                return;
            }
        };
        if formatted == source {
            self.request_notice = Some(format!("The {label} script is already formatted."));
            cx.notify();
            return;
        }

        replace_editor_source(&editor, &source, &formatted, window, cx);
        self.request_notice = Some(format!("Formatted {label} JavaScript."));
        cx.notify();
    }

    pub(super) fn select_body_mode(
        &mut self,
        mode: BodyMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.body_mode = mode;
        if matches!(mode, BodyMode::FormUrlEncoded | BodyMode::MultipartFormData)
            && self.body_fields.is_empty()
        {
            self.push_body_field_row("", "", true, BodyFieldKind::Text, window, cx);
        }
        if mode == BodyMode::Raw {
            self.update_body_language(cx);
        }
        self.refresh_request_dirty_part(RequestDirtyPart::BodyMode, cx);
        cx.notify();
    }

    pub(super) fn select_raw_body_language(
        &mut self,
        language: RawBodyLanguage,
        cx: &mut Context<Self>,
    ) {
        self.raw_body_language = language;
        self.update_body_language(cx);
        self.refresh_request_dirty_part(RequestDirtyPart::RawBodyLanguage, cx);
        cx.notify();
    }

    pub(super) fn choose_body_file(
        &mut self,
        row_id: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose file".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = receiver.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                if let Some(row) = this.body_fields.iter().find(|row| row.id == row_id) {
                    row.value.update(cx, |state, cx| {
                        state.set_value(path.display().to_string(), window, cx);
                    });
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn load_template_unchecked(
        &mut self,
        template: RequestTemplate,
        collection_id: Option<String>,
        saved_request_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.request_dirty.begin_hydration();
        let RequestTemplate { request, scripts } = template;
        self.body_mode = request.body_mode;
        self.raw_body_language = request.raw_body_language;
        self.method.update(cx, |state, cx| {
            state.set_value(request.method, window, cx);
        });
        self.url.update(cx, |state, cx| {
            state.set_value(request.url, window, cx);
        });
        self.body.update(cx, |state, cx| {
            state.set_value(request.body, window, cx);
        });
        self.pre_request_script.update(cx, |editor, cx| {
            editor.set_value(scripts.pre_request, window, cx);
        });
        self.post_response_script.update(cx, |editor, cx| {
            editor.set_value(scripts.post_response, window, cx);
        });

        self.headers.clear();
        for header in request.headers {
            let was_redacted = header.value == REDACTED_VALUE;
            self.push_header_row(
                header.name,
                if was_redacted {
                    String::new()
                } else {
                    header.value
                },
                header.enabled && !was_redacted,
                window,
                cx,
            );
        }
        if self.headers.is_empty() {
            self.push_header_row("", "", true, window, cx);
        }

        self.body_fields.clear();
        for field in request.body_fields {
            self.push_body_field_row(
                field.name,
                field.value,
                field.enabled,
                field.kind,
                window,
                cx,
            );
        }
        if matches!(
            self.body_mode,
            BodyMode::FormUrlEncoded | BodyMode::MultipartFormData
        ) && self.body_fields.is_empty()
        {
            self.push_body_field_row("", "", true, BodyFieldKind::Text, window, cx);
        }
        self.update_body_language(cx);

        self.response = None;
        self.response_request = None;
        self.response_sensitive_values.clear();
        self.request_error = None;
        self.script_diagnostic = None;
        self.pre_script_report = None;
        self.post_script_report = None;
        self.preview_error = None;
        self.copied = false;
        if let Some(collection_id) = collection_id
            && let Some(collection) = self.workspace.collection(&collection_id)
        {
            self.selected_collection_id = Some(collection_id.clone());
            self.expanded_collection_ids.insert(collection_id);
            self.collection_name.update(cx, |input, cx| {
                input.set_value(collection.name.clone(), window, cx)
            });
            self.sidebar_tab = SidebarTab::Collections;
        }
        self.active_saved_request_id = saved_request_id.clone();
        self.detached_request_dirty = false;
        let request_name = saved_request_id
            .as_deref()
            .and_then(|id| self.workspace.saved_request(id))
            .map(|(_, request)| request.name.clone())
            .unwrap_or_default();
        self.saved_request_name
            .update(cx, |input, cx| input.set_value(request_name, window, cx));
        self.refresh_variable_intelligence(cx);
        self.loaded_request_baseline = self.request_template(cx);
        self.request_dirty.end_hydration();
        self.request_notice = None;
        self.hide_preview(cx);
        cx.notify();
    }
}

fn replace_editor_source(
    editor: &Entity<CodeEditor>,
    source: &str,
    formatted: &str,
    window: &mut Window,
    cx: &mut Context<ApiTester>,
) {
    let input = editor.read(cx).input_state();
    input.update(cx, |input, cx| {
        let original_cursor = input.cursor();
        let full_range = 0..source.encode_utf16().count();
        EntityInputHandler::replace_text_in_range(input, Some(full_range), formatted, window, cx);

        let mut restored_offset = original_cursor.min(formatted.len());
        while !formatted.is_char_boundary(restored_offset) {
            restored_offset = restored_offset.saturating_sub(1);
        }
        let cursor = input.text().offset_to_position(restored_offset);
        input.set_cursor_position(cursor, window, cx);
    });
}
