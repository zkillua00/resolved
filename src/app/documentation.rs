use super::*;
use crate::documentation_intelligence::{DocumentationIntelligence, TargetKind, Targets};
use crate::documentation_references::{ReferenceActions, ReferenceCatalog, Resource};

pub(super) fn new_editor(
    window: &mut Window,
    cx: &mut Context<ApiTester>,
) -> (Entity<CodeEditor>, Rc<DocumentationIntelligence>) {
    let intelligence = DocumentationIntelligence::shared();
    let diagnostics = intelligence.clone();
    let editor = cx.new(|cx| {
        CodeEditor::new(
            CodeEditorConfig::default()
                .framed(false)
                .embedded(true)
                .language(CodeLanguage::Markdown)
                .placeholder("Markdown notes\n\n@param query.limit Maximum records per page.\n@header Authorization Token obtained from Login.\n@body /user/name Display name (JSON).\n@body(XML) /request/user/@id User identifier.\n\nType @ at the start of a line to reference a field.")
                .rows(12)
                .soft_wrap(true)
                .completion_provider(intelligence.clone())
                .diagnostic_provider(move |source| {
                    diagnostics.diagnostics(source).into_iter().map(Into::into).collect()
                }),
            window,
            cx,
        )
    });
    let links = intelligence.clone();
    // Detached subscriptions still end when this editor is dropped, including
    // secondary-pane sessions. The closure does not retain the editor entity.
    cx.subscribe_in(
        &editor,
        window,
        move |this, editor, event: &CodeEditorEvent, window, cx| {
            let CodeEditorEvent::InlineActionRequested { id } = event else {
                return;
            };
            let source = editor.read(cx).value(cx);
            let target = {
                let actions = links.reference_actions.borrow();
                if actions.source != source.as_ref() {
                    return;
                }
                actions.targets.get(id).cloned()
            };
            if let Some(targets) = target {
                if let [(expression, target)] = targets.as_slice() {
                    this.open_documentation_reference(expression, target, window, cx);
                } else {
                    let owner = cx.entity().downgrade();
                    window.open_dialog(cx, move |dialog, _, _| {
                        dialog.title("Open reference").child(
                            v_flex().gap_2().children(targets.iter().enumerate().map(
                                |(index, (expression, target))| {
                                    let owner = owner.clone();
                                    let expression = expression.clone();
                                    let target = target.clone();
                                    Button::new(("documentation-reference-choice", index))
                                        .label(expression.clone())
                                        .on_click(move |_, window, cx| {
                                            window.close_dialog(cx);
                                            if let Some(owner) = owner.upgrade() {
                                                owner.update(cx, |this, cx| {
                                                    this.open_documentation_reference(
                                                        &expression,
                                                        &target,
                                                        window,
                                                        cx,
                                                    );
                                                });
                                            }
                                        })
                                },
                            )),
                        )
                    });
                }
            }
        },
    )
    .detach();
    cx.subscribe(&editor, |_, _, event: &InputEvent, cx| {
        if matches!(event, InputEvent::Change) {
            cx.notify();
        }
    })
    .detach();
    (editor, intelligence)
}

impl ApiTester {
    fn open_documentation_reference(
        &mut self,
        expression: &str,
        expected: &Resource,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Revalidate on activation: a remote update or workspace/environment
        // switch may have happened since this link was rendered.
        if ReferenceCatalog::from_workspace(&self.workspace)
            .0
            .get(expression)
            != Some(expected)
        {
            self.workspace_warning =
                Some("This documentation reference is no longer available.".into());
            cx.notify();
            return;
        }
        match expected {
            Resource::Request { request_id } => {
                if let Some((collection, _)) = self.workspace.saved_request(request_id) {
                    self.open_saved_request_tab(
                        collection.id.clone(),
                        request_id.clone(),
                        window,
                        cx,
                    );
                }
            }
            Resource::EnvironmentVariable {
                environment_id,
                variable_id,
            } => {
                self.activate_request_workspace(SidebarTab::Environments, window, cx);
                self.select_environment(environment_id.clone(), window, cx);
                if self.selected_environment_id.as_ref() == Some(environment_id) {
                    if let Some((index, row)) = self
                        .environment_variables
                        .iter()
                        .enumerate()
                        .find(|(_, row)| &row.id == variable_id)
                    {
                        self.environment_variable_scroll.scroll_to_item(index);
                        row.key.read(cx).focus_handle(cx).focus(window);
                    }
                }
            }
        }
    }
}

