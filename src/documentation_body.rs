//! Value-free body target discovery for documentation annotations.
//!
//! Paths are decoded annotation arguments: the JSON root is the empty string
//! (written `""` in an annotation), not two quote characters. Ranges are UTF-8
//! byte offsets into the input: JSON members (key through value), array values
//! and root values; XML opening names and attribute
//! names. Neither source text nor scalar values survive parsing.

use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
};

use gpui_component::highlighter::LanguageRegistry;
use quick_xml::{Reader, events::Event};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyMode {
    Json,
    Xml,
}

#[derive(Default, PartialEq, Eq)]
struct Targets {
    names: BTreeSet<String>,
    ranges: BTreeMap<String, Vec<Range<usize>>>,
    complete: bool,
}

impl Targets {
    fn insert(&mut self, path: String, range: Range<usize>) {
        self.names.insert(path.clone());
        self.ranges.entry(path).or_default().push(range);
    }
}

#[derive(Default, PartialEq, Eq)]
pub struct BodyTargets {
    json: Targets,
    xml: Targets,
}

impl BodyTargets {
    pub fn parse(mode: BodyMode, source: &str) -> Self {
        let mut result = Self::default();
        match mode {
            BodyMode::Json => result.json = parse_json(source),
            BodyMode::Xml => result.xml = parse_xml(source),
        }
        result
    }

    fn targets(&self, mode: BodyMode) -> &Targets {
        match mode {
            BodyMode::Json => &self.json,
            BodyMode::Xml => &self.xml,
        }
    }

    pub fn names(&self, mode: BodyMode) -> &BTreeSet<String> {
        &self.targets(mode).names
    }

    /// A discovered target remains present even if another part is malformed.
    /// Absence is conclusive only after a complete, valid parse of this mode.
    pub fn contains(&self, mode: BodyMode, path: &str) -> Option<bool> {
        let targets = self.targets(mode);
        if targets.names.contains(path) {
            Some(true)
        } else if targets.complete {
            Some(false)
        } else {
            None
        }
    }

    pub fn ranges(&self, mode: BodyMode, path: &str) -> Vec<Range<usize>> {
        self.targets(mode)
            .ranges
            .get(path)
            .cloned()
            .unwrap_or_default()
    }
}

/// JSON Pointer syntax (RFC 6901), or literal XML child/attribute paths.
/// JSON tokens are not restricted to numbers: `"01"` can be an object key.
/// XML predicates, wildcards, descendants, and namespace expansion are absent.
pub fn valid_path(mode: BodyMode, path: &str) -> bool {
    match mode {
        BodyMode::Json => {
            if path.is_empty() {
                return true;
            }
            if !path.starts_with('/') {
                return false;
            }
            let mut chars = path.chars();
            while let Some(ch) = chars.next() {
                if ch == '~' && !matches!(chars.next(), Some('0' | '1')) {
                    return false;
                }
            }
            true
        }
        BodyMode::Xml => {
            let Some(path) = path.strip_prefix('/') else {
                return false;
            };
            let mut segments = path.split('/').peekable();
            let mut element_seen = false;
            while let Some(segment) = segments.next() {
                if let Some(name) = segment.strip_prefix('@') {
                    return element_seen && segments.peek().is_none() && qualified_name(name);
                }
                if !qualified_name(segment) {
                    return false;
                }
                element_seen = true;
            }
            element_seen
        }
    }
}

fn qualified_name(name: &str) -> bool {
    let mut parts = name.split(':');
    let first = parts.next().is_some_and(xml_name);
    first && parts.next().is_none_or(xml_name) && parts.next().is_none()
}

fn xml_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(name_start)
        && chars.all(|ch| {
            name_start(ch)
                || matches!(ch, '-' | '.' | '0'..='9' | '\u{b7}' | '\u{300}'..='\u{36f}' | '\u{203f}'..='\u{2040}')
        })
}

// XML 1.0 Fifth Edition NameStartChar, excluding ':' (handled as QName syntax).
fn name_start(ch: char) -> bool {
    matches!(ch, '_' | 'A'..='Z' | 'a'..='z'
        | '\u{c0}'..='\u{d6}' | '\u{d8}'..='\u{f6}' | '\u{f8}'..='\u{2ff}'
        | '\u{370}'..='\u{37d}' | '\u{37f}'..='\u{1fff}'
        | '\u{200c}'..='\u{200d}' | '\u{2070}'..='\u{218f}'
        | '\u{2c00}'..='\u{2fef}' | '\u{3001}'..='\u{d7ff}'
        | '\u{f900}'..='\u{fdcf}' | '\u{fdf0}'..='\u{fffd}'
        | '\u{10000}'..='\u{effff}')
}

