//! Request-local explanations authored in Markdown, never copied into request fields.
//!
//! Only column-zero annotation lines in top-level Markdown paragraphs are active.
//! Using the Markdown AST keeps code blocks, lists, quotes and HTML examples inert.
use std::{cell::RefCell, collections::BTreeSet, ops::Range, rc::Rc};

use anyhow::Result;
use gpui::{Context, Task, Window};
use gpui_component::input::{CompletionProvider, InputState, Rope};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit,
    Diagnostic, DiagnosticSeverity, TextEdit,
};
use markdown::mdast::Node;

use crate::documentation_references::{Reference, ReferenceActions, ReferenceCatalog, references};
use crate::editor_util::{clipped_char_boundary, source_range};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetKind {
    Query,
    Header,
}

impl TargetKind {
    fn key(self, name: &str) -> String {
        match self {
            Self::Query => name.to_owned(),
            Self::Header => name.to_ascii_lowercase(),
        }
    }
}

/// Names only: disabled rows are documentable; values never enter intelligence.
/// An annotation describes a name, so repeated rows share its explanation.
#[derive(Default, PartialEq, Eq)]
pub struct Targets {
    query: BTreeSet<String>,
    headers: BTreeSet<String>,
}

impl Targets {
    pub fn new(query: impl Iterator<Item = String>, headers: impl Iterator<Item = String>) -> Self {
        Self {
            query: query.filter(|name| !name.is_empty()).collect(),
            headers: headers
                .filter(|name| !name.is_empty())
                .map(|name| name.to_ascii_lowercase())
                .collect(),
        }
    }

    fn names(&self, kind: TargetKind) -> &BTreeSet<String> {
        match kind {
            TargetKind::Query => &self.query,
            TargetKind::Header => &self.headers,
        }
    }

    fn contains(&self, kind: TargetKind, name: &str) -> bool {
        self.names(kind).contains(&kind.key(name))
    }
}

#[derive(Debug)]
struct Annotation {
    kind: TargetKind,
    name: String,
    explanation: String,
    range: Range<usize>,
}

#[derive(Default)]
struct Index {
    lines: Vec<Range<usize>>,
    annotations: Vec<Annotation>,
    diagnostics: Vec<Diagnostic>,
    references: Vec<Reference>,
}

fn warning(source: &str, range: Range<usize>, message: impl Into<String>) -> Diagnostic {
    Diagnostic {
        range: source_range(source, range.start, range.end),
        severity: Some(DiagnosticSeverity::WARNING),
        source: Some("request-documentation".into()),
        message: message.into(),
        ..Default::default()
    }
}

/// Offsets come from the parser, not decoded Markdown text (which loses escapes).
fn annotation_lines(source: &str) -> Vec<Range<usize>> {
    let Ok(Node::Root(root)) = markdown::to_mdast(source, &markdown::ParseOptions::default())
    else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    for node in root.children {
        let Node::Paragraph(paragraph) = node else {
            continue;
        };
        let Some(position) = paragraph.position else {
            continue;
        };
        let mut start = position.start.offset;
        for line in source[position.start.offset..position.end.offset].split_inclusive('\n') {
            let end = start + line.trim_end_matches(['\r', '\n']).len();
            if (start == 0 || source.as_bytes()[start - 1] == b'\n')
                && source[start..end].starts_with('@')
                && paragraph.children.iter().any(|node| {
                    matches!(node, Node::Text(text) if text.position.as_ref().is_some_and(
                        |position| position.start.offset <= start && start < position.end.offset
                    ))
                })
            {
                lines.push(start..end);
            }
            start += line.len();
        }
    }
    lines
}

/// JSON quoting permits names containing spaces, quotes and other unusual bytes.
fn name_token(text: &str) -> Option<(String, usize)> {
    if text.starts_with('"') {
        let mut escaped = false;
        for (offset, character) in text.char_indices().skip(1) {
            if character == '"' && !escaped {
                let end = offset + 1;
                return serde_json::from_str(&text[..end])
                    .ok()
                    .map(|name| (name, end));
            }
            escaped = character == '\\' && !escaped;
        }
        None
    } else {
        let end = text.find(char::is_whitespace).unwrap_or(text.len());
        (end > 0).then(|| (text[..end].to_owned(), end))
    }
}

