//! Completion, hover, and diagnostics for API Tester's constrained CSS theme
//! format.

use std::{collections::BTreeSet, ops::Range as ByteRange};

use anyhow::Result;
use gpui::{App, Context, Task, Window};
use gpui_component::{
    highlighter::Diagnostic,
    input::{CompletionProvider, HoverProvider, InputState, Rope},
};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit,
    DiagnosticSeverity, Documentation, Hover, HoverContents, MarkupContent, MarkupKind,
    NumberOrString, Position, Range, TextEdit,
};

use super::{
    ThemeError, parse_css,
    schema::{
        THEME_CLASS_PROPERTIES, THEME_CLASSES, THEME_PROPERTIES, ThemeClassPropertyKind,
        ThemeClassPropertySpec, ThemePropertyKind, ThemePropertySpec, theme_class_property,
        theme_property,
    },
};

const DIAGNOSTIC_SOURCE: &str = "API Tester theme";
const DIAGNOSTIC_CODE: &str = "api-theme-css";

#[derive(Clone, Debug, Default)]
pub(crate) struct ThemeCssIntelligence;

impl ThemeCssIntelligence {
    pub(crate) fn completion_items_for_source(
        &self,
        source: &str,
        offset: usize,
    ) -> Vec<CompletionItem> {
        let Some(context) = completion_context(source, offset) else {
            return Vec::new();
        };
        completion_items(source, context)
    }

    pub(crate) fn hover_for_source(&self, source: &str, offset: usize) -> Option<Hover> {
        hover_for_source(source, offset)
    }
}

impl CompletionProvider for ThemeCssIntelligence {
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
    }

    fn is_completion_trigger_in_text(
        &self,
        text: &Rope,
        cursor_offset: usize,
        _edit_start: usize,
        new_text: &str,
        _cx: &mut Context<InputState>,
    ) -> bool {
        if new_text.is_empty() {
            return false;
        }
        completion_context(&text.to_string(), cursor_offset).is_some()
    }
}

