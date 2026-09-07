use std::{ops::Range, path::Path};

use dprint_plugin_typescript::{
    FormatTextOptions,
    configuration::{ConfigurationBuilder, QuoteStyle, SemiColons, TrailingCommas},
};
use serde::Serialize as _;
use serde_json::Value;

use super::{
    FormatterQuoteStyle, FormatterSemicolons, FormatterSettings, FormatterTrailingCommas,
    RawBodyLanguage,
};

fn pretty_json_with_settings(body: &[u8], settings: &FormatterSettings) -> Option<String> {
    let value: Value = serde_json::from_slice(body).ok()?;
    serialize_json_value(&value, settings).ok()
}

/// Produce the text shown in the response body pane.
///
/// When pretty-printing is enabled, valid JSON is formatted regardless of the
/// server's Content-Type. This helps with APIs that return JSON under a generic
/// or missing media type.
pub fn format_body(body: &[u8], pretty: bool, settings: &FormatterSettings) -> String {
    if pretty && let Some(json) = pretty_json_with_settings(body, settings) {
        return json;
    }

    String::from_utf8_lossy(body).into_owned()
}

/// Format an editable raw-body buffer using the formatter settings that are
/// shared with script editors.
///
/// Only languages backed by a parser/printer are accepted. Returning an error
/// for every other language keeps formatting from guessing and changing a
/// request's wire payload accidentally.
pub fn format_raw_source(
    language: RawBodyLanguage,
    source: &str,
    settings: &FormatterSettings,
) -> Result<String, String> {
    match language {
        RawBodyLanguage::Json => format_json_source(source, settings),
        RawBodyLanguage::JsonLines => format_json_lines_source(source),
        RawBodyLanguage::JavaScript => {
            format_javascript_source(source, Path::new("request-body.js"), settings)
        }
        RawBodyLanguage::TypeScript => {
            format_javascript_source(source, Path::new("request-body.ts"), settings)
        }
        language => Err(format!(
            "Format buffer is not available for {language} yet. The buffer was not changed."
        )),
    }
}

/// Format a pre-request or post-response JavaScript buffer.
pub fn format_script_source(source: &str, settings: &FormatterSettings) -> Result<String, String> {
    format_javascript_source(source, Path::new("request-script.js"), settings)
}

fn format_json_source(source: &str, settings: &FormatterSettings) -> Result<String, String> {
    let value = serde_json::from_str::<Value>(source)
        .map_err(|error| format!("JSON could not be formatted: {error}"))?;
    serialize_json_value(&value, settings)
}

#[derive(Debug)]
pub struct JsonLineRecord {
    pub range: Range<usize>,
    pub start_line: usize,
    pub value: Value,
}

/// Parse a whitespace-delimited stream of complete JSON values.
///
/// This intentionally accepts a value that spans physical lines. The composer
/// still emits one WebSocket frame per value, while letting people keep a
/// larger object readable before it is sent or formatted.
pub fn parse_json_lines(source: &str) -> Result<Vec<JsonLineRecord>, String> {
    let mut records = Vec::new();
    let mut stream = serde_json::Deserializer::from_str(source).into_iter::<Value>();
    let mut previous_end = 0;

    loop {
        let start = source[previous_end..]
            .char_indices()
            .find_map(|(offset, character)| {
                (!character.is_whitespace()).then_some(previous_end + offset)
            });
        let Some(start) = start else {
            break;
        };
        let start_line = source[..start]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count()
            + 1;

        let Some(value) = stream.next() else {
            break;
        };
        let value = value.map_err(|error| {
            format!(
                "JSONL record {} starting on line {} is not valid JSON: {error}",
                records.len() + 1,
                start_line
            )
        })?;
        let end = stream.byte_offset();
        records.push(JsonLineRecord {
            range: start..end,
            start_line,
            value,
        });
        previous_end = end;
    }

    Ok(records)
}