fn encode_name(name: &str) -> String {
    if name
        .chars()
        .any(|c| c.is_whitespace() || matches!(c, '"' | '\\' | '`'))
    {
        serde_json::to_string(name).expect("serialize a string")
    } else {
        name.to_owned()
    }
}

fn parse(source: &str) -> Index {
    let mut index = Index {
        lines: annotation_lines(source),
        references: references(source),
        ..Default::default()
    };
    for range in index.lines.clone() {
        let line = &source[range.clone()];
        let tag_end = line.find(char::is_whitespace).unwrap_or(line.len());
        let (kind, prefix) = match &line[..tag_end] {
            "@param" => (TargetKind::Query, "query."),
            "@header" => (TargetKind::Header, ""),
            _ => continue,
        };
        let target = line[tag_end..].trim_start();
        let Some(text) = target.strip_prefix(prefix) else {
            index.diagnostics.push(warning(
                source,
                range,
                "Expected @param query.<name> <explanation>.",
            ));
            continue;
        };
        let Some((name, length)) = name_token(text) else {
            index.diagnostics.push(warning(
                source,
                range,
                "Expected a target name; use JSON quotes for names containing spaces.",
            ));
            continue;
        };
        let remainder = &text[length..];
        if name.is_empty() || (!remainder.is_empty() && !remainder.starts_with(char::is_whitespace))
        {
            index
                .diagnostics
                .push(warning(source, range, "Invalid documentation target."));
            continue;
        }
        let target_end = range.end - remainder.len();
        let explanation = remainder.trim().to_owned();
        if explanation.is_empty() {
            index.diagnostics.push(warning(
                source,
                range.clone(),
                "Add an explanation after the target name.",
            ));
        }
        index.annotations.push(Annotation {
            kind,
            name,
            explanation,
            range: range.start..target_end,
        });
    }
    index
}

/// Each editor owns a separate handle, including secondary workspace panes.
/// Parsing is cached by buffer contents; the index is disposable derived state.
#[derive(Default)]
pub struct DocumentationIntelligence {
    targets: RefCell<Targets>,
    parsed: RefCell<(String, Index)>,
    pub resources: RefCell<ReferenceCatalog>,
    pub reference_actions: RefCell<ReferenceActions>,
}

impl DocumentationIntelligence {
    pub fn shared() -> Rc<Self> {
        Rc::new(Self::default())
    }

    pub fn replace_targets(&self, targets: Targets) -> bool {
        let mut current = self.targets.borrow_mut();
        if *current == targets {
            return false;
        }
        *current = targets;
        true
    }

    fn with_index<T>(&self, source: &str, read: impl FnOnce(&Index) -> T) -> T {
        let mut cached = self.parsed.borrow_mut();
        if cached.0 != source {
            *cached = (source.to_owned(), parse(source));
        }
        read(&cached.1)
    }

    pub fn explanation(&self, source: &str, kind: TargetKind, name: &str) -> Option<String> {
        self.with_index(source, |index| {
            let key = kind.key(name);
            let mut matches = index
                .annotations
                .iter()
                .filter(|annotation| annotation.kind == kind && kind.key(&annotation.name) == key);
            let annotation = matches.next()?;
            // Conflicting annotations never pick an arbitrary winner.
            (matches.next().is_none() && !annotation.explanation.is_empty())
                .then(|| annotation.explanation.clone())
        })
    }

    pub fn references(&self, source: &str) -> Vec<Reference> {
        self.with_index(source, |index| index.references.clone())
    }