impl HoverProvider for ThemeCssIntelligence {
    fn hover(
        &self,
        text: &Rope,
        offset: usize,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Task<Result<Option<Hover>>> {
        Task::ready(Ok(self.hover_for_source(&text.to_string(), offset)))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ThemeCompletionKind {
    Selector,
    Property,
    ClassProperty { selector: String },
    Variable,
    Appearance,
    ThemeName,
    Color { property: String },
    ClassValue { selector: String, property: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ThemeCompletionContext {
    replace_range: ByteRange<usize>,
    prefix: String,
    kind: ThemeCompletionKind,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum CssLexicalState {
    #[default]
    Code,
    Comment,
    String {
        quote: u8,
        escaped: bool,
    },
}

/// Marks bytes which belong to CSS code rather than comments or strings.
///
/// Theme files are intentionally small, and `Vec<bool>` keeps this map compact
/// while allowing all completion and hover scans to share the same lexical
/// interpretation of the source.
#[derive(Clone, Debug)]
struct CssLexicalMap {
    code: Vec<bool>,
    state_at_end: CssLexicalState,
}

impl CssLexicalMap {
    fn through(source: &str, requested_end: usize) -> Self {
        let end = clipped_char_boundary(source, requested_end);
        let bytes = source.as_bytes();
        let mut code = vec![false; end];
        let mut state = CssLexicalState::Code;
        let mut index = 0;

        while index < end {
            match state {
                CssLexicalState::Code => {
                    if index + 1 < end && bytes[index..index + 2] == *b"/*" {
                        state = CssLexicalState::Comment;
                        index += 2;
                    } else if matches!(bytes[index], b'\'' | b'"') {
                        state = CssLexicalState::String {
                            quote: bytes[index],
                            escaped: false,
                        };
                        index += 1;
                    } else {
                        code[index] = true;
                        index += 1;
                    }
                }
                CssLexicalState::Comment => {
                    if index + 1 < end && bytes[index..index + 2] == *b"*/" {
                        state = CssLexicalState::Code;
                        index += 2;
                    } else {
                        index += 1;
                    }
                }
                CssLexicalState::String { quote, escaped } => {
                    let byte = bytes[index];
                    if escaped {
                        state = CssLexicalState::String {
                            quote,
                            escaped: false,
                        };
                    } else if byte == b'\\' {
                        state = CssLexicalState::String {
                            quote,
                            escaped: true,
                        };
                    } else if byte == quote {
                        state = CssLexicalState::Code;
                    }
                    index += 1;
                }
            }
        }

        Self {
            code,
            state_at_end: state,
        }
    }

    fn is_code_context(&self) -> bool {
        self.state_at_end == CssLexicalState::Code
    }

    fn is_code_position(&self, offset: usize) -> bool {
        if offset < self.code.len() {
            self.is_code(offset)
        } else {
            self.is_code_context()
        }
    }

    fn is_code(&self, index: usize) -> bool {
        self.code.get(index).is_some_and(|is_code| *is_code)
    }

    fn is_code_range(&self, range: ByteRange<usize>) -> bool {
        range.start < range.end
            && range.end <= self.code.len()
            && self.code[range].iter().all(|is_code| *is_code)
    }

    fn rfind_byte(&self, source: &str, byte: u8) -> Option<usize> {
        self.code
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, is_code)| {
                (*is_code && source.as_bytes()[index] == byte).then_some(index)
            })
    }

    fn rfind_any_byte(&self, source: &str, bytes: &[u8]) -> Option<usize> {
        self.code
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, is_code)| {
                (*is_code && bytes.contains(&source.as_bytes()[index])).then_some(index)
            })
    }

    fn find_byte_in(&self, source: &str, range: ByteRange<usize>, byte: u8) -> Option<usize> {
        let end = range.end.min(self.code.len());
        (range.start.min(end)..end)
            .find(|index| self.is_code(*index) && source.as_bytes()[*index] == byte)
    }

    fn rfind_pattern(&self, source: &str, pattern: &str) -> Option<usize> {
        source[..self.code.len()]
            .match_indices(pattern)
            .filter_map(|(index, _)| {
                self.is_code_range(index..index + pattern.len())
                    .then_some(index)
            })
            .last()
    }

    fn first_significant_code(&self, source: &str, range: ByteRange<usize>) -> Option<usize> {
        let end = range.end.min(self.code.len());
        source[range.start.min(end)..end]
            .char_indices()
            .find_map(|(relative, character)| {
                let index = range.start + relative;
                (self.is_code(index) && !character.is_whitespace()).then_some(index)
            })
    }
}

fn completion_context(source: &str, requested_offset: usize) -> Option<ThemeCompletionContext> {
    let offset = clipped_char_boundary(source, requested_offset);
    let lexical = CssLexicalMap::through(source, offset);
    if offset > source.len() || !lexical.is_code_context() {
        return None;
    }

    let last_open = lexical.rfind_byte(source, b'{');
    let last_close = lexical.rfind_byte(source, b'}');
    let inside_rule = last_open.is_some() && last_open > last_close;
    if !inside_rule {
        let start = last_close.map_or(0, |position| position.saturating_add(1));
        let replace_start = lexical
            .first_significant_code(source, start..offset)
            .unwrap_or(offset);
        let prefix = source[replace_start..offset].trim().to_owned();
        return (prefix.is_empty() || prefix.starts_with([':', '.'])).then_some(
            ThemeCompletionContext {
                replace_range: replace_start..offset,
                prefix,
                kind: ThemeCompletionKind::Selector,
            },
        );
    }
    let selector = selector_before_open(source, &lexical, last_open?)?;

    if let Some(var_start) = lexical
        .rfind_pattern(source, "var(")
        .filter(|var_start| !token_continues_before(source, *var_start))
    {
        let argument_start = var_start + "var(".len();
        if lexical
            .find_byte_in(source, argument_start..offset, b')')
            .is_none()
            && lexical
                .find_byte_in(source, argument_start..offset, b',')
                .is_none()
        {
            let mut replace_start = offset;
            while replace_start > argument_start {
                let Some((previous, character)) =
                    source[..replace_start].char_indices().next_back()
                else {
                    break;
                };
                if !lexical.is_code(previous) || !is_custom_property_character(character) {
                    break;
                }
                replace_start = previous;
            }
            let prefix = source[replace_start..offset].to_owned();
            if prefix.is_empty() || prefix.starts_with('-') {
                return Some(ThemeCompletionContext {
                    replace_range: replace_start..offset,
                    prefix,
                    kind: ThemeCompletionKind::Variable,
                });
            }
        }
    }

    let declaration_start = lexical
        .rfind_any_byte(source, b"{;")
        .map_or(0, |position| position.saturating_add(1));
    let content_start = lexical
        .first_significant_code(source, declaration_start..offset)
        .unwrap_or(offset);
    let content = &source[content_start..offset];

    let Some(colon) = lexical.find_byte_in(source, content_start..offset, b':') else {
        let valid_property_prefix = if selector == ":root" {
            content.is_empty()
                || (content.starts_with('-') && content.chars().all(is_custom_property_character))
        } else {
            content.is_empty()
                || content
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '-')
        };
        if valid_property_prefix {
            return Some(ThemeCompletionContext {
                replace_range: content_start..offset,
                prefix: content.to_owned(),
                kind: if selector == ":root" {
                    ThemeCompletionKind::Property
                } else {
                    ThemeCompletionKind::ClassProperty { selector }
                },
            });
        }
        return None;
    };

    let property = source[content_start..colon].trim();
    let value_start = lexical
        .first_significant_code(source, colon + 1..offset)
        .unwrap_or(offset);
    let prefix = source[value_start..offset].to_owned();
    let kind = if selector == ":root" {
        match theme_property(property)?.kind {
            ThemePropertyKind::Name => ThemeCompletionKind::ThemeName,
            ThemePropertyKind::Appearance => ThemeCompletionKind::Appearance,
            ThemePropertyKind::Color => ThemeCompletionKind::Color {
                property: property.to_owned(),
            },
        }
    } else {
        theme_class_property(&selector, property)?;
        ThemeCompletionKind::ClassValue {
            selector,
            property: property.to_owned(),
        }
    };
    Some(ThemeCompletionContext {
        replace_range: value_start..offset,
        prefix,
        kind,
    })
}

fn selector_before_open(source: &str, lexical: &CssLexicalMap, open: usize) -> Option<String> {
    std::iter::once(":root")
        .chain(THEME_CLASSES.iter().map(|class| class.selector))
        .filter_map(|selector| {
            source[..open]
                .match_indices(selector)
                .filter(|(start, _)| {
                    lexical.is_code_range(*start..*start + selector.len())
                        && !token_continues_before(source, *start)
                        && !token_continues_after(source, *start + selector.len())
                })
                .last()
                .map(|(start, _)| (start, selector))
        })
        .max_by_key(|(start, _)| *start)
        .map(|(_, selector)| selector.to_owned())
}

fn completion_items(source: &str, context: ThemeCompletionContext) -> Vec<CompletionItem> {
    match context.kind {
        ThemeCompletionKind::Selector => {
            let declared = declared_selectors(source);
            std::iter::once((
                ":root",
                "Color tokens",
                "Theme metadata and semantic color custom properties.",
            ))
            .chain(
                THEME_CLASSES
                    .iter()
                    .map(|class| (class.selector, "Native style class", class.documentation)),
            )
            .filter(|(selector, _, _)| {
                completion_matches(&context.prefix, selector)
                    && (!declared.contains(*selector) || context.prefix == *selector)
            })
            .map(|(selector, detail, documentation)| {
                let replacement = selector_rule_template(selector);
                completion_item(
                    source,
                    context.replace_range.clone(),
                    selector,
                    &replacement,
                    CompletionItemKind::CLASS,
                    detail,
                    documentation,
                )
            })
            .collect()
        }
        ThemeCompletionKind::Property => {
            let declared = declared_custom_properties(source);
            THEME_PROPERTIES
                .iter()
                .filter(|property| {
                    completion_matches(&context.prefix, property.name)
                        && (!declared.contains(property.name) || context.prefix == property.name)
                })
                .map(|property| {
                    completion_item(
                        source,
                        context.replace_range.clone(),
                        property.name,
                        &format!("{}: ", property.name),
                        CompletionItemKind::PROPERTY,
                        &format!(
                            "{} · {}",
                            if property.required {
                                "Required"
                            } else {
                                "Optional"
                            },
                            property.category.label()
                        ),
                        property.documentation,
                    )
                })
                .collect()
        }
        ThemeCompletionKind::ClassProperty { selector } => {
            let declared = declared_rule_properties(source, &selector);
            THEME_CLASS_PROPERTIES
                .iter()
                .filter(|property| {
                    property.selector == selector
                        && completion_matches(&context.prefix, property.name)
                        && (!declared.contains(property.name) || context.prefix == property.name)
                })
                .map(|property| {
                    completion_item(
                        source,
                        context.replace_range.clone(),
                        property.name,
                        &format!("{}: ", property.name),
                        CompletionItemKind::PROPERTY,
                        selector.trim_start_matches('.'),
                        property.documentation,
                    )
                })
                .collect()
        }
        ThemeCompletionKind::Variable => {
            let mut names = declared_custom_properties(source);
            names.extend(
                THEME_PROPERTIES
                    .iter()
                    .map(|property| property.name.to_owned()),
            );
            names
                .into_iter()
                .filter(|name| completion_matches(&context.prefix, name))
                .map(|name| {
                    let documentation = theme_property(&name).map_or(
                        "A helper custom property declared in this theme.",
                        |property| property.documentation,
                    );
                    completion_item(
                        source,
                        context.replace_range.clone(),
                        &name,
                        &name,
                        CompletionItemKind::VARIABLE,
                        "CSS custom property",
                        documentation,
                    )
                })
                .collect()
        }
        ThemeCompletionKind::Appearance => ["dark", "light"]
            .into_iter()
            .filter(|value| completion_matches(&context.prefix, value))
            .map(|value| {
                completion_item(
                    source,
                    context.replace_range.clone(),
                    value,
                    value,
                    CompletionItemKind::VALUE,
                    "GPUI appearance",
                    "Controls the dark or light component baseline.",
                )
            })
            .collect(),
        ThemeCompletionKind::ThemeName => {
            let value = "\"My API Tester Theme\"";
            completion_matches(&context.prefix, value)
                .then(|| {
                    completion_item(
                        source,
                        context.replace_range,
                        value,
                        value,
                        CompletionItemKind::VALUE,
                        "Quoted theme name",
                        "Theme names must be non-empty CSS strings.",
                    )
                })
                .into_iter()
                .collect()
        }
        ThemeCompletionKind::Color { property } => {
            let mut candidates = Vec::new();
            if let Some(spec) = theme_property(&property) {
                candidates.push((
                    spec.default_value,
                    spec.default_value,
                    "Default value",
                    spec.documentation,
                ));
            }
            candidates.extend([
                (
                    "#rrggbb",
                    "#000000",
                    "Hex color",
                    "Six-digit hexadecimal CSS color.",
                ),
                (
                    "rgb(…)",
                    "rgb(0 0 0)",
                    "RGB color",
                    "CSS RGB functional color.",
                ),
                (
                    "hsl(…)",
                    "hsl(0 0% 0%)",
                    "HSL color",
                    "CSS HSL functional color.",
                ),
                (
                    "var(…)",
                    "var(--api-primary)",
                    "Custom property",
                    "Reference another color property in this theme.",
                ),
            ]);
            candidates
                .into_iter()
                .filter(|(label, replacement, _, _)| {
                    completion_matches(&context.prefix, label)
                        || completion_matches(&context.prefix, replacement)
                })
                .map(|(label, replacement, detail, documentation)| {
                    completion_item(
                        source,
                        context.replace_range.clone(),
                        label,
                        replacement,
                        CompletionItemKind::COLOR,
                        detail,
                        documentation,
                    )
                })
                .collect()
        }
        ThemeCompletionKind::ClassValue { selector, property } => {
            let Some(spec) = theme_class_property(&selector, &property) else {
                return Vec::new();
            };
            class_value_candidates(spec)
                .into_iter()
                .filter(|value| completion_matches(&context.prefix, value))
                .map(|value| {
                    completion_item(
                        source,
                        context.replace_range.clone(),
                        value,
                        value,
                        CompletionItemKind::VALUE,
                        &format!("{selector} {property}"),
                        spec.documentation,
                    )
                })
                .collect()
        }
    }
}

fn selector_rule_template(selector: &str) -> String {
    let declarations = if selector == ":root" {
        THEME_PROPERTIES
            .iter()
            .map(|property| format!("    {}: {};", property.name, property.default_value))
            .collect::<Vec<_>>()
    } else {
        THEME_CLASS_PROPERTIES
            .iter()
            .filter(|property| property.selector == selector)
            .map(|property| format!("    {}: {};", property.name, property.default_value))
            .collect::<Vec<_>>()
    };
    format!("{selector} {{\n{}\n}}", declarations.join("\n"))
}

fn class_value_candidates(spec: &ThemeClassPropertySpec) -> Vec<&'static str> {
    let mut values = vec![spec.default_value];
    match spec.kind {
        ThemeClassPropertyKind::Zoom => values.extend(["100%", "1", "125%"]),
        ThemeClassPropertyKind::FontFamily => {
            if spec.selector != ".app" {
                values.push("inherit");
            }
            values.extend(["\".SystemUIFont\"", "\"Menlo\""]);
        }
        ThemeClassPropertyKind::Length if spec.name == "font-size" => {
            if spec.selector == ".app" {
                values.extend(["8px", "16px", "24px", "48px"]);
            } else {
                values.extend(["inherit", "0", "13px", "0.875rem"]);
            }
        }
        ThemeClassPropertyKind::Length => values.extend(["auto", "0", "8px", "0.5rem"]),
        ThemeClassPropertyKind::Box => values.extend(["0", "4px 8px", "0.25rem 0.5rem"]),
    }
    values.sort_unstable();
    values.dedup();
    values
}

