use std::collections::BTreeMap;

use cssparser::{
    BasicParseError, Delimiter, ParseError, ParseErrorKind, Parser, ParserInput, SourceLocation,
    ToCss, Token,
};
use gpui::{Hsla, Rgba};
use gpui_component::ThemeMode;
use thiserror::Error;

use super::palette::ApiTheme;

pub const BUILTIN_THEME_CSS: &str = include_str!("../../assets/themes/api-tester-dark.css");
const MAX_THEME_BYTES: usize = 256 * 1024;
const MAX_RESOLUTION_DEPTH: usize = 64;
const MAX_VAR_SUBSTITUTIONS: usize = 4_096;
const MAX_RESOLVED_VALUE_BYTES: usize = 64 * 1024;
const MAX_TOTAL_RESOLUTION_WORK: usize = 4 * 1024 * 1024;

const REQUIRED_COLOR_PROPERTIES: &[&str] = &[
    "--api-surface",
    "--api-surface-lowest",
    "--api-surface-low",
    "--api-surface-container",
    "--api-surface-high",
    "--api-surface-highest",
    "--api-foreground",
    "--api-muted-foreground",
    "--api-outline",
    "--api-primary",
    "--api-primary-hover",
    "--api-primary-active",
    "--api-primary-foreground",
    "--api-selection",
    "--api-danger",
    "--api-danger-foreground",
    "--api-warning",
    "--api-warning-foreground",
    "--api-success",
    "--api-success-foreground",
    "--api-info",
    "--api-info-foreground",
    "--api-red",
    "--api-green",
    "--api-blue",
    "--api-yellow",
    "--api-magenta",
    "--api-cyan",
];

const COLOR_PROPERTIES: &[&str] = &[
    "--api-surface",
    "--api-surface-lowest",
    "--api-surface-low",
    "--api-surface-container",
    "--api-surface-high",
    "--api-surface-highest",
    "--api-foreground",
    "--api-muted-foreground",
    "--api-outline",
    "--api-primary",
    "--api-primary-hover",
    "--api-primary-active",
    "--api-primary-foreground",
    "--api-selection",
    "--api-secondary-active",
    "--api-secondary-foreground",
    "--api-danger",
    "--api-danger-hover",
    "--api-danger-active",
    "--api-danger-foreground",
    "--api-warning",
    "--api-warning-hover",
    "--api-warning-active",
    "--api-warning-foreground",
    "--api-success",
    "--api-success-hover",
    "--api-success-active",
    "--api-success-foreground",
    "--api-info",
    "--api-info-hover",
    "--api-info-active",
    "--api-info-foreground",
    "--api-red",
    "--api-red-light",
    "--api-green",
    "--api-green-light",
    "--api-blue",
    "--api-blue-light",
    "--api-yellow",
    "--api-yellow-light",
    "--api-magenta",
    "--api-magenta-light",
    "--api-cyan",
    "--api-cyan-light",
    "--api-editor-background",
    "--api-editor-foreground",
    "--api-editor-active-line",
    "--api-editor-line-number",
    "--api-editor-active-line-number",
    "--api-syntax-property",
    "--api-syntax-string",
    "--api-syntax-number",
    "--api-syntax-boolean",
    "--api-syntax-keyword",
    "--api-syntax-comment",
    "--api-syntax-punctuation",
    "--api-syntax-variable",
    "--api-syntax-type",
    "--api-syntax-function",
];

/// A deterministic error produced while parsing the API Tester CSS contract.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ThemeCssError {
    #[error("theme CSS exceeds the {limit}-byte limit")]
    TooLarge { limit: usize },
    #[error("theme CSS must contain exactly one :root rule")]
    ExpectedSingleRoot,
    #[error("CSS syntax error at {line}:{column}: {message}")]
    Syntax {
        line: u32,
        column: u32,
        message: String,
    },
    #[error("unsupported theme property `{0}`")]
    UnknownProperty(String),
    #[error("theme property `{0}` is declared more than once")]
    DuplicateProperty(String),
    #[error("missing required theme property `{0}`")]
    MissingProperty(String),
    #[error("invalid value for `{property}` at {line}:{column}: {message} (`{value}`)")]
    InvalidValue {
        property: String,
        value: String,
        message: String,
        line: u32,
        column: u32,
    },
    #[error("`{property}` at {line}:{column} references undefined variable `{variable}`")]
    UnresolvedVariable {
        property: String,
        variable: String,
        line: u32,
        column: u32,
    },
    #[error("cyclic theme variables at {line}:{column}: {chain}")]
    VariableCycle {
        chain: String,
        line: u32,
        column: u32,
    },
    #[error(
        "theme value resolution for `{property}` at {line}:{column} exceeded the {resource} limit of {limit}"
    )]
    ResolutionLimit {
        property: String,
        resource: &'static str,
        limit: usize,
        line: u32,
        column: u32,
    },
}

