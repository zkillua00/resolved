use std::collections::BTreeMap;

use cssparser::{
    BasicParseError, Delimiter, ParseError, ParseErrorKind, Parser, ParserInput, SourceLocation,
    ToCss, Token,
};
use gpui::{Edges, Hsla, Pixels, Rgba, px};
use gpui_component::{ButtonClassStyle, ThemeMode};
use thiserror::Error;

use super::{
    palette::{ApiTheme, ApiThemeClasses},
    schema::{
        THEME_CLASS_PROPERTIES, THEME_CLASSES, THEME_PROPERTIES, ThemePropertyKind, theme_class,
        theme_class_property, theme_property,
    },
};

pub const BUILTIN_THEME_CSS: &str = include_str!("../../assets/themes/api-tester-dark.css");
const MAX_THEME_BYTES: usize = 256 * 1024;
const MAX_RESOLUTION_DEPTH: usize = 64;
const MAX_VAR_SUBSTITUTIONS: usize = 4_096;
const MAX_RESOLVED_VALUE_BYTES: usize = 64 * 1024;
const MAX_TOTAL_RESOLUTION_WORK: usize = 4 * 1024 * 1024;

/// A deterministic error produced while parsing the Resolved CSS contract.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ThemeCssError {
    #[error("theme CSS exceeds the {limit}-byte limit")]
    TooLarge { limit: usize },
    #[error("theme CSS must contain exactly one :root rule")]
    ExpectedSingleRoot,
    #[error("unsupported theme selector `{0}`")]
    UnknownSelector(String),
    #[error("theme selector `{0}` is declared more than once")]
    DuplicateSelector(String),
    #[error("missing required theme selector `{0}`")]
    MissingSelector(String),
    #[error("unsupported property `{property}` in `{selector}`")]
    UnknownClassProperty { selector: String, property: String },
    #[error("property `{property}` is declared more than once in `{selector}`")]
    DuplicateClassProperty { selector: String, property: String },
    #[error("missing required property `{property}` in `{selector}`")]
    MissingClassProperty { selector: String, property: String },
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

#[derive(Clone, Debug)]
struct RawStylesheet {
    root: RawDeclarations,
    classes: BTreeMap<String, RawDeclarations>,
}

