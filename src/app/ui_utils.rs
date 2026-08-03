use super::*;

pub(super) fn resolved_brand_lockup(cx: &App) -> AnyElement {
    h_flex()
        .items_center()
        .gap_2()
        .child(img(ICON_ASSET_PATH).size(px(24.)))
        .child(
            div()
                .text_xl()
                .font_semibold()
                .text_color(cx.theme().foreground)
                .child(PRODUCT_NAME),
        )
        .into_any_element()
}

pub(super) fn input_text_equals(input: &Entity<InputState>, expected: &str, cx: &App) -> bool {
    let input = input.read(cx);
    let text = input.text();
    text.len() == expected.len() && text.chars().eq(expected.chars())
}

pub(super) fn input_text_is_blank(input: &Entity<InputState>, cx: &App) -> bool {
    input.read(cx).text().chars().all(char::is_whitespace)
}

pub(super) fn compact_url(url: &str) -> String {
    const MAX_CHARS: usize = 64;
    if url.chars().count() <= MAX_CHARS {
        return url.to_owned();
    }
    let prefix: String = url.chars().take(MAX_CHARS - 1).collect();
    format!("{prefix}…")
}

pub(super) fn unique_name<'a>(base: &str, existing: impl IntoIterator<Item = &'a str>) -> String {
    let existing = existing.into_iter().collect::<Vec<_>>();
    if !existing.iter().any(|name| name.eq_ignore_ascii_case(base)) {
        return base.to_owned();
    }

    for suffix in 2.. {
        let candidate = format!("{base} {suffix}");
        if !existing
            .iter()
            .any(|name| name.eq_ignore_ascii_case(&candidate))
        {
            return candidate;
        }
    }
    unreachable!("an unused numeric suffix must exist")
}

pub(super) fn default_request_name(request: &RequestDraft) -> String {
    let endpoint = url::Url::parse(request.url.trim())
        .ok()
        .and_then(|url| {
            let host = url.host_str()?.to_owned();
            let path = url.path().trim_matches('/');
            Some(if path.is_empty() {
                host
            } else {
                format!("{host}/{path}")
            })
        })
        .unwrap_or_else(|| compact_url(request.url.trim()));
    format!(
        "{} {}",
        request.method.trim().to_ascii_uppercase(),
        endpoint
    )
    .trim()
    .to_owned()
}

pub(super) fn code_language_for_raw_body(language: RawBodyLanguage) -> CodeLanguage {
    match language {
        RawBodyLanguage::Text => CodeLanguage::Plain,
        RawBodyLanguage::Json => CodeLanguage::Json,
        RawBodyLanguage::Xml => CodeLanguage::Html,
        RawBodyLanguage::Html => CodeLanguage::Html,
        RawBodyLanguage::JavaScript => CodeLanguage::JavaScript,
        RawBodyLanguage::TypeScript => CodeLanguage::TypeScript,
        RawBodyLanguage::Css => CodeLanguage::Css,
        RawBodyLanguage::Markdown => CodeLanguage::Markdown,
        RawBodyLanguage::GraphQl => CodeLanguage::GraphQl,
        RawBodyLanguage::Yaml => CodeLanguage::Yaml,
        RawBodyLanguage::Toml => CodeLanguage::Toml,
        RawBodyLanguage::Sql => CodeLanguage::Sql,
        RawBodyLanguage::Shell => CodeLanguage::Shell,
        RawBodyLanguage::Rust => CodeLanguage::Rust,
        RawBodyLanguage::Python => CodeLanguage::Python,
    }
}

pub(super) fn format_raw_body_source(
    language: RawBodyLanguage,
    source: &str,
    settings: &crate::core::FormatterSettings,
) -> Result<String, String> {
    crate::core::format_raw_source(language, source, settings)
}

pub(super) fn response_language(response: &ResponseData) -> CodeLanguage {
    let content_type = response
        .content_type
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();

    if content_type.contains("json") {
        CodeLanguage::Json
    } else if content_type.contains("javascript") || content_type.contains("ecmascript") {
        CodeLanguage::JavaScript
    } else if content_type.contains("html") {
        CodeLanguage::Html
    } else if content_type.contains("css") {
        CodeLanguage::Css
    } else if content_type.contains("markdown") {
        CodeLanguage::Markdown
    } else if content_type.contains("yaml") {
        CodeLanguage::Yaml
    } else if content_type.contains("toml") {
        CodeLanguage::Toml
    } else if content_type.contains("xml") {
        CodeLanguage::Html
    } else if serde_json::from_slice::<serde_json::Value>(&response.body).is_ok() {
        CodeLanguage::Json
    } else {
        CodeLanguage::from("text")
    }
}

pub(super) fn compact_label(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }
    let visible = max_chars.saturating_sub(1);
    let head = visible.saturating_mul(2) / 3;
    let tail = visible.saturating_sub(head);
    let prefix = value.chars().take(head).collect::<String>();
    let suffix = value
        .chars()
        .rev()
        .take(tail)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("{prefix}…{suffix}")
}

pub(super) fn format_duration(duration: std::time::Duration) -> String {
    if duration.as_secs() >= 1 {
        format!("{:.2} s", duration.as_secs_f64())
    } else {
        format!("{} ms", duration.as_millis())
    }
}

pub(super) fn format_bytes(bytes: usize) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    if bytes as f64 >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB)
    } else if bytes as f64 >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB)
    } else {
        format!("{bytes} B")
    }
}

pub(super) fn method_color(method: &str, cx: &App) -> Hsla {
    match method.trim().to_ascii_uppercase().as_str() {
        "GET" => cx.theme().green,
        "POST" => cx.theme().yellow,
        "PUT" => cx.theme().blue,
        "PATCH" => cx.theme().magenta,
        "DELETE" => cx.theme().red,
        "HEAD" => cx.theme().cyan,
        "OPTIONS" => cx.theme().magenta_light,
        _ => cx.theme().cyan,
    }
}

pub(super) fn status_color(status: u16, cx: &App) -> Hsla {
    match status {
        200..=299 => cx.theme().success,
        300..=399 => cx.theme().info,
        400..=499 => cx.theme().warning,
        _ => cx.theme().danger,
    }
}