fn format_json_lines_source(source: &str) -> Result<String, String> {
    parse_json_lines(source)?
        .into_iter()
        .map(|record| {
            serde_json::to_string(&record.value).map_err(|error| {
                format!(
                    "JSONL record starting on line {} could not be formatted: {error}",
                    record.start_line
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|records| records.join("\n"))
}

fn serialize_json_value(value: &Value, settings: &FormatterSettings) -> Result<String, String> {
    let indent = if settings.hard_tabs {
        vec![b'\t']
    } else {
        vec![b' '; settings.effective_indent_size()]
    };
    let formatter = serde_json::ser::PrettyFormatter::with_indent(&indent);
    let mut output = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut output, formatter);
    value
        .serialize(&mut serializer)
        .map_err(|error| format!("JSON could not be formatted: {error}"))?;
    String::from_utf8(output)
        .map_err(|error| format!("JSON formatter produced invalid UTF-8: {error}"))
}

fn format_javascript_source(
    source: &str,
    path: &Path,
    settings: &FormatterSettings,
) -> Result<String, String> {
    let language_label = if path.extension().and_then(|extension| extension.to_str()) == Some("ts")
    {
        "TypeScript"
    } else {
        "JavaScript"
    };
    let mut builder = ConfigurationBuilder::new();
    builder
        .line_width(settings.effective_line_width() as u32)
        .indent_width(settings.effective_indent_size() as u8)
        .use_tabs(settings.hard_tabs)
        .quote_style(match settings.quote_style {
            FormatterQuoteStyle::Double => QuoteStyle::PreferDouble,
            FormatterQuoteStyle::Single => QuoteStyle::PreferSingle,
        })
        .semi_colons(match settings.semicolons {
            FormatterSemicolons::Prefer => SemiColons::Prefer,
            FormatterSemicolons::Asi => SemiColons::Asi,
        })
        .trailing_commas(match settings.trailing_commas {
            FormatterTrailingCommas::Never => TrailingCommas::Never,
            FormatterTrailingCommas::MultiLine => TrailingCommas::OnlyMultiLine,
            FormatterTrailingCommas::Always => TrailingCommas::Always,
        });
    let config = builder.build();
    dprint_plugin_typescript::format_text(FormatTextOptions {
        path,
        extension: None,
        text: source.to_owned(),
        config: &config,
        external_formatter: None,
    })
    .map(|formatted| formatted.unwrap_or_else(|| source.to_owned()))
    .map_err(|error| format!("{language_label} could not be formatted: {error}"))
}

/// Conservative heuristic used to avoid dumping arbitrary binary data into a
/// text editor. An empty response is considered text.
pub fn is_probably_text(body: &[u8]) -> bool {
    if body.is_empty() || std::str::from_utf8(body).is_ok() {
        return true;
    }

    // Permit a small number of non-UTF-8 bytes for otherwise textual legacy
    // responses, but reject embedded NULs immediately.
    if body.contains(&0) {
        return false;
    }

    let sample = &body[..body.len().min(8 * 1024)];
    let control_bytes = sample
        .iter()
        .filter(|byte| matches!(byte, 0x00..=0x08 | 0x0b | 0x0c | 0x0e..=0x1f))
        .count();
    control_bytes * 100 <= sample.len().max(1) * 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pretty_prints_json() {
        assert_eq!(
            format_body(
                br#"{"ok":true,"items":[1,2]}"#,
                true,
                &FormatterSettings::default()
            ),
            "{\n  \"ok\": true,\n  \"items\": [\n    1,\n    2\n  ]\n}"
        );
    }

    #[test]
    fn format_body_falls_back_to_original_text() {
        assert_eq!(
            format_body(b"plain response", true, &FormatterSettings::default()),
            "plain response"
        );
    }

    #[test]
    fn response_json_pretty_printing_uses_formatter_indentation() {
        let settings = FormatterSettings {
            indent_size: 4,
            ..Default::default()
        };
        assert_eq!(
            format_body(br#"{"nested":{"ok":true}}"#, true, &settings),
            "{\n    \"nested\": {\n        \"ok\": true\n    }\n}"
        );
    }

    #[test]
    fn editable_json_formatting_uses_configured_indentation() {
        let settings = FormatterSettings {
            indent_size: 4,
            ..Default::default()
        };
        assert_eq!(
            format_raw_source(
                RawBodyLanguage::Json,
                r#"{"nested":{"ok":true}}"#,
                &settings
            )
            .unwrap(),
            "{\n    \"nested\": {\n        \"ok\": true\n    }\n}"
        );

        let settings = FormatterSettings {
            hard_tabs: true,
            ..Default::default()
        };
        assert_eq!(
            format_raw_source(
                RawBodyLanguage::Json,
                r#"{"nested":{"ok":true}}"#,
                &settings
            )
            .unwrap(),
            "{\n\t\"nested\": {\n\t\t\"ok\": true\n\t}\n}"
        );
    }

    #[test]
    fn json_lines_formatting_accepts_a_multiline_value() {
        assert_eq!(
            format_raw_source(
                RawBodyLanguage::JsonLines,
                "{\n  \"type\": \"one\",\n  \"nested\": true\n}\n\n[1, 2]\n",
                &FormatterSettings::default(),
            )
            .unwrap(),
            "{\"type\":\"one\",\"nested\":true}\n[1,2]"
        );
    }

    #[test]
    fn json_lines_parser_returns_complete_multiline_record_ranges() {
        let source = "{\n  \"type\": \"one\"\n}\n{\"type\":\"two\"}";
        let records = parse_json_lines(source).unwrap();

        assert_eq!(records.len(), 2);
        assert_eq!(
            &source[records[0].range.clone()],
            "{\n  \"type\": \"one\"\n}"
        );
        assert_eq!(&source[records[1].range.clone()], "{\"type\":\"two\"}");
        assert_eq!(records[0].start_line, 1);
        assert_eq!(records[1].start_line, 4);
    }

    #[test]
    fn json_lines_formatting_reports_the_invalid_line() {
        let error = format_raw_source(
            RawBodyLanguage::JsonLines,
            "{\"ok\":true}\nnot-json",
            &FormatterSettings::default(),
        )
        .unwrap_err();
        assert!(error.starts_with("JSONL record 2 starting on line 2 is not valid JSON:"));
    }

    #[test]
    fn javascript_formatting_honors_visible_style_preferences() {
        let settings = FormatterSettings {
            quote_style: FormatterQuoteStyle::Single,
            semicolons: FormatterSemicolons::Asi,
            trailing_commas: FormatterTrailingCommas::Never,
            ..Default::default()
        };
        let formatted =
            format_script_source("const value={message:\"hello\",items:[1,2,],};", &settings)
                .unwrap();
        assert_eq!(
            formatted,
            "const value = { message: 'hello', items: [1, 2] }\n"
        );
    }

    #[test]
    fn raw_typescript_uses_the_typescript_parser() {
        let formatted = format_raw_source(
            RawBodyLanguage::TypeScript,
            "const value:{name:string}={name:\"ok\"};",
            &FormatterSettings::default(),
        )
        .unwrap();

        assert_eq!(
            formatted,
            "const value: { name: string } = { name: \"ok\" };\n"
        );
    }

    #[test]
    fn binary_heuristic_rejects_nuls() {
        assert!(!is_probably_text(&[0x89, b'P', b'N', b'G', 0, 1, 2]));
        assert!(is_probably_text("hello, dünya".as_bytes()));
    }
}
