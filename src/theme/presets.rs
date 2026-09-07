use crate::core::{SavedTheme, ThemeSettings};

/// Bundled sources become ordinary saved themes. Never overwrite an edited
/// copy or resurrect a deleted theme on startup.
const PRESETS: &[(&str, &str)] = &[
    (
        "resolved-light",
        include_str!("../../assets/themes/resolved-light.css"),
    ),
    (
        "kanagawa-wave",
        include_str!("../../assets/themes/kanagawa-wave.css"),
    ),
    (
        "kanagawa-dragon",
        include_str!("../../assets/themes/kanagawa-dragon.css"),
    ),
    (
        "kanagawa-lotus",
        include_str!("../../assets/themes/kanagawa-lotus.css"),
    ),
    (
        "catppuccin-latte",
        include_str!("../../assets/themes/catppuccin-latte.css"),
    ),
    (
        "catppuccin-frappe",
        include_str!("../../assets/themes/catppuccin-frappe.css"),
    ),
    (
        "catppuccin-macchiato",
        include_str!("../../assets/themes/catppuccin-macchiato.css"),
    ),
    (
        "catppuccin-mocha",
        include_str!("../../assets/themes/catppuccin-mocha.css"),
    ),
    (
        "tokyo-night-night",
        include_str!("../../assets/themes/tokyo-night-night.css"),
    ),
    (
        "tokyo-night-storm",
        include_str!("../../assets/themes/tokyo-night-storm.css"),
    ),
    (
        "tokyo-night-moon",
        include_str!("../../assets/themes/tokyo-night-moon.css"),
    ),
    (
        "tokyo-night-day",
        include_str!("../../assets/themes/tokyo-night-day.css"),
    ),
];

pub(crate) fn install_bundled_themes(settings: &mut ThemeSettings) -> bool {
    if settings.bundled_themes_initialized {
        return false;
    }
    for (slug, source) in PRESETS {
        let parsed = super::parse_css(source).expect("bundled theme must be valid");
        let id = format!("bundled-{slug}");
        if settings.saved_theme(&id).is_none() {
            let mut saved = SavedTheme::new(parsed.name.as_ref(), *source, None);
            saved.id = id;
            let attribution = if slug.starts_with("kanagawa-") {
                Some("Tommaso Laurenzi · MIT")
            } else if slug.starts_with("catppuccin-") {
                Some("Catppuccin · MIT")
            } else if slug.starts_with("tokyo-night-") {
                Some("folke · Apache-2.0")
            } else {
                None
            };
            if let Some(attribution) = attribution {
                saved.extra.insert("attribution".into(), attribution.into());
            }
            settings.saved_themes.push(saved);
        }
    }
    settings.bundled_themes_initialized = true;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_catalog_preserves_selection_edits_and_deletions() {
        let mut settings = ThemeSettings::default();
        let custom = SavedTheme::new("My theme", super::super::bundled_css(), None);
        settings.active_theme_id = Some(custom.id.clone());
        settings.css_source = Some(custom.css_source.clone());
        settings.saved_themes.push(custom.clone());
        assert!(install_bundled_themes(&mut settings));
        assert_eq!(settings.saved_themes.len(), PRESETS.len() + 1);
        assert_eq!(
            settings.active_theme_id.as_deref(),
            Some(custom.id.as_str())
        );
        assert_eq!(
            settings.css_source.as_deref(),
            Some(custom.css_source.as_str())
        );
        settings.saved_themes.remove(1);
        settings.saved_themes[1].name = "Edited preset".into();
        let serialized = serde_json::to_string(&settings).unwrap();
        let mut restored: ThemeSettings = serde_json::from_str(&serialized).unwrap();
        assert!(!install_bundled_themes(&mut restored));
        assert_eq!(restored, settings);
    }

    #[test]
    fn all_variants_parse_and_switches_have_contrast() {
        fn luminance(color: gpui::Hsla) -> f32 {
            let rgb = color.to_rgb();
            let linear = |v: f32| {
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * linear(rgb.r) + 0.7152 * linear(rgb.g) + 0.0722 * linear(rgb.b)
        }
        let mut light_count = 0;
        for (slug, source) in PRESETS {
            let theme = super::super::parse_css(source).unwrap();
            light_count += usize::from(!theme.mode.is_dark());
            let c = theme.palette.component_colors;
            for (track, thumb) in [
                (c.primary, c.primary_foreground),
                (c.switch, c.switch_thumb),
            ] {
                let (a, b) = (luminance(track), luminance(thumb));
                let ratio = (a.max(b) + 0.05) / (a.min(b) + 0.05);
                assert!(ratio >= 3.0, "{slug}: switch contrast {ratio}");
            }
        }
        assert_eq!(PRESETS.len(), 12);
        assert_eq!(light_count, 4);
    }
}