/// Parse a complete Resolved theme stylesheet.
pub fn parse_theme_css(source: &str) -> Result<ApiTheme, ThemeCssError> {
    if source.len() > MAX_THEME_BYTES {
        return Err(ThemeCssError::TooLarge {
            limit: MAX_THEME_BYTES,
        });
    }

    let RawStylesheet {
        root: declarations,
        classes,
    } = parse_stylesheet(source)?;
    for required in THEME_PROPERTIES.iter().filter(|property| property.required) {
        if !declarations.contains_key(required.name) {
            return Err(ThemeCssError::MissingProperty(required.name.to_owned()));
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
    for property in THEME_PROPERTIES
        .iter()
        .filter(|property| property.kind == ThemePropertyKind::Color)
    {
        if !declarations.contains_key(property.name) {
            continue;
        }
        let resolved = resolver.resolve_property(property.name)?;
        let (line, column) = display_location(declarations[property.name].location);
        let parsed =
            csscolorparser::parse(&resolved).map_err(|error| ThemeCssError::InvalidValue {
                property: property.name.to_owned(),
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
                property: property.name.to_owned(),
                value: resolved,
                message: "color channels must be finite".to_owned(),
                line,
                column,
            });
        }
        colors.insert(
            property.name.to_owned(),
            Hsla::from(Rgba {
                r: parsed.r.clamp(0.0, 1.0),
                g: parsed.g.clamp(0.0, 1.0),
                b: parsed.b.clamp(0.0, 1.0),
                a: parsed.a.clamp(0.0, 1.0),
            }),
        );
    }

    let class_styles = parse_class_styles(&classes)?;
    Ok(ApiTheme::from_resolved(name, mode, colors, class_styles))
}

fn parse_stylesheet(source: &str) -> Result<RawStylesheet, ThemeCssError> {
    let mut input = ParserInput::new(source);
    let mut parser = Parser::new(&mut input);
    let mut root = None;
    let mut classes = BTreeMap::new();

    while !parser.is_exhausted() {
        let selector = match parser
            .next()
            .map_err(|error| basic_error(error, "expected a theme selector"))?
            .clone()
        {
            Token::Colon => {
                let name = parser
                    .expect_ident_cloned()
                    .map_err(|error| basic_error(error, "expected `root` after `:`"))?;
                format!(":{name}")
            }
            Token::Delim('.') => {
                let name = parser
                    .expect_ident_cloned()
                    .map_err(|error| basic_error(error, "expected a class name after `.`"))?;
                format!(".{name}")
            }
            token => {
                let mut selector = String::new();
                token
                    .to_css(&mut selector)
                    .expect("writing CSS to a String cannot fail");
                return Err(ThemeCssError::UnknownSelector(selector));
            }
        };

        if selector != ":root" && theme_class(&selector).is_none() {
            return Err(ThemeCssError::UnknownSelector(selector));
        }
        parser
            .expect_curly_bracket_block()
            .map_err(|error| basic_error(error, "expected a declaration block"))?;
        let declarations = if selector == ":root" {
            parser
                .parse_nested_block(parse_root_declarations)
                .map_err(|error| parse_error(error, "invalid :root declaration block"))?
        } else {
            parser
                .parse_nested_block(|input| parse_class_declarations(input, &selector))
                .map_err(|error| parse_error(error, "invalid theme class declaration block"))?
        };

        if selector == ":root" {
            if root.replace(declarations).is_some() {
                return Err(ThemeCssError::ExpectedSingleRoot);
            }
        } else if classes.insert(selector.clone(), declarations).is_some() {
            return Err(ThemeCssError::DuplicateSelector(selector));
        }
    }

    let root = root.ok_or(ThemeCssError::ExpectedSingleRoot)?;
    for class in THEME_CLASSES {
        if !classes.contains_key(class.selector) {
            return Err(ThemeCssError::MissingSelector(class.selector.to_owned()));
        }
    }
    for property in THEME_CLASS_PROPERTIES {
        if !classes[property.selector].contains_key(property.name) {
            return Err(ThemeCssError::MissingClassProperty {
                selector: property.selector.to_owned(),
                property: property.name.to_owned(),
            });
        }
    }
    Ok(RawStylesheet { root, classes })
}

fn parse_root_declarations<'i, 't>(
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

fn parse_class_declarations<'i, 't>(
    input: &mut Parser<'i, 't>,
    selector: &str,
) -> Result<RawDeclarations, ParseError<'i, ThemeCssError>> {
    let mut declarations = BTreeMap::new();
    loop {
        while input.try_parse(|input| input.expect_semicolon()).is_ok() {}
        if input.is_exhausted() {
            break;
        }

        let property = input.expect_ident_cloned()?.to_string();
        if theme_class_property(selector, &property).is_none() {
            return Err(input.new_custom_error(ThemeCssError::UnknownClassProperty {
                selector: selector.to_owned(),
                property,
            }));
        }
        input.expect_colon()?;
        let location = input.current_source_location();
        let value = input.parse_until_before(
            Delimiter::Semicolon,
            |value| -> Result<String, ParseError<'i, ThemeCssError>> {
                let start = value.position();
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
            return Err(
                input.new_custom_error(ThemeCssError::DuplicateClassProperty {
                    selector: selector.to_owned(),
                    property,
                }),
            );
        }
        if !input.is_exhausted() {
            input.expect_semicolon()?;
        }
    }
    Ok(declarations)
}

#[derive(Clone, Copy, Debug)]
enum CssLength {
    Pixels(f32),
    Rems(f32),
}

#[derive(Clone, Copy, Debug)]
enum CssLengthValue {
    Length(CssLength),
    Auto,
    Inherit,
}

fn parse_class_styles(
    classes: &BTreeMap<String, RawDeclarations>,
) -> Result<ApiThemeClasses, ThemeCssError> {
    let app = &classes[".app"];
    let button = &classes[".button"];
    let editor = &classes[".editor"];

    let zoom = parse_class_value(app, ".app", "zoom", parse_zoom)?;
    let app_font_family = parse_class_value(app, ".app", "font-family", |value| {
        parse_font_family(value, false)
    })?
    .expect("the .app font cannot inherit");
    let app_font_size = parse_class_value(app, ".app", "font-size", |value| {
        let value = parse_length_value(value)?;
        let CssLengthValue::Length(CssLength::Pixels(size)) = value else {
            return Err("font-size must use px".to_owned());
        };
        if !(8. ..=48.).contains(&size) {
            return Err("font-size must be between 8px and 48px".to_owned());
        }
        Ok(size)
    })?;
    let app_margin = parse_class_box(app, ".app", "margin", true, app_font_size, zoom)?;
    let app_padding = parse_class_box(app, ".app", "padding", false, app_font_size, zoom)?;
    let scaled_app_font_size = px(app_font_size * zoom);

    let editor_font_family = parse_class_value(editor, ".editor", "font-family", |value| {
        parse_font_family(value, true)
    })?
    .unwrap_or_else(|| app_font_family.clone());
    let editor_font_size =
        match parse_class_value(editor, ".editor", "font-size", parse_length_value)? {
            CssLengthValue::Length(length) => {
                scale_non_negative_length(length, app_font_size, zoom, "font-size")
                    .map_err(|message| invalid_class_value(editor, "font-size", &message))?
            }
            CssLengthValue::Inherit => scaled_app_font_size,
            CssLengthValue::Auto => {
                return Err(invalid_class_value(
                    editor,
                    "font-size",
                    "auto is not valid here",
                ));
            }
        };
    let editor_border_radius = parse_optional_length(
        editor,
        ".editor",
        "border-radius",
        app_font_size,
        zoom,
        false,
    )?;
    let editor_margin = parse_class_box(editor, ".editor", "margin", true, app_font_size, zoom)?;
    let editor_padding = parse_class_box(editor, ".editor", "padding", false, app_font_size, zoom)?;

    let button_border_radius = parse_optional_length(
        button,
        ".button",
        "border-radius",
        app_font_size,
        zoom,
        false,
    )?;
    let button_width =
        parse_optional_length(button, ".button", "width", app_font_size, zoom, false)?;
    let button_min_width =
        parse_optional_length(button, ".button", "min-width", app_font_size, zoom, false)?;
    let button_height =
        parse_optional_length(button, ".button", "height", app_font_size, zoom, false)?;
    let button_margin = parse_class_box(button, ".button", "margin", true, app_font_size, zoom)?;
    let button_padding =
        parse_optional_box(button, ".button", "padding", false, app_font_size, zoom)?;
    let button_gap = parse_optional_length(button, ".button", "gap", app_font_size, zoom, false)?;
    let button_font_family = parse_class_value(button, ".button", "font-family", |value| {
        parse_font_family(value, true)
    })?;
    let button_font_size =
        match parse_class_value(button, ".button", "font-size", parse_length_value)? {
            CssLengthValue::Length(length) => Some(
                scale_non_negative_length(length, app_font_size, zoom, "font-size")
                    .map_err(|message| invalid_class_value(button, "font-size", &message))?,
            ),
            CssLengthValue::Inherit => None,
            CssLengthValue::Auto => {
                return Err(invalid_class_value(
                    button,
                    "font-size",
                    "use inherit to keep the native button size",
                ));
            }
        };

    Ok(ApiThemeClasses {
        app: super::palette::AppClassStyle {
            font_family: app_font_family.into(),
            font_size: scaled_app_font_size,
            margin: app_margin,
            padding: app_padding,
        },
        button: ButtonClassStyle {
            border_radius: button_border_radius,
            width: button_width,
            min_width: button_min_width,
            height: button_height,
            margin: Some(button_margin),
            padding: button_padding,
            gap: button_gap,
            font_family: button_font_family.map(Into::into),
            font_size: button_font_size,
        },
        editor: super::palette::EditorClassStyle {
            font_family: editor_font_family.into(),
            font_size: editor_font_size,
            border_radius: editor_border_radius,
            margin: editor_margin,
            padding: editor_padding,
        },
    })
}

fn parse_class_value<T>(
    declarations: &RawDeclarations,
    _selector: &str,
    property: &str,
    parser: impl FnOnce(&str) -> Result<T, String>,
) -> Result<T, ThemeCssError> {
    let declaration = &declarations[property];
    parser(&declaration.value).map_err(|message| {
        let (line, column) = display_location(declaration.location);
        ThemeCssError::InvalidValue {
            property: property.to_owned(),
            value: declaration.value.clone(),
            message,
            line,
            column,
        }
    })
}

fn invalid_class_value(
    declarations: &RawDeclarations,
    property: &str,
    message: &str,
) -> ThemeCssError {
    let declaration = &declarations[property];
    let (line, column) = display_location(declaration.location);
    ThemeCssError::InvalidValue {
        property: property.to_owned(),
        value: declaration.value.clone(),
        message: message.to_owned(),
        line,
        column,
    }
}

fn parse_zoom(value: &str) -> Result<f32, String> {
    let mut input = ParserInput::new(value);
    let mut parser = Parser::new(&mut input);
    let zoom = match parser.next().map_err(|error| format!("{error:?}"))? {
        Token::Number { value, .. } => *value,
        Token::Percentage { unit_value, .. } => *unit_value,
        _ => return Err("zoom must be a number or percentage".to_owned()),
    };
    parser
        .expect_exhausted()
        .map_err(|error| format!("{error:?}"))?;
    if !zoom.is_finite() || !(0.5..=2.).contains(&zoom) {
        return Err("zoom must be between 50% and 200%".to_owned());
    }
    Ok(zoom)
}

fn parse_font_family(value: &str, allow_inherit: bool) -> Result<Option<String>, String> {
    let mut input = ParserInput::new(value);
    let mut parser = Parser::new(&mut input);
    let first = parser.next().map_err(|error| format!("{error:?}"))?.clone();
    let family = match first {
        Token::QuotedString(family) => {
            parser
                .expect_exhausted()
                .map_err(|error| format!("{error:?}"))?;
            family.to_string()
        }
        Token::Ident(ident) => {
            let mut family = ident.to_string();
            while !parser.is_exhausted() {
                let ident = parser
                    .expect_ident_cloned()
                    .map_err(|_| "font-family must be one quoted name or identifier sequence")?;
                family.push(' ');
                family.push_str(&ident);
            }
            family
        }
        _ => return Err("font-family must be a quoted name or identifier sequence".to_owned()),
    };
    if family.eq_ignore_ascii_case("inherit") {
        return if allow_inherit {
            Ok(None)
        } else {
            Err("inherit is not valid here".to_owned())
        };
    }
    if family.eq_ignore_ascii_case("auto") {
        return Err("auto is not a font family".to_owned());
    }
    if family.trim().is_empty() {
        return Err("font-family cannot be empty".to_owned());
    }
    Ok(Some(family))
}

fn parse_length_value(value: &str) -> Result<CssLengthValue, String> {
    let mut input = ParserInput::new(value);
    let mut parser = Parser::new(&mut input);
    let token = parser.next().map_err(|error| format!("{error:?}"))?.clone();
    let length = match token {
        Token::Ident(keyword) if keyword.eq_ignore_ascii_case("auto") => CssLengthValue::Auto,
        Token::Ident(keyword) if keyword.eq_ignore_ascii_case("inherit") => CssLengthValue::Inherit,
        Token::Dimension { value, unit, .. } if unit.eq_ignore_ascii_case("px") => {
            CssLengthValue::Length(CssLength::Pixels(value))
        }
        Token::Dimension { value, unit, .. } if unit.eq_ignore_ascii_case("rem") => {
            CssLengthValue::Length(CssLength::Rems(value))
        }
        Token::Number { value: 0., .. } => CssLengthValue::Length(CssLength::Pixels(0.)),
        _ => return Err("expected px, rem, unitless 0, auto, or inherit".to_owned()),
    };
    parser
        .expect_exhausted()
        .map_err(|error| format!("{error:?}"))?;
    Ok(length)
}

fn parse_optional_length(
    declarations: &RawDeclarations,
    selector: &str,
    property: &str,
    base_font_size: f32,
    zoom: f32,
    allow_inherit: bool,
) -> Result<Option<Pixels>, ThemeCssError> {
    match parse_class_value(declarations, selector, property, parse_length_value)? {
        CssLengthValue::Auto => Ok(None),
        CssLengthValue::Inherit if allow_inherit => Ok(None),
        CssLengthValue::Inherit => Err(invalid_class_value(
            declarations,
            property,
            "inherit is not valid here",
        )),
        CssLengthValue::Length(length) => {
            scale_non_negative_length(length, base_font_size, zoom, property)
                .map(Some)
                .map_err(|message| invalid_class_value(declarations, property, &message))
        }
    }
}

fn parse_class_box(
    declarations: &RawDeclarations,
    selector: &str,
    property: &str,
    allow_negative: bool,
    base_font_size: f32,
    zoom: f32,
) -> Result<Edges<Pixels>, ThemeCssError> {
    parse_class_value(declarations, selector, property, |value| {
        parse_box(value, allow_negative, base_font_size, zoom)
    })
}

fn parse_optional_box(
    declarations: &RawDeclarations,
    selector: &str,
    property: &str,
    allow_negative: bool,
    base_font_size: f32,
    zoom: f32,
) -> Result<Option<Edges<Pixels>>, ThemeCssError> {
    if declarations[property].value.eq_ignore_ascii_case("auto") {
        return Ok(None);
    }
    parse_class_box(
        declarations,
        selector,
        property,
        allow_negative,
        base_font_size,
        zoom,
    )
    .map(Some)
}

fn parse_box(
    value: &str,
    allow_negative: bool,
    base_font_size: f32,
    zoom: f32,
) -> Result<Edges<Pixels>, String> {
    let mut input = ParserInput::new(value);
    let mut parser = Parser::new(&mut input);
    let mut values = Vec::with_capacity(4);
    while !parser.is_exhausted() {
        let start = parser.position();
        parser.next().map_err(|error| format!("{error:?}"))?;
        let raw = parser.slice_from(start);
        let length = match parse_length_value(raw)? {
            CssLengthValue::Length(length) => {
                scale_length(length, base_font_size, zoom, "spacing")?
            }
            CssLengthValue::Auto | CssLengthValue::Inherit => {
                return Err("spacing values must use px, rem, or unitless 0".to_owned());
            }
        };
        if !allow_negative && length < Pixels::ZERO {
            return Err("padding cannot be negative".to_owned());
        }
        values.push(length);
        if values.len() > 4 {
            return Err("spacing accepts one to four values".to_owned());
        }
    }
    let [top, right, bottom, left] = match values.as_slice() {
        [all] => [*all, *all, *all, *all],
        [vertical, horizontal] => [*vertical, *horizontal, *vertical, *horizontal],
        [top, horizontal, bottom] => [*top, *horizontal, *bottom, *horizontal],
        [top, right, bottom, left] => [*top, *right, *bottom, *left],
        _ => return Err("spacing accepts one to four values".to_owned()),
    };
    Ok(Edges {
        top,
        right,
        bottom,
        left,
    })
}

fn scale_length(
    length: CssLength,
    base_font_size: f32,
    zoom: f32,
    property: &str,
) -> Result<Pixels, String> {
    let value = match length {
        CssLength::Pixels(value) => value * zoom,
        CssLength::Rems(value) => value * base_font_size * zoom,
    };
    if !value.is_finite() || !(-512. ..=4096.).contains(&value) {
        return Err(format!("{property} is outside the supported size range"));
    }
    Ok(px(value))
}

fn scale_non_negative_length(
    length: CssLength,
    base_font_size: f32,
    zoom: f32,
    property: &str,
) -> Result<Pixels, String> {
    let value = scale_length(length, base_font_size, zoom, property)?;
    if value < Pixels::ZERO {
        return Err(format!("{property} cannot be negative"));
    }
    Ok(value)
}

fn is_known_api_property(property: &str) -> bool {
    theme_property(property).is_some()
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
    use gpui_component::PixelsExt as _;

    #[test]
    fn parses_built_in_theme_and_resolves_aliases() {
        let theme = parse_theme_css(BUILTIN_THEME_CSS).unwrap();
        assert_eq!(theme.name.as_ref(), "Resolved Material Dark");
        assert_eq!(theme.mode, ThemeMode::Dark);
        assert_eq!(
            theme.palette.token("--api-yellow"),
            theme.palette.token("--api-warning")
        );
        assert_eq!(theme.classes.app.font_size.as_f32(), 16.);
        assert_eq!(theme.classes.editor.font_size.as_f32(), 14.);
        assert!(theme.classes.button.border_radius.is_none());
        assert!(theme.classes.button.padding.is_none());
        assert_eq!(
            theme
                .classes
                .button
                .margin
                .expect("the default button margin is explicit")
                .top
                .as_f32(),
            0.
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
    fn requires_the_native_style_classes() {
        let classless = BUILTIN_THEME_CSS
            .split("\n.app {")
            .next()
            .expect("the bundled stylesheet contains .app");
        assert!(matches!(
            parse_theme_css(classless),
            Err(ThemeCssError::MissingSelector(selector)) if selector == ".app"
        ));

        let missing_property = BUILTIN_THEME_CSS.replace("    gap: auto;\n", "");
        assert!(matches!(
            parse_theme_css(&missing_property),
            Err(ThemeCssError::MissingClassProperty { selector, property })
                if selector == ".button" && property == "gap"
        ));
    }

    #[test]
    fn rejects_unknown_and_duplicate_class_rules_and_properties() {
        let unknown_selector = format!("{BUILTIN_THEME_CSS}\n.card {{ padding: 0; }}");
        assert!(matches!(
            parse_theme_css(&unknown_selector),
            Err(ThemeCssError::UnknownSelector(selector)) if selector == ".card"
        ));

        let duplicate_selector = BUILTIN_THEME_CSS.replace(".button {", ".button {}\n.button {");
        assert!(matches!(
            parse_theme_css(&duplicate_selector),
            Err(ThemeCssError::DuplicateSelector(selector)) if selector == ".button"
        ));

        let unknown_property = BUILTIN_THEME_CSS.replace(".button {", ".button {\n    color: red;");
        assert!(matches!(
            parse_theme_css(&unknown_property),
            Err(ThemeCssError::UnknownClassProperty { selector, property })
                if selector == ".button" && property == "color"
        ));

        let duplicate_property =
            BUILTIN_THEME_CSS.replace(".button {", ".button {\n    margin: 0;");
        assert!(matches!(
            parse_theme_css(&duplicate_property),
            Err(ThemeCssError::DuplicateClassProperty { selector, property })
                if selector == ".button" && property == "margin"
        ));
    }

    #[test]
    fn parses_zoom_lengths_and_css_box_shorthand() {
        let css = BUILTIN_THEME_CSS
            .replace("zoom: 100%;", "zoom: 125%;")
            .replace("font-size: 16px;", "font-size: 18px;")
            .replace("border-radius: auto;", "border-radius: 0.5rem;")
            .replace("margin: 0;", "margin: 1px 2px 3px 4px;")
            .replace("padding: auto;", "padding: 2px 4px 6px 8px;")
            .replace("font-size: inherit;", "font-size: 0.875rem;");
        let theme = parse_theme_css(&css).unwrap();

        assert_eq!(theme.classes.app.font_size.as_f32(), 22.5);
        assert_eq!(
            theme
                .classes
                .button
                .border_radius
                .expect("radius override")
                .as_f32(),
            11.25
        );
        let margin = theme.classes.button.margin.expect("button margin");
        assert_eq!(
            [
                margin.top.as_f32(),
                margin.right.as_f32(),
                margin.bottom.as_f32(),
                margin.left.as_f32(),
            ],
            [1.25, 2.5, 3.75, 5.]
        );
        let padding = theme.classes.button.padding.expect("button padding");
        assert_eq!(
            [
                padding.top.as_f32(),
                padding.right.as_f32(),
                padding.bottom.as_f32(),
                padding.left.as_f32(),
            ],
            [2.5, 5., 7.5, 10.]
        );
        assert_eq!(
            theme
                .classes
                .button
                .font_size
                .expect("button font override")
                .as_f32(),
            19.6875
        );
    }

    #[test]
    fn rejects_unsafe_or_unsupported_layout_values() {
        let excessive_zoom = BUILTIN_THEME_CSS.replace("zoom: 100%;", "zoom: 250%;");
        assert!(matches!(
            parse_theme_css(&excessive_zoom),
            Err(ThemeCssError::InvalidValue { property, .. }) if property == "zoom"
        ));

        let negative_padding = BUILTIN_THEME_CSS.replace("padding: auto;", "padding: -1px;");
        assert!(matches!(
            parse_theme_css(&negative_padding),
            Err(ThemeCssError::InvalidValue { property, .. }) if property == "padding"
        ));

        let unsupported_unit = BUILTIN_THEME_CSS.replace("height: auto;", "height: 2vh;");
        assert!(matches!(
            parse_theme_css(&unsupported_unit),
            Err(ThemeCssError::InvalidValue { property, .. }) if property == "height"
        ));

        let negative_radius =
            BUILTIN_THEME_CSS.replace("border-radius: auto;", "border-radius: -4px;");
        assert!(matches!(
            parse_theme_css(&negative_radius),
            Err(ThemeCssError::InvalidValue { property, .. }) if property == "border-radius"
        ));

        let negative_font_size =
            BUILTIN_THEME_CSS.replace("font-size: inherit;", "font-size: -1rem;");
        assert!(matches!(
            parse_theme_css(&negative_font_size),
            Err(ThemeCssError::InvalidValue { property, .. }) if property == "font-size"
        ));

        let inherited_app_font =
            BUILTIN_THEME_CSS.replace("font-family: \".SystemUIFont\";", "font-family: inherit;");
        assert!(matches!(
            parse_theme_css(&inherited_app_font),
            Err(ThemeCssError::InvalidValue { property, .. }) if property == "font-family"
        ));
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
            parse_theme_css(":root !"),
            Err(ThemeCssError::Syntax { line: 1, .. })
        ));
    }
}