    pub fn diagnostics(&self, source: &str) -> Vec<Diagnostic> {
        self.with_index(source, |index| {
            let targets = self.targets.borrow();
            let mut diagnostics = index.diagnostics.clone();
            for reference in &index.references {
                if !reference.complete {
                    diagnostics.push(warning(source, reference.range.clone(), "Close this resource reference with )."));
                } else if !self.resources.borrow().0.contains_key(reference.expression(source)) {
                    diagnostics.push(warning(source, reference.range.clone(), "Resource not found in the current workspace or active environment. Use a literal path from autocomplete."));
                }
            }
            for annotation in &index.annotations {
                if !targets.contains(annotation.kind, &annotation.name) {
                    diagnostics.push(warning(
                        source,
                        annotation.range.clone(),
                        format!("No matching {:?} field named '{}'. Update this reference if the field was renamed or removed.", annotation.kind, annotation.name),
                    ));
                }
                if index.annotations.iter().filter(|other| {
                    other.kind == annotation.kind
                        && other.kind.key(&other.name) == annotation.kind.key(&annotation.name)
                }).count() > 1 {
                    diagnostics.push(warning(
                        source, annotation.range.clone(),
                        "Multiple explanations for this target. Keep one annotation per name.",
                    ));
                }
            }
            diagnostics
        })
    }

    fn completion_items(&self, source: &str, offset: usize) -> Vec<CompletionItem> {
        let offset = clipped_char_boundary(source, offset);
        if let Some(reference) = self.references(source).into_iter().find(|reference| {
            reference.expression_range.start <= offset && offset <= reference.expression_range.end
        }) {
            let prefix = source[reference.expression_range.start..offset].trim_start();
            return self
                .resources
                .borrow()
                .0
                .keys()
                .filter(|expression| expression.starts_with(prefix))
                .map(|expression| CompletionItem {
                    label: expression.clone(),
                    kind: Some(CompletionItemKind::REFERENCE),
                    detail: Some("Open a Resolved resource; never executes a request".into()),
                    text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                        range: source_range(
                            source,
                            reference.expression_range.start,
                            reference.expression_range.end,
                        ),
                        new_text: format!(
                            "{expression}{}",
                            if reference.complete { "" } else { ")" }
                        ),
                    })),
                    ..Default::default()
                })
                .collect();
        }
        let Some(line_range) = self.with_index(source, |index| {
            index
                .lines
                .iter()
                .find(|range| range.start <= offset && offset <= range.end)
                .cloned()
        }) else {
            return Vec::new();
        };
        let line = &source[line_range.clone()];
        let before = &source[line_range.start..offset];
        let tag_end = line.find(char::is_whitespace).unwrap_or(line.len());
        let (start, end, prefix, candidates) = if before.len() <= tag_end {
            (
                line_range.start,
                line_range.start + tag_end,
                before,
                vec![
                    "@param query.".to_owned(),
                    "@header ".to_owned(),
                    "@Ref(".to_owned(),
                ],
            )
        } else {
            let (kind, scope) = match &line[..tag_end] {
                "@param" => (TargetKind::Query, "query."),
                "@header" => (TargetKind::Header, ""),
                _ => return Vec::new(),
            };
            let rest = line[tag_end..].trim_start();
            let start = line_range.end - rest.len();
            if offset < start {
                return Vec::new();
            }
            let token_start = if kind == TargetKind::Query && rest.starts_with(scope) {
                scope.len()
            } else {
                0
            };
            let token = &rest[token_start..];
            let end = start + token_start + name_token(token).map_or(token.len(), |(_, end)| end);
            if offset > end {
                return Vec::new();
            }
            let candidates = self
                .targets
                .borrow()
                .names(kind)
                .iter()
                .map(|name| format!("{scope}{}", encode_name(name)))
                .collect();
            (start, end, &source[start..offset], candidates)
        };
        candidates
            .into_iter()
            .filter(|candidate| candidate.to_lowercase().starts_with(&prefix.to_lowercase()))
            .map(|candidate| CompletionItem {
                label: candidate.trim_end().to_owned(),
                kind: Some(CompletionItemKind::REFERENCE),
                detail: Some("Request documentation reference (names only)".into()),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range: source_range(source, start, end),
                    new_text: candidate,
                })),
                ..Default::default()
            })
            .collect()
    }
}

