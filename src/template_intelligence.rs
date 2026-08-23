//! Editor intelligence for `{{environment_variable}}` request templates.
//!
//! This module deliberately stops at describing template text and the active
//! environment. Persisting a missing variable is an application-level action:
//! the caller must revalidate the active environment and commit the workspace
//! before dismissing any creation UI.

use std::{cell::RefCell, collections::BTreeMap, fmt, ops::Range as ByteRange, rc::Rc};

use anyhow::Result;
use gpui::{Context, HighlightStyle, Hsla, Task, Window};
use gpui_component::input::{CompletionProvider, InputState, Rope};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit,
    Documentation, Range, TextEdit,
};

use crate::core::Environment;
use crate::editor_util::{clipped_char_boundary, source_range};

const COMPLETION_CONTEXT_PADDING: usize = 32;

/// A value-aware snapshot of one active-environment variable.
///
/// Unlike script completions, request-template hover deliberately needs the
/// value. Callers must keep this catalog scoped to request-template UI and
/// must not copy values into completion labels, diagnostics, or logs.
#[derive(Clone, PartialEq, Eq)]
pub struct TemplateVariable {
    pub id: String,
    pub name: String,
    pub value: String,
    pub enabled: bool,
    pub secret: bool,
}

impl fmt::Debug for TemplateVariable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TemplateVariable")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("value", &"[OMITTED]")
            .field("enabled", &self.enabled)
            .field("secret", &self.secret)
            .finish()
    }
}

/// A replaceable snapshot of the environment used to resolve outgoing
/// requests.
///
/// Each name maps to a vector so corrupt or legacy duplicate keys remain
/// visible as invalid instead of one value silently winning.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TemplateVariableCatalog {
    environment_id: Option<String>,
    environment_name: Option<String>,
    variables: BTreeMap<String, Vec<TemplateVariable>>,
}

impl TemplateVariableCatalog {
    pub fn from_environment(environment: Option<&Environment>) -> Self {
        let Some(environment) = environment else {
            return Self::default();
        };

        Self::from_parts(
            Some(environment.id.clone()),
            Some(environment.name.clone()),
            environment
                .variables
                .iter()
                .map(|variable| TemplateVariable {
                    id: variable.id.clone(),
                    name: variable.key.trim().to_owned(),
                    value: variable.value.clone(),
                    enabled: variable.enabled,
                    secret: variable.secret,
                }),
        )
    }

    /// Builds a catalog without requiring a workspace, useful for adapters and
    /// focused tests.
    pub fn from_parts<I>(
        environment_id: Option<String>,
        environment_name: Option<String>,
        variables: I,
    ) -> Self
    where
        I: IntoIterator<Item = TemplateVariable>,
    {
        let mut grouped = BTreeMap::<String, Vec<TemplateVariable>>::new();
        for mut variable in variables {
            variable.name = variable.name.trim().to_owned();
            grouped
                .entry(variable.name.clone())
                .or_default()
                .push(variable);
        }

        Self {
            environment_id,
            environment_name,
            variables: grouped,
        }
    }

    pub fn replace_environment(&mut self, environment: Option<&Environment>) {
        *self = Self::from_environment(environment);
    }

    pub fn shared(self) -> TemplateVariableCatalogHandle {
        Rc::new(RefCell::new(self))
    }

    pub fn environment_id(&self) -> Option<&str> {
        self.environment_id.as_deref()
    }

    pub fn environment_name(&self) -> Option<&str> {
        self.environment_name.as_deref()
    }

    /// Returns unique, enabled variables in deterministic name order.
    ///
    /// Duplicate keys are intentionally omitted because request resolution
    /// must treat them as invalid.
    pub fn enabled_variables(&self) -> impl Iterator<Item = &TemplateVariable> {
        self.variables
            .values()
            .filter_map(|entries| match entries.as_slice() {
                [variable] if variable.enabled => Some(variable),
                _ => None,
            })
    }

    pub fn variable(&self, name: &str) -> Option<&TemplateVariable> {
        match self.variables.get(name.trim()).map(Vec::as_slice) {
            Some([variable]) => Some(variable),
            _ => None,
        }
    }