fn completion_item(
    source: &str,
    replace_range: ByteRange<usize>,
    label: &str,
    replacement: &str,
    kind: CompletionItemKind,
    detail: &str,
    documentation: &str,
) -> CompletionItem {
    CompletionItem {
        label: label.to_owned(),
        kind: Some(kind),
        detail: Some(detail.to_owned()),
        documentation: Some(Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: documentation.to_owned(),
        })),
        text_edit: Some(CompletionTextEdit::Edit(TextEdit {
            range: source_range(source, replace_range.start, replace_range.end),
            new_text: replacement.to_owned(),
        })),
        ..Default::default()
    }
}

fn completion_matches(prefix: &str, candidate: &str) -> bool {
    prefix.is_empty()
        || candidate
            .to_ascii_lowercase()
            .starts_with(&prefix.to_ascii_lowercase())
}

fn hover_for_source(source: &str, requested_offset: usize) -> Option<Hover> {
    let offset = clipped_char_boundary(source, requested_offset);
    let lexical = CssLexicalMap::through(source, source.len());
    if !lexical.is_code_position(offset) {
        return None;
    }
    if let Some(range) = custom_property_range_at(source, offset)
        .filter(|range| lexical.is_code_range(range.clone()))
    {
        let name = &source[range.clone()];
        let markdown = if let Some(property) = theme_property(name) {
            property_hover_markdown(property)
        } else {
            let value = declaration_value(source, name, &lexical)
                .map(|value| format!("\n\nCurrent value: `{}`", markdown_inline_code(value)))
                .unwrap_or_default();
            let references = custom_property_reference_count(source, name, &lexical);
            format!(
                "`{name}`\n\nUser-defined helper custom property. Referenced {references} time(s).{value}"
            )
        };
        return Some(markdown_hover(source, range, markdown));
    }

    for class in THEME_CLASSES {
        if let Some((start, _)) = source.match_indices(class.selector).find(|(start, _)| {
            let range = *start..*start + class.selector.len();
            lexical.is_code_range(range.clone()) && range.start <= offset && offset <= range.end
        }) {
            let range = start..start + class.selector.len();
            return Some(markdown_hover(
                source,
                range,
                format!("`{}`\n\n{}", class.selector, class.documentation),
            ));
        }
    }

    if let Some(range) = token_range_at(source, offset, |character| {
        character.is_ascii_alphanumeric() || character == '-'
    })
    .filter(|range| lexical.is_code_range(range.clone()))
    {
        let through_offset = CssLexicalMap::through(source, offset);
        if let Some(open) = through_offset.rfind_byte(source, b'{')
            && let Some(selector) = selector_before_open(source, &through_offset, open)
            && let Some(property) = theme_class_property(&selector, &source[range.clone()])
        {
            return Some(markdown_hover(
                source,
                range,
                class_property_hover_markdown(property),
            ));
        }
    }

    let root_range = token_range_at(source, offset, |character| {
        character == ':' || character.is_ascii_alphabetic()
    })
    .filter(|range| lexical.is_code_range(range.clone()))?;
    let token = &source[root_range.clone()];
    if token == ":root" {
        return Some(markdown_hover(
            source,
            root_range,
            "`:root`\n\nTheme metadata and semantic color custom properties.".to_owned(),
        ));
    }
    None
}