fn parse_json(source: &str) -> Targets {
    let mut targets = Targets::default();
    let Some(config) = LanguageRegistry::singleton().language("json") else {
        return targets;
    };
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&config.language).is_err() {
        return targets;
    }
    let Some(tree) = parser.parse(source, None) else {
        return targets;
    };
    // The highlighting grammar accepts comments and recovery nodes. A strict
    // value-discarding deserializer decides whether missing targets are safe.
    targets.complete = serde_json::from_str::<serde::de::IgnoredAny>(source).is_ok();
    let root = tree.root_node();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        if child.kind() == "comment" {
            continue;
        }
        discover_json(child, child.byte_range(), "", source, &mut targets, 0);
        // Never assign multiple document values to the same root.
        break;
    }
    targets
}

fn discover_json(
    node: tree_sitter::Node<'_>,
    range: Range<usize>,
    path: &str,
    source: &str,
    targets: &mut Targets,
    depth: usize,
) {
    // A resource limit is uncertainty, never evidence that a path is missing.
    if depth > 256 {
        targets.complete = false;
        return;
    }
    if node.is_missing() || node.is_error() {
        return;
    }
    match node.kind() {
        "object" | "array" => targets.insert(path.to_owned(), range),
        "string" | "number" | "true" | "false" | "null" if !node.has_error() => {
            targets.insert(path.to_owned(), range);
            return;
        }
        _ => return,
    }
    let mut cursor = node.walk();
    let mut index = 0;
    for child in node.named_children(&mut cursor) {
        // Recovery may have lost structure; do not invent ancestry or indices.
        if child.is_error() || child.is_missing() {
            break;
        }
        if child.kind() == "comment" {
            continue;
        }
        if node.kind() == "array" {
            discover_json(
                child,
                child.byte_range(),
                &format!("{path}/{index}"),
                source,
                targets,
                depth + 1,
            );
            index += 1;
        } else if child.kind() == "pair" {
            let (Some(key), Some(value)) = (
                child.child_by_field_name("key"),
                child.child_by_field_name("value"),
            ) else {
                continue;
            };
            // A missing colon is parser recovery, not a confirmed object field.
            let mut pair_cursor = child.walk();
            if !child
                .children(&mut pair_cursor)
                .any(|part| part.kind() == ":" && !part.is_missing())
            {
                continue;
            }
            let Ok(key) = serde_json::from_str::<String>(&source[key.byte_range()]) else {
                continue;
            };
            let token = key.replace('~', "~0").replace('/', "~1");
            discover_json(
                value,
                child.byte_range(),
                &format!("{path}/{token}"),
                source,
                targets,
                depth + 1,
            );
        }
    }
}

