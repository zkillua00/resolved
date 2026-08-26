//! Extra Tree-sitter languages used by generated request previews.
//!
//! `gpui-component` ships the common web and systems languages, but its C#
//! registration has no highlight query and it does not bundle PHP, Kotlin, or
//! PowerShell. Register the real grammars here so those export previews do not
//! silently fall back to plain text.

use gpui_component::highlighter::{LanguageConfig, LanguageRegistry};

const KOTLIN_HIGHLIGHTS_QUERY: &str = r#"
[
  (line_comment)
  (block_comment)
] @comment

[
  (string_literal)
  (multiline_string_literal)
  (character_literal)
] @string

(escape_sequence) @string.escape

[
  (number_literal)
  (float_literal)
] @number

(class_declaration name: (identifier) @type)
(function_declaration name: (identifier) @function)
(user_type (identifier) @type)
(call_expression (identifier) @function)
(annotation (user_type (identifier) @attribute))

[
  "abstract"
  "actual"
  "annotation"
  "as"
  "as?"
  "by"
  "catch"
  "class"
  "companion"
  "const"
  "constructor"
  "crossinline"
  "data"
  "delegate"
  "do"
  "dynamic"
  "else"
  "enum"
  "expect"
  "external"
  "field"
  "file"
  "final"
  "finally"
  "for"
  "fun"
  "get"
  "if"
  "import"
  "in"
  "infix"
  "init"
  "inline"
  "inner"
  "interface"
  "internal"
  "is"
  "lateinit"
  "noinline"
  "object"
  "open"
  "operator"
  "out"
  "override"
  "package"
  "param"
  "private"
  "property"
  "protected"
  "public"
  "receiver"
  "return"
  "return@"
  "sealed"
  "set"
  "setparam"
  "super"
  "super@"
  "suspend"
  "tailrec"
  "this"
  "this@"
  "throw"
  "try"
  "typealias"
  "val"
  "value"
  "var"
  "vararg"
  "when"
  "where"
  "while"
] @keyword

[
  "!"
  "!!"
  "!="
  "!=="
  "!in"
  "!is"
  "%"
  "%="
  "&"
  "&&"
  "*"
  "*="
  "+"
  "++"
  "+="
  "-"
  "--"
  "-="
  "->"
  ".."
  "..<"
  "/"
  "/="
  "::"
  "<"
  "<="
  "="
  "=="
  "==="
  ">"
  ">="
  "?."
  "?:"
  "||"
] @operator

[
  "("
  ")"
  "["
  "]"
  "{"
  "}"
] @punctuation.bracket

[
  ","
  "."
  ":"
  ";"
] @punctuation.delimiter
"#;

/// Register syntax definitions missing from `gpui-component`'s built-in set.
/// Re-registering is intentional and safe: the singleton replaces entries by
/// name, which also repairs the built-in C# entry that lacks a query.
pub fn register() {
    let registry = LanguageRegistry::singleton();

    registry.register(
        "csharp",
        &LanguageConfig::new(
            "csharp",
            tree_sitter::Language::new(tree_sitter_c_sharp::LANGUAGE),
            vec![],
            tree_sitter_c_sharp::HIGHLIGHTS_QUERY,
            "",
            "",
        ),
    );
    registry.register(
        "php",
        &LanguageConfig::new(
            "php",
            tree_sitter::Language::new(tree_sitter_php::LANGUAGE_PHP),
            vec![],
            tree_sitter_php::HIGHLIGHTS_QUERY,
            "",
            "",
        ),
    );
    registry.register(
        "kotlin",
        &LanguageConfig::new(
            "kotlin",
            tree_sitter::Language::new(tree_sitter_kotlin_ng::LANGUAGE),
            vec![],
            KOTLIN_HIGHLIGHTS_QUERY,
            "",
            "",
        ),
    );
    registry.register(
        "powershell",
        &LanguageConfig::new(
            "powershell",
            tree_sitter::Language::new(tree_sitter_powershell::LANGUAGE),
            vec![],
            tree_sitter_powershell::HIGHLIGHTS_QUERY,
            "",
            "",
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_component::{
        Rope,
        highlighter::{HighlightTheme, SyntaxHighlighter},
    };

    #[test]
    fn generated_language_grammars_produce_colored_ranges() {
        register();

        let samples = [
            (
                "csharp",
                "using System.Net.Http; var request = new HttpRequestMessage(HttpMethod.Post, \"https://example.com\");",
            ),
            (
                "php",
                "<?php $client = new GuzzleHttp\\Client(); echo $client->get('https://example.com');",
            ),
            (
                "kotlin",
                "import io.ktor.client.*\nsuspend fun main() { val client = HttpClient(); println(\"ready\") }",
            ),
            (
                "powershell",
                "$headers = @{ 'Accept' = 'application/json' }; Invoke-WebRequest -Uri 'https://example.com' -Headers $headers",
            ),
        ];
        let theme = HighlightTheme::default_dark();

        for (language, source) in samples {
            let rope = Rope::from_str(source);
            let mut highlighter = SyntaxHighlighter::new(language);
            highlighter.update(None, &rope);
            let styles = highlighter.styles(&(0..source.len()), &theme);
            assert!(
                styles.iter().any(|(range, style)| {
                    !range.is_empty()
                        && (style.color.is_some()
                            || style.font_style.is_some()
                            || style.font_weight.is_some())
                }),
                "{language} must produce at least one visible syntax style"
            );
        }
    }

    // Fold-region tests for the tree-derived folding used by the code editors.
    // Rows are 0-based natural lines; each region's end row is inclusive, and
    // a region only appears when it spans more than one line.
    fn fold_regions_for(language: &str, source: &str) -> Vec<(usize, usize)> {
        let rope = Rope::from_str(source);
        let mut highlighter = SyntaxHighlighter::new(language);
        highlighter.update(None, &rope);
        highlighter.fold_regions()
    }

    #[test]
    fn json_derives_object_and_array_fold_regions() {
        register();
        let regions = fold_regions_for(
            "json",
            "{\n  \"user\": {\n    \"name\": \"ada\",\n    \"roles\": [\"a\",\"b\",\"c\"]\n  },\n  \"count\": 3\n}",
        );
        // Top object 0..=6; nested object 1..=4 (the pair on the same line is
        // deduplicated); the array is single-line and therefore not foldable.
        assert_eq!(regions, vec![(0, 6), (1, 4)]);
    }

    #[test]
    fn json_deduplicates_nested_regions_sharing_a_header_line() {
        register();
        let regions = fold_regions_for(
            "json",
            "{\n  \"nested\": {\n    \"a\": 1,\n    \"b\": {\n      \"c\": [\n        2,\n        3\n      ]\n    }\n  }\n}",
        );
        // Outer object 0..=10, "nested" object 1..=9, "b" object 3..=8 and the
        // array 4..=7 nested inside it. Each starts on its own header line.
        assert_eq!(regions, vec![(0, 10), (1, 9), (3, 8), (4, 7)]);
    }

    #[test]
    fn javascript_derives_function_and_block_fold_regions() {
        register();
        let regions = fold_regions_for(
            "javascript",
            "function greet(name) {\n  const items = [\n    1,\n    2,\n  ];\n  if (name) {\n    console.log(\"hi\");\n  }\n}",
        );
        // The function and its body share a range and dedupe into 0..=8; the
        // array folds 1..=4 and the if-block 5..=7.
        assert_eq!(regions, vec![(0, 8), (1, 4), (5, 7)]);
    }
}