fn class_property_hover_markdown(property: &ThemeClassPropertySpec) -> String {
    let value_type = match (property.selector, property.name) {
        (".app", "zoom") => "0.5–2 or 50%–200%",
        (".app", "font-family") => "one font family name",
        (".app", "font-size") => "8px–48px",
        (_, "font-family") => "inherit or one font family name",
        (_, "margin") => "one to four px/rem/0 lengths; negatives allowed",
        (".button", "padding") => "auto or one to four non-negative px/rem/0 lengths",
        (_, "padding") => "one to four non-negative px/rem/0 lengths",
        (_, "font-size") => "inherit or a non-negative px/rem/0 length",
        _ => "auto or a non-negative px/rem/0 length",
    };
    format!(
        "`{} {}`\n\n**Native style class · {}**\n\n{}\n\nDefault: `{}`",
        property.selector,
        property.name,
        value_type,
        property.documentation,
        property.default_value,
    )
}

fn property_hover_markdown(property: &ThemePropertySpec) -> String {
    format!(
        "`{}`\n\n**{} · {} · {}**\n\n{}\n\nDefault: `{}`",
        property.name,
        property.category.label(),
        if property.required {
            "required"
        } else {
            "optional"
        },
        match property.kind {
            ThemePropertyKind::Name => "quoted string",
            ThemePropertyKind::Appearance => "`dark` or `light`",
            ThemePropertyKind::Color => "CSS color",
        },
        property.documentation,
        property.default_value,
    )
}