    pub fn classify_name(&self, name: &str) -> TemplateClassification {
        let name = name.trim();
        if name.is_empty() {
            return TemplateClassification::Invalid(TemplateInvalidReason::EmptyName);
        }
        if name.contains("{{") || name.contains("}}") {
            return TemplateClassification::Invalid(TemplateInvalidReason::NestedDelimiter);
        }

        match self.variables.get(name).map(Vec::as_slice) {
            Some([variable]) if variable.enabled => TemplateClassification::Available,
            Some([_]) => TemplateClassification::Disabled,
            Some(_) => TemplateClassification::Invalid(TemplateInvalidReason::DuplicateName),
            None => TemplateClassification::Missing,
        }
    }
}

pub type TemplateVariableCatalogHandle = Rc<RefCell<TemplateVariableCatalog>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TemplateInvalidReason {
    EmptyName,
    Unclosed,
    NestedDelimiter,
    DuplicateName,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TemplateClassification {
    Available,
    Disabled,
    Missing,
    Invalid(TemplateInvalidReason),
}

/// A byte-accurate `{{...}}` occurrence.
///
/// `range` covers both delimiters when complete. `inner_range` covers all text
/// between the delimiters, while `name_range` excludes surrounding Unicode
/// whitespace. An incomplete span extends through the end of the source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateSpan {
    pub range: ByteRange<usize>,
    pub inner_range: ByteRange<usize>,
    pub name_range: ByteRange<usize>,
    pub complete: bool,
}

impl TemplateSpan {
    pub fn name<'a>(&self, source: &'a str) -> &'a str {
        source.get(self.name_range.clone()).unwrap_or_default()
    }

    pub fn inner_text<'a>(&self, source: &'a str) -> &'a str {
        source.get(self.inner_range.clone()).unwrap_or_default()
    }

    pub fn classification(
        &self,
        source: &str,
        catalog: &TemplateVariableCatalog,
    ) -> TemplateClassification {
        if !self.complete {
            return TemplateClassification::Invalid(TemplateInvalidReason::Unclosed);
        }
        let inner = self.inner_text(source);
        if inner.contains("{{") || inner.contains("}}") {
            return TemplateClassification::Invalid(TemplateInvalidReason::NestedDelimiter);
        }
        catalog.classify_name(self.name(source))
    }
}

/// Scans non-overlapping request-template spans from left to right.
///
/// A nested opening delimiter is retained inside the outer span so it can be
/// classified as invalid, matching request-resolution behavior.
pub fn scan_template_spans(source: &str) -> Vec<TemplateSpan> {
    let mut spans = Vec::new();
    let mut cursor = 0;

    while cursor < source.len() {
        let Some(relative_opening) = source[cursor..].find("{{") else {
            break;
        };
        let opening = cursor + relative_opening;
        let inner_start = opening + 2;

        let (inner_end, range_end, complete) =
            if let Some(relative_closing) = source[inner_start..].find("}}") {
                let closing = inner_start + relative_closing;
                (closing, closing + 2, true)
            } else {
                (source.len(), source.len(), false)
            };

        let name_range = trimmed_range(source, inner_start..inner_end);
        spans.push(TemplateSpan {
            range: opening..range_end,
            inner_range: inner_start..inner_end,
            name_range,
            complete,
        });

        if !complete {
            break;
        }
        cursor = range_end;
    }

    spans
}

fn trimmed_range(source: &str, range: ByteRange<usize>) -> ByteRange<usize> {
    let raw = source.get(range.clone()).unwrap_or_default();
    let leading = raw.len() - raw.trim_start().len();
    let trimmed_start = range.start + leading;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return trimmed_start..trimmed_start;
    }
    trimmed_start..trimmed_start + trimmed.len()
}

/// Colors used for complete template spans.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TemplateHighlightColors {
    pub valid: Hsla,
    pub warning: Hsla,
    pub error: Hsla,
}