#[derive(Clone, Debug)]
struct RawDeclaration {
    value: String,
    location: SourceLocation,
}

type RawDeclarations = BTreeMap<String, RawDeclaration>;

/// Parse a complete API Tester theme from one CSS `:root` rule.
pub fn parse_theme_css(source: &str) -> Result<ApiTheme, ThemeCssError> {
    if source.len() > MAX_THEME_BYTES {
        return Err(ThemeCssError::TooLarge {
            limit: MAX_THEME_BYTES,
        });
    }

    let declarations = parse_root(source)?;
    for required in ["--api-theme-name", "--api-appearance"] {
        if !declarations.contains_key(required) {
            return Err(ThemeCssError::MissingProperty(required.to_owned()));
        }
    }
    for required in REQUIRED_COLOR_PROPERTIES {
        if !declarations.contains_key(*required) {
            return Err(ThemeCssError::MissingProperty((*required).to_owned()));
        }
    }

    let mut resolver = Resolver::new(&declarations);
    let resolved_name = resolver.resolve_property("--api-theme-name")?;
    let name = parse_theme_name(&resolved_name, declarations["--api-theme-name"].location)?;
    let resolved_appearance = resolver.resolve_property("--api-appearance")?;
    let mode = parse_appearance(
        &resolved_appearance,
        declarations["--api-appearance"].location,
    )?;

    let mut colors = BTreeMap::new();
    for property in COLOR_PROPERTIES {
        if !declarations.contains_key(*property) {
            continue;
        }
        let resolved = resolver.resolve_property(property)?;
        let (line, column) = display_location(declarations[*property].location);
        let parsed =
            csscolorparser::parse(&resolved).map_err(|error| ThemeCssError::InvalidValue {
                property: (*property).to_owned(),
                value: resolved.clone(),
                message: error.to_string(),
                line,
                column,
            })?;
        if ![parsed.r, parsed.g, parsed.b, parsed.a]
            .into_iter()
            .all(f32::is_finite)
        {
            return Err(ThemeCssError::InvalidValue {
                property: (*property).to_owned(),
                value: resolved,
                message: "color channels must be finite".to_owned(),
                line,
                column,
            });
        }
        colors.insert(
            (*property).to_owned(),
            Hsla::from(Rgba {
                r: parsed.r.clamp(0.0, 1.0),
                g: parsed.g.clamp(0.0, 1.0),
                b: parsed.b.clamp(0.0, 1.0),
                a: parsed.a.clamp(0.0, 1.0),
            }),
        );
    }

    Ok(ApiTheme::from_resolved(name, mode, colors))
}

fn parse_root(source: &str) -> Result<RawDeclarations, ThemeCssError> {
    let mut input = ParserInput::new(source);
    let mut parser = Parser::new(&mut input);
    if parser.is_exhausted() {
        return Err(ThemeCssError::ExpectedSingleRoot);
    }

    parser
        .expect_colon()
        .map_err(|error| basic_error(error, "expected `:root`"))?;
    let selector = parser
        .expect_ident_cloned()
        .map_err(|error| basic_error(error, "expected `root`"))?;
    if !selector.eq_ignore_ascii_case("root") {
        return Err(ThemeCssError::ExpectedSingleRoot);
    }
    parser
        .expect_curly_bracket_block()
        .map_err(|error| basic_error(error, "expected the :root declaration block"))?;
    let declarations = parser
        .parse_nested_block(parse_declarations)
        .map_err(|error| parse_error(error, "invalid :root declaration block"))?;

    if !parser.is_exhausted() {
        return Err(ThemeCssError::ExpectedSingleRoot);
    }
    Ok(declarations)
}