/// Refresh names at the request-surface boundary, covering URL edits, row edits,
/// tab hydration and remote updates without duplicating mutation hooks.
pub(super) fn refresh_targets(
    intelligence: &DocumentationIntelligence,
    editor: &Entity<CodeEditor>,
    query: &[QueryParamRow],
    headers: &[HeaderRow],
    body: Option<(BodyMode, RawBodyLanguage, &Entity<CodeEditor>)>,
    workspace: &Workspace,
    cx: &mut App,
) {
    let targets = Targets::new(
        query.iter().map(|row| row.key.read(cx).value().to_string()),
        headers
            .iter()
            .map(|row| row.name.read(cx).value().trim().to_owned()),
    );
    let resources = ReferenceCatalog::from_workspace(workspace);
    let resources_changed = *intelligence.resources.borrow() != resources;
    *intelligence.resources.borrow_mut() = resources;
    let body_changed = match body {
        Some((mode, language, body)) => intelligence.replace_body(
            documentation_body_mode(mode, language),
            body.read(cx).value(cx).as_ref(),
        ),
        None => intelligence.replace_body(None, ""),
    };
    if intelligence.replace_targets(targets) || resources_changed || body_changed {
        editor.update(cx, |editor, cx| editor.refresh_diagnostics(cx));
    }
    let source = editor.read(cx).value(cx).to_string();
    let mut snapshot = ReferenceActions {
        source: source.clone(),
        ..Default::default()
    };
    for reference in intelligence
        .references(&source)
        .into_iter()
        .filter(|reference| reference.complete)
    {
        let expression = reference.expression(&source);
        if let Some(resource) = intelligence.resources.borrow().0.get(expression) {
            let row =
                crate::editor_util::source_position(&source, reference.range.start).line as usize;
            snapshot
                .targets
                .entry(row)
                .or_default()
                .push((expression.to_owned(), resource.clone()));
        }
    }
    let actions = snapshot
        .targets
        .iter()
        .map(|(&row, targets)| InputInlineAction {
            id: row,
            row,
            label: if let [(expression, _)] = targets.as_slice() {
                format!("Open {expression}").into()
            } else {
                format!("Open {} references…", targets.len()).into()
            },
            placement: InputInlineActionPlacement::After,
        })
        .collect();
    *intelligence.reference_actions.borrow_mut() = snapshot;
    editor.update(cx, |editor, cx| editor.set_inline_actions(actions, cx));
    let syntax = &cx.theme().highlight_theme;
    let highlights = intelligence.semantic_highlights(
        &source,
        syntax.style("keyword").unwrap_or_default(),
        syntax.style("variable").unwrap_or_default(),
        reference_link_style(cx),
    );
    editor.read(cx).input_state().update(cx, |input, cx| {
        input.set_semantic_highlights(highlights, cx);
    });
}

fn documentation_body_mode(
    mode: BodyMode,
    language: RawBodyLanguage,
) -> Option<crate::documentation_body::BodyMode> {
    if mode != BodyMode::Raw {
        return None;
    }
    match language {
        RawBodyLanguage::Json => Some(crate::documentation_body::BodyMode::Json),
        RawBodyLanguage::Xml => Some(crate::documentation_body::BodyMode::Xml),
        _ => None,
    }
}

