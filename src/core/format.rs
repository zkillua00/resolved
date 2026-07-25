use serde_json::Value;

/// Parse a UTF-8 JSON response and return a stable, indented representation.
pub fn pretty_json(body: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(body).ok()?;
    serde_json::to_string_pretty(&value).ok()
}

/// Produce the text shown in the response body pane.
///
/// When pretty-printing is enabled, valid JSON is formatted regardless of the
/// server's Content-Type. This helps with APIs that return JSON under a generic
/// or missing media type.
pub fn format_body(body: &[u8], pretty: bool) -> String {
    if pretty && let Some(json) = pretty_json(body) {
        return json;
    }

    String::from_utf8_lossy(body).into_owned()
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
            pretty_json(br#"{"ok":true,"items":[1,2]}"#).as_deref(),
            Some("{\n  \"ok\": true,\n  \"items\": [\n    1,\n    2\n  ]\n}")
        );
    }

    #[test]
    fn format_body_falls_back_to_original_text() {
        assert_eq!(format_body(b"plain response", true), "plain response");
    }

    #[test]
    fn binary_heuristic_rejects_nuls() {
        assert!(!is_probably_text(&[0x89, b'P', b'N', b'G', 0, 1, 2]));
        assert!(is_probably_text("hello, dünya".as_bytes()));
    }
}