fn parse_declarations<'i, 't>(
    input: &mut Parser<'i, 't>,
) -> Result<RawDeclarations, ParseError<'i, ThemeCssError>> {
    let mut declarations = BTreeMap::new();
    loop {
        while input.try_parse(|input| input.expect_semicolon()).is_ok() {}
        if input.is_exhausted() {
            break;
        }

        let property = input.expect_ident_cloned()?.to_string();
        if !is_allowed_property(&property) {
            return Err(input.new_custom_error(ThemeCssError::UnknownProperty(property)));
        }
        input.expect_colon()?;
        let location = input.current_source_location();
        let value = input.parse_until_before(
            Delimiter::Semicolon,
            |value| -> Result<String, ParseError<'i, ThemeCssError>> {
                let start = value.position();
                // Consume nested blocks iteratively. They are validated later by
                // the bounded resolver, avoiding cssparser's recursive
                // `expect_no_error_token` walk on attacker-controlled input.
                while value.next_including_whitespace_and_comments().is_ok() {}
                let raw = value.slice_from(start).trim().to_owned();
                if raw.is_empty() {
                    let (line, column) = display_location(location);
                    return Err(value.new_custom_error(ThemeCssError::InvalidValue {
                        property: property.clone(),
                        value: raw,
                        message: "value cannot be empty".to_owned(),
                        line,
                        column,
                    }));
                }
                Ok(raw)
            },
        )?;

        if declarations
            .insert(property.clone(), RawDeclaration { value, location })
            .is_some()
        {
            return Err(input.new_custom_error(ThemeCssError::DuplicateProperty(property)));
        }
        if !input.is_exhausted() {
            input.expect_semicolon()?;
        }
    }
    Ok(declarations)
}

fn is_known_api_property(property: &str) -> bool {
    matches!(property, "--api-theme-name" | "--api-appearance")
        || COLOR_PROPERTIES.contains(&property)
}

fn is_allowed_property(property: &str) -> bool {
    property.starts_with("--")
        && (!property.starts_with("--api-") || is_known_api_property(property))
}

fn parse_theme_name(value: &str, location: SourceLocation) -> Result<String, ThemeCssError> {
    let mut input = ParserInput::new(value);
    let mut parser = Parser::new(&mut input);
    let name = parser
        .expect_string_cloned()
        .map_err(|error| basic_value_error("--api-theme-name", value, location, error))?
        .to_string();
    parser
        .expect_exhausted()
        .map_err(|error| basic_value_error("--api-theme-name", value, location, error))?;
    if name.trim().is_empty() {
        let (line, column) = display_location(location);
        return Err(ThemeCssError::InvalidValue {
            property: "--api-theme-name".to_owned(),
            value: value.to_owned(),
            message: "theme name cannot be empty".to_owned(),
            line,
            column,
        });
    }
    Ok(name)
}

fn parse_appearance(value: &str, location: SourceLocation) -> Result<ThemeMode, ThemeCssError> {
    let mut input = ParserInput::new(value);
    let mut parser = Parser::new(&mut input);
    let appearance = parser
        .expect_ident_cloned()
        .map_err(|error| basic_value_error("--api-appearance", value, location, error))?;
    parser
        .expect_exhausted()
        .map_err(|error| basic_value_error("--api-appearance", value, location, error))?;
    if appearance.eq_ignore_ascii_case("dark") {
        Ok(ThemeMode::Dark)
    } else if appearance.eq_ignore_ascii_case("light") {
        Ok(ThemeMode::Light)
    } else {
        let (line, column) = display_location(location);
        Err(ThemeCssError::InvalidValue {
            property: "--api-appearance".to_owned(),
            value: value.to_owned(),
            message: "expected `dark` or `light`".to_owned(),
            line,
            column,
        })
    }
}

struct Resolver<'a> {
    declarations: &'a RawDeclarations,
    cache: BTreeMap<String, String>,
    stack: Vec<String>,
    substitutions: usize,
    total_work: usize,
}