/// Produces semantic colors for every complete or malformed template span.
///
/// Ranges use the same Unicode-scalar LSP position contract currently used by
/// `gpui-component` when mapping edits and diagnostics back into its Rope.
pub fn semantic_style_spans(
    source: &str,
    catalog: &TemplateVariableCatalog,
    colors: TemplateHighlightColors,
) -> Vec<(Range, HighlightStyle)> {
    scan_template_spans(source)
        .into_iter()
        .map(|span| {
            let color = match span.classification(source, catalog) {
                TemplateClassification::Available => colors.valid,
                TemplateClassification::Disabled => colors.warning,
                TemplateClassification::Missing | TemplateClassification::Invalid(_) => {
                    colors.error
                }
            };
            (
                source_range(source, span.range.start, span.range.end),
                HighlightStyle {
                    color: Some(color),
                    ..Default::default()
                },
            )
        })
        .collect()
}

#[derive(Clone, Debug)]
pub struct TemplateCompletionProvider {
    catalog: TemplateVariableCatalogHandle,
}

impl TemplateCompletionProvider {
    pub fn new(catalog: TemplateVariableCatalogHandle) -> Self {
        Self { catalog }
    }

    pub fn completion_items_for_source(
        &self,
        source: &str,
        requested_offset: usize,
    ) -> Vec<CompletionItem> {
        let Some(context) = completion_context(source, requested_offset) else {
            return Vec::new();
        };
        let catalog = self.catalog.borrow();
        catalog
            .enabled_variables()
            .filter(|variable| variable.name.starts_with(&context.typed_prefix))
            .map(|variable| CompletionItem {
                label: variable.name.clone(),
                kind: Some(CompletionItemKind::VARIABLE),
                detail: Some(if variable.secret {
                    "secret environment variable".to_owned()
                } else {
                    "environment variable".to_owned()
                }),
                documentation: Some(Documentation::String(
                    "Variable from the active environment. Values are omitted from completion items."
                        .to_owned(),
                )),
                sort_text: Some(variable.name.clone()),
                filter_text: Some(variable.name.clone()),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range: source_range(
                        source,
                        context.replace_range.start,
                        context.replace_range.end,
                    ),
                    new_text: variable.name.clone(),
                })),
                ..Default::default()
            })
            .collect()
    }
}

impl CompletionProvider for TemplateCompletionProvider {
    fn supports_inline_completion(&self) -> bool {
        false
    }

    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        _trigger: CompletionContext,
        _window: &mut Window,
        _cx: &mut Context<InputState>,
    ) -> Task<Result<CompletionResponse>> {
        Task::ready(Ok(CompletionResponse::Array(
            self.completion_items_for_source(&text.to_string(), offset),
        )))
    }

    fn is_completion_trigger(
        &self,
        _offset: usize,
        new_text: &str,
        _cx: &mut Context<InputState>,
    ) -> bool {
        !new_text.is_empty()
            && new_text
                .chars()
                .all(|character| !matches!(character, '\n' | '\r' | '}'))
    }

    fn is_completion_trigger_in_text(
        &self,
        text: &Rope,
        cursor_offset: usize,
        _edit_start: usize,
        new_text: &str,
        _cx: &mut Context<InputState>,
    ) -> bool {
        if new_text.is_empty()
            || new_text
                .chars()
                .any(|character| matches!(character, '\n' | '\r' | '}'))
        {
            return false;
        }

        let longest_enabled_name = self
            .catalog
            .borrow()
            .enabled_variables()
            .map(|variable| variable.name.chars().count())
            .max()
            .unwrap_or_default();
        has_open_template_before_cursor(text, cursor_offset, longest_enabled_name)
    }
}

fn has_open_template_before_cursor(
    text: &Rope,
    requested_cursor: usize,
    longest_enabled_name: usize,
) -> bool {
    let cursor = requested_cursor.min(text.len());
    let scan_limit = longest_enabled_name
        .saturating_add(COMPLETION_CONTEXT_PADDING)
        .saturating_add(2);
    let mut newer_character = None;

    for character in text.chars_at(cursor).reversed().take(scan_limit) {
        if matches!(character, '\n' | '\r') {
            return false;
        }
        match (character, newer_character) {
            ('{', Some('{')) => return true,
            ('}', Some('}')) => return false,
            _ => newer_character = Some(character),
        }
    }
    false
}

#[derive(Debug)]
struct TemplateCompletionContext {
    typed_prefix: String,
    replace_range: ByteRange<usize>,
}