impl CompletionProvider for DocumentationIntelligence {
    fn supports_inline_completion(&self) -> bool {
        false
    }

    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        _: CompletionContext,
        _: &mut Window,
        _: &mut Context<InputState>,
    ) -> Task<Result<CompletionResponse>> {
        Task::ready(Ok(CompletionResponse::Array(
            self.completion_items(&text.to_string(), offset),
        )))
    }

    fn is_completion_trigger(&self, _: usize, text: &str, _: &mut Context<InputState>) -> bool {
        !text.is_empty() && !text.contains(['\r', '\n'])
    }

    fn is_completion_trigger_in_text(
        &self,
        text: &Rope,
        offset: usize,
        _: usize,
        inserted: &str,
        _: &mut Context<InputState>,
    ) -> bool {
        !inserted.is_empty()
            && !inserted.contains(['\r', '\n'])
            && !self.completion_items(&text.to_string(), offset).is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intelligence() -> Rc<DocumentationIntelligence> {
        let intelligence = DocumentationIntelligence::shared();
        intelligence.replace_targets(Targets::new(
            ["limit", "标签", "two words"]
                .map(str::to_owned)
                .into_iter(),
            ["Authorization", "X-Trace", "authorization"]
                .map(str::to_owned)
                .into_iter(),
        ));
        intelligence
    }

    #[test]
    fn annotations_resolve_by_name_with_http_header_case_rules() {
        let intelligence = intelligence();
        let source =
            "# Request\n\n@param query.limit Maximum records.\n@header Authorization Login token.";
        assert!(intelligence.diagnostics(source).is_empty());
        assert_eq!(
            intelligence
                .explanation(source, TargetKind::Query, "limit")
                .as_deref(),
            Some("Maximum records.")
        );
        assert_eq!(
            intelligence
                .explanation(source, TargetKind::Header, "AUTHORIZATION")
                .as_deref(),
            Some("Login token.")
        );
        assert_eq!(
            intelligence.explanation(source, TargetKind::Query, "Limit"),
            None
        );
    }

    #[test]
    fn markdown_examples_are_never_annotations_or_completion_contexts() {
        let intelligence = intelligence();
        for source in [
            "```markdown\n@header Missing Example.\n```",
            "~~~~\n@header Missing Example.\n~~~~",
            "    @header Missing Example.",
            "> @header Missing Example.",
            "- @header Missing Example.",
            "<div>\n@header Missing Example.\n</div>",
            "<!--\n@header Missing Example.\n-->",
            "`@header Missing Example.`",
            "`Code spanning lines\n@header Missing Example.\n`",
            "[Link spanning lines\n@header Missing Example.\n](https://example.com)",
            "Prose about @header Missing.",
            "```\n@header Missing",
        ] {
            assert!(intelligence.diagnostics(source).is_empty(), "{source}");
            assert!(
                intelligence
                    .explanation(source, TargetKind::Header, "Missing")
                    .is_none(),
                "{source}"
            );
            let offset = source.find("@header").unwrap() + "@header".len();
            assert!(
                intelligence.completion_items(source, offset).is_empty(),
                "{source}"
            );
        }
    }

    #[test]
    fn missing_duplicate_and_empty_annotations_have_diagnostics() {
        let intelligence = intelligence();
        assert_eq!(
            intelligence
                .diagnostics("@header Missing Explanation.")
                .len(),
            1
        );
        assert_eq!(intelligence.diagnostics("@param query.limit").len(), 1);
        assert_eq!(
            intelligence
                .diagnostics("@param body.name Explanation.")
                .len(),
            1
        );
        let source = "@header Authorization First.\n@header authorization Second.";
        assert_eq!(intelligence.diagnostics(source).len(), 2);
        assert_eq!(
            intelligence.explanation(source, TargetKind::Header, "Authorization"),
            None
        );
    }

    #[test]
    fn quoted_names_unicode_and_crlf_keep_source_coordinates() {
        let intelligence = intelligence();
        let source = "@param query.\"two words\" A phrase.\r\n@param query.标签 Unicode.\r\n@header Absent Missing.";
        assert_eq!(
            intelligence
                .explanation(source, TargetKind::Query, "two words")
                .as_deref(),
            Some("A phrase.")
        );
        assert_eq!(
            intelligence
                .explanation(source, TargetKind::Query, "标签")
                .as_deref(),
            Some("Unicode.")
        );
        let diagnostics = intelligence.diagnostics(source);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].range.start.line, 2);
        assert_eq!(diagnostics[0].range.end.character, 14);
    }

    #[test]
    fn completion_replaces_the_reference_not_the_explanation() {
        let intelligence = intelligence();
        let source = "@param query.lim Old explanation.";
        let items = intelligence.completion_items(source, "@param query.li".len());
        assert_eq!(items.len(), 1);
        let Some(CompletionTextEdit::Edit(edit)) = &items[0].text_edit else {
            panic!("text edit");
        };
        assert_eq!(edit.new_text, "query.limit");
        assert_eq!(edit.range.start.character, 7);
        assert_eq!(edit.range.end.character, 16);
        assert!(
            intelligence
                .completion_items(source, source.len())
                .is_empty()
        );
        assert_eq!(intelligence.completion_items("@", 1).len(), 3);
        assert_eq!(intelligence.completion_items("@header ", 8).len(), 2);
        assert_eq!(intelligence.completion_items("@param query.", 13).len(), 3);
    }

    #[test]
    fn resource_completion_and_diagnostics_are_literal_and_workspace_scoped() {
        let intelligence = intelligence();
        *intelligence.resources.borrow_mut() = ReferenceCatalog::from_workspace(
            &crate::documentation_references::tests::workspace_fixture(),
        );
        let source = "First use @Ref(Backend.Auth.Lo)";
        let items = intelligence.completion_items(source, source.len() - 1);
        assert_eq!(items.len(), 1);
        let Some(CompletionTextEdit::Edit(edit)) = &items[0].text_edit else {
            panic!("edit");
        };
        assert_eq!(edit.new_text, "Backend.Auth.Login");
        let source = "Uses @Ref(api.environment[\"var";
        let items = intelligence.completion_items(source, source.len());
        assert_eq!(items.len(), 1);
        let Some(CompletionTextEdit::Edit(edit)) = &items[0].text_edit else {
            panic!("edit");
        };
        assert_eq!(edit.new_text, "api.environment[\"var_name\"])");
        let source = "@Ref(Backend.Auth.Login)\n@Ref(api.environment[\"var_name\"])";
        assert!(intelligence.diagnostics(source).is_empty());
        *intelligence.resources.borrow_mut() = ReferenceCatalog::default();
        assert_eq!(intelligence.diagnostics(source).len(), 2);
        assert_eq!(
            intelligence
                .diagnostics("@Ref(api.environment[compute()])")
                .len(),
            1
        );
    }

    #[test]
    fn source_and_target_changes_invalidate_derived_state() {
        let intelligence = intelligence();
        assert_eq!(
            intelligence
                .explanation("@param query.limit Old.", TargetKind::Query, "limit")
                .as_deref(),
            Some("Old.")
        );
        assert_eq!(
            intelligence
                .explanation("@param query.limit New.", TargetKind::Query, "limit")
                .as_deref(),
            Some("New.")
        );
        assert_eq!(
            intelligence.explanation("", TargetKind::Query, "limit"),
            None
        );
        let source = "@param query.limit Explanation.";
        assert!(intelligence.diagnostics(source).is_empty());
        intelligence.replace_targets(Targets::default());
        assert_eq!(intelligence.diagnostics(source).len(), 1);
        let other_pane = DocumentationIntelligence::shared();
        assert_eq!(other_pane.explanation("", TargetKind::Query, "limit"), None);
    }
}
