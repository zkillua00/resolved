use std::{collections::BTreeMap, sync::Arc};

use gpui::{Edges, Hsla, Pixels, SharedString, px};
use gpui_component::{
    ButtonClassStyle, ThemeColor, ThemeMode,
    highlighter::{HighlightTheme, HighlightThemeStyle},
};

/// A parsed, immutable Resolved theme.
#[derive(Clone, Debug)]
pub struct ApiTheme {
    pub name: SharedString,
    pub mode: ThemeMode,
    pub palette: ApiPalette,
    pub classes: ApiThemeClasses,
}

#[derive(Clone, Debug)]
pub struct ApiThemeClasses {
    pub app: AppClassStyle,
    pub button: ButtonClassStyle,
    pub editor: EditorClassStyle,
}

#[derive(Clone, Debug)]
pub struct AppClassStyle {
    pub font_family: SharedString,
    pub font_size: Pixels,
    pub margin: Edges<Pixels>,
    pub padding: Edges<Pixels>,
}

#[derive(Clone, Debug)]
pub struct EditorClassStyle {
    pub font_family: SharedString,
    pub font_size: Pixels,
    pub border_radius: Option<Pixels>,
    pub margin: Edges<Pixels>,
    pub padding: Edges<Pixels>,
}

impl Default for ApiThemeClasses {
    fn default() -> Self {
        Self {
            app: AppClassStyle {
                font_family: ".SystemUIFont".into(),
                font_size: px(16.),
                margin: Edges::all(Pixels::ZERO),
                padding: Edges::all(Pixels::ZERO),
            },
            button: ButtonClassStyle::default(),
            editor: EditorClassStyle {
                font_family: ".SystemUIFont".into(),
                font_size: px(14.),
                border_radius: None,
                margin: Edges::all(Pixels::ZERO),
                padding: Edges::all(Pixels::ZERO),
            },
        }
    }
}

/// Application-specific colors plus their gpui-component projections.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct ApiPalette {
    pub surface: Hsla,
    pub surface_lowest: Hsla,
    pub surface_low: Hsla,
    pub surface_container: Hsla,
    pub surface_high: Hsla,
    pub surface_highest: Hsla,
    pub outline_variant: Hsla,
    pub primary_bright: Hsla,
    pub primary_lavender: Hsla,
    pub selected_container: Hsla,
    pub component_colors: ThemeColor,
    pub highlight_theme: Arc<HighlightTheme>,
    #[cfg(test)]
    tokens: Arc<BTreeMap<String, Hsla>>,
}

#[cfg(test)]
impl ApiPalette {
    pub fn token(&self, name: &str) -> Option<Hsla> {
        self.tokens.get(name).copied()
    }
}

