//! The single source of truth for Resolved's CSS theme contract.
//!
//! Parsing, editor intelligence, and documentation all consume these specs so
//! a property cannot be accepted by one surface and omitted from another.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ThemePropertyKind {
    Name,
    Appearance,
    Color,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ThemePropertyCategory {
    Metadata,
    Surfaces,
    Primary,
    Status,
    Semantic,
    Editor,
    Syntax,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ThemeClassPropertyKind {
    Zoom,
    FontFamily,
    Length,
    Box,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ThemeClassSpec {
    pub(crate) selector: &'static str,
    pub(crate) documentation: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ThemeClassPropertySpec {
    pub(crate) selector: &'static str,
    pub(crate) name: &'static str,
    pub(crate) kind: ThemeClassPropertyKind,
    pub(crate) default_value: &'static str,
    pub(crate) documentation: &'static str,
}

impl ThemePropertyCategory {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Metadata => "Theme metadata",
            Self::Surfaces => "Surfaces and text",
            Self::Primary => "Primary interaction",
            Self::Status => "Status colors",
            Self::Semantic => "Semantic palette",
            Self::Editor => "Code editor",
            Self::Syntax => "Syntax highlighting",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ThemePropertySpec {
    pub(crate) name: &'static str,
    pub(crate) kind: ThemePropertyKind,
    pub(crate) category: ThemePropertyCategory,
    pub(crate) required: bool,
    pub(crate) default_value: &'static str,
    pub(crate) documentation: &'static str,
}

const fn property(
    name: &'static str,
    kind: ThemePropertyKind,
    category: ThemePropertyCategory,
    required: bool,
    default_value: &'static str,
    documentation: &'static str,
) -> ThemePropertySpec {
    ThemePropertySpec {
        name,
        kind,
        category,
        required,
        default_value,
        documentation,
    }
}

pub(crate) const THEME_PROPERTIES: &[ThemePropertySpec] = &[
    property(
        "--api-theme-name",
        ThemePropertyKind::Name,
        ThemePropertyCategory::Metadata,
        true,
        "\"Resolved Dark\"",
        "Theme metadata used as the default library name when importing or saving CSS.",
    ),
    property(
        "--api-appearance",
        ThemePropertyKind::Appearance,
        ThemePropertyCategory::Metadata,
        true,
        "dark",
        "Selects the dark or light GPUI baseline used by components without an explicit token.",
    ),
    property(
        "--api-surface",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Surfaces,
        true,
        "#17171b",
        "Main window, workspace, tab, table, and accordion background.",
    ),
    property(
        "--api-surface-lowest",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Surfaces,
        true,
        "#111114",
        "Deepest recessed surface, including editor backgrounds and dark overlays.",
    ),
    property(
        "--api-surface-low",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Surfaces,
        true,
        "#1e1e23",
        "Navigation rails, sidebars, group boxes, and low-elevation panels.",
    ),
    property(
        "--api-surface-container",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Surfaces,
        true,
        "#25252b",
        "Nested containers, popovers, and the default active editor line.",
    ),
    property(
        "--api-surface-high",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Surfaces,
        true,
        "#2e2d34",
        "Hover surfaces, muted controls, progress tracks, and scrollbar thumbs.",
    ),
    property(
        "--api-surface-highest",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Surfaces,
        true,
        "#37363d",
        "Strong hover and elevated secondary-control surfaces.",
    ),
    property(
        "--api-foreground",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Surfaces,
        true,
        "#f4f0e7",
        "Primary application text and the default editor foreground.",
    ),
    property(
        "--api-muted-foreground",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Surfaces,
        true,
        "#b8b6bf",
        "Secondary labels, placeholders, inactive tabs, and subdued metadata.",
    ),
    property(
        "--api-outline",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Surfaces,
        true,
        "#4b4a54",
        "Borders, dividers, input outlines, table rows, and the window outline.",
    ),
    property(
        "--api-primary",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Primary,
        true,
        "#3b4bea",
        "Primary buttons, enabled switch tracks, and the syntax-keyword fallback.",
    ),
    property(
        "--api-primary-hover",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Primary,
        true,
        "#4f5ef0",
        "Hovered primary controls, caret, active line numbers, and property syntax.",
    ),
    property(
        "--api-primary-active",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Primary,
        true,
        "#303fc9",
        "Pressed primary controls and active links.",
    ),
    property(
        "--api-primary-foreground",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Primary,
        true,
        "#f4f0e7",
        "Text and icons displayed on a primary-colored surface.",
    ),
    property(
        "--api-primary-text",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Primary,
        false,
        "#a9b0ff",
        "Standalone accent text and focus indicators; falls back to primary hover when omitted.",
    ),
    property(
        "--api-selection",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Primary,
        true,
        "#30365b",
        "Text selection, selected rows, active sidebar entries, and drop targets.",
    ),
    property(
        "--api-secondary-active",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Primary,
        false,
        "#3c4470",
        "Pressed secondary controls; falls back to the selection color when omitted.",
    ),
    property(
        "--api-secondary-foreground",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Primary,
        false,
        "#f4f0e7",
        "Text on secondary controls; falls back to the main foreground when omitted.",
    ),
    property(
        "--api-danger",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Status,
        true,
        "#8c1d18",
        "Destructive actions, error badges, and danger status.",
    ),
    property(
        "--api-danger-hover",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Status,
        false,
        "#a52a24",
        "Hovered destructive controls; falls back to danger when omitted.",
    ),
    property(
        "--api-danger-active",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Status,
        false,
        "#71120e",
        "Pressed destructive controls; falls back to danger when omitted.",
    ),
    property(
        "--api-danger-foreground",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Status,
        true,
        "#ffdad6",
        "Text and icons placed on danger surfaces.",
    ),
    property(
        "--api-warning",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Status,
        true,
        "#f4d06f",
        "Warnings, modified-state indicators, and caution status.",
    ),
    property(
        "--api-warning-hover",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Status,
        false,
        "#ffdc82",
        "Hovered warning controls; falls back to warning when omitted.",
    ),
    property(
        "--api-warning-active",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Status,
        false,
        "#d7b34f",
        "Pressed warning controls; falls back to warning when omitted.",
    ),
    property(
        "--api-warning-foreground",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Status,
        true,
        "#332b00",
        "Text and icons placed on warning surfaces.",
    ),
    property(
        "--api-success",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Status,
        true,
        "#2f6f44",
        "Successful requests, positive status, and enabled confirmations.",
    ),
    property(
        "--api-success-hover",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Status,
        false,
        "#3d8253",
        "Hovered success controls; falls back to success when omitted.",
    ),
    property(
        "--api-success-active",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Status,
        false,
        "#245c36",
        "Pressed success controls; falls back to success when omitted.",
    ),
    property(
        "--api-success-foreground",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Status,
        true,
        "#d0f8d8",
        "Text and icons placed on success surfaces.",
    ),
    property(
        "--api-info",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Status,
        true,
        "#385f8e",
        "Informational controls, script output, and 3xx status feedback.",
    ),
    property(
        "--api-info-hover",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Status,
        false,
        "#4771a4",
        "Hovered informational controls; falls back to info when omitted.",
    ),
    property(
        "--api-info-active",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Status,
        false,
        "#2c4d77",
        "Pressed informational controls; falls back to info when omitted.",
    ),
    property(
        "--api-info-foreground",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Status,
        true,
        "#d7e9ff",
        "Text and icons placed on informational surfaces.",
    ),
    property(
        "--api-red",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Semantic,
        true,
        "#ffb4ab",
        "Semantic red used by HTTP methods, charts, booleans, and bearish status.",
    ),
    property(
        "--api-red-light",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Semantic,
        false,
        "#ffdad6",
        "Lighter semantic red used where a softer red accent is needed.",
    ),
    property(
        "--api-green",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Semantic,
        true,
        "#4ade80",
        "Semantic green used by HTTP methods, charts, and bullish status.",
    ),
    property(
        "--api-green-light",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Semantic,
        false,
        "#b8f5c8",
        "Lighter semantic green accent.",
    ),
    property(
        "--api-blue",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Semantic,
        true,
        "#93c5fd",
        "Semantic blue used by HTTP methods, charts, and informational accents.",
    ),
    property(
        "--api-blue-light",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Semantic,
        false,
        "#d7e9ff",
        "Lighter semantic blue accent.",
    ),
    property(
        "--api-yellow",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Semantic,
        true,
        "var(--api-warning)",
        "Semantic yellow used by HTTP methods and charts.",
    ),
    property(
        "--api-yellow-light",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Semantic,
        false,
        "#ffecb3",
        "Lighter semantic yellow accent.",
    ),
    property(
        "--api-magenta",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Semantic,
        true,
        "#f0abfc",
        "Semantic magenta used by HTTP methods and charts.",
    ),
    property(
        "--api-magenta-light",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Semantic,
        false,
        "#f7d5ff",
        "Lighter semantic magenta accent.",
    ),
    property(
        "--api-cyan",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Semantic,
        true,
        "#67e8f9",
        "Semantic cyan used by HTTP methods and charts.",
    ),
    property(
        "--api-cyan-light",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Semantic,
        false,
        "#c9f8ff",
        "Lighter semantic cyan accent.",
    ),
    property(
        "--api-editor-background",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Editor,
        false,
        "var(--api-surface-lowest)",
        "Code editor canvas; falls back to the lowest surface.",
    ),
    property(
        "--api-editor-gutter-background",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Editor,
        false,
        "var(--api-surface-lowest)",
        "Line-number and fold gutter background; falls back to the editor canvas.",
    ),
    property(
        "--api-editor-foreground",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Editor,
        false,
        "var(--api-foreground)",
        "Default code editor text; falls back to the main foreground.",
    ),
    property(
        "--api-editor-active-line",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Editor,
        false,
        "var(--api-surface-container)",
        "Background of the line containing the caret.",
    ),
    property(
        "--api-editor-line-number",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Editor,
        false,
        "#94929e",
        "Inactive code editor line numbers.",
    ),
    property(
        "--api-editor-active-line-number",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Editor,
        false,
        "var(--api-primary-text)",
        "Line number for the line containing the caret.",
    ),
    property(
        "--api-syntax-property",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Syntax,
        false,
        "var(--api-primary-text)",
        "Object keys, CSS properties, and similar property-name captures.",
    ),
    property(
        "--api-syntax-string",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Syntax,
        false,
        "#ffd9e3",
        "String literals in request bodies and scripts.",
    ),
    property(
        "--api-syntax-number",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Syntax,
        false,
        "#efb8c8",
        "Numeric literals.",
    ),
    property(
        "--api-syntax-boolean",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Syntax,
        false,
        "var(--api-red)",
        "Boolean and null-like literals.",
    ),
    property(
        "--api-syntax-keyword",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Syntax,
        false,
        "var(--api-primary-text)",
        "Language keywords such as const, if, and return.",
    ),
    property(
        "--api-syntax-comment",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Syntax,
        false,
        "#94929e",
        "Source-code comments.",
    ),
    property(
        "--api-syntax-punctuation",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Syntax,
        false,
        "var(--api-muted-foreground)",
        "Braces, commas, operators, and other punctuation.",
    ),
    property(
        "--api-syntax-variable",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Syntax,
        false,
        "var(--api-foreground)",
        "Variables and identifiers without a more specific capture.",
    ),
    property(
        "--api-syntax-type",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Syntax,
        false,
        "#ccc2dc",
        "Type names and type annotations.",
    ),
    property(
        "--api-syntax-function",
        ThemePropertyKind::Color,
        ThemePropertyCategory::Syntax,
        false,
        "var(--api-primary-text)",
        "Function names and calls.",
    ),
];

pub(crate) fn theme_property(name: &str) -> Option<&'static ThemePropertySpec> {
    THEME_PROPERTIES
        .iter()
        .find(|property| property.name == name)
}

pub(crate) const THEME_CLASSES: &[ThemeClassSpec] = &[
    ThemeClassSpec {
        selector: ".app",
        documentation: "Global typography, rem-based interface scale, and outer spacing.",
    },
    ThemeClassSpec {
        selector: ".button",
        documentation: "Shared geometry, spacing, and typography for native buttons.",
    },
    ThemeClassSpec {
        selector: ".editor",
        documentation: "Typography, shape, and inner and outer spacing for code editor surfaces.",
    },
];

pub(crate) const THEME_CLASS_PROPERTIES: &[ThemeClassPropertySpec] = &[
    ThemeClassPropertySpec {
        selector: ".app",
        name: "zoom",
        kind: ThemeClassPropertyKind::Zoom,
        default_value: "100%",
        documentation: "Scales class lengths, rem-based interface dimensions, and typography from 50% to 200%.",
    },
    ThemeClassPropertySpec {
        selector: ".app",
        name: "font-family",
        kind: ThemeClassPropertyKind::FontFamily,
        default_value: "\".SystemUIFont\"",
        documentation: "Application font family. Use one quoted or unquoted family name.",
    },
    ThemeClassPropertySpec {
        selector: ".app",
        name: "font-size",
        kind: ThemeClassPropertyKind::Length,
        default_value: "16px",
        documentation: "Unscaled base font size and the base unit used by interface zoom.",
    },
    ThemeClassPropertySpec {
        selector: ".app",
        name: "margin",
        kind: ThemeClassPropertyKind::Box,
        default_value: "0",
        documentation: "Outer spacing around the application canvas using CSS box shorthand.",
    },
    ThemeClassPropertySpec {
        selector: ".app",
        name: "padding",
        kind: ThemeClassPropertyKind::Box,
        default_value: "0",
        documentation: "Inner spacing around the application canvas using CSS box shorthand.",
    },
    ThemeClassPropertySpec {
        selector: ".button",
        name: "border-radius",
        kind: ThemeClassPropertyKind::Length,
        default_value: "auto",
        documentation: "Overrides every button corner radius; auto preserves each native button shape.",
    },
    ThemeClassPropertySpec {
        selector: ".button",
        name: "width",
        kind: ThemeClassPropertyKind::Length,
        default_value: "auto",
        documentation: "Fixed button width; auto keeps content and icon button sizing.",
    },
    ThemeClassPropertySpec {
        selector: ".button",
        name: "min-width",
        kind: ThemeClassPropertyKind::Length,
        default_value: "auto",
        documentation: "Minimum button width; auto keeps the component default.",
    },
    ThemeClassPropertySpec {
        selector: ".button",
        name: "height",
        kind: ThemeClassPropertyKind::Length,
        default_value: "auto",
        documentation: "Fixed button height; auto preserves small, medium, large, and custom sizes.",
    },
    ThemeClassPropertySpec {
        selector: ".button",
        name: "margin",
        kind: ThemeClassPropertyKind::Box,
        default_value: "0",
        documentation: "Spacing outside every button using CSS box shorthand.",
    },
    ThemeClassPropertySpec {
        selector: ".button",
        name: "padding",
        kind: ThemeClassPropertyKind::Box,
        default_value: "auto",
        documentation: "Spacing inside every button; auto preserves native size and compact variants.",
    },
    ThemeClassPropertySpec {
        selector: ".button",
        name: "gap",
        kind: ThemeClassPropertyKind::Length,
        default_value: "auto",
        documentation: "Space between a button icon, label, and dropdown caret.",
    },
    ThemeClassPropertySpec {
        selector: ".button",
        name: "font-family",
        kind: ThemeClassPropertyKind::FontFamily,
        default_value: "inherit",
        documentation: "Button font family; inherit uses the application font.",
    },
    ThemeClassPropertySpec {
        selector: ".button",
        name: "font-size",
        kind: ThemeClassPropertyKind::Length,
        default_value: "inherit",
        documentation: "Button font size; inherit preserves native size variants.",
    },
    ThemeClassPropertySpec {
        selector: ".editor",
        name: "font-family",
        kind: ThemeClassPropertyKind::FontFamily,
        default_value: "\"Menlo\"",
        documentation: "Font family used for request, response, script, and theme code editors.",
    },
    ThemeClassPropertySpec {
        selector: ".editor",
        name: "font-size",
        kind: ThemeClassPropertyKind::Length,
        default_value: "0.8125rem",
        documentation: "Code editor font size before interface zoom is applied.",
    },
    ThemeClassPropertySpec {
        selector: ".editor",
        name: "border-radius",
        kind: ThemeClassPropertyKind::Length,
        default_value: "auto",
        documentation: "Code editor corner radius; auto preserves framed and unframed editor shapes.",
    },
    ThemeClassPropertySpec {
        selector: ".editor",
        name: "margin",
        kind: ThemeClassPropertyKind::Box,
        default_value: "0",
        documentation: "Spacing outside code editor surfaces using CSS box shorthand.",
    },
    ThemeClassPropertySpec {
        selector: ".editor",
        name: "padding",
        kind: ThemeClassPropertyKind::Box,
        default_value: "0",
        documentation: "Spacing inside code editor surfaces using CSS box shorthand.",
    },
];

pub(crate) fn theme_class(selector: &str) -> Option<&'static ThemeClassSpec> {
    THEME_CLASSES
        .iter()
        .find(|class| class.selector == selector)
}

pub(crate) fn theme_class_property(
    selector: &str,
    name: &str,
) -> Option<&'static ThemeClassPropertySpec> {
    THEME_CLASS_PROPERTIES
        .iter()
        .find(|property| property.selector == selector && property.name == name)
}
