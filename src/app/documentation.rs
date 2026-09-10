use super::*;
use crate::documentation_intelligence::{DocumentationIntelligence, TargetKind, Targets};

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
                .placeholder("Markdown notes\n\n@param query.limit Maximum records per page.\n@header Authorization Token obtained from Login.\n\nType @ at the start of a line to reference a field.")
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
    (editor, intelligence)
}

/// Refresh names at the request-surface boundary, covering URL edits, row edits,
/// tab hydration and remote updates without duplicating mutation hooks.
pub(super) fn refresh_targets(
    intelligence: &DocumentationIntelligence,
    editor: &Entity<CodeEditor>,
    query: &[QueryParamRow],
    headers: &[HeaderRow],
    cx: &mut App,
) {
    let targets = Targets::new(
        query.iter().map(|row| row.key.read(cx).value().to_string()),
        headers
            .iter()
            .map(|row| row.name.read(cx).value().trim().to_owned()),
    );
    if intelligence.replace_targets(targets) {
        editor.update(cx, |editor, cx| editor.refresh_diagnostics(cx));
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

/// A read-only projection: no second input buffer or persisted description.
pub(super) fn description_cell(
    id: SharedString,
    description: Option<String>,
    cx: &App,
) -> impl IntoElement {
    let tooltip = description
        .clone()
        .unwrap_or_else(|| "Add an @param query.<name> explanation in Documentation.".to_owned());
    div()
        .id(id)
        .flex_1()
        .min_w_0()
        .h_full()
        .px_3()
        .border_l_1()
        .border_color(cx.api_outline_variant())
        .flex()
        .items_center()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
        .child(
            div()
                .truncate()
                .child(description.unwrap_or_else(|| "—".to_owned())),
        )
}
