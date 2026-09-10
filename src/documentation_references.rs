//! Literal resource links in Markdown. No expression evaluation and no values.
use std::{collections::BTreeMap, ops::Range};

use markdown::mdast::Node;

use crate::core::{NodeKind, NodeStatus, RequestNamespaceCatalog, RequestNamespaceNode, Workspace};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resource {
    Request {
        request_id: String,
    },
    EnvironmentVariable {
        environment_id: String,
        variable_id: String,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReferenceCatalog(pub BTreeMap<String, Resource>);

#[derive(Default)]
pub struct ReferenceActions {
    pub source: String,
    pub targets: BTreeMap<usize, Vec<(String, Resource)>>,
}

impl ReferenceCatalog {
    pub fn from_workspace(workspace: &Workspace) -> Self {
        fn visit(node: &RequestNamespaceNode, path: String, catalog: &mut ReferenceCatalog) {
            if node.status != NodeStatus::Exposed {
                return;
            }
            match &node.kind {
                NodeKind::Request(info) => {
                    catalog.0.insert(
                        path,
                        Resource::Request {
                            request_id: info.request_id.clone(),
                        },
                    );
                }
                NodeKind::Namespace => {
                    for child in &node.children {
                        visit(child, format!("{path}{}", child.access.render()), catalog);
                    }
                }
            }
        }
        let mut catalog = Self::default();
        for root in RequestNamespaceCatalog::for_navigation(workspace).roots() {
            visit(
                root,
                root.access.render().trim_start_matches('.').to_owned(),
                &mut catalog,
            );
        }
        if let Some(environment) = workspace.active_environment() {
            let mut names = BTreeMap::<&str, Vec<&crate::core::EnvironmentVariable>>::new();
            for variable in &environment.variables {
                names
                    .entry(variable.key.as_str())
                    .or_default()
                    .push(variable);
            }
            for (name, variables) in names {
                if name.is_empty() || variables.len() != 1 {
                    continue;
                }
                let quoted = serde_json::to_string(name).expect("string");
                catalog.0.insert(
                    format!("api.environment[{quoted}]"),
                    Resource::EnvironmentVariable {
                        environment_id: environment.id.clone(),
                        variable_id: variables[0].id.clone(),
                    },
                );
            }
        }
        catalog
    }
}

#[derive(Clone, Debug)]
pub struct Reference {
    pub range: Range<usize>,
    pub expression_range: Range<usize>,
    pub complete: bool,
}

impl Reference {
    pub fn expression<'a>(&self, source: &'a str) -> &'a str {
        source[self.expression_range.clone()].trim()
    }
}