fn markdown_hover(source: &str, range: ByteRange<usize>, value: String) -> Hover {
    Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value,
        }),
        range: Some(source_range(source, range.start, range.end)),
    }
}

fn markdown_inline_code(value: &str) -> String {
    value.replace('`', "\\`")
}

pub(crate) fn theme_css_diagnostics(source: &str) -> Vec<Diagnostic> {
    let Err(error) = parse_css(source) else {
        return Vec::new();
    };
    let range = diagnostic_byte_range(source, &error);
    vec![
        lsp_types::Diagnostic {
            range: source_range(source, range.start, range.end),
            severity: Some(DiagnosticSeverity::ERROR),
            code: Some(NumberOrString::String(DIAGNOSTIC_CODE.to_owned())),
            source: Some(DIAGNOSTIC_SOURCE.to_owned()),
            message: error.to_string(),
            ..Default::default()
        }
        .into(),
    ]
}

fn diagnostic_byte_range(source: &str, error: &ThemeError) -> ByteRange<usize> {
    let range = match error {
        ThemeError::UnknownProperty(property) => property_occurrence(source, property, false),
        ThemeError::DuplicateProperty(property) => property_occurrence(source, property, true),
        ThemeError::UnknownSelector(selector) => property_occurrence(source, selector, false),
        ThemeError::DuplicateSelector(selector) => property_occurrence(source, selector, true),
        ThemeError::MissingSelector(_) => property_occurrence(source, ":root", false),
        ThemeError::UnknownClassProperty { property, .. } => {
            property_occurrence(source, property, false)
        }
        ThemeError::DuplicateClassProperty { property, .. } => {
            property_occurrence(source, property, true)
        }
        ThemeError::MissingClassProperty { selector, .. } => {
            property_occurrence(source, selector, false)
        }
        ThemeError::MissingProperty(_) => property_occurrence(source, ":root", false),
        ThemeError::InvalidValue {
            property,
            line,
            column,
            ..
        }
        | ThemeError::UnresolvedVariable {
            property,
            line,
            column,
            ..
        }
        | ThemeError::ResolutionLimit {
            property,
            line,
            column,
            ..
        } => property_occurrence_on_line(source, property, *line)
            .or_else(|| property_occurrence(source, property, false))
            .or_else(|| range_at_display_location(source, *line, *column)),
        ThemeError::VariableCycle {
            chain,
            line,
            column,
        } => chain
            .split(" -> ")
            .next()
            .and_then(|property| property_occurrence(source, property, false))
            .or_else(|| range_at_display_location(source, *line, *column)),
        ThemeError::Syntax { line, column, .. } => {
            range_at_display_location(source, *line, *column)
        }
        ThemeError::ExpectedSingleRoot => {
            let lexical = CssLexicalMap::through(source, source.len());
            let roots = source
                .match_indices(":root")
                .filter(|(start, _)| {
                    lexical.is_code_range(*start..*start + 5)
                        && !token_continues_before(source, *start)
                        && !token_continues_after(source, *start + 5)
                })
                .collect::<Vec<_>>();
            roots
                .get(1)
                .or_else(|| roots.first())
                .map(|(start, _)| *start..*start + 5)
        }
        ThemeError::TooLarge { .. } => None,
    };
    visible_range(source, range.unwrap_or(0..0))
}

fn property_occurrence_on_line(
    source: &str,
    property: &str,
    line: u32,
) -> Option<ByteRange<usize>> {
    let line_index = line.saturating_sub(1) as usize;
    let line_start = if line_index == 0 {
        0
    } else {
        source
            .match_indices('\n')
            .nth(line_index.saturating_sub(1))
            .map(|(index, _)| index + 1)?
    };
    let line_end = source[line_start..]
        .find('\n')
        .map_or(source.len(), |relative| line_start + relative);
    let lexical = CssLexicalMap::through(source, line_end);
    source[line_start..line_end]
        .match_indices(property)
        .map(|(relative, _)| line_start + relative)
        .find(|start| {
            lexical.is_code_range(*start..*start + property.len())
                && !token_continues_before(source, *start)
                && !token_continues_after(source, *start + property.len())
        })
        .map(|start| start..start + property.len())
}

fn property_occurrence(source: &str, property: &str, last: bool) -> Option<ByteRange<usize>> {
    let lexical = CssLexicalMap::through(source, source.len());
    let mut occurrences = source
        .match_indices(property)
        .map(|(start, _)| start)
        .filter(|start| {
            lexical.is_code_range(*start..*start + property.len())
                && !token_continues_before(source, *start)
                && !token_continues_after(source, *start + property.len())
        });
    let start = if last {
        occurrences.last()
    } else {
        occurrences.next()
    }?;
    Some(start..start + property.len())
}

fn range_at_display_location(source: &str, line: u32, column: u32) -> Option<ByteRange<usize>> {
    let line_index = line.saturating_sub(1) as usize;
    let line_start = if line_index == 0 {
        0
    } else {
        source
            .match_indices('\n')
            .nth(line_index.saturating_sub(1))
            .map(|(index, _)| index + 1)?
    };
    let line_end = source[line_start..]
        .find('\n')
        .map_or(source.len(), |relative| line_start + relative);
    let requested_column = column.saturating_sub(1) as usize;
    let start = source[line_start..line_end]
        .char_indices()
        .nth(requested_column)
        .map_or(line_end, |(relative, _)| line_start + relative);
    Some(start..start)
}