impl ApiTheme {
    pub(crate) fn from_resolved(
        name: String,
        mode: ThemeMode,
        tokens: BTreeMap<String, Hsla>,
        classes: ApiThemeClasses,
    ) -> Self {
        let surface = required(&tokens, "--api-surface");
        let surface_lowest = required(&tokens, "--api-surface-lowest");
        let surface_low = required(&tokens, "--api-surface-low");
        let surface_container = required(&tokens, "--api-surface-container");
        let surface_high = required(&tokens, "--api-surface-high");
        let surface_highest = required(&tokens, "--api-surface-highest");
        let foreground = required(&tokens, "--api-foreground");
        let muted_foreground = required(&tokens, "--api-muted-foreground");
        let outline_variant = required(&tokens, "--api-outline");
        let primary_bright = required(&tokens, "--api-primary");
        let primary_lavender = required(&tokens, "--api-primary-hover");
        let primary_active = required(&tokens, "--api-primary-active");
        let primary_foreground = required(&tokens, "--api-primary-foreground");
        let selected_container = required(&tokens, "--api-selection");

        let danger = required(&tokens, "--api-danger");
        let danger_hover = optional(&tokens, "--api-danger-hover", danger);
        let danger_active = optional(&tokens, "--api-danger-active", danger);
        let danger_foreground = required(&tokens, "--api-danger-foreground");
        let warning = required(&tokens, "--api-warning");
        let warning_hover = optional(&tokens, "--api-warning-hover", warning);
        let warning_active = optional(&tokens, "--api-warning-active", warning);
        let warning_foreground = required(&tokens, "--api-warning-foreground");
        let success = required(&tokens, "--api-success");
        let success_hover = optional(&tokens, "--api-success-hover", success);
        let success_active = optional(&tokens, "--api-success-active", success);
        let success_foreground = required(&tokens, "--api-success-foreground");
        let info = required(&tokens, "--api-info");
        let info_hover = optional(&tokens, "--api-info-hover", info);
        let info_active = optional(&tokens, "--api-info-active", info);
        let info_foreground = required(&tokens, "--api-info-foreground");

        let red = required(&tokens, "--api-red");
        let red_light = optional(&tokens, "--api-red-light", red);
        let green = required(&tokens, "--api-green");
        let green_light = optional(&tokens, "--api-green-light", green);
        let blue = required(&tokens, "--api-blue");
        let blue_light = optional(&tokens, "--api-blue-light", blue);
        let yellow = required(&tokens, "--api-yellow");
        let yellow_light = optional(&tokens, "--api-yellow-light", yellow);
        let magenta = required(&tokens, "--api-magenta");
        let magenta_light = optional(&tokens, "--api-magenta-light", magenta);
        let cyan = required(&tokens, "--api-cyan");
        let cyan_light = optional(&tokens, "--api-cyan-light", cyan);

        let mut colors = if mode.is_dark() {
            *ThemeColor::dark()
        } else {
            *ThemeColor::light()
        };
        colors.accent = surface_high;
        colors.accent_foreground = foreground;
        colors.accordion = surface;
        colors.accordion_hover = surface_high;
        colors.background = surface;
        colors.border = outline_variant;
        colors.group_box = surface_low;
        colors.group_box_foreground = foreground;
        colors.caret = primary_lavender;
        colors.chart_1 = primary_lavender;
        colors.chart_2 = magenta;
        colors.chart_3 = cyan;
        colors.chart_4 = yellow;
        colors.chart_5 = red;
        colors.danger = danger;
        colors.danger_active = danger_active;
        colors.danger_foreground = danger_foreground;
        colors.danger_hover = danger_hover;
        colors.description_list_label = surface_low;
        colors.description_list_label_foreground = muted_foreground;
        colors.drag_border = primary_lavender;
        colors.drop_target = selected_container.opacity(0.62);
        colors.foreground = foreground;
        colors.info = info;
        colors.info_active = info_active;
        colors.info_foreground = info_foreground;
        colors.info_hover = info_hover;
        colors.input = outline_variant;
        colors.link = primary_lavender;
        colors.link_active = primary_active;
        colors.link_hover = primary_bright;
        colors.list = surface;
        colors.list_active = selected_container.opacity(0.58);
        colors.list_active_border = primary_lavender;
        colors.list_even = surface;
        colors.list_head = surface_low;
        colors.list_hover = surface_high;
        colors.muted = surface_high;
        colors.muted_foreground = muted_foreground;
        colors.popover = surface_container;
        colors.popover_foreground = foreground;
        colors.primary = primary_bright;
        colors.primary_active = primary_active;
        colors.primary_foreground = primary_foreground;
        colors.primary_hover = primary_lavender;
        colors.progress_bar = primary_lavender;
        colors.ring = primary_lavender;
        colors.scrollbar = surface_lowest;
        colors.scrollbar_thumb = surface_high;
        colors.scrollbar_thumb_hover = surface_highest;
        colors.secondary = selected_container;
        colors.secondary_active = optional(&tokens, "--api-secondary-active", selected_container);
        colors.secondary_foreground = optional(&tokens, "--api-secondary-foreground", foreground);
        colors.secondary_hover = surface_highest;
        colors.selection = selected_container;
        colors.sidebar = surface_low;
        colors.sidebar_accent = selected_container.opacity(0.62);
        colors.sidebar_accent_foreground = primary_bright;
        colors.sidebar_border = outline_variant;
        colors.sidebar_foreground = foreground;
        colors.sidebar_primary = primary_lavender;
        colors.sidebar_primary_foreground = primary_foreground;
        colors.skeleton = surface_high;
        colors.slider_bar = surface_high;
        colors.slider_thumb = primary_lavender;
        colors.success = success;
        colors.success_foreground = success_foreground;
        colors.success_hover = success_hover;
        colors.success_active = success_active;
        colors.bullish = green;
        colors.bearish = red;
        colors.switch = surface_highest;
        colors.switch_thumb = foreground;
        colors.tab = surface;
        colors.tab_active = surface;
        colors.tab_active_foreground = primary_bright;
        colors.tab_bar = surface;
        colors.tab_bar_segmented = surface_high;
        colors.tab_foreground = muted_foreground;
        colors.table = surface;
        colors.table_active = selected_container.opacity(0.58);
        colors.table_active_border = primary_lavender;
        colors.table_even = surface;
        colors.table_head = surface_low;
        colors.table_head_foreground = muted_foreground;
        colors.table_hover = surface_high;
        colors.table_row_border = outline_variant;
        colors.title_bar = surface;
        colors.title_bar_border = outline_variant;
        colors.tiles = surface;
        colors.warning = warning;
        colors.warning_active = warning_active;
        colors.warning_hover = warning_hover;
        colors.warning_foreground = warning_foreground;
        colors.overlay = surface_lowest.opacity(0.78);
        colors.window_border = outline_variant;
        colors.red = red;
        colors.red_light = red_light;
        colors.green = green;
        colors.green_light = green_light;
        colors.blue = blue;
        colors.blue_light = blue_light;
        colors.yellow = yellow;
        colors.yellow_light = yellow_light;
        colors.magenta = magenta;
        colors.magenta_light = magenta_light;
        colors.cyan = cyan;
        colors.cyan_light = cyan_light;

        let highlight_theme = Arc::new(build_highlight_theme(&name, mode, &tokens));
        Self {
            name: name.into(),
            mode,
            classes,
            palette: ApiPalette {
                surface,
                surface_lowest,
                surface_low,
                surface_container,
                surface_high,
                surface_highest,
                outline_variant,
                primary_bright,
                primary_lavender,
                selected_container,
                component_colors: colors,
                highlight_theme,
                #[cfg(test)]
                tokens: Arc::new(tokens),
            },
        }
    }
}