fn completion_context(source: &str, requested_offset: usize) -> Option<TemplateCompletionContext> {
    let offset = clipped_char_boundary(source, requested_offset);
    let span = scan_template_spans(source).into_iter().find(|span| {
        span.inner_range.start <= offset
            && offset <= span.inner_range.end
            && (span.complete || offset <= span.range.end)
    })?;

    let inner = span.inner_text(source);
    if inner.contains("{{") || inner.contains("}}") {
        return None;
    }

    let prefix_end = offset.min(span.inner_range.end);
    let typed_prefix = source
        .get(span.inner_range.start..prefix_end)?
        .trim()
        .to_owned();

    Some(TemplateCompletionContext {
        typed_prefix,
        replace_range: span.name_range,
    })
}

#[cfg(test)]
mod tests {
    use gpui::{HighlightStyle, hsla};
    use lsp_types::{CompletionTextEdit, Position, Range};

    use super::*;

    fn variable(
        id: &str,
        name: &str,
        value: &str,
        enabled: bool,
        secret: bool,
    ) -> TemplateVariable {
        TemplateVariable {
            id: id.to_owned(),
            name: name.to_owned(),
            value: value.to_owned(),
            enabled,
            secret,
        }
    }

    fn catalog() -> TemplateVariableCatalog {
        TemplateVariableCatalog::from_parts(
            Some("environment-1".to_owned()),
            Some("Development".to_owned()),
            [
                variable("3", "zebra", "last", true, false),
                variable("1", "base_url", "https://example.test", true, false),
                variable("4", "disabled", "not-used", false, false),
                variable("2", "token", "s3cr3t", true, true),
            ],
        )
    }

    fn completion_edit(item: &CompletionItem) -> (&Range, &str) {
        match item.text_edit.as_ref().expect("completion has edit") {
            CompletionTextEdit::Edit(edit) => (&edit.range, edit.new_text.as_str()),
            CompletionTextEdit::InsertAndReplace(_) => panic!("expected exact text edit"),
        }
    }