impl<'a> Resolver<'a> {
    fn new(declarations: &'a RawDeclarations) -> Self {
        Self {
            declarations,
            cache: BTreeMap::new(),
            stack: Vec::new(),
            substitutions: 0,
            total_work: 0,
        }
    }

    fn resolve_property(&mut self, property: &str) -> Result<String, ThemeCssError> {
        if let Some(resolved) = self.cache.get(property) {
            return Ok(resolved.clone());
        }
        if let Some(index) = self.stack.iter().position(|item| item == property) {
            let mut cycle = self.stack[index..].to_vec();
            cycle.push(property.to_owned());
            let (line, column) = self.property_location(property);
            return Err(ThemeCssError::VariableCycle {
                chain: cycle.join(" -> "),
                line,
                column,
            });
        }
        if self.stack.len() >= MAX_RESOLUTION_DEPTH {
            return Err(self.limit_error(property, "variable nesting", MAX_RESOLUTION_DEPTH));
        }

        let declaration = self
            .declarations
            .get(property)
            .ok_or_else(|| ThemeCssError::MissingProperty(property.to_owned()))?;
        let raw = declaration.value.clone();
        self.stack.push(property.to_owned());
        let result = self.resolve_raw_value(&raw, property, 0);
        self.stack.pop();
        let resolved = result?;
        self.cache.insert(property.to_owned(), resolved.clone());
        Ok(resolved)
    }

    fn resolve_raw_value(
        &mut self,
        raw: &str,
        owner: &str,
        nesting_depth: usize,
    ) -> Result<String, ThemeCssError> {
        self.charge_work(owner, raw.len())?;
        let mut input = ParserInput::new(raw);
        let mut parser = Parser::new(&mut input);
        self.serialize_resolved(&mut parser, owner, nesting_depth)
            .map_err(|error| parse_error(error, "invalid custom-property value"))
    }