fn required(tokens: &BTreeMap<String, Hsla>, name: &str) -> Hsla {
    *tokens
        .get(name)
        .unwrap_or_else(|| panic!("validated theme is missing {name}"))
}

fn optional(tokens: &BTreeMap<String, Hsla>, name: &str, fallback: Hsla) -> Hsla {
    tokens.get(name).copied().unwrap_or(fallback)
}

fn build_highlight_theme(
    name: &str,
    mode: ThemeMode,
    tokens: &BTreeMap<String, Hsla>,
) -> HighlightTheme {
    let foreground = required(tokens, "--api-foreground");
    let muted_foreground = required(tokens, "--api-muted-foreground");
    let primary = required(tokens, "--api-primary");
    let primary_hover = required(tokens, "--api-primary-hover");
    let red = required(tokens, "--api-red");
    let style = serde_json::json!({
        "editor.background": color_hex(optional(
            tokens,
            "--api-editor-background",
            required(tokens, "--api-surface-lowest"),
        )),
        "editor.foreground": color_hex(optional(
            tokens,
            "--api-editor-foreground",
            foreground,
        )),
        "editor.active_line.background": color_hex(optional(
            tokens,
            "--api-editor-active-line",
            required(tokens, "--api-surface-container"),
        )),
        "editor.line_number": color_hex(optional(
            tokens,
            "--api-editor-line-number",
            muted_foreground,
        )),
        "editor.active_line_number": color_hex(optional(
            tokens,
            "--api-editor-active-line-number",
            primary_hover,
        )),
        "syntax": {
            "property": {
                "color": color_hex(optional(tokens, "--api-syntax-property", primary_hover)),
            },
            "string": {
                "color": color_hex(optional(tokens, "--api-syntax-string", foreground)),
            },
            "number": {
                "color": color_hex(optional(tokens, "--api-syntax-number", primary_hover)),
            },
            "boolean": {
                "color": color_hex(optional(tokens, "--api-syntax-boolean", red)),
            },
            "keyword": {
                "color": color_hex(optional(tokens, "--api-syntax-keyword", primary)),
                "font_weight": 500,
            },
            "comment": {
                "color": color_hex(optional(tokens, "--api-syntax-comment", muted_foreground)),
                "font_style": "italic",
            },
            "punctuation": {
                "color": color_hex(optional(
                    tokens,
                    "--api-syntax-punctuation",
                    muted_foreground,
                )),
            },
            "variable": {
                "color": color_hex(optional(tokens, "--api-syntax-variable", foreground)),
            },
            "type": {
                "color": color_hex(optional(tokens, "--api-syntax-type", primary_hover)),
            },
            "function": {
                "color": color_hex(optional(tokens, "--api-syntax-function", primary)),
            },
        },
    });
    let overrides: HighlightThemeStyle =
        serde_json::from_value(style).expect("generated highlight theme is valid");
    let mut theme = if mode.is_dark() {
        HighlightTheme::default_dark().as_ref().clone()
    } else {
        HighlightTheme::default_light().as_ref().clone()
    };

    theme.name = name.to_owned();
    theme.appearance = mode;
    theme.style.editor_background = overrides.editor_background;
    theme.style.editor_foreground = overrides.editor_foreground;
    theme.style.editor_active_line = overrides.editor_active_line;
    theme.style.editor_line_number = overrides.editor_line_number;
    theme.style.editor_active_line_number = overrides.editor_active_line_number;
    theme.style.syntax.property = overrides.syntax.property;
    theme.style.syntax.string = overrides.syntax.string;
    theme.style.syntax.number = overrides.syntax.number;
    theme.style.syntax.boolean = overrides.syntax.boolean;
    theme.style.syntax.keyword = overrides.syntax.keyword;
    theme.style.syntax.comment = overrides.syntax.comment;
    theme.style.syntax.punctuation = overrides.syntax.punctuation;
    theme.style.syntax.variable = overrides.syntax.variable;
    theme.style.syntax.type_ = overrides.syntax.type_;
    theme.style.syntax.function = overrides.syntax.function;
    theme
}

fn color_hex(color: Hsla) -> String {
    let rgba = color.to_rgb();
    let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}{:02x}",
        channel(rgba.r),
        channel(rgba.g),
        channel(rgba.b),
        channel(rgba.a)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::css::{BUILTIN_THEME_CSS, parse_theme_css};

    #[test]
    fn preserves_unconfigured_default_syntax_captures() {
        let theme = parse_theme_css(BUILTIN_THEME_CSS).unwrap();
        let default = HighlightTheme::default_dark();

        assert!(default.style.syntax.constant.is_some());
        assert_eq!(
            theme.palette.highlight_theme.style.syntax.constant,
            default.style.syntax.constant
        );
        assert_eq!(
            theme.palette.highlight_theme.style.syntax.string_escape,
            default.style.syntax.string_escape
        );
    }
}