    #[test]
    fn scans_adjacent_whitespace_unicode_and_incomplete_spans() {
        let source = "π {{ base_url }}{{名}}\nend {{ unfinished";
        let spans = scan_template_spans(source);

        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0].name(source), "base_url");
        assert_eq!(&source[spans[0].range.clone()], "{{ base_url }}");
        assert!(spans[0].complete);
        assert_eq!(spans[1].name(source), "名");
        assert_eq!(&source[spans[1].range.clone()], "{{名}}");
        assert!(spans[1].complete);
        assert_eq!(spans[2].name(source), "unfinished");
        assert_eq!(&source[spans[2].range.clone()], "{{ unfinished");
        assert!(!spans[2].complete);
    }

    #[test]
    fn whitespace_only_name_has_a_non_inverted_insertion_range() {
        let source = "{{   }}";
        let span = &scan_template_spans(source)[0];
        assert_eq!(span.name(source), "");
        assert_eq!(span.name_range, 5..5);
        assert_eq!(
            span.classification(source, &catalog()),
            TemplateClassification::Invalid(TemplateInvalidReason::EmptyName)
        );
    }

    #[test]
    fn classifies_available_disabled_missing_nested_and_unclosed() {
        let catalog = catalog();
        let source = "{{base_url}} {{ disabled }} {{missing}} {{{{nested}} {{open";
        let spans = scan_template_spans(source);

        assert_eq!(
            spans[0].classification(source, &catalog),
            TemplateClassification::Available
        );
        assert_eq!(
            spans[1].classification(source, &catalog),
            TemplateClassification::Disabled
        );
        assert_eq!(
            spans[2].classification(source, &catalog),
            TemplateClassification::Missing
        );
        assert_eq!(
            spans[3].classification(source, &catalog),
            TemplateClassification::Invalid(TemplateInvalidReason::NestedDelimiter)
        );
        assert_eq!(
            spans[4].classification(source, &catalog),
            TemplateClassification::Invalid(TemplateInvalidReason::Unclosed)
        );
    }

    #[test]
    fn duplicate_names_are_invalid_and_are_not_completed() {
        let catalog = TemplateVariableCatalog::from_parts(
            Some("environment-1".to_owned()),
            Some("Development".to_owned()),
            [
                variable("1", "same", "one", true, false),
                variable("2", "same", "two", true, false),
            ],
        );
        assert_eq!(
            catalog.classify_name("same"),
            TemplateClassification::Invalid(TemplateInvalidReason::DuplicateName)
        );

        let provider = TemplateCompletionProvider::new(catalog.shared());
        assert!(provider.completion_items_for_source("{{sa}}", 4).is_empty());
    }

    #[test]
    fn completions_are_sorted_enabled_only_and_replace_the_exact_name() {
        let provider = TemplateCompletionProvider::new(catalog().shared());
        let source = "é/{{ ba }}";
        let cursor = source.find("ba").unwrap() + 2;
        let items = provider.completion_items_for_source(source, cursor);

        assert_eq!(
            items
                .iter()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>(),
            vec!["base_url"]
        );
        let (range, new_text) = completion_edit(&items[0]);
        assert_eq!(new_text, "base_url");
        assert_eq!(*range, Range::new(Position::new(0, 5), Position::new(0, 7)));

        let all = provider.completion_items_for_source("{{}}", 2);
        assert_eq!(
            all.iter()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>(),
            vec!["base_url", "token", "zebra"]
        );
        assert!(
            all.iter()
                .all(|item| item.label != "disabled" && item.documentation.is_some())
        );
    }

    #[test]
    fn completion_trigger_is_limited_to_an_open_template() {
        let longest_name = catalog()
            .enabled_variables()
            .map(|variable| variable.name.chars().count())
            .max()
            .unwrap();

        let ordinary = Rope::from("a long ordinary JSON value");
        assert!(!has_open_template_before_cursor(
            &ordinary,
            ordinary.len(),
            longest_name
        ));

        let first_brace = Rope::from("prefix {");
        assert!(!has_open_template_before_cursor(
            &first_brace,
            first_brace.len(),
            longest_name
        ));

        let open = Rope::from("prefix {{ba}} suffix");
        let cursor = "prefix {{ba".len();
        assert!(has_open_template_before_cursor(&open, cursor, longest_name));
        assert!(!has_open_template_before_cursor(
            &open,
            open.len(),
            longest_name
        ));

        let previous_line = Rope::from("{{base_url\nordinary");
        assert!(!has_open_template_before_cursor(
            &previous_line,
            previous_line.len(),
            longest_name
        ));
    }

    #[test]
    fn completion_works_for_an_incomplete_placeholder_and_unicode_name() {
        let catalog = TemplateVariableCatalog::from_parts(
            Some("environment-1".to_owned()),
            Some("Development".to_owned()),
            [variable("1", "名", "value", true, false)],
        );
        let provider = TemplateCompletionProvider::new(catalog.shared());
        let source = "π {{名";
        let items = provider.completion_items_for_source(source, source.len());

        assert_eq!(items.len(), 1);
        let (range, new_text) = completion_edit(&items[0]);
        assert_eq!(new_text, "名");
        assert_eq!(*range, Range::new(Position::new(0, 4), Position::new(0, 5)));
    }

    #[test]
    fn debug_output_never_contains_variable_values() {
        let debug = format!("{:?}", catalog());
        assert!(!debug.contains("s3cr3t"));
        assert!(!debug.contains("https://example.test"));
        assert!(debug.contains("[OMITTED]"));
    }

    #[test]
    fn semantic_styles_use_the_requested_status_colors_and_unicode_ranges() {
        let source = "é {{base_url}} {{disabled}} {{missing}}";
        let colors = TemplateHighlightColors {
            valid: hsla(0.1, 0.2, 0.3, 1.0),
            warning: hsla(0.2, 0.3, 0.4, 1.0),
            error: hsla(0.3, 0.4, 0.5, 1.0),
        };
        let styles = semantic_style_spans(source, &catalog(), colors);

        assert_eq!(styles.len(), 3);
        assert_eq!(
            styles[0],
            (
                Range::new(Position::new(0, 2), Position::new(0, 14)),
                HighlightStyle {
                    color: Some(colors.valid),
                    ..Default::default()
                }
            )
        );
        assert_eq!(styles[1].1.color, Some(colors.warning));
        assert_eq!(styles[2].1.color, Some(colors.error));
    }
}