    fn serialize_resolved<'i, 't>(
        &mut self,
        input: &mut Parser<'i, 't>,
        owner: &str,
        nesting_depth: usize,
    ) -> Result<String, ParseError<'i, ThemeCssError>> {
        let mut output = String::new();
        while !input.is_exhausted() {
            let token = input.next_including_whitespace_and_comments()?.clone();
            if token.is_parse_error() {
                let (line, column) = self.property_location(owner);
                return Err(input.new_custom_error(ThemeCssError::InvalidValue {
                    property: owner.to_owned(),
                    value: String::new(),
                    message: "malformed CSS token".to_owned(),
                    line,
                    column,
                }));
            }

            match token {
                Token::WhiteSpace(_) | Token::Comment(_) => {
                    self.push_space(&mut output, owner)
                        .map_err(|error| input.new_custom_error(error))?;
                }
                Token::Function(ref name) if name.eq_ignore_ascii_case("var") => {
                    self.enter_nesting(owner, nesting_depth)
                        .map_err(|error| input.new_custom_error(error))?;
                    self.record_substitution(owner)
                        .map_err(|error| input.new_custom_error(error))?;
                    let resolved = input.parse_nested_block(|nested| {
                        self.resolve_var(nested, owner, nesting_depth + 1)
                    })?;
                    // CSS custom properties substitute component-value tokens.
                    // Separating the replacement prevents adjacent source tokens
                    // from being re-tokenized into another value (`#fff` + `f`
                    // must not silently become the valid color `#ffff`).
                    self.push_space(&mut output, owner)
                        .map_err(|error| input.new_custom_error(error))?;
                    self.append(&mut output, &resolved, owner)
                        .map_err(|error| input.new_custom_error(error))?;
                    self.push_space(&mut output, owner)
                        .map_err(|error| input.new_custom_error(error))?;
                }
                Token::Function(_) => {
                    self.enter_nesting(owner, nesting_depth)
                        .map_err(|error| input.new_custom_error(error))?;
                    let mut serialized = String::new();
                    token
                        .to_css(&mut serialized)
                        .expect("writing CSS to a String cannot fail");
                    self.append(&mut output, &serialized, owner)
                        .map_err(|error| input.new_custom_error(error))?;
                    let nested = input.parse_nested_block(|nested| {
                        self.serialize_resolved(nested, owner, nesting_depth + 1)
                    })?;
                    self.append(&mut output, &nested, owner)
                        .map_err(|error| input.new_custom_error(error))?;
                    self.append(&mut output, ")", owner)
                        .map_err(|error| input.new_custom_error(error))?;
                }
                Token::ParenthesisBlock => {
                    self.enter_nesting(owner, nesting_depth)
                        .map_err(|error| input.new_custom_error(error))?;
                    self.append(&mut output, "(", owner)
                        .map_err(|error| input.new_custom_error(error))?;
                    let nested = input.parse_nested_block(|nested| {
                        self.serialize_resolved(nested, owner, nesting_depth + 1)
                    })?;
                    self.append(&mut output, &nested, owner)
                        .map_err(|error| input.new_custom_error(error))?;
                    self.append(&mut output, ")", owner)
                        .map_err(|error| input.new_custom_error(error))?;
                }
                Token::SquareBracketBlock
                | Token::CurlyBracketBlock
                | Token::UnquotedUrl(_)
                | Token::AtKeyword(_) => {
                    let (line, column) = self.property_location(owner);
                    return Err(input.new_custom_error(ThemeCssError::InvalidValue {
                        property: owner.to_owned(),
                        value: String::new(),
                        message: "blocks, URLs, and at-keywords are not valid theme values"
                            .to_owned(),
                        line,
                        column,
                    }));
                }
                _ => {
                    let mut serialized = String::new();
                    token
                        .to_css(&mut serialized)
                        .expect("writing CSS to a String cannot fail");
                    self.append(&mut output, &serialized, owner)
                        .map_err(|error| input.new_custom_error(error))?;
                }
            }
        }
        Ok(output.trim().to_owned())
    }

    fn resolve_var<'i, 't>(
        &mut self,
        input: &mut Parser<'i, 't>,
        owner: &str,
        nesting_depth: usize,
    ) -> Result<String, ParseError<'i, ThemeCssError>> {
        let variable = input.expect_ident_cloned()?.to_string();
        if !variable.starts_with("--") {
            let (line, column) = self.property_location(owner);
            return Err(input.new_custom_error(ThemeCssError::InvalidValue {
                property: owner.to_owned(),
                value: variable,
                message: "var() expects a custom-property name".to_owned(),
                line,
                column,
            }));
        }

        let fallback = if input.try_parse(|input| input.expect_comma()).is_ok() {
            let start = input.position();
            while input.next_including_whitespace_and_comments().is_ok() {}
            Some(input.slice_from(start).trim().to_owned())
        } else {
            input.expect_exhausted()?;
            None
        };

        let resolved = if self.declarations.contains_key(&variable) {
            self.resolve_property(&variable)
        } else if let Some(fallback) = fallback {
            self.resolve_raw_value(&fallback, owner, nesting_depth)
        } else {
            let (line, column) = self.property_location(owner);
            Err(ThemeCssError::UnresolvedVariable {
                property: owner.to_owned(),
                variable,
                line,
                column,
            })
        };
        resolved.map_err(|error| input.new_custom_error(error))
    }

    fn enter_nesting(&self, owner: &str, nesting_depth: usize) -> Result<(), ThemeCssError> {
        if nesting_depth >= MAX_RESOLUTION_DEPTH {
            Err(self.limit_error(owner, "CSS nesting", MAX_RESOLUTION_DEPTH))
        } else {
            Ok(())
        }
    }

    fn record_substitution(&mut self, owner: &str) -> Result<(), ThemeCssError> {
        self.substitutions = self.substitutions.saturating_add(1);
        if self.substitutions > MAX_VAR_SUBSTITUTIONS {
            Err(self.limit_error(owner, "variable substitutions", MAX_VAR_SUBSTITUTIONS))
        } else {
            Ok(())
        }
    }

    fn append(
        &mut self,
        output: &mut String,
        fragment: &str,
        owner: &str,
    ) -> Result<(), ThemeCssError> {
        let next_len = output.len().checked_add(fragment.len()).ok_or_else(|| {
            self.limit_error(owner, "resolved value size", MAX_RESOLVED_VALUE_BYTES)
        })?;
        if next_len > MAX_RESOLVED_VALUE_BYTES {
            return Err(self.limit_error(owner, "resolved value size", MAX_RESOLVED_VALUE_BYTES));
        }
        self.charge_work(owner, fragment.len())?;
        output.push_str(fragment);
        Ok(())
    }

    fn push_space(&mut self, output: &mut String, owner: &str) -> Result<(), ThemeCssError> {
        if !output.is_empty() && !output.ends_with(char::is_whitespace) {
            self.append(output, " ", owner)?;
        }
        Ok(())
    }

    fn charge_work(&mut self, owner: &str, amount: usize) -> Result<(), ThemeCssError> {
        self.total_work = self.total_work.saturating_add(amount);
        if self.total_work > MAX_TOTAL_RESOLUTION_WORK {
            Err(self.limit_error(owner, "total resolution work", MAX_TOTAL_RESOLUTION_WORK))
        } else {
            Ok(())
        }
    }

    fn limit_error(&self, property: &str, resource: &'static str, limit: usize) -> ThemeCssError {
        let (line, column) = self.property_location(property);
        ThemeCssError::ResolutionLimit {
            property: property.to_owned(),
            resource,
            limit,
            line,
            column,
        }
    }

    fn property_location(&self, property: &str) -> (u32, u32) {
        self.declarations
            .get(property)
            .map(|declaration| display_location(declaration.location))
            .unwrap_or((1, 1))
    }
}