/// The body owns this provider; keep the documentation editor weak so closing a
/// pane releases its session. Always read current Markdown at hover time.
pub(super) fn body_hover_provider(
    intelligence: Rc<DocumentationIntelligence>,
    documentation: &Entity<CodeEditor>,
    owner: WeakEntity<ApiTester>,
) -> Rc<dyn gpui_component::input::HoverProvider> {
    Rc::new(BodyHoverProvider {
        intelligence,
        documentation: documentation.downgrade(),
        owner,
    })
}

struct BodyHoverProvider {
    intelligence: Rc<DocumentationIntelligence>,
    documentation: WeakEntity<CodeEditor>,
    owner: WeakEntity<ApiTester>,
}

impl gpui_component::input::HoverProvider for BodyHoverProvider {
    fn hover(
        &self,
        text: &gpui_component::input::Rope,
        offset: usize,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<Option<lsp_types::Hover>>> {
        let source = text.to_string();
        // Body edits can arrive before the request-surface refresh. Never
        // interpret a new offset using the previous body's field ranges.
        if !self.intelligence.body_source_matches(&source) {
            return Task::ready(Ok(None));
        }
        let Some(documentation) = self.documentation.upgrade() else {
            return Task::ready(Ok(None));
        };
        let explanations = self
            .intelligence
            .body_explanations(documentation.read(cx).value(cx).as_ref(), offset);
        Task::ready(Ok((!explanations.is_empty()).then(|| lsp_types::Hover {
            contents: lsp_types::HoverContents::Markup(lsp_types::MarkupContent {
                kind: lsp_types::MarkupKind::Markdown,
                value: explanations.join("\n\n---\n\n"),
            }),
            range: None,
        })))
    }

    fn render_hover(
        &self,
        hover: &lsp_types::Hover,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<gpui::AnyElement> {
        let lsp_types::HoverContents::Markup(markup) = &hover.contents else {
            return None;
        };
        Some(explanation_content(&markup.value, self.owner.clone(), cx))
    }
}

pub(super) fn explanation(
    intelligence: &DocumentationIntelligence,
    editor: &Entity<CodeEditor>,
    kind: TargetKind,
    name: &str,
    cx: &App,
) -> Option<String> {
    intelligence.explanation(editor.read(cx).value(cx).as_ref(), kind, name)
}

fn reference_link_style(cx: &App) -> gpui::HighlightStyle {
    gpui::HighlightStyle {
        color: Some(cx.api_primary_lavender()),
        underline: Some(gpui::UnderlineStyle {
            thickness: px(1.),
            color: Some(cx.api_primary_lavender()),
            wavy: false,
        }),
        ..Default::default()
    }
}

/// One renderer for body, query, and header explanations. Resolved references
/// capture resource IDs; activation revalidates them before navigation.
fn explanation_content(
    description: &str,
    owner: WeakEntity<ApiTester>,
    cx: &App,
) -> gpui::AnyElement {
    let catalog = owner
        .upgrade()
        .map(|owner| ReferenceCatalog::from_workspace(&owner.read(cx).workspace))
        .unwrap_or_default();
    let text = Rc::new(crate::documentation_references::ReferenceText::new(
        description,
        &catalog,
    ));
    let ranges = text
        .links
        .iter()
        .map(|(range, _, _)| range.clone())
        .collect::<Vec<_>>();
    let styled = gpui::StyledText::new(text.text.clone()).with_highlights(
        ranges
            .iter()
            .cloned()
            .map(|range| (range, reference_link_style(cx))),
    );
    div()
        .debug_selector(|| "documentation-explanation-hover".to_owned())
        .max_w(px(480.))
        .child(
            gpui::InteractiveText::new("documentation-explanation-text", styled).on_click(
                ranges,
                move |index, window, cx| {
                    if let Some(owner) = owner.upgrade() {
                        let (_, expression, resource) = &text.links[index];
                        owner.update(cx, |this, cx| {
                            this.open_documentation_reference(expression, resource, window, cx);
                        });
                    }
                },
            ),
        )
        .into_any_element()
}

/// Use with `hoverable_tooltip`, so links remain reachable after leaving the field.
pub(super) fn explanation_tooltip(
    description: String,
    owner: WeakEntity<ApiTester>,
) -> impl Fn(&mut Window, &mut App) -> gpui::AnyView {
    move |window, cx| {
        let description = description.clone();
        let owner = owner.clone();
        Tooltip::element(move |_, cx| explanation_content(&description, owner.clone(), cx))
            .build(window, cx)
    }
}

/// A read-only projection: no second input buffer or persisted description.
pub(super) fn description_cell(
    id: SharedString,
    description: Option<String>,
    cx: &Context<ApiTester>,
) -> impl IntoElement {
    let tooltip = description
        .clone()
        .unwrap_or_else(|| "Add an @param query.<name> explanation in Documentation.".to_owned());
    query_params_editor::query_param_cell(cx)
        .id(id)
        .debug_selector(|| "query-param-description-cell".to_owned())
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .hoverable_tooltip(explanation_tooltip(tooltip, cx.entity().downgrade()))
        .child(
            div().w_full().min_w_0().px_3().child(
                div()
                    .truncate()
                    .child(description.unwrap_or_else(|| "—".to_owned())),
            ),
        )
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use gpui::{TestAppContext, px, size};

    pub(in crate::app) fn body_hover(
        body: &Entity<CodeEditor>,
        offset: usize,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<String> {
        use futures::FutureExt as _;
        let input = body.read(cx).input_state();
        let provider = input
            .read(cx)
            .lsp
            .hover_provider
            .clone()
            .expect("body hover provider");
        let source = gpui_component::input::Rope::from(body.read(cx).value(cx).as_ref());
        let hover = provider
            .hover(&source, offset, window, cx)
            .now_or_never()
            .expect("local documentation hover is synchronous")
            .expect("hover succeeds")?;
        let lsp_types::HoverContents::Markup(markup) = hover.contents else {
            panic!("documentation hover uses Markdown");
        };
        Some(markup.value)
    }

    #[gpui::test]
    fn body_documentation_hover_links_navigate_without_executing(cx: &mut TestAppContext) {
        assert_body_hover_link(
            cx,
            RawBodyLanguage::Json,
            "{ \"session_id\": \"example\",\n  \"underneath\": \"another documented field\"\n}",
            "@body /session_id @Ref([\"AI Chat Admin\"].Auth.Login)\n\
             @body /underneath Underlying field must not replace the open hover.",
        );
    }

    #[gpui::test]
    fn xml_body_documentation_hover_links_navigate_without_executing(cx: &mut TestAppContext) {
        assert_body_hover_link(
            cx,
            RawBodyLanguage::Xml,
            "<session_id/>",
            "@body(XML) /session_id @Ref([\"AI Chat Admin\"].Auth.Login)",
        );
    }

    fn assert_body_hover_link(
        cx: &mut TestAppContext,
        language: RawBodyLanguage,
        source: &'static str,
        annotation: &'static str,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let store = DatabaseStore::new(directory.path().join("api-tester.sqlite3"));
        store.initialize().unwrap();
        let mut app = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            gpui_component::init(cx);
            crate::theme::configure(cx);
            let bindings = shortcuts::capture_base_key_bindings(cx);
            let view = cx.new(|cx| ApiTester::new_with_database_store(bindings, store, window, cx));
            let body = view.update(cx, |app, cx| {
                app.workspace = crate::documentation_references::tests::workspace_fixture();
                // Exercise bracket-quoted resource names, as in session annotations.
                app.workspace.collections[0].name = "AI Chat Admin".to_owned();
                app.raw_body_language = language;
                app.body
                    .update(cx, |editor, cx| editor.set_value(source, window, cx));
                app.documentation
                    .update(cx, |editor, cx| editor.set_value(annotation, window, cx));
                app.render_request_panel(cx);
                app.body.clone()
            });
            app = Some(view);
            // Display the actual installed body editor, not a stand-in tooltip.
            Root::new(body, window, cx)
        });
        let app = app.unwrap();
        cx.update(|window, _| window.activate_window());
        cx.simulate_resize(size(px(800.), px(300.)));
        cx.run_until_parked();
        // The first line's session_id key follows the line-number gutter.
        cx.simulate_mouse_move(gpui::point(px(80.), px(10.)), None, gpui::Modifiers::none());
        cx.run_until_parked();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(600));
        cx.run_until_parked();
        let hover = cx
            .debug_bounds("documentation-explanation-hover")
            .expect("body hover uses the shared reference renderer");
        let link_point = hover.origin + gpui::point(px(10.), hover.size.height / 2.);
        cx.simulate_mouse_move(link_point, None, gpui::Modifiers::none());
        cx.run_until_parked();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(600));
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("documentation-explanation-hover").is_some(),
            "body hover stays open while moving onto its link"
        );
        cx.simulate_click(link_point, gpui::Modifiers::none());
        cx.run_until_parked();
        cx.read(|cx| {
            assert_eq!(
                app.read(cx).request_template(cx).request.url,
                "https://example.test/login?secret=never-expose-url"
            );
            assert!(!app.read(cx).sending);
        });
    }

    #[test]
    fn body_documentation_is_only_enabled_for_raw_json_and_xml() {
        for &mode in BodyMode::all() {
            for &language in RawBodyLanguage::all() {
                assert_eq!(
                    documentation_body_mode(mode, language).is_some(),
                    mode == BodyMode::Raw
                        && matches!(language, RawBodyLanguage::Json | RawBodyLanguage::Xml),
                );
            }
        }
    }

    #[gpui::test]
    fn body_documentation_refreshes_from_live_editors_and_rejects_stale_offsets(
        cx: &mut TestAppContext,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let store = DatabaseStore::new(directory.path().join("api-tester.sqlite3"));
        store.initialize().unwrap();
        let mut app = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            gpui_component::init(cx);
            crate::theme::configure(cx);
            let bindings = shortcuts::capture_base_key_bindings(cx);
            let view = cx.new(|cx| ApiTester::new_with_database_store(bindings, store, window, cx));
            app = Some(view.clone());
            Root::new(view, window, cx)
        });
        let app = app.unwrap();
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                let source = r#"{"name":"{{display_name}}"}"#;
                app.body
                    .update(cx, |editor, cx| editor.set_value(source, window, cx));
                app.documentation.update(cx, |editor, cx| {
                    editor.set_value("@body /name Display name.", window, cx)
                });
                app.render_request_panel(cx);
                let offset = source.find("{{").unwrap();
                assert!(app.documentation_intelligence.body_source_matches(source));
                assert_eq!(
                    body_hover(&app.body, offset, window, cx).as_deref(),
                    Some("Display name.")
                );

                // Documentation changes do not need another request render.
                app.documentation.update(cx, |editor, cx| {
                    editor.set_value("@body(JSON) /name Updated name.", window, cx)
                });
                assert_eq!(
                    body_hover(&app.body, offset, window, cx).as_deref(),
                    Some("Updated name.")
                );

                app.body.update(cx, |editor, cx| {
                    editor.set_value(r#"{"other":"new"}"#, window, cx)
                });
                assert!(body_hover(&app.body, offset, window, cx).is_none());
                app.render_request_panel(cx);
                assert!(body_hover(&app.body, 11, window, cx).is_none());
                let input = app.documentation.read(cx).input_state();
                assert!(!input.read(cx).diagnostics().unwrap().is_empty());

                // Losing the ability to resolve the body clears the previous
                // absent-field notice without changing the annotation.
                app.body
                    .update(cx, |editor, cx| editor.set_value("{", window, cx));
                app.render_request_panel(cx);
                assert!(input.read(cx).diagnostics().unwrap().is_empty());
                assert_eq!(
                    app.documentation.read(cx).value(cx).as_ref(),
                    "@body(JSON) /name Updated name."
                );

                app.raw_body_language = RawBodyLanguage::Xml;
                app.body.update(cx, |editor, cx| {
                    editor.set_value("<request><user id=\"7\"/></request>", window, cx)
                });
                app.documentation.update(cx, |editor, cx| {
                    editor.set_value("@body(XML) /request/user/@id User identifier.", window, cx)
                });
                app.render_request_panel(cx);
                assert_eq!(
                    body_hover(&app.body, 16, window, cx).as_deref(),
                    Some("User identifier.")
                );

                app.body_mode = BodyMode::None;
                app.render_request_panel(cx);
                assert!(body_hover(&app.body, 16, window, cx).is_none());
                assert!(input.read(cx).diagnostics().unwrap().is_empty());
                app.body_mode = BodyMode::Raw;
                app.render_request_panel(cx);
                assert!(body_hover(&app.body, 16, window, cx).is_some());
                app.render_websocket_workspace(cx);
                assert!(body_hover(&app.body, 16, window, cx).is_none());
            });
        });
    }

    #[gpui::test]
    fn query_documentation_hover_links_survive_overlapping_rows(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let store = DatabaseStore::new(directory.path().join("api-tester.sqlite3"));
        store.initialize().unwrap();
        let mut app = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            gpui_component::init(cx);
            crate::theme::configure(cx);
            let bindings = shortcuts::capture_base_key_bindings(cx);
            let view = cx.new(|cx| ApiTester::new_with_database_store(bindings, store, window, cx));
            app = Some(view.clone());
            Root::new(view, window, cx)
        });
        let app = app.unwrap();
        cx.update(|window, _| window.activate_window());
        cx.simulate_resize(size(px(1200.), px(800.)));

        for selector in ["query-param-key-cell", "query-param-description-cell"] {
            cx.update(|window, cx| {
                app.update(cx, |app, cx| {
                    app.workspace = crate::documentation_references::tests::workspace_fixture();
                    app.open_blank_request_tab(window, cx);
                    app.request_pane = RequestPane::Params;
                    app.url.update(cx, |input, cx| {
                        input.set_value(
                            "https://example.test/search?first=1&underneath=2",
                            window,
                            cx,
                        )
                    });
                    app.documentation.update(cx, |editor, cx| {
                        editor.set_value(
                            "@param query.first @Ref(Backend.Auth.Login)\n\
                             @param query.underneath Underlying parameter must not replace the hover.",
                            window,
                            cx,
                        )
                    });
                    cx.notify();
                });
            });
            cx.run_until_parked();
            let underneath = cx.debug_bounds(selector).unwrap();
            // Stay near the left edge so a tooltip on the rightmost column
            // fits without being flipped into a different column.
            let source_point =
                underneath.origin + gpui::point(px(20.), -underneath.size.height / 2.);
            cx.simulate_mouse_move(source_point, None, gpui::Modifiers::none());
            cx.run_until_parked();
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(600));
            cx.run_until_parked();
            let hover = cx
                .debug_bounds("documentation-explanation-hover")
                .unwrap_or_else(|| panic!("{selector}: hover must open"));
            let link_point = hover.origin + gpui::point(px(10.), hover.size.height / 2.);
            assert!(
                underneath.contains(&link_point),
                "{selector}: overlap required"
            );
            // Cross the padding before settling over the link for longer than
            // the underlying row's tooltip delay.
            for x in [-4., 10.] {
                let point = hover.origin + gpui::point(px(x), hover.size.height / 2.);
                cx.simulate_mouse_move(point, None, gpui::Modifiers::none());
                cx.run_until_parked();
                cx.executor()
                    .advance_clock(std::time::Duration::from_millis(600));
                cx.run_until_parked();
            }
            cx.simulate_click(link_point, gpui::Modifiers::none());
            cx.run_until_parked();
            cx.read(|cx| {
                assert_eq!(
                    app.read(cx).request_template(cx).request.url,
                    "https://example.test/login?secret=never-expose-url",
                    "{selector}: the original hover's link must remain clickable",
                );
                assert!(!app.read(cx).sending);
            });
        }
    }

    #[gpui::test]
    fn documentation_links_navigate_without_executing_and_reject_stale_targets(
        cx: &mut TestAppContext,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let store = DatabaseStore::new(directory.path().join("api-tester.sqlite3"));
        store.initialize().unwrap();
        let mut app = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            gpui_component::init(cx);
            crate::theme::configure(cx);
            let bindings = shortcuts::capture_base_key_bindings(cx);
            let view = cx.new(|cx| ApiTester::new_with_database_store(bindings, store, window, cx));
            app = Some(view.clone());
            Root::new(view, window, cx)
        });
        let app = app.unwrap();
        cx.update(|window, _| window.activate_window());
        cx.simulate_resize(size(px(1200.), px(800.)));
        let source = "See @Ref(Backend.Auth.Login)\nUses @Ref(api.environment[\"var_name\"])";
        let input = cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.workspace = crate::documentation_references::tests::workspace_fixture();
                app.selected_environment_id = None;
                app.request_pane = RequestPane::Documentation;
                app.documentation
                    .update(cx, |editor, cx| editor.set_value(source, window, cx));
                app.render_request_panel(cx);
                assert_eq!(
                    app.documentation_intelligence
                        .reference_actions
                        .borrow()
                        .targets
                        .len(),
                    2
                );
                let input = app.documentation.read(cx).input_state();
                assert_eq!(input.read(cx).diagnostics().unwrap().len(), 0);
                cx.notify();
                input
            })
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            app.read(cx)
                .documentation
                .clone()
                .update(cx, |editor, cx| editor.set_value("See ", window, cx));
            input.update(cx, |input, cx| {
                input.set_cursor_position(lsp_types::Position::new(0, 4), window, cx);
                input.focus_handle(cx).focus(window);
            });
        });
        cx.simulate_input("@Ref(Backend.Auth.Lo");
        cx.run_until_parked();
        cx.simulate_keystrokes("tab");
        assert_eq!(
            cx.read(|cx| input.read(cx).value().to_string()),
            "See @Ref(Backend.Auth.Login)"
        );
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.documentation
                    .update(cx, |editor, cx| editor.set_value(source, window, cx));
                app.render_request_panel(cx);
            })
        });
        cx.run_until_parked();
        let bounds = cx
            .debug_bounds("request-documentation-editor")
            .expect("documentation visible");
        let line_height = cx.read(|cx| cx.api_theme().classes.editor.font_size * (20. / 13.));
        cx.simulate_click(
            bounds.origin + gpui::point(px(100.), line_height * 1.5),
            gpui::Modifiers::none(),
        );
        cx.run_until_parked();
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                assert_eq!(
                    app.request_template(cx).request.url,
                    "https://example.test/login?secret=never-expose-url"
                );
                assert!(!app.sending, "a reference only opens a request");
                app.request_pane = RequestPane::Documentation;
                app.documentation
                    .update(cx, |editor, cx| editor.set_value(source, window, cx));
                app.render_request_panel(cx);
            })
        });
        cx.run_until_parked();
        cx.update(|_, cx| input.update(cx, |_, cx| cx.emit(InputEvent::InlineAction { id: 1 })));
        cx.run_until_parked();
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                assert_eq!(
                    app.selected_environment_id,
                    app.workspace.active_environment_id
                );
                assert_eq!(app.sidebar_tab, SidebarTab::Environments);
                let variable = app
                    .environment_variables
                    .iter()
                    .find(|row| row.key.read(cx).value().as_ref() == "var_name")
                    .unwrap();
                assert!(variable.key.read(cx).focus_handle(cx).is_focused(window));
                assert!(!app.sending);
                // A rendered link must not redirect to a different active environment.
                let stale = app
                    .documentation_intelligence
                    .reference_actions
                    .borrow()
                    .targets[&1][0]
                    .clone();
                app.workspace.set_active_environment(None).unwrap();
                app.open_documentation_reference(&stale.0, &stale.1, window, cx);
                assert!(
                    app.workspace_warning
                        .as_deref()
                        .unwrap()
                        .contains("no longer available")
                );
                // Same-line links share a chooser rather than hiding all but the first.
                app.workspace = crate::documentation_references::tests::workspace_fixture();
                app.documentation.update(cx, |editor, cx| {
                    editor.set_value(
                        "@Ref(Backend.Auth.Login) and @Ref(api.environment[\"var_name\"])",
                        window,
                        cx,
                    )
                });
                app.render_request_panel(cx);
                assert_eq!(
                    app.documentation_intelligence
                        .reference_actions
                        .borrow()
                        .targets[&0]
                        .len(),
                    2
                );
            })
        });
        cx.run_until_parked();
        cx.update(|_, cx| input.update(cx, |_, cx| cx.emit(InputEvent::InlineAction { id: 0 })));
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert!(window.has_active_dialog(cx));
            window.close_dialog(cx);
        });

        // Exercise the actual header hover, including moving into it and clicking
        // a projected link rather than invoking navigation directly.
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.activate_request_workspace(SidebarTab::Collections, window, cx);
                app.open_blank_request_tab(window, cx);
                assert_ne!(
                    app.request_template(cx).request.url,
                    "https://example.test/login?secret=never-expose-url"
                );
                app.request_pane = RequestPane::Headers;
                app.headers.clear();
                app.push_header_row("X-CSRF", "", true, false, window, cx);
                app.push_header_row("X-Underneath", "", true, false, window, cx);
                app.documentation.update(cx, |editor, cx| {
                    editor.set_value(
                        "@header X-CSRF @Ref(Backend.Auth.Login)\n\
                         @header X-Underneath Underlying header must not replace the open hover.",
                        window,
                        cx,
                    )
                });
                cx.notify();
            })
        });
        cx.run_until_parked();
        // Repeated debug selectors retain the last row's bounds. Target the
        // first of these two adjacent, equally sized header rows.
        let bounds = cx.debug_bounds("header-explanation-source").unwrap();
        let source_point = bounds.center() - gpui::point(px(0.), bounds.size.height);
        cx.simulate_mouse_move(source_point, None, gpui::Modifiers::none());
        cx.run_until_parked();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(600));
        cx.run_until_parked();
        let hover = cx
            .debug_bounds("documentation-explanation-hover")
            .expect("hover visible");
        let link_point = hover.origin + gpui::point(px(10.), hover.size.height / 2.);
        // The tooltip's left padding also overlaps the next documented row.
        // It must block underlying hover triggers, not just the text itself.
        let padding_point = hover.origin + gpui::point(px(-4.), hover.size.height / 2.);
        assert!(
            bounds.contains(&link_point),
            "link must overlap the next header"
        );
        assert!(
            bounds.contains(&padding_point),
            "padding must overlap the next header"
        );
        cx.simulate_mouse_move(padding_point, None, gpui::Modifiers::none());
        cx.run_until_parked();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(600));
        cx.run_until_parked();
        cx.simulate_mouse_move(link_point, None, gpui::Modifiers::none());
        cx.run_until_parked();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(600));
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("documentation-explanation-hover").is_some(),
            "hover stays open over its link"
        );
        cx.simulate_click(link_point, gpui::Modifiers::none());
        cx.run_until_parked();
        cx.read(|cx| {
            assert_eq!(
                app.read(cx).request_template(cx).request.url,
                "https://example.test/login?secret=never-expose-url"
            );
            assert!(!app.read(cx).sending);
        });
    }
}
