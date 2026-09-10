//! Request-local explanations authored in Markdown, never copied into request fields.
//!
//! Only column-zero annotation lines in top-level Markdown paragraphs are active.
//! Using the Markdown AST keeps code blocks, lists, quotes and HTML examples inert.
use std::{cell::RefCell, collections::BTreeSet, ops::Range, rc::Rc};

use anyhow::Result;
use gpui::{Context, HighlightStyle, Task, Window};
use gpui_component::input::{CompletionProvider, InputState, Rope};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit,
    Diagnostic, DiagnosticSeverity, TextEdit,
};
use markdown::mdast::Node;

use crate::documentation_body::{BodyMode, BodyTargets, valid_path};
use crate::documentation_references::{Reference, ReferenceActions, ReferenceCatalog, references};
use crate::editor_util::{clipped_char_boundary, source_range};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetKind {
    Query,
    Header,
    Body(BodyMode),
}

impl TargetKind {
    fn key(self, name: &str) -> String {
        match self {
            Self::Query | Self::Body(_) => name.to_owned(),
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
            TargetKind::Body(_) => unreachable!("body targets are resolved by format adapters"),
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
    annotation_tokens: Vec<(Range<usize>, bool)>,
}

fn warning(source: &str, range: Range<usize>, message: impl Into<String>) -> Diagnostic {
    diagnostic(source, range, message, DiagnosticSeverity::WARNING)
}

fn information(source: &str, range: Range<usize>, message: impl Into<String>) -> Diagnostic {
    diagnostic(source, range, message, DiagnosticSeverity::INFORMATION)
}

fn diagnostic(
    source: &str,
    range: Range<usize>,
    message: impl Into<String>,
    severity: DiagnosticSeverity,
) -> Diagnostic {
    Diagnostic {
        range: source_range(source, range.start, range.end),
        severity: Some(severity),
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
    if name.is_empty()
        || name
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '"' | '\\' | '`'))
    {
        serde_json::to_string(name).expect("serialize a string")
    } else {
        name.to_owned()
    }
}

/// Keep tag recognition identical for parsing and completion.
fn annotation_tag(tag: &str) -> Result<Option<(TargetKind, &'static str)>, &'static str> {
    match tag {
        "@param" => Ok(Some((TargetKind::Query, "query."))),
        "@header" => Ok(Some((TargetKind::Header, ""))),
        "@body" => Ok(Some((TargetKind::Body(BodyMode::Json), ""))),
        _ if tag.starts_with("@body(") => {
            match tag
                .strip_prefix("@body(")
                .and_then(|mode| mode.strip_suffix(')'))
            {
                Some(mode) if mode.eq_ignore_ascii_case("JSON") => {
                    Ok(Some((TargetKind::Body(BodyMode::Json), "")))
                }
                Some(mode) if mode.eq_ignore_ascii_case("XML") => {
                    Ok(Some((TargetKind::Body(BodyMode::Xml), "")))
                }
                _ => Err("Expected @body(JSON) or @body(XML); @body defaults to JSON."),
            }
        }
        _ => Ok(None),
    }
}

fn parse(source: &str) -> Index {
    let mut index = Index {
        lines: annotation_lines(source),
        references: references(source),
        ..Default::default()
    };
    let mut literal_targets = Vec::new();
    for range in index.lines.clone() {
        let line = &source[range.clone()];
        let tag_end = line.find(char::is_whitespace).unwrap_or(line.len());
        let (kind, prefix) = match annotation_tag(&line[..tag_end]) {
            Ok(Some(tag)) => tag,
            Ok(None) => continue,
            Err(message) => {
                index
                    .annotation_tokens
                    .push((range.start..range.start + tag_end, true));
                index.diagnostics.push(warning(source, range, message));
                continue;
            }
        };
        index
            .annotation_tokens
            .push((range.start..range.start + tag_end, true));
        let target = line[tag_end..].trim_start();
        // Names are literal, even if they contain @Ref(...). Reserve incomplete
        // quoted tokens too, so reference completion cannot steal path editing.
        let token = target.strip_prefix(prefix).unwrap_or(target);
        let token_end =
            range.end - token.len() + name_token(token).map_or(token.len(), |(_, length)| length);
        literal_targets.push(range.end - target.len()..token_end);
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
        if (name.is_empty() && !matches!(kind, TargetKind::Body(BodyMode::Json)))
            || (!remainder.is_empty() && !remainder.starts_with(char::is_whitespace))
        {
            index
                .diagnostics
                .push(warning(source, range, "Invalid documentation target."));
            continue;
        }
        let target_end = range.end - remainder.len();
        let target_start = range.end - target.len();
        index
            .annotation_tokens
            .push((target_start..target_end, false));
        if let TargetKind::Body(mode) = kind {
            if !valid_path(mode, &name) {
                index.diagnostics.push(warning(
                    source,
                    target_start..target_end,
                    match mode {
                        BodyMode::Json => "Expected a JSON Pointer: /user/name, /items/0, or \"\" for the root. Escape ~ as ~0 and / as ~1.",
                        BodyMode::Xml => "Expected an absolute XML element path, optionally ending in /@attribute. Selectors and wildcards are not supported.",
                    },
                ));
                continue;
            }
        }
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
    index.references.retain(|reference| {
        !literal_targets
            .iter()
            .any(|range| range.contains(&reference.range.start))
    });
    index
}

#[derive(Default)]
struct BodySnapshot {
    mode: Option<BodyMode>,
    source: String,
    targets: BodyTargets,
}

/// Each editor owns a separate handle, including secondary workspace panes.
/// Parsing is cached by buffer contents; the index is disposable derived state.
#[derive(Default)]
pub struct DocumentationIntelligence {
    targets: RefCell<Targets>,
    body: RefCell<BodySnapshot>,
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

    /// Parse only when the literal editor buffer or selected body mode changes.
    pub fn replace_body(&self, mode: Option<BodyMode>, source: &str) -> bool {
        let mut body = self.body.borrow_mut();
        let source = if mode.is_some() { source } else { "" };
        if body.mode == mode && body.source == source {
            return false;
        }
        *body = BodySnapshot {
            mode,
            source: source.to_owned(),
            targets: mode
                .map(|mode| BodyTargets::parse(mode, source))
                .unwrap_or_default(),
        };
        true
    }

    pub fn body_source_matches(&self, source: &str) -> bool {
        let body = self.body.borrow();
        body.mode.is_some() && body.source == source
    }

    /// Prefer the innermost field at the hover location, even if it has no
    /// explanation. A parent's annotation must not masquerade as its child's.
    pub fn body_explanations(&self, source: &str, offset: usize) -> Vec<String> {
        let body = self.body.borrow();
        let Some(mode) = body.mode else {
            return Vec::new();
        };
        let matches: Vec<_> = body
            .targets
            .names(mode)
            .iter()
            .filter_map(|path| {
                body.targets
                    .ranges(mode, path)
                    .iter()
                    .filter(|range| range.contains(&offset))
                    .map(|range| range.len())
                    .min()
                    .map(|length| (path, length))
            })
            .collect();
        let shortest = matches.iter().map(|(_, length)| *length).min();
        matches
            .into_iter()
            .filter(|(_, length)| Some(*length) == shortest)
            .filter_map(|(path, _)| self.explanation(source, TargetKind::Body(mode), path))
            .collect()
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

    /// Overlay only structural tokens; explanation prose retains Markdown styles.
    /// The same parsed ranges drive diagnostics and navigation, including exclusions
    /// for escaped markers and code examples.
    pub fn semantic_highlights(
        &self,
        source: &str,
        annotation: HighlightStyle,
        target: HighlightStyle,
        link: HighlightStyle,
    ) -> Vec<(lsp_types::Range, HighlightStyle)> {
        self.with_index(source, |index| {
            let mut spans = index
                .annotation_tokens
                .iter()
                .map(|(range, is_tag)| (range.clone(), if *is_tag { annotation } else { target }))
                .collect::<Vec<_>>();
            let resources = self.resources.borrow();
            for reference in &index.references {
                spans.push((
                    reference.range.start..reference.expression_range.start,
                    annotation,
                ));
                if !reference.expression_range.is_empty() {
                    let style = if reference.complete
                        && resources.0.contains_key(reference.expression(source))
                    {
                        link
                    } else {
                        target
                    };
                    spans.push((reference.expression_range.clone(), style));
                }
                if reference.complete {
                    spans.push((
                        reference.expression_range.end..reference.range.end,
                        annotation,
                    ));
                }
            }
            spans.sort_by_key(|(range, _)| range.start);
            spans
                .into_iter()
                .map(|(range, style)| (source_range(source, range.start, range.end), style))
                .collect()
        })
    }

    pub fn diagnostics(&self, source: &str) -> Vec<Diagnostic> {
        self.with_index(source, |index| {
            let targets = self.targets.borrow();
            let mut diagnostics = index.diagnostics.clone();
            for reference in &index.references {
                if !reference.complete {
                    diagnostics.push(warning(source, reference.range.clone(), "Close this resource reference with )."));
                } else if !self.resources.borrow().0.contains_key(reference.expression(source)) {
                    diagnostics.push(information(source, reference.range.clone(), "Resource is not currently available in this workspace or active environment. This reference remains text until it resolves."));
                }
            }
            for annotation in &index.annotations {
                let found = match annotation.kind {
                    TargetKind::Body(mode) => self.body.borrow().targets.contains(mode, &annotation.name),
                    kind => Some(targets.contains(kind, &annotation.name)),
                };
                if found == Some(false) {
                    diagnostics.push(information(
                        source,
                        annotation.range.clone(),
                        "Not present in the current request. This annotation can document an optional or generated field.",
                    ));
                }
                // An unavailable or malformed body is uncertainty, not an
                // authoring mistake. Keep annotations without per-path notices.
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
                    "@body ".to_owned(),
                    "@body(JSON) ".to_owned(),
                    "@body(XML) ".to_owned(),
                    "@Ref(".to_owned(),
                ],
            )
        } else {
            let Ok(Some((kind, scope))) = annotation_tag(&line[..tag_end]) else {
                return Vec::new();
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
            let encode = |names: &BTreeSet<String>| {
                names
                    .iter()
                    .map(|name| {
                        let name = if token.starts_with('"') {
                            serde_json::to_string(name).expect("serialize a string")
                        } else {
                            encode_name(name)
                        };
                        format!("{scope}{name}")
                    })
                    .collect()
            };
            let candidates = match kind {
                TargetKind::Body(mode) => encode(self.body.borrow().targets.names(mode)),
                kind => encode(self.targets.borrow().names(kind)),
            };
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

    #[test]
    fn semantic_overlay_colors_structure_and_preserves_markdown_prose() {
        let intelligence = intelligence();
        *intelligence.resources.borrow_mut() = ReferenceCatalog::from_workspace(
            &crate::documentation_references::tests::workspace_fixture(),
        );
        let tag = HighlightStyle {
            color: Some(gpui::rgb(0xff0000).into()),
            ..Default::default()
        };
        let target = HighlightStyle {
            color: Some(gpui::rgb(0x00ff00).into()),
            ..Default::default()
        };
        let link = HighlightStyle {
            color: Some(gpui::rgb(0x0000ff).into()),
            ..Default::default()
        };
        let source =
            "@param query.标签 **解释** @Ref(Backend.Auth.Login)\n@header \"X Trace\" `token`";
        let spans = intelligence.semantic_highlights(source, tag, target, link);
        let expected = [
            ("@param", tag),
            ("query.标签", target),
            ("@Ref(", tag),
            ("Backend.Auth.Login", link),
            (")", tag),
            ("@header", tag),
            ("\"X Trace\"", target),
        ]
        .map(|(token, style)| {
            let start = source.find(token).unwrap();
            (source_range(source, start, start + token.len()), style)
        });
        assert_eq!(spans, expected);
        intelligence.resources.borrow_mut().0.clear();
        let missing = intelligence.semantic_highlights(source, tag, target, link);
        assert_eq!(
            missing[3].1, target,
            "missing resources must not look like links"
        );
        assert!(
            intelligence
                .semantic_highlights("", tag, target, link)
                .is_empty()
        );
    }

    #[test]
    fn semantic_overlay_ignores_examples_and_handles_incomplete_references() {
        let intelligence = intelligence();
        let style = HighlightStyle::default();
        let examples = "```md\n@header Authorization @Ref(Backend.Auth.Login)\n```\n\n\
            `@param query.limit` and `@Ref(Backend.Auth.Login)`\n\n\
            \\@header Authorization example\n\n\\@Ref(Backend.Auth.Login)";
        assert!(
            intelligence
                .semantic_highlights(examples, style, style, style)
                .is_empty()
        );
        let source = "@header\n\nSee @Ref(未知";
        let spans = intelligence.semantic_highlights(source, style, style, style);
        assert_eq!(
            spans.len(),
            3,
            "incomplete tag/reference still receives syntax color"
        );
    }

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
        assert_eq!(intelligence.completion_items("@", 1).len(), 6);
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

    #[test]
    fn body_modes_resolve_default_json_and_xml_attributes() {
        let intelligence = intelligence();
        intelligence.replace_body(Some(BodyMode::Json), r#"{"user":{"name":"Ada"}}"#);
        let source = "@body /user/name Display name.\n@body(JSON) /user User object.";
        assert!(intelligence.diagnostics(source).is_empty());
        assert_eq!(
            intelligence
                .explanation(source, TargetKind::Body(BodyMode::Json), "/user/name")
                .as_deref(),
            Some("Display name.")
        );
        intelligence.replace_body(
            Some(BodyMode::Xml),
            "<request><user id=\"42\"/><user id=\"43\"/></request>",
        );
        let source = "@body(XML) /request/user/@id User identifier.";
        assert!(intelligence.diagnostics(source).is_empty());
        assert!(
            intelligence
                .diagnostics("@body /request JSON is still the default.")
                .is_empty()
        );
        assert_eq!(
            intelligence.diagnostics("@body(XML) /request/absent Missing.")[0].severity,
            Some(DiagnosticSeverity::INFORMATION)
        );
    }

    #[test]
    fn body_paths_quoting_duplicates_and_invalid_modes_are_explicit() {
        let intelligence = intelligence();
        intelligence.replace_body(
            Some(BodyMode::Json),
            r#"{"display name":"Ada","a/b":1,"~":2}"#,
        );
        let source = "@body \"/display name\" Display name.\n@body /a~1b Slash key.\n@body /~0 Tilde key.\n@body \"\" Whole body.";
        assert!(intelligence.diagnostics(source).is_empty());
        let duplicate = "@body /a~1b First.\n@body(json) /a~1b Second.";
        assert_eq!(intelligence.diagnostics(duplicate).len(), 2);
        assert_eq!(
            intelligence.explanation(duplicate, TargetKind::Body(BodyMode::Json), "/a~1b"),
            None
        );
        for source in [
            "@body(YAML) /user Unsupported.",
            "@body(XML /user Incomplete mode.",
            "@body() /user Missing mode.",
            "@body user Relative pointer.",
            "@body /bad~2escape Invalid escape.",
            "@body(XML) /user/* No wildcards.",
            "@body(XML) /user[1] No selectors.",
        ] {
            assert_eq!(intelligence.diagnostics(source).len(), 1, "{source}");
            assert!(parse(source).annotations.is_empty(), "{source}");
        }
    }

    #[test]
    fn body_completion_replaces_only_path_and_preserves_quoted_unicode() {
        let intelligence = intelligence();
        intelligence.replace_body(
            Some(BodyMode::Json),
            r#"{"user":{"name":"Ada"},"标签 空格":true}"#,
        );
        let source = "@body(JSON) /user/na Keep this explanation.";
        let items = intelligence.completion_items(source, "@body(JSON) /user/n".len());
        assert_eq!(items.len(), 1);
        let Some(CompletionTextEdit::Edit(edit)) = &items[0].text_edit else {
            panic!("edit")
        };
        assert_eq!(edit.new_text, "/user/name");
        assert_eq!(edit.range, source_range(source, 12, 20));
        assert!(
            intelligence
                .completion_items(source, source.len())
                .is_empty()
        );
        let items = intelligence.completion_items("@body ", 6);
        assert!(items.iter().any(|item| item.label == "\"/标签 空格\""));
        assert!(items.iter().any(|item| item.label == "\"\""));
        assert!(intelligence.completion_items("@body(XML) ", 11).is_empty());
    }

    #[test]
    fn unresolved_bodies_are_quiet_and_cache_tracks_mode() {
        let intelligence = intelligence();
        let source = "@body /absent Description.";
        assert!(intelligence.replace_body(Some(BodyMode::Json), "{}"));
        assert!(!intelligence.replace_body(Some(BodyMode::Json), "{}"));
        assert!(intelligence.body_source_matches("{}"));
        assert_eq!(
            intelligence.diagnostics(source)[0].severity,
            Some(DiagnosticSeverity::INFORMATION)
        );
        assert!(intelligence.replace_body(Some(BodyMode::Json), "{"));
        assert!(!intelligence.body_source_matches("{}"));
        assert!(intelligence.diagnostics(source).is_empty());
        intelligence.replace_body(Some(BodyMode::Xml), "<root>");
        assert!(
            intelligence
                .diagnostics("@body(XML) /root/absent Description.")
                .is_empty()
        );
        intelligence.replace_body(None, "{}");
        assert!(!intelligence.body_source_matches("{}"));
        assert!(intelligence.diagnostics(source).is_empty());
        assert!(intelligence.completion_items("@body ", 6).is_empty());
    }

    #[test]
    fn valid_absent_targets_are_informational_and_resolve_when_fields_appear() {
        let intelligence = intelligence();
        intelligence.replace_body(Some(BodyMode::Json), "{}");
        let source = "@param query.optional Optional parameter.\n@header X-Optional Optional header.\n@body /filter Valid JSON filter.";
        let diagnostics = intelligence.diagnostics(source);
        assert_eq!(diagnostics.len(), 3);
        assert!(diagnostics.iter().all(|diagnostic| {
            diagnostic.severity == Some(DiagnosticSeverity::INFORMATION)
                && diagnostic
                    .message
                    .starts_with("Not present in the current request.")
        }));
        assert_eq!(
            intelligence
                .explanation(source, TargetKind::Body(BodyMode::Json), "/filter")
                .as_deref(),
            Some("Valid JSON filter.")
        );
        assert!(
            intelligence
                .completion_items("@body /filter", 13)
                .is_empty()
        );
        assert!(intelligence.body_explanations(source, 0).is_empty());

        intelligence.replace_targets(Targets::new(
            ["optional".to_owned()].into_iter(),
            ["X-Optional".to_owned()].into_iter(),
        ));
        let body = r#"{"filter":{}}"#;
        intelligence.replace_body(Some(BodyMode::Json), body);
        assert!(intelligence.diagnostics(source).is_empty());
        assert_eq!(
            intelligence.body_explanations(source, body.find("filter").unwrap()),
            ["Valid JSON filter."]
        );
        // Best effort never changes the matching rules or guesses a nearby path.
        intelligence.replace_body(Some(BodyMode::Json), r#"{"filters":{}}"#);
        assert!(intelligence.body_explanations(source, 2).is_empty());
        assert_eq!(intelligence.diagnostics(source).len(), 1);
    }

    #[test]
    fn authoring_mistakes_remain_warnings_even_when_body_resolution_is_unavailable() {
        let intelligence = intelligence();
        for source in [
            "@param optional Missing query scope.",
            "@header",
            "@body filter Missing leading slash.",
            "@body /filter",
            "@body(YAML) /filter Unsupported mode.",
            "@body /bad~2path Invalid pointer escape.",
            "@body(XML) /root/* Unsupported selector.",
            "See @Ref(unfinished",
        ] {
            let diagnostics = intelligence.diagnostics(source);
            assert_eq!(diagnostics.len(), 1, "{source}");
            assert_eq!(
                diagnostics[0].severity,
                Some(DiagnosticSeverity::WARNING),
                "{source}"
            );
        }
        let source = "@body /filter First.\n@body(JSON) /filter Second.";
        let diagnostics = intelligence.diagnostics(source);
        assert_eq!(diagnostics.len(), 2);
        assert!(diagnostics.iter().all(|diagnostic| {
            diagnostic.severity == Some(DiagnosticSeverity::WARNING)
                && diagnostic.message.starts_with("Multiple explanations")
        }));
        assert!(
            intelligence
                .explanation(source, TargetKind::Body(BodyMode::Json), "/filter")
                .is_none()
        );
    }

    #[test]
    fn unresolved_resources_are_informational_until_the_catalog_resolves_them() {
        let intelligence = intelligence();
        let source = "See @Ref(Backend.Auth.Login)";
        let diagnostics = intelligence.diagnostics(source);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].severity,
            Some(DiagnosticSeverity::INFORMATION)
        );
        let references = intelligence.references(source);
        assert_eq!(references.len(), 1);
        assert_eq!(references[0].expression(source), "Backend.Auth.Login");
        *intelligence.resources.borrow_mut() = ReferenceCatalog::from_workspace(
            &crate::documentation_references::tests::workspace_fixture(),
        );
        assert!(intelligence.diagnostics(source).is_empty());
        intelligence.resources.borrow_mut().0.clear();
        assert_eq!(
            intelligence.diagnostics(source)[0].severity,
            Some(DiagnosticSeverity::INFORMATION)
        );
    }

    #[test]
    fn body_hovers_choose_innermost_field_and_never_use_duplicate_explanations() {
        let intelligence = intelligence();
        let body = r#"{"user":{"name":"Ada","age":42}}"#;
        intelligence.replace_body(Some(BodyMode::Json), body);
        let source = "@body /user User object.\n@body /user/name Display name.";
        assert_eq!(
            intelligence.body_explanations(source, body.find("name").unwrap()),
            ["Display name."]
        );
        assert_eq!(
            intelligence.body_explanations(source, body.find("Ada").unwrap()),
            ["Display name."]
        );
        assert!(
            intelligence
                .body_explanations(source, body.find("age").unwrap())
                .is_empty()
        );
        let duplicate = format!("{source}\n@body(JSON) /user/name Conflicting.");
        assert!(
            intelligence
                .body_explanations(&duplicate, body.find("name").unwrap())
                .is_empty()
        );
        let body = "<root><item id=\"1\"/><item id=\"2\"/></root>";
        intelligence.replace_body(Some(BodyMode::Xml), body);
        for (offset, _) in body.match_indices("id=") {
            assert_eq!(
                intelligence.body_explanations("@body(XML) /root/item/@id Identifier.", offset),
                ["Identifier."]
            );
        }
    }

    #[test]
    fn body_annotations_share_markdown_exclusions_and_semantic_highlights() {
        let intelligence = intelligence();
        let style = HighlightStyle::default();
        for source in [
            "```md\n@body /missing Example.\n```",
            "> @body(XML) /root Example.",
            "- @body /missing Example.",
            "\\@body /missing Example.",
            "`@body /missing Example.`",
        ] {
            assert!(intelligence.diagnostics(source).is_empty(), "{source}");
            assert!(parse(source).annotations.is_empty(), "{source}");
            assert!(
                intelligence
                    .completion_items(source, source.find("@body").unwrap() + 5)
                    .is_empty()
            );
        }
        let source = "@body(XML) /root/@id **Identifier.**";
        let spans = intelligence.semantic_highlights(source, style, style, style);
        assert_eq!(
            spans,
            [
                (source_range(source, 0, 10), style),
                (source_range(source, 11, 20), style),
            ]
        );
    }

    #[test]
    fn annotation_targets_are_literal_even_when_they_contain_resource_syntax() {
        let intelligence = intelligence();
        intelligence.replace_body(Some(BodyMode::Json), r#"{"@Ref(missing)":1}"#);
        *intelligence.resources.borrow_mut() = ReferenceCatalog::from_workspace(
            &crate::documentation_references::tests::workspace_fixture(),
        );
        for path in ["/@Ref(missing)", "\"/@Ref(missing)\""] {
            let source = format!("@body {path} Literal key. See @Ref(Backend.Auth.Login)");
            assert!(intelligence.diagnostics(&source).is_empty(), "{source}");
            let refs = intelligence.references(&source);
            assert_eq!(refs.len(), 1);
            assert_eq!(refs[0].expression(&source), "Backend.Auth.Login");
            let items = intelligence.completion_items(&source, source.find("missing").unwrap());
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].label, path);
        }
        // The same lexical rule applies to headers and query parameters, and
        // remains in effect while a quoted target is still being typed.
        for source in [
            "@header @Ref(missing) Literal header.",
            "@param query.\"@Ref(missing)\" Literal parameter.",
            "@body \"/@Ref(missing",
        ] {
            assert!(intelligence.references(source).is_empty(), "{source}");
        }
    }
}