fn parse_xml(source: &str) -> Targets {
    // quick-xml supplies borrowed, streaming tokens and useful partial results.
    // It is not a document validator (e.g. multiple roots); roxmltree is. DTDs
    // are deliberately not enabled, so no external resources are ever fetched.
    let mut targets = Targets {
        complete: roxmltree::Document::parse(source).is_ok(),
        ..Targets::default()
    };
    let mut reader = Reader::from_str(source);
    let mut stack: Vec<String> = Vec::new();
    let mut root_seen = false;
    loop {
        match reader.read_event() {
            Ok(event @ (Event::Start(_) | Event::Empty(_))) => {
                // Do not advertise a tag that is still being typed.
                let end = reader.buffer_position() as usize;
                if end == 0 || source.as_bytes().get(end - 1) != Some(&b'>') {
                    targets.complete = false;
                    break;
                }
                if stack.len() > 256 {
                    targets.complete = false;
                    break;
                }
                if stack.is_empty() && root_seen {
                    break;
                }
                let empty = matches!(&event, Event::Empty(_));
                let (Event::Start(tag) | Event::Empty(tag)) = event else {
                    unreachable!()
                };
                let tag_name = tag.name();
                let Ok(name) = std::str::from_utf8(tag_name.as_ref()) else {
                    break;
                };
                if !qualified_name(name) {
                    break;
                }
                let path = format!("{}/{name}", stack.last().map(String::as_str).unwrap_or(""));
                let mut attributes = Vec::new();
                let mut valid = true;
                for attribute in tag.attributes() {
                    let Ok(attribute) = attribute else {
                        valid = false;
                        break;
                    };
                    let Ok(name) = std::str::from_utf8(attribute.key.as_ref()) else {
                        valid = false;
                        break;
                    };
                    if !qualified_name(name) {
                        valid = false;
                        break;
                    }
                    if let Some(range) = borrowed_range(source, attribute.key.as_ref()) {
                        attributes.push((format!("{path}/@{name}"), range));
                    }
                }
                if !valid {
                    break;
                }
                if let Some(range) = borrowed_range(source, tag_name.as_ref()) {
                    targets.insert(path.clone(), range);
                }
                for (path, range) in attributes {
                    targets.insert(path, range);
                }
                root_seen = true;
                if !empty {
                    stack.push(path);
                }
            }
            Ok(Event::End(_)) => {
                if stack.pop().is_none() {
                    break;
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    targets
}

/// Slice-reader events borrow their names directly from the original input.
fn borrowed_range(source: &str, bytes: &[u8]) -> Option<Range<usize>> {
    let start = (bytes.as_ptr() as usize).checked_sub(source.as_ptr() as usize)?;
    let end = start.checked_add(bytes.len())?;
    (end <= source.len()).then_some(start..end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(mode: BodyMode, source: &str) -> BTreeSet<String> {
        BodyTargets::parse(mode, source).names(mode).clone()
    }

    #[test]
    fn json_pointer_validation() {
        for path in ["", "/", "//", "/0", "/01", "/-", "/a~1b/~0", "/雪", "/a b"] {
            assert!(valid_path(BodyMode::Json, path), "{path}");
        }
        for path in ["\"\"", "a", "#/a", "/~", "/~2", "/a~01~"] {
            assert!(!valid_path(BodyMode::Json, path), "{path}");
        }
    }

    #[test]
    fn xml_paths_are_literal_not_xpath() {
        for path in ["/root", "/ns:root/child/@xml:lang", "/雪/é", "/a-b/a.b/_a"] {
            assert!(valid_path(BodyMode::Xml, path), "{path}");
        }
        for path in [
            "",
            "/",
            "//a",
            "a",
            "/a/",
            "/@a",
            "/a/@b/c",
            "/a[1]",
            "/a/*",
            "/a/..",
            "/a/text()",
            "/a:b:c",
            "/:a",
            "/a:",
            "/1a",
            "/a/@",
        ] {
            assert!(!valid_path(BodyMode::Xml, path), "{path}");
        }
    }

    #[test]
    fn json_discovers_root_containers_arrays_and_escaped_keys() {
        let source = r#"{"a/b":{"~":[true,null,{"":1}]},"雪":"secret","\u0061":2}"#;
        assert_eq!(
            names(BodyMode::Json, source),
            [
                "",
                "/a~1b",
                "/a~1b/~0",
                "/a~1b/~0/0",
                "/a~1b/~0/1",
                "/a~1b/~0/2",
                "/a~1b/~0/2/",
                "/雪",
                "/a",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect()
        );
    }

    #[test]
    fn json_scalar_roots_and_empty_containers() {
        for source in ["null", "false", "42", "\"secret\"", "{}", "[]"] {
            let targets = BodyTargets::parse(BodyMode::Json, source);
            assert_eq!(targets.contains(BodyMode::Json, ""), Some(true));
            assert_eq!(targets.contains(BodyMode::Json, "/missing"), Some(false));
            assert_eq!(targets.ranges(BodyMode::Json, ""), vec![0..source.len()]);
        }
    }

    #[test]
    fn json_ranges_include_keys_and_duplicate_keys_accumulate() {
        let source = r#"{"雪":1,"雪":22}"#;
        let targets = BodyTargets::parse(BodyMode::Json, source);
        let slices: Vec<_> = targets
            .ranges(BodyMode::Json, "/雪")
            .into_iter()
            .map(|range| &source[range])
            .collect();
        assert_eq!(slices, [r#""雪":1"#, r#""雪":22"#]);
    }

    #[test]
    fn malformed_json_never_proves_absence() {
        for source in [
            "",
            "{",
            "[",
            r#"{"a":1,"b":}"#,
            r#"{"a":1"#,
            "/* comment */ {}",
            "{} {}",
            "[1,,2]",
            r#"{"a" 1}"#,
        ] {
            let targets = BodyTargets::parse(BodyMode::Json, source);
            assert_eq!(
                targets.contains(BodyMode::Json, "/missing"),
                None,
                "{source}"
            );
        }
        let targets = BodyTargets::parse(BodyMode::Json, r#"{"a":1,"b":}"#);
        assert_eq!(targets.contains(BodyMode::Json, "/a"), Some(true));
        assert_eq!(targets.contains(BodyMode::Json, "/b"), None);
    }

    #[test]
    fn modes_and_unavailable_state_are_separate() {
        let targets = BodyTargets::default();
        assert_eq!(targets.contains(BodyMode::Json, ""), None);
        assert_eq!(targets.contains(BodyMode::Xml, "/root"), None);
        let targets = BodyTargets::parse(BodyMode::Json, "{}");
        assert_eq!(targets.contains(BodyMode::Xml, "/root"), None);
        assert!(targets.names(BodyMode::Xml).is_empty());
        assert!(targets.ranges(BodyMode::Json, "/missing").is_empty());
    }

    #[test]
    fn xml_repeated_elements_attributes_and_literal_namespaces() {
        let source =
            r#"<p:root xmlns:p="urn:test"><p:item id="secret"/><p:item id="other"/></p:root>"#;
        let targets = BodyTargets::parse(BodyMode::Xml, source);
        assert_eq!(
            targets.names(BodyMode::Xml),
            &[
                "/p:root",
                "/p:root/@xmlns:p",
                "/p:root/p:item",
                "/p:root/p:item/@id",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect()
        );
        assert_eq!(targets.contains(BodyMode::Xml, "/p:root/item"), Some(false));
        for (path, expected) in [
            ("/p:root/p:item", vec!["p:item", "p:item"]),
            ("/p:root/p:item/@id", vec!["id", "id"]),
        ] {
            let actual: Vec<_> = targets
                .ranges(BodyMode::Xml, path)
                .into_iter()
                .map(|range| &source[range])
                .collect();
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn xml_comments_cdata_declarations_and_text_are_not_targets() {
        let source = "<?xml version=\"1.0\"?><雪 a=\"&amp;\">text<!-- <fake/> --><![CDATA[<fake/>]]><child/></雪>";
        let targets = BodyTargets::parse(BodyMode::Xml, source);
        assert_eq!(
            targets.names(BodyMode::Xml),
            &["/雪", "/雪/@a", "/雪/child"]
                .into_iter()
                .map(str::to_owned)
                .collect()
        );
        assert_eq!(targets.contains(BodyMode::Xml, "/fake"), Some(false));
        assert_eq!(
            &source[targets.ranges(BodyMode::Xml, "/雪")[0].clone()],
            "雪"
        );
    }

    #[test]
    fn malformed_xml_preserves_confirmed_prefix_without_false_missing() {
        for source in [
            "",
            "<",
            "<root>",
            "<root><child/></wrong>",
            "<root/><other/>",
            "<root>&undefined;</root>",
            "<root a='1' a='2'/>",
            "<root><child",
            "<root><child/></root>garbage",
            "<p:root/>",
        ] {
            let targets = BodyTargets::parse(BodyMode::Xml, source);
            assert_eq!(
                targets.contains(BodyMode::Xml, "/missing"),
                None,
                "{source}"
            );
        }
        let targets = BodyTargets::parse(BodyMode::Xml, "<root><child a='x'/><broken");
        assert_eq!(targets.contains(BodyMode::Xml, "/root"), Some(true));
        assert_eq!(
            targets.contains(BodyMode::Xml, "/root/child/@a"),
            Some(true)
        );
        assert_eq!(targets.contains(BodyMode::Xml, "/root/broken"), None);
    }

    #[test]
    fn changing_equal_length_values_does_not_change_the_index() {
        assert!(
            BodyTargets::parse(BodyMode::Json, r#"{"a":"secret"}"#)
                == BodyTargets::parse(BodyMode::Json, r#"{"a":"hidden"}"#)
        );
        assert!(
            BodyTargets::parse(BodyMode::Xml, "<root a='secret'>secret</root>")
                == BodyTargets::parse(BodyMode::Xml, "<root a='hidden'>hidden</root>")
        );
    }

    #[test]
    fn depth_limits_make_absence_uncertain() {
        let json = format!("{}0{}", "[".repeat(300), "]".repeat(300));
        let targets = BodyTargets::parse(BodyMode::Json, &json);
        assert_eq!(targets.contains(BodyMode::Json, ""), Some(true));
        assert_eq!(targets.contains(BodyMode::Json, "/missing"), None);
        let xml = format!("{}{}", "<a>".repeat(300), "</a>".repeat(300));
        let targets = BodyTargets::parse(BodyMode::Xml, &xml);
        assert_eq!(targets.contains(BodyMode::Xml, "/a"), Some(true));
        assert_eq!(targets.contains(BodyMode::Xml, "/missing"), None);
    }
}