fn basic_error(error: BasicParseError<'_>, context: &str) -> ThemeCssError {
    ThemeCssError::Syntax {
        line: error.location.line.saturating_add(1),
        column: error.location.column,
        message: format!("{context}: {:?}", error.kind),
    }
}

fn basic_value_error(
    property: &str,
    value: &str,
    location: SourceLocation,
    error: BasicParseError<'_>,
) -> ThemeCssError {
    let (line, column) = display_location(location);
    ThemeCssError::InvalidValue {
        property: property.to_owned(),
        value: value.to_owned(),
        message: format!("{:?}", error.kind),
        line,
        column,
    }
}

fn parse_error(error: ParseError<'_, ThemeCssError>, context: &str) -> ThemeCssError {
    match error.kind {
        ParseErrorKind::Custom(error) => error,
        ParseErrorKind::Basic(kind) => ThemeCssError::Syntax {
            line: error.location.line.saturating_add(1),
            column: error.location.column,
            message: format!("{context}: {kind:?}"),
        },
    }
}

fn display_location(location: SourceLocation) -> (u32, u32) {
    (location.line.saturating_add(1), location.column)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_built_in_theme_and_resolves_aliases() {
        let theme = parse_theme_css(BUILTIN_THEME_CSS).unwrap();
        assert_eq!(theme.name.as_ref(), "API Tester Material Dark");
        assert_eq!(theme.mode, ThemeMode::Dark);
        assert_eq!(
            theme.palette.token("--api-yellow"),
            theme.palette.token("--api-warning")
        );
    }

    #[test]
    fn accepts_comments_and_var_fallbacks() {
        let css = BUILTIN_THEME_CSS.replace(
            "--api-yellow: var(--api-warning);",
            "--api-yellow: var(--missing, /* safe fallback */ #010203);",
        );
        let theme = parse_theme_css(&css).unwrap();
        let actual = theme.palette.token("--api-yellow").unwrap().to_rgb();
        assert_eq!(
            [actual.r, actual.g, actual.b].map(|channel| (channel * 255.0).round() as u8),
            [1, 2, 3]
        );
    }

    #[test]
    fn rejects_more_than_one_root_rule() {
        let css = format!("{BUILTIN_THEME_CSS}\n:root {{}}");
        assert_eq!(
            parse_theme_css(&css).unwrap_err(),
            ThemeCssError::ExpectedSingleRoot
        );
    }

    #[test]
    fn rejects_unknown_and_duplicate_properties() {
        let unknown = BUILTIN_THEME_CSS.replace(":root {", ":root { --api-typo: red;");
        assert!(matches!(
            parse_theme_css(&unknown),
            Err(ThemeCssError::UnknownProperty(property)) if property == "--api-typo"
        ));

        let duplicate = BUILTIN_THEME_CSS.replace(":root {", ":root { --api-surface: black;");
        assert!(matches!(
            parse_theme_css(&duplicate),
            Err(ThemeCssError::DuplicateProperty(property)) if property == "--api-surface"
        ));
    }

    #[test]
    fn rejects_unresolved_and_cyclic_variables() {
        let unresolved = BUILTIN_THEME_CSS.replace(
            "--api-yellow: var(--api-warning);",
            "--api-yellow: var(--not-defined);",
        );
        assert!(matches!(
            parse_theme_css(&unresolved),
            Err(ThemeCssError::UnresolvedVariable { variable, .. })
                if variable == "--not-defined"
        ));

        let cyclic = BUILTIN_THEME_CSS.replace(
            "--api-yellow: var(--api-warning);",
            "--api-yellow: var(--api-yellow);",
        );
        assert!(matches!(
            parse_theme_css(&cyclic),
            Err(ThemeCssError::VariableCycle { chain, .. })
                if chain == "--api-yellow -> --api-yellow"
        ));
    }

    #[test]
    fn rejects_invalid_color_values() {
        let css = BUILTIN_THEME_CSS.replace(
            "--api-yellow: var(--api-warning);",
            "--api-yellow: definitely-not-a-color;",
        );
        assert!(matches!(
            parse_theme_css(&css),
            Err(ThemeCssError::InvalidValue { property, .. })
                if property == "--api-yellow"
        ));
    }

    #[test]
    fn accepts_helper_properties_without_allowing_unknown_api_tokens() {
        let css = BUILTIN_THEME_CSS
            .replace(":root {", ":root {\n    --brand-warning: #010203;")
            .replace(
                "--api-yellow: var(--api-warning);",
                "--api-yellow: var(--brand-warning);",
            );
        let theme = parse_theme_css(&css).unwrap();
        let actual = theme.palette.token("--api-yellow").unwrap().to_rgb();
        assert_eq!(
            [actual.r, actual.g, actual.b].map(|channel| (channel * 255.0).round() as u8),
            [1, 2, 3]
        );
    }

    #[test]
    fn preserves_token_boundaries_around_variable_substitutions() {
        let css = BUILTIN_THEME_CSS
            .replace(":root {", ":root {\n    --brand-white: #fff;")
            .replace(
                "--api-yellow: var(--api-warning);",
                "--api-yellow: var(--brand-white)f;",
            );
        assert!(matches!(
            parse_theme_css(&css),
            Err(ThemeCssError::InvalidValue {
                property,
                value,
                ..
            }) if property == "--api-yellow" && value == "#fff f"
        ));
    }

    #[test]
    fn bounds_exponentially_expanding_variable_values() {
        let mut helpers = String::new();
        for index in 0..20 {
            helpers.push_str(&format!(
                "    --helper-{index}: var(--helper-{}) var(--helper-{});\n",
                index + 1,
                index + 1
            ));
        }
        helpers.push_str("    --helper-20: #fff;\n");
        let css = BUILTIN_THEME_CSS
            .replace(":root {", &format!(":root {{\n{helpers}"))
            .replace(
                "--api-yellow: var(--api-warning);",
                "--api-yellow: var(--helper-0);",
            );
        assert!(matches!(
            parse_theme_css(&css),
            Err(ThemeCssError::ResolutionLimit {
                resource: "resolved value size",
                ..
            })
        ));
    }

    #[test]
    fn bounds_nested_css_functions() {
        let nested = format!(
            "{}0{}",
            "rgb(".repeat(MAX_RESOLUTION_DEPTH + 1),
            ")".repeat(MAX_RESOLUTION_DEPTH + 1)
        );
        let css = BUILTIN_THEME_CSS.replace(
            "--api-yellow: var(--api-warning);",
            &format!("--api-yellow: {nested};"),
        );
        assert!(matches!(
            parse_theme_css(&css),
            Err(ThemeCssError::ResolutionLimit {
                resource: "CSS nesting",
                ..
            })
        ));
    }

    #[test]
    fn reports_syntax_lines_as_one_based() {
        assert!(matches!(
            parse_theme_css("not-a-root"),
            Err(ThemeCssError::Syntax { line: 1, .. })
        ));
    }
}