fn visible_range(source: &str, range: ByteRange<usize>) -> ByteRange<usize> {
    let start = clipped_char_boundary(source, range.start.min(source.len()));
    let mut end = clipped_char_boundary(source, range.end.min(source.len()));
    if end <= start {
        end = source[start..]
            .chars()
            .next()
            .map_or(start, |character| start + character.len_utf8());
    }
    start..end
}

fn declared_custom_properties(source: &str) -> BTreeSet<String> {
    let mut declarations = BTreeSet::new();
    let lexical = CssLexicalMap::through(source, source.len());
    let mut index = 0;
    while index + 2 <= source.len() {
        if source[index..].starts_with("--") && lexical.is_code_range(index..index + 2) {
            let start = index;
            index += 2;
            while index < source.len()
                && lexical.is_code(index)
                && source[index..]
                    .chars()
                    .next()
                    .is_some_and(is_custom_property_character)
            {
                index += source[index..]
                    .chars()
                    .next()
                    .expect("checked above")
                    .len_utf8();
            }
            let name = &source[start..index];
            if lexical
                .first_significant_code(source, index..source.len())
                .is_some_and(|next| source.as_bytes()[next] == b':')
            {
                declarations.insert(name.to_owned());
            }
            continue;
        }
        index += source[index..]
            .chars()
            .next()
            .expect("index is within source")
            .len_utf8();
    }
    declarations
}

fn declared_selectors(source: &str) -> BTreeSet<String> {
    let lexical = CssLexicalMap::through(source, source.len());
    std::iter::once(":root")
        .chain(THEME_CLASSES.iter().map(|class| class.selector))
        .filter(|selector| {
            source.match_indices(selector).any(|(start, _)| {
                let end = start + selector.len();
                lexical.is_code_range(start..end)
                    && !token_continues_before(source, start)
                    && !token_continues_after(source, end)
                    && lexical
                        .first_significant_code(source, end..source.len())
                        .is_some_and(|next| source.as_bytes()[next] == b'{')
            })
        })
        .map(str::to_owned)
        .collect()
}

fn declared_rule_properties(source: &str, selector: &str) -> BTreeSet<String> {
    let lexical = CssLexicalMap::through(source, source.len());
    let Some(selector_start) = source.match_indices(selector).find_map(|(start, _)| {
        lexical
            .is_code_range(start..start + selector.len())
            .then_some(start)
    }) else {
        return BTreeSet::new();
    };
    let Some(open) =
        lexical.find_byte_in(source, selector_start + selector.len()..source.len(), b'{')
    else {
        return BTreeSet::new();
    };
    let close = lexical
        .find_byte_in(source, open + 1..source.len(), b'}')
        .unwrap_or(source.len());
    THEME_CLASS_PROPERTIES
        .iter()
        .filter(|property| property.selector == selector)
        .filter(|property| {
            source[open + 1..close]
                .match_indices(property.name)
                .any(|(relative, _)| {
                    let start = open + 1 + relative;
                    let end = start + property.name.len();
                    lexical.is_code_range(start..end)
                        && !token_continues_before(source, start)
                        && !token_continues_after(source, end)
                        && lexical
                            .first_significant_code(source, end..close)
                            .is_some_and(|next| source.as_bytes()[next] == b':')
                })
        })
        .map(|property| property.name.to_owned())
        .collect()
}

fn declaration_value<'a>(source: &'a str, name: &str, lexical: &CssLexicalMap) -> Option<&'a str> {
    for (start, _) in source.match_indices(name) {
        let end = start + name.len();
        if !lexical.is_code_range(start..end)
            || token_continues_before(source, start)
            || token_continues_after(source, end)
        {
            continue;
        }
        let Some(colon) = lexical.first_significant_code(source, end..source.len()) else {
            continue;
        };
        if source.as_bytes()[colon] != b':' {
            continue;
        }
        let value_start = lexical
            .first_significant_code(source, colon + 1..source.len())
            .unwrap_or(colon + 1);
        let value_end = lexical
            .find_byte_in(source, value_start..source.len(), b';')
            .into_iter()
            .chain(lexical.find_byte_in(source, value_start..source.len(), b'}'))
            .min()
            .unwrap_or(source.len());
        return Some(source[value_start..value_end].trim());
    }
    None
}

fn custom_property_reference_count(source: &str, name: &str, lexical: &CssLexicalMap) -> usize {
    source
        .match_indices("var(")
        .filter(|(var_start, _)| lexical.is_code_range(*var_start..*var_start + "var(".len()))
        .filter(|(var_start, _)| !token_continues_before(source, *var_start))
        .filter(|(var_start, _)| {
            let argument_start = *var_start + "var(".len();
            let Some(name_start) =
                lexical.first_significant_code(source, argument_start..source.len())
            else {
                return false;
            };
            let name_end = name_start + name.len();
            name_end <= source.len()
                && source[name_start..].starts_with(name)
                && lexical.is_code_range(name_start..name_end)
                && !token_continues_after(source, name_end)
        })
        .count()
}

fn token_continues_before(source: &str, start: usize) -> bool {
    source[..start]
        .chars()
        .next_back()
        .is_some_and(is_custom_property_character)
}

fn token_continues_after(source: &str, end: usize) -> bool {
    source[end..]
        .chars()
        .next()
        .is_some_and(is_custom_property_character)
}

fn custom_property_range_at(source: &str, requested_offset: usize) -> Option<ByteRange<usize>> {
    let offset = clipped_char_boundary(source, requested_offset);
    let mut start = offset;
    while start > 0 {
        let previous = source[..start].char_indices().next_back()?;
        if !is_custom_property_character(previous.1) {
            break;
        }
        start = previous.0;
    }
    let mut end = offset;
    while end < source.len() {
        let character = source[end..].chars().next()?;
        if !is_custom_property_character(character) {
            break;
        }
        end += character.len_utf8();
    }
    (start < end && source[start..end].starts_with("--")).then_some(start..end)
}

