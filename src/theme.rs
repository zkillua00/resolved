use std::sync::Arc;

use gpui::{App, Hsla, px, rgb};
use gpui_component::{
    Theme, ThemeMode,
    highlighter::{HighlightTheme, HighlightThemeStyle},
};

pub fn color(hex: u32) -> Hsla {
    rgb(hex).into()
}

pub fn surface() -> Hsla {
    color(0x14121a)
}

pub fn surface_lowest() -> Hsla {
    color(0x0f0d14)
}

pub fn surface_low() -> Hsla {
    color(0x1c1a22)
}

pub fn surface_container() -> Hsla {
    color(0x201e26)
}

pub fn surface_high() -> Hsla {
    color(0x2b2931)
}

pub fn surface_highest() -> Hsla {
    color(0x36343c)
}

pub fn outline_variant() -> Hsla {
    color(0x49454f)
}

pub fn primary_bright() -> Hsla {
    color(0xe9ddff)
}

pub fn primary_lavender() -> Hsla {
    color(0xd0bcff)
}

pub fn selected_container() -> Hsla {
    color(0x4a4359)
}

pub fn configure(cx: &mut App) {
    const HIGHLIGHT_THEME: &str = r##"{
        "editor.background": "#0f0d14",
        "editor.foreground": "#e6e0eb",
        "editor.active_line.background": "#201e26",
        "editor.line_number": "#948f9a",
        "editor.active_line_number": "#d0bcff",
        "syntax": {
            "property": { "color": "#d0bcff" },
            "string": { "color": "#ffd9e3" },
            "number": { "color": "#efb8c8" },
            "boolean": { "color": "#ffb4ab" },
            "keyword": { "color": "#e9ddff", "font_weight": 500 },
            "comment": { "color": "#948f9a", "font_style": "italic" },
            "punctuation": { "color": "#cac4d0" },
            "variable": { "color": "#e6e0eb" },
            "type": { "color": "#ccc2dc" },
            "function": { "color": "#e9ddff" }
        }
    }"##;
    let highlight_style: HighlightThemeStyle =
        serde_json::from_str(HIGHLIGHT_THEME).expect("valid API Tester highlight theme");
    let highlight = HighlightTheme {
        name: "API Tester Material Dark".to_owned(),
        appearance: ThemeMode::Dark,
        style: highlight_style,
    };

    let theme = Theme::global_mut(cx);
    theme.mode = ThemeMode::Dark;
    theme.font_family = ".SystemUIFont".into();
    theme.font_size = px(16.);
    theme.mono_font_family = "Menlo".into();
    theme.mono_font_size = px(13.);
    theme.radius = px(8.);
    theme.radius_lg = px(12.);
    theme.shadow = false;
    theme.highlight_theme = Arc::new(highlight);

    let colors = &mut theme.colors;
    colors.background = surface();
    colors.foreground = color(0xe6e0eb);
    colors.muted = surface_high();
    colors.muted_foreground = color(0xcac4d0);
    colors.border = outline_variant();
    colors.input = outline_variant();
    colors.ring = primary_lavender();
    colors.drag_border = primary_lavender();
    colors.selection = selected_container();
    colors.caret = primary_lavender();

    colors.primary = primary_bright();
    colors.primary_hover = primary_lavender();
    colors.primary_active = color(0xb9a6ed);
    colors.primary_foreground = color(0x37265e);

    colors.secondary = selected_container();
    colors.secondary_hover = surface_highest();
    colors.secondary_active = color(0x5a526a);
    colors.secondary_foreground = color(0xe9def9);

    colors.accent = surface_high();
    colors.accent_foreground = color(0xe6e0eb);
    colors.popover = surface_container();
    colors.popover_foreground = color(0xe6e0eb);
    colors.overlay = surface_lowest().opacity(0.78);

    colors.sidebar = surface_low();
    colors.sidebar_foreground = color(0xe6e0eb);
    colors.sidebar_border = outline_variant();
    colors.sidebar_accent = selected_container().opacity(0.62);
    colors.sidebar_accent_foreground = primary_bright();
    colors.sidebar_primary = primary_lavender();
    colors.sidebar_primary_foreground = color(0x37265e);

    colors.title_bar = surface();
    colors.title_bar_border = outline_variant();
    colors.tab = surface();
    colors.tab_bar = surface();
    colors.tab_bar_segmented = surface_high();
    colors.tab_foreground = color(0xcac4d0);
    colors.tab_active = surface();
    colors.tab_active_foreground = primary_bright();

    colors.list = surface();
    colors.list_head = surface_low();
    colors.list_even = surface();
    colors.list_hover = surface_high();
    colors.list_active = selected_container().opacity(0.58);
    colors.list_active_border = primary_lavender();
    colors.table = surface();
    colors.table_head = surface_low();
    colors.table_head_foreground = color(0xcac4d0);
    colors.table_even = surface();
    colors.table_hover = surface_high();
    colors.table_active = selected_container().opacity(0.58);
    colors.table_active_border = primary_lavender();
    colors.table_row_border = outline_variant();

    colors.scrollbar = surface_lowest();
    colors.scrollbar_thumb = surface_high();
    colors.scrollbar_thumb_hover = surface_highest();
    colors.progress_bar = primary_lavender();

    colors.danger = color(0x8c1d18);
    colors.danger_hover = color(0xa52a24);
    colors.danger_active = color(0x71120e);
    colors.danger_foreground = color(0xffdad6);
    colors.warning = color(0xf4d06f);
    colors.warning_hover = color(0xffdc82);
    colors.warning_active = color(0xd7b34f);
    colors.warning_foreground = color(0x332b00);
    colors.success = color(0x2f6f44);
    colors.success_hover = color(0x3d8253);
    colors.success_active = color(0x245c36);
    colors.success_foreground = color(0xd0f8d8);
    colors.info = color(0x385f8e);
    colors.info_hover = color(0x4771a4);
    colors.info_active = color(0x2c4d77);
    colors.info_foreground = color(0xd7e9ff);

    colors.red = color(0xffb4ab);
    colors.red_light = color(0xffdad6);
    colors.green = color(0x4ade80);
    colors.green_light = color(0xb8f5c8);
    colors.blue = color(0x93c5fd);
    colors.blue_light = color(0xd7e9ff);
    colors.yellow = color(0xf4d06f);
    colors.yellow_light = color(0xffecb3);
    colors.magenta = color(0xf0abfc);
    colors.magenta_light = color(0xf7d5ff);
    colors.cyan = color(0x67e8f9);
    colors.cyan_light = color(0xc9f8ff);
}
