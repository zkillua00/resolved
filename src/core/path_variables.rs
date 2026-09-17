//! Lexical URL path placeholders. Do not parse/serialize the URL here: doing so
//! changes templates and can mistake credentials, query strings, or opaque URLs
//! for paths.
use std::ops::Range;

/// Names of `{name}` placeholders in the URL path, in first occurrence order.
/// Double-braced environment placeholders and percent-encoded braces are data.
pub fn path_variable_names(url: &str) -> Vec<String> {
    let mut names = Vec::new();
    for span in spans(url) {
        let name = &url[span.start + 1..span.end - 1];
        if !names.iter().any(|existing| existing == name) {
            names.push(name.to_owned());
        }
    }
    names
}

pub(super) fn spans(url: &str) -> Vec<Range<usize>> {
    let bytes = url.as_bytes();
    let start = url.len() - url.trim_start().len();
    // Find structural delimiters outside environment placeholders.
    let mut delimiters = Vec::new();
    let mut i = start;
    while i < bytes.len() {
        if url[i..].starts_with("{{") {
            let Some(end) = url[i + 2..].find("}}") else {
                break;
            };
            i += end + 4;
        } else {
            if b":/\\?#".contains(&bytes[i]) {
                delimiters.push(i);
            }
            i += url[i..].chars().next().unwrap().len_utf8();
        }
    }
    let mut authority = None;
    if url[start..].starts_with("//") {
        authority = Some(start + 2);
    } else if let Some(&colon) = delimiters.first()
        && bytes[colon] == b':'
    {
        // Never interpret even an invalid/templated scheme as a path.
        // Opaque scheme payloads have no hierarchical URL path.
        if !url[colon + 1..].starts_with("//") {
            return Vec::new();
        }
        authority = Some(colon + 3);
    } else if url[start..].starts_with("{{") {
        // A leading environment value may contain an entire base URL. Only
        // explicit path suffixes are local-variable syntax.
        if let Some(end) = url[start + 2..].find("}}") {
            authority = Some(start + end + 4);
        }
    }
    let path_start = if let Some(authority) = authority {
        match delimiters
            .iter()
            .copied()
            .find(|&i| i >= authority && bytes[i] != b':')
        {
            Some(i) if bytes[i] == b'/' => i,
            _ => return Vec::new(),
        }
    } else {
        start
    };
    let path_end = delimiters
        .iter()
        .copied()
        .find(|&i| i >= path_start && b"?#".contains(&bytes[i]))
        .unwrap_or(url.len());
    let mut result = Vec::new();
    i = path_start;
    while i < path_end {
        if url[i..].starts_with("{{") {
            let Some(end) = url[i + 2..path_end].find("}}") else {
                break;
            };
            i += end + 4;
        } else if bytes[i] == b'{' {
            let Some(end) = url[i + 1..path_end].find('}') else {
                break;
            };
            let end = i + 1 + end;
            let name = &url[i + 1..end];
            if name
                .as_bytes()
                .first()
                .is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_')
                && name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
            {
                result.push(i..end + 1);
            }
            i = end + 1;
        } else {
            i += url[i..].chars().next().unwrap().len_utf8();
        }
    }
    result
}

/// RFC 3986 segment data: only unreserved ASCII is emitted literally.
pub(super) fn encode(value: &str) -> String {
    let mut encoded = String::new();
    const HEX: &[u8] = b"0123456789ABCDEF";
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            encoded.push(byte as char);
        } else {
            encoded.push('%');
            encoded.push(HEX[(byte >> 4) as usize] as char);
            encoded.push(HEX[(byte & 15) as usize] as char);
        }
    }
    encoded
}

/// WHATWG URL parsers normalize even percent-encoded dot segments. Reject a
/// substituted segment instead of silently sending a different resource path.
pub(super) fn is_dot_segment(segment: &str) -> bool {
    let normalized = segment
        .replace(['\t', '\r', '\n'], "")
        .to_ascii_lowercase()
        .replace("%2e", ".");
    matches!(normalized.as_str(), "." | "..")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_only_path_names_in_order() {
        assert_eq!(
            path_variable_names(
                "https://{host}:{port}/{{env}}/{id}/{_a.b-c}/{id}/{9bad}/{a b}/{é}/%7Bid%7D?x={q}#{f}"
            ),
            ["id", "_a.b-c"]
        );
        for url in [
            "https://u:{password}@[::1]:80/{id}?q={q}#{f}",
            "//user:pass@[::1]:80/{id}",
            "wss://host/{id}",
            "custom+scheme://host/{id}",
            "{{base_url}}/users/{id}",
            "{{scheme}}://host/{id}",
            "{scheme}://host/{id}",
        ] {
            assert_eq!(path_variable_names(url), ["id"], "{url}");
        }
        for url in [
            "mailto:a/{id}",
            "urn:thing/{id}",
            "https://{host}",
            "{{base_url}}?q={id}",
            "https://host/{{id}}",
        ] {
            assert!(path_variable_names(url).is_empty(), "{url}");
        }
    }

    #[test]
    fn encodes_segment_data_and_detects_dot_spellings() {
        assert_eq!(encode("a/雪?#% {id}"), "a%2F%E9%9B%AA%3F%23%25%20%7Bid%7D");
        assert_eq!(encode("AZaz09-._~"), "AZaz09-._~");
        for dot in [".", "..", ".%2E", "%2e.", "%2E%2e", ".\t."] {
            assert!(is_dot_segment(dot));
        }
        assert!(!is_dot_segment("a.."));
    }
}