fn token_range_at(
    source: &str,
    requested_offset: usize,
    predicate: impl Fn(char) -> bool,
) -> Option<ByteRange<usize>> {
    let offset = clipped_char_boundary(source, requested_offset);
    let mut start = offset;
    while start > 0 {
        let previous = source[..start].char_indices().next_back()?;
        if !predicate(previous.1) {
            break;
        }
        start = previous.0;
    }
    let mut end = offset;
    while end < source.len() {
        let character = source[end..].chars().next()?;
        if !predicate(character) {
            break;
        }
        end += character.len_utf8();
    }
    (start < end).then_some(start..end)
}

fn is_custom_property_character(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '-' | '_')
}

fn source_range(source: &str, start: usize, end: usize) -> Range {
    Range::new(source_position(source, start), source_position(source, end))
}

// gpui-component 0.5.1 interprets LSP columns as Unicode scalar indices.
fn source_position(source: &str, requested_offset: usize) -> Position {
    let offset = clipped_char_boundary(source, requested_offset);
    let prefix = &source[..offset];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() as u32;
    let column = prefix
        .rsplit_once('\n')
        .map_or(prefix, |(_, line)| line)
        .chars()
        .count() as u32;
    Position::new(line, column)
}

fn clipped_char_boundary(source: &str, requested_offset: usize) -> usize {
    let mut offset = requested_offset.min(source.len());
    while !source.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::CompletionTextEdit;

    fn labels(source: &str) -> Vec<String> {
        ThemeCssIntelligence
            .completion_items_for_source(source, source.len())
            .into_iter()
            .map(|item| item.label)
            .collect()
    }

    #[test]
    fn schema_is_unique_documented_and_matches_the_bundled_template() {
        let mut names = BTreeSet::new();
        for property in THEME_PROPERTIES {
            assert!(names.insert(property.name), "duplicate {}", property.name);
            assert!(!property.documentation.trim().is_empty());
            assert!(
                super::super::bundled_css().contains(property.name),
                "template is missing {}",
                property.name
            );
        }
        let mut selectors = BTreeSet::new();
        for class in THEME_CLASSES {
            assert!(
                selectors.insert(class.selector),
                "duplicate {}",
                class.selector
            );
            assert!(!class.documentation.trim().is_empty());
            assert!(
                super::super::bundled_css().contains(class.selector),
                "template is missing {}",
                class.selector
            );
        }
        let mut class_properties = BTreeSet::new();
        for property in THEME_CLASS_PROPERTIES {
            assert!(
                class_properties.insert((property.selector, property.name)),
                "duplicate {} {}",
                property.selector,
                property.name
            );
            assert!(!property.documentation.trim().is_empty());
        }
        assert!(parse_css(super::super::bundled_css()).is_ok());
    }

    #[test]
    fn completes_partial_properties_and_excludes_existing_declarations() {
        let source = ":root {\n    --api-surface: #000;\n    --api-pri";
        let items = ThemeCssIntelligence.completion_items_for_source(source, source.len());
        assert!(items.iter().any(|item| item.label == "--api-primary"));
        assert!(!items.iter().any(|item| item.label == "--api-surface"));
        let edit = items
            .iter()
            .find(|item| item.label == "--api-primary")
            .and_then(|item| item.text_edit.as_ref())
            .unwrap();
        let CompletionTextEdit::Edit(edit) = edit else {
            panic!("expected a text edit")
        };
        assert_eq!(edit.range.start.line, 2);
        assert_eq!(edit.range.start.character, 4);
    }

    #[test]
    fn completes_declared_helper_variables_and_appearance_values() {
        let helper = ":root { --brand: #fff; --api-primary: var(--br";
        assert!(labels(helper).contains(&"--brand".to_owned()));
        assert_eq!(labels(":root { --api-appearance: d"), ["dark"]);
    }

    #[test]
    fn suppresses_completion_in_comments_and_strings() {
        assert!(labels("/* --api-pri").is_empty());
        assert!(labels(":root { --api-theme-name: \"--api-pri").is_empty());
    }

    #[test]
    fn completes_class_selectors_properties_and_values() {
        let selector_items = ThemeCssIntelligence.completion_items_for_source(".bu", ".bu".len());
        let button_selector = selector_items
            .iter()
            .find(|item| item.label == ".button")
            .expect("button selector completion");
        let CompletionTextEdit::Edit(button_edit) = button_selector
            .text_edit
            .as_ref()
            .expect("selector text edit")
        else {
            panic!("expected a text edit")
        };
        for property in THEME_CLASS_PROPERTIES
            .iter()
            .filter(|property| property.selector == ".button")
        {
            assert!(
                button_edit
                    .new_text
                    .contains(&format!("{}: {};", property.name, property.default_value)),
                "selector scaffold is missing {}",
                property.name
            );
        }

        let properties = labels(".button {\n    bor");
        assert!(properties.contains(&"border-radius".to_owned()));
        assert!(!properties.contains(&"zoom".to_owned()));

        let values = labels(".app {\n    zoom: 1");
        assert!(values.contains(&"1".to_owned()));
        assert!(values.contains(&"100%".to_owned()));

        let font_values = labels(".button {\n    font-family: in");
        assert!(font_values.contains(&"inherit".to_owned()));

        let app_font_sizes = labels(".app {\n    font-size: ");
        assert!(app_font_sizes.contains(&"16px".to_owned()));
        assert!(!app_font_sizes.contains(&"auto".to_owned()));
        assert!(!app_font_sizes.contains(&"inherit".to_owned()));
        assert!(!app_font_sizes.contains(&"0.5rem".to_owned()));

        let button_widths = labels(".button {\n    width: ");
        assert!(button_widths.contains(&"auto".to_owned()));
        assert!(!button_widths.contains(&"inherit".to_owned()));

        let editor_font_sizes = labels(".editor {\n    font-size: ");
        assert!(editor_font_sizes.contains(&"inherit".to_owned()));
        assert!(!editor_font_sizes.contains(&"auto".to_owned()));
    }

    #[test]
    fn hover_documents_native_classes_and_properties() {
        let source = "/* 😀 */ .button { padding: 0; }";
        let selector_start = source.find(".button").unwrap();
        let selector_hover = ThemeCssIntelligence
            .hover_for_source(source, selector_start + 2)
            .unwrap();
        let HoverContents::Markup(selector_markup) = selector_hover.contents else {
            panic!("expected markdown hover")
        };
        assert!(selector_markup.value.contains("native buttons"));

        let property_start = source.find("padding").unwrap();
        let property_hover = ThemeCssIntelligence
            .hover_for_source(source, property_start + 2)
            .unwrap();
        let property_range = property_hover.range.unwrap();
        assert_eq!(property_range.start.character, 18);
        assert_eq!(property_range.end.character, 25);
        let HoverContents::Markup(property_markup) = property_hover.contents else {
            panic!("expected markdown hover")
        };
        assert!(
            property_markup
                .value
                .contains("Spacing inside every button")
        );
    }

    #[test]
    fn class_diagnostics_point_to_the_correct_repeated_property_name() {
        let source = super::super::bundled_css().replace("font-size: inherit;", "font-size: 12vh;");
        let diagnostics = theme_css_diagnostics(&source);
        assert_eq!(diagnostics.len(), 1);
        let line = diagnostics[0].range.start.line as usize;
        assert!(
            source
                .lines()
                .nth(line)
                .is_some_and(|line| line.contains("font-size: 12vh"))
        );
    }

    #[test]
    fn missing_class_property_diagnostic_ignores_selectors_in_comments() {
        let source = super::super::bundled_css().replace("    gap: auto;\n", "");
        let diagnostics = theme_css_diagnostics(&source);
        assert_eq!(diagnostics.len(), 1);
        let line = diagnostics[0].range.start.line as usize;
        assert_eq!(source.lines().nth(line).map(str::trim), Some(".button {"));
    }

    #[test]
    fn ignores_declaration_delimiters_and_properties_in_comments_and_strings() {
        let source = concat!(
            ":root {\n",
            "    --api-theme-name: \"fake } ; --api-primary: #fff\";\n",
            "    /* } ; --api-selection: #fff; */\n",
            "    --api-",
        );
        let completions = labels(source);

        assert!(completions.contains(&"--api-primary".to_owned()));
        assert!(completions.contains(&"--api-selection".to_owned()));
    }

    #[test]
    fn variable_completion_only_reads_real_declarations() {
        let source = concat!(
            ":root {\n",
            "    /* --comment-only: #fff; var(--wrong-context */\n",
            "    --api-theme-name: \"--string-only: #fff\";\n",
            "    --real-helper: #fff;\n",
            "    --api-primary: var(--",
        );
        let completions = labels(source);

        assert!(completions.contains(&"--real-helper".to_owned()));
        assert!(!completions.contains(&"--comment-only".to_owned()));
        assert!(!completions.contains(&"--string-only".to_owned()));
    }

    #[test]
    fn hover_uses_real_helper_declarations_and_references_only() {
        let source = concat!(
            "/* --brand: #bad; var(--brand) */\n",
            ":root {\n",
            "    --api-theme-name: \"var(--brand)\";\n",
            "    --brand: #123456;\n",
            "    --api-primary: var(--brand);\n",
            "}",
        );
        let reference = source.rfind("--brand").unwrap();
        let hover = ThemeCssIntelligence
            .hover_for_source(source, reference + 3)
            .unwrap();
        let HoverContents::Markup(markup) = hover.contents else {
            panic!("expected markdown hover")
        };

        assert!(markup.value.contains("Current value: `#123456`"));
        assert!(markup.value.contains("Referenced 1 time(s)"));
    }

    #[test]
    fn suppresses_hover_for_property_like_text_in_comments_and_strings() {
        let source = concat!(
            "/* --comment-only */\n",
            ":root { --api-theme-name: \"--string-only\"; }",
        );
        let comment = source.find("--comment-only").unwrap();
        let string = source.find("--string-only").unwrap();

        assert!(
            ThemeCssIntelligence
                .hover_for_source(source, comment + 3)
                .is_none()
        );
        assert!(
            ThemeCssIntelligence
                .hover_for_source(source, string + 3)
                .is_none()
        );
    }

    #[test]
    fn hover_documents_supported_properties_with_exact_unicode_safe_range() {
        let source = "/* 😀 */ :root { --api-selection: #fff; }";
        let start = source.find("--api-selection").unwrap();
        let hover = ThemeCssIntelligence
            .hover_for_source(source, start + 4)
            .unwrap();
        let range = hover.range.unwrap();
        assert_eq!(range.start.character, 16);
        assert_eq!(range.end.character, 31);
        let HoverContents::Markup(markup) = hover.contents else {
            panic!("expected markdown hover")
        };
        assert!(markup.value.contains("selected rows"));
        assert!(markup.value.contains("required"));
    }

    #[test]
    fn parser_failures_become_visible_editor_diagnostics() {
        let source = ":root { --api-typo: #fff; }";
        let diagnostics = theme_css_diagnostics(source);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].severity,
            gpui_component::highlighter::DiagnosticSeverity::Error
        );
        assert!(diagnostics[0].message.contains("--api-typo"));
        assert!(diagnostics[0].range.start < diagnostics[0].range.end);
    }
}