/// Scan only literal Markdown text, never code, HTML, existing links or images.
/// Quoted names may contain `)`; an unfinished reference ends at the line end.
pub fn references(source: &str) -> Vec<Reference> {
    fn visit(node: &Node, source: &str, found: &mut Vec<Reference>) {
        if let Node::Text(text) = node {
            let Some(position) = &text.position else {
                return;
            };
            let end = position.end.offset;
            let mut cursor = position.start.offset;
            while let Some(relative) = source[cursor..end].find("@Ref(") {
                let start = cursor + relative;
                if source[..start]
                    .bytes()
                    .rev()
                    .take_while(|byte| *byte == b'\\')
                    .count()
                    % 2
                    == 1
                {
                    cursor = start + 5;
                    continue;
                }
                let inner = start + 5;
                let mut quoted = false;
                let mut escaped = false;
                // A quoted resource name may contain Markdown punctuation.
                // The opener must be literal text, but its name can span the
                // Markdown parser's emphasis/code nodes on this same line.
                let line_end = source[inner..]
                    .find(['\r', '\n'])
                    .map_or(source.len(), |offset| inner + offset);
                let mut stop = line_end;
                let mut complete = false;
                for (relative, character) in source[inner..line_end].char_indices() {
                    if matches!(character, '\r' | '\n') {
                        stop = inner + relative;
                        break;
                    }
                    if character == '"' && !escaped {
                        quoted = !quoted;
                    }
                    if character == ')' && !quoted {
                        stop = inner + relative;
                        complete = true;
                        break;
                    }
                    escaped = character == '\\' && !escaped;
                }
                found.push(Reference {
                    range: start..stop + usize::from(complete),
                    expression_range: inner..stop,
                    complete,
                });
                cursor = stop + usize::from(complete);
                if cursor >= end {
                    break;
                }
            }
        } else if !matches!(
            node,
            Node::Code(_)
                | Node::InlineCode(_)
                | Node::Html(_)
                | Node::Link(_)
                | Node::Image(_)
                | Node::LinkReference(_)
                | Node::ImageReference(_)
        ) {
            if let Some(children) = node.children() {
                for child in children {
                    visit(child, source, found);
                }
            }
        }
    }
    let mut found = Vec::new();
    if let Ok(root) = markdown::to_mdast(source, &markdown::ParseOptions::default()) {
        visit(&root, source, &mut found);
    }
    found.sort_by_key(|reference| reference.range.start);
    let mut previous_end = 0;
    found.retain(|reference| {
        if reference.range.start < previous_end {
            return false;
        }
        previous_end = reference.range.end;
        true
    });
    found
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn workspace_fixture() -> Workspace {
        let mut workspace = Workspace::default();
        let collection = workspace.create_collection("Backend").unwrap();
        let folder = workspace
            .create_collection_folder(&collection, None, "Auth")
            .unwrap();
        workspace
            .create_saved_request_in_folder(
                &collection,
                Some(&folder),
                "Login",
                crate::core::RequestTemplate::new(crate::core::RequestDraft::new(
                    "POST",
                    "https://example.test/login?secret=never-expose-url",
                )),
            )
            .unwrap();
        workspace
            .create_saved_request(
                &collection,
                "Socket",
                crate::core::RequestTemplate::websocket(crate::core::WebSocketWorkspace::default()),
            )
            .unwrap();
        let environment = workspace.create_environment("Development").unwrap();
        workspace
            .add_environment_variable(&environment, "var_name", "never-expose-value", false, true)
            .unwrap();
        workspace
            .set_active_environment(Some(&environment))
            .unwrap();
        workspace
    }

    #[test]
    fn catalogs_link_requests_and_variables_without_values() {
        let workspace = workspace_fixture();
        let catalog = ReferenceCatalog::from_workspace(&workspace);
        assert!(matches!(
            catalog.0["Backend.Auth.Login"],
            Resource::Request { .. }
        ));
        assert!(matches!(
            catalog.0["Backend.Socket"],
            Resource::Request { .. }
        ));
        assert!(matches!(
            catalog.0["api.environment[\"var_name\"]"],
            Resource::EnvironmentVariable { .. }
        ));
        assert!(!format!("{catalog:?}").contains("never-expose"));
        assert!(
            RequestNamespaceCatalog::from_workspace(&workspace)
                .request_at(&["Backend".into(), "Socket".into()])
                .is_none()
        );
    }

    #[test]
    fn deleted_ambiguous_and_inactive_targets_are_not_resolved() {
        let mut workspace = workspace_fixture();
        let mut duplicate = workspace
            .collections
            .iter()
            .find(|collection| collection.name == "Backend")
            .unwrap()
            .clone();
        duplicate.id = "duplicate".into();
        workspace.collections.push(duplicate);
        let environment = workspace
            .environments
            .iter_mut()
            .find(|environment| environment.name == "Development")
            .unwrap();
        let mut duplicate = environment.variables[0].clone();
        duplicate.id = "duplicate-variable".into();
        environment.variables.push(duplicate);
        let catalog = ReferenceCatalog::from_workspace(&workspace);
        assert!(!catalog.0.contains_key("Backend.Auth.Login"));
        assert!(!catalog.0.contains_key("api.environment[\"var_name\"]"));
        workspace.set_active_environment(None).unwrap();
        assert!(
            !ReferenceCatalog::from_workspace(&workspace)
                .0
                .keys()
                .any(|key| key.starts_with("api.environment"))
        );
    }

    #[test]
    fn inline_references_preserve_offsets_and_quoted_parentheses() {
        let source = "先 @Ref(Backend.Auth.Login) then @Ref(api.environment[\"var)name\"]).";
        let refs = references(source);
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].expression(source), "Backend.Auth.Login");
        assert_eq!(refs[1].expression(source), "api.environment[\"var)name\"]");
        assert!(refs.iter().all(|reference| reference.complete));
        assert_eq!(&source[refs[0].range.clone()], "@Ref(Backend.Auth.Login)");
    }

    #[test]
    fn examples_and_existing_links_are_inert() {
        for source in [
            "```\n@Ref(Backend.Login)\n```",
            "    @Ref(Backend.Login)",
            "`@Ref(Backend.Login)`",
            "<!-- @Ref(Backend.Login) -->",
            "[see @Ref(Backend.Login)](https://example.com)",
            "\\@Ref(Backend.Login)",
        ] {
            assert!(references(source).is_empty(), "{source}");
        }
    }

    #[test]
    fn incomplete_reference_does_not_consume_the_next_line() {
        let source = "@Ref(Backend.\nNext @Ref(Backend.Login)";
        let refs = references(source);
        assert_eq!(refs.len(), 2);
        assert!(!refs[0].complete);
        assert!(refs[1].complete);
    }

    #[test]
    fn quoted_resource_names_can_contain_markdown_punctuation() {
        for expression in [
            "Backend[\"*Saved* request\"]",
            "Backend[\"`Saved` request\"]",
            "api.environment[\"<token>\"]",
        ] {
            let source = format!("See @Ref({expression}).");
            let refs = references(&source);
            assert_eq!(refs.len(), 1, "{source}");
            assert!(refs[0].complete, "{source}");
            assert_eq!(refs[0].expression(&source), expression);
        }
    }
}
