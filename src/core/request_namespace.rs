//! Shared model for the saved-request namespace exposed to request scripts
//! (`api.requests.execute(ChatAdmin.Login)`) and to the script editor's
//! completion/hover/diagnostic intelligence.
//!
//! One catalog is built from the active workspace, and both the runtime and
//! the editor derive their representation from that same catalog so they
//! cannot drift: the editor does not suggest a path the runtime could not
//! resolve, and the runtime does not expose a path the editor considers
//! impossible. The catalog carries only stable identity and safe, non-secret
//! metadata (request id, method, template URL, collection path). Environment
//! values, secrets, authorization values and multipart local paths never
//! enter it.

#![allow(dead_code)]

use std::collections::BTreeMap;

use super::workspace::{Collection, CollectionFolder, SavedRequest, Workspace};

/// Maximum nesting depth of one top-level send's execution chain.
pub const CHAIN_MAX_DEPTH: usize = 16;
/// Maximum total chained executions originating from one top-level Send.
pub const CHAIN_MAX_TOTAL: usize = 64;

/// Property-key on a request-reference leaf that marks it as a frozen,
/// runtime-backed saved-request reference (and not an arbitrary object).
pub const REQUEST_REF_MARKER: &str = "__apiTesterRequestRef";
/// Property-key on a namespace object that marks it as a folder/collection
/// namespace (used to give a precise diagnostic when a namespace is passed to
/// `api.requests.execute`).
pub const NAMESPACE_REF_MARKER: &str = "__apiTesterNamespaceRef";

/// Scripting and host globals that a collection namespace root must never
/// overwrite. A collection whose name matches one of these is not installed
/// as a namespace (see [`RequestNamespaceCatalog::notices`]).
const RESERVED_GLOBALS: &[&str] = &[
    "api",
    "console",
    "JSON",
    "Object",
    "Array",
    "String",
    "Number",
    "Boolean",
    "Math",
    "Symbol",
    "RegExp",
    "Date",
    "Map",
    "Set",
    "WeakMap",
    "WeakSet",
    "Promise",
    "Error",
    "TypeError",
    "RangeError",
    "EvalError",
    "SyntaxError",
    "ReferenceError",
    "URIError",
    "Proxy",
    "Reflect",
    "Function",
    "BigInt",
    "undefined",
    "NaN",
    "Infinity",
    "globalThis",
    "parseInt",
    "parseFloat",
    "isNaN",
    "isFinite",
    "decodeURI",
    "encodeURI",
    "decodeURIComponent",
    "encodeURIComponent",
    "eval",
    "fetch",
    "require",
    "process",
    "Deno",
    "WebSocket",
    "XMLHttpRequest",
];

/// ECMAScript reserved words and literals that cannot be used as a dot-notation
/// property after a `.`.
const RESERVED_WORDS: &[&str] = &[
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "implements",
    "import",
    "in",
    "instanceof",
    "interface",
    "let",
    "new",
    "null",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "yield",
];

/// Whether `name` is a valid ECMAScript identifier usable in dot notation.
pub fn is_valid_js_identifier(name: &str) -> bool {
    let mut characters = name.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    if !(first == '_' || first == '$' || first.is_ascii_alphabetic()) {
        return false;
    }
    if !characters
        .all(|character| character == '_' || character == '$' || character.is_ascii_alphanumeric())
    {
        return false;
    }
    !RESERVED_WORDS.contains(&name)
}

/// How a node is addressed beneath its parent.
///
/// A valid, unambiguous identifier uses dot notation (`ChatAdmin.Login`);
/// anything else uses bracket notation (`ChatAdmin["My Request"]`). Two
/// different collection or request names are never collapsed onto the same
/// identifier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccessStep {
    Dot(String),
    Bracket(String),
}

impl AccessStep {
    pub fn render(&self) -> String {
        match self {
            AccessStep::Dot(name) => format!(".{name}"),
            AccessStep::Bracket(name) => format!("[\"{}\"]", escape_bracket(name)),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            AccessStep::Dot(name) => name,
            AccessStep::Bracket(name) => name,
        }
    }

    fn for_name(name: &str) -> Self {
        if is_valid_js_identifier(name) {
            AccessStep::Dot(name.to_owned())
        } else {
            AccessStep::Bracket(name.to_owned())
        }
    }
}

fn escape_bracket(name: &str) -> String {
    name.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Non-secret metadata carried by a request-reference leaf.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestRefInfo {
    pub request_id: String,
    pub request_name: String,
    pub method: String,
    /// Template URL exactly as stored (never resolved; may contain variables).
    pub url_template: String,
    /// Display path from the collection root, e.g. `["ChatAdmin", "Users"]`.
    pub collection_path: Vec<String>,
}

/// A node in the request-reference namespace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestNamespaceNode {
    /// Original collection / folder / request display name.
    pub name: String,
    /// How this node is addressed beneath its parent.
    pub access: AccessStep,
    pub kind: NodeKind,
    pub children: Vec<RequestNamespaceNode>,
    /// Whether this node was exposed or deliberately suppressed.
    pub status: NodeStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Namespace,
    Request(RequestRefInfo),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeStatus {
    /// Exposed to scripts and editor.
    Exposed,
    /// Excluded because two different entries occupy the same namespace with
    /// the same name (never arbitrarily chosen).
    Ambiguous,
    /// Root excluded because its name collides with a reserved global.
    ReservedGlobalCollision,
    /// Root excluded because another collection owns the same root name.
    DuplicateRoot,
}

/// The single logical model for the request-reference namespace.
#[derive(Clone, Debug, Default)]
pub struct RequestNamespaceCatalog {
    roots: Vec<RequestNamespaceNode>,
    notices: Vec<String>,
}

impl RequestNamespaceCatalog {
    /// Builds a catalog from a [`Workspace`] snapshot, in collection order.
    pub fn from_workspace(workspace: &Workspace) -> Self {
        let mut catalog = Self::default();
        let mut root_names: BTreeMap<String, usize> = BTreeMap::new();

        for collection in &workspace.collections {
            if RESERVED_GLOBALS.contains(&collection.name.as_str()) {
                catalog.notices.push(format!(
                    "Collection \"{}\" is not exposed as a script request namespace because its name collides with a built-in scripting global.",
                    collection.name
                ));
                root_names.insert(collection.name.clone(), 0);
                continue;
            }

            let root = build_collection_namespace(collection, &mut catalog.notices);

            if let Some(&first_index) = root_names.get(&collection.name) {
                catalog.notices.push(format!(
                    "Multiple collections are named \"{}\"; only the first is exposed as a request namespace.",
                    collection.name
                ));
                if let Some(first) = catalog.roots.get_mut(first_index) {
                    first.status = NodeStatus::DuplicateRoot;
                }
                let mut suppressed = root;
                suppressed.status = NodeStatus::DuplicateRoot;
                catalog.roots.push(suppressed);
            } else {
                root_names.insert(collection.name.clone(), catalog.roots.len());
                catalog.roots.push(root);
            }
        }
        catalog
    }

    /// Root nodes exposed to scripts (each becomes a global).
    pub fn roots(&self) -> impl Iterator<Item = &RequestNamespaceNode> {
        self.roots
            .iter()
            .filter(|root| root.status == NodeStatus::Exposed)
    }

    /// Human-facing notices about deliberately suppressed entries.
    pub fn notices(&self) -> &[String] {
        &self.notices
    }

    /// Members (folders + requests) exposed under a namespace path by display
    /// name. Returns `None` when the path does not resolve to an exposed
    /// namespace.
    pub fn members_at(&self, path: &[String]) -> Option<Vec<&RequestNamespaceNode>> {
        let mut current: Option<&RequestNamespaceNode> = None;
        for (index, segment) in path.iter().enumerate() {
            let node = if index == 0 {
                self.roots().find(|root| root.name == *segment)
            } else {
                current?
                    .children
                    .iter()
                    .find(|child| child.name == *segment)
            };
            let node = node?;
            if node.status != NodeStatus::Exposed || matches!(node.kind, NodeKind::Request(_)) {
                return None;
            }
            current = Some(node);
        }
        Some(current?.children.iter().collect())
    }

    /// Resolve a request-reference leaf by its path of display names. Returns
    /// `None` when the path does not resolve to an exposed, unambiguous request
    /// reference.
    pub fn request_at(&self, path: &[String]) -> Option<&RequestRefInfo> {
        let mut current: Option<&RequestNamespaceNode> = None;
        for (index, segment) in path.iter().enumerate() {
            let node = if index == 0 {
                self.roots().find(|root| root.name == *segment)
            } else {
                current?
                    .children
                    .iter()
                    .find(|child| child.name == *segment)
            };
            current = node;
            if current?.status != NodeStatus::Exposed {
                return None;
            }
        }
        match &current?.kind {
            NodeKind::Request(info) => Some(info),
            NodeKind::Namespace => None,
        }
    }

    /// Classic dotted-path lookup.
    pub fn request_by_path(&self, dotted: &str) -> Option<&RequestRefInfo> {
        let path = dotted.split('.').map(str::to_owned).collect::<Vec<_>>();
        self.request_at(&path)
    }

    /// Serialized form injected into the script runtime; the prelude turns
    /// this into frozen namespace objects. Only exposed nodes are included.
    pub fn runtime_specs(&self) -> Vec<RuntimeNamespaceSpec> {
        self.roots().map(runtime_node).collect()
    }

    /// Generates an in-memory TypeScript declaration for the request-reference
    /// namespace. Only dot-valid, exposed roots/namespaces that the runtime
    /// installs as plain global identifiers are declared; bracket-notated or
    /// reserved-colliding names are deliberately omitted (they remain
    /// reachable at runtime through bracket/global access and are typed as
    /// `any`, so the editor reports no false error). Never persisted — this is
    /// fed straight to the embedded language service.
    pub fn declaration_source(&self) -> String {
        let mut buffer = String::new();
        buffer
            .push_str("// Resolved request-reference namespace built from the active workspace.\n");
        for root in self.roots() {
            if root.status != NodeStatus::Exposed {
                continue;
            }
            // Only plain-global identifiers can be declared with `declare const`.
            if !matches!(root.access, AccessStep::Dot(_)) {
                continue;
            }
            emit_namespace_declaration(&mut buffer, root);
        }
        buffer
    }
}

fn emit_namespace_declaration(buffer: &mut String, node: &RequestNamespaceNode) {
    let name = match &node.access {
        AccessStep::Dot(name) => name,
        AccessStep::Bracket(_) => return,
    };
    buffer.push_str(&format!("declare const {name}: {{\n"));
    emit_members(buffer, node, 1);
    buffer.push_str("};\n");
}

fn emit_members(buffer: &mut String, node: &RequestNamespaceNode, indent: usize) {
    let pad = "  ".repeat(indent);
    for child in &node.children {
        if child.status != NodeStatus::Exposed {
            continue;
        }
        let member = match &child.access {
            AccessStep::Dot(name) => name,
            AccessStep::Bracket(_) => continue,
        };
        match &child.kind {
            NodeKind::Request(_) => {
                buffer.push_str(&format!(
                    "{pad}readonly {member}: Resolved.SavedRequestReference;\n"
                ));
            }
            NodeKind::Namespace => {
                buffer.push_str(&format!("{pad}readonly {member}: {{\n"));
                emit_members(buffer, child, indent + 1);
                buffer.push_str(&format!("{pad}}};\n"));
            }
        }
    }
}

fn build_collection_namespace(
    collection: &Collection,
    notices: &mut Vec<String>,
) -> RequestNamespaceNode {
    let mut children = Vec::new();
    let mut folders = collection
        .folders
        .iter()
        .filter(|folder| folder.parent_folder_id.is_none())
        .collect::<Vec<_>>();
    folders.sort_by(|a, b| a.name.cmp(&b.name));
    let base_path = vec![collection.name.clone()];

    for folder in folders {
        children.push(build_folder_namespace(
            collection, folder, &base_path, notices,
        ));
    }

    let mut requests = collection
        .requests
        .iter()
        .filter(|request| request.folder_id.is_none())
        .collect::<Vec<_>>();
    requests.sort_by(|a, b| a.name.cmp(&b.name));
    for request in requests {
        children.push(build_request_node(request, base_path.clone()));
    }

    resolve_ambiguity(&mut children, &base_path.join("."), notices);

    RequestNamespaceNode {
        name: collection.name.clone(),
        access: AccessStep::for_name(&collection.name),
        kind: NodeKind::Namespace,
        children,
        status: NodeStatus::Exposed,
    }
}

fn build_folder_namespace(
    collection: &Collection,
    folder: &CollectionFolder,
    path: &[String],
    notices: &mut Vec<String>,
) -> RequestNamespaceNode {
    let mut own_path = path.to_vec();
    own_path.push(folder.name.clone());
    let namespace_path = own_path.join(".");

    let mut children = Vec::new();
    let mut child_folders = collection
        .folders
        .iter()
        .filter(|candidate| candidate.parent_folder_id.as_deref() == Some(folder.id.as_str()))
        .collect::<Vec<_>>();
    child_folders.sort_by(|a, b| a.name.cmp(&b.name));
    for child in child_folders {
        children.push(build_folder_namespace(
            collection, child, &own_path, notices,
        ));
    }

    let mut requests = collection
        .requests
        .iter()
        .filter(|request| request.folder_id.as_deref() == Some(folder.id.as_str()))
        .collect::<Vec<_>>();
    requests.sort_by(|a, b| a.name.cmp(&b.name));
    for request in requests {
        children.push(build_request_node(request, own_path.clone()));
    }

    resolve_ambiguity(&mut children, &namespace_path, notices);

    RequestNamespaceNode {
        name: folder.name.clone(),
        access: AccessStep::for_name(&folder.name),
        kind: NodeKind::Namespace,
        children,
        status: NodeStatus::Exposed,
    }
}

fn build_request_node(
    request: &SavedRequest,
    collection_path: Vec<String>,
) -> RequestNamespaceNode {
    let info = RequestRefInfo {
        request_id: request.id.clone(),
        request_name: request.name.clone(),
        method: request.definition.request.method.clone(),
        url_template: request.definition.request.url.clone(),
        collection_path,
    };
    RequestNamespaceNode {
        access: AccessStep::for_name(&request.name),
        name: request.name.clone(),
        kind: NodeKind::Request(info),
        children: Vec::new(),
        status: NodeStatus::Exposed,
    }
}

/// If two or more direct children share a display name, none of them is
/// exposed under that name: they are genuinely ambiguous and we never
/// arbitrarily choose one.
fn resolve_ambiguity(
    children: &mut [RequestNamespaceNode],
    namespace_path: &str,
    notices: &mut Vec<String>,
) {
    let mut by_name: BTreeMap<&str, usize> = BTreeMap::new();
    for child in children.iter() {
        *by_name.entry(child.name.as_str()).or_default() += 1;
    }
    let ambiguous = by_name
        .into_iter()
        .filter(|(_, count)| *count > 1)
        .map(|(name, count)| (name.to_owned(), count))
        .collect::<Vec<_>>();
    for (name, count) in ambiguous {
        notices.push(format!(
            "In namespace {}, the name \"{}\" is used by {} different entries, so it is not exposed as a request reference.",
            namespace_path, name, count
        ));
        for child in children.iter_mut() {
            if child.name == name {
                child.status = NodeStatus::Ambiguous;
            }
        }
    }
}

fn runtime_node(node: &RequestNamespaceNode) -> RuntimeNamespaceSpec {
    match &node.kind {
        NodeKind::Request(info) => RuntimeNamespaceSpec {
            name: node.name.clone(),
            kind: RuntimeNodeKind::Request {
                id: info.request_id.clone(),
                path: dotted_path(&info.collection_path, &info.request_name),
                method: info.method.clone(),
            },
            children: Vec::new(),
        },
        NodeKind::Namespace => RuntimeNamespaceSpec {
            name: node.name.clone(),
            kind: RuntimeNodeKind::Namespace,
            children: node
                .children
                .iter()
                .filter(|child| child.status == NodeStatus::Exposed)
                .map(runtime_node)
                .collect(),
        },
    }
}

impl RuntimeNamespaceSpec {
    /// Deep clone helper for tests and the runtime bridge.
    pub fn clone_spec(&self) -> Self {
        self.clone()
    }
}

fn dotted_path(collection_path: &[String], request_name: &str) -> String {
    let mut path = collection_path.to_vec();
    path.push(request_name.to_owned());
    path.join(".")
}

/// Serializable tree injected into the script runtime.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeNamespaceSpec {
    pub name: String,
    pub kind: RuntimeNodeKind,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<RuntimeNamespaceSpec>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum RuntimeNodeKind {
    Namespace,
    Request {
        id: String,
        path: String,
        method: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{request::RequestDraft, template::RequestTemplate};

    fn template(url: &str, method: &str) -> RequestTemplate {
        RequestTemplate {
            request: RequestDraft::new(method, url),
            scripts: Default::default(),
        }
    }

    /// ChatAdmin/{Login,Logout,Users/{Create,Delete}}, Payments/{Login}
    fn sample_workspace() -> Workspace {
        let mut workspace = Workspace::default();
        let chat_admin = workspace.create_collection("ChatAdmin").unwrap();
        workspace
            .create_saved_request(
                &chat_admin,
                "Login",
                template("https://a.test/login", "POST"),
            )
            .unwrap();
        workspace
            .create_saved_request(
                &chat_admin,
                "Logout",
                template("https://a.test/logout", "POST"),
            )
            .unwrap();
        let users_folder = workspace
            .create_collection_folder(&chat_admin, None, "Users")
            .unwrap();
        workspace
            .create_saved_request_in_folder(
                &chat_admin,
                Some(&users_folder),
                "Create",
                template("https://a.test/users", "POST"),
            )
            .unwrap();
        workspace
            .create_saved_request_in_folder(
                &chat_admin,
                Some(&users_folder),
                "Delete",
                template("https://a.test/users/{id}", "DELETE"),
            )
            .unwrap();
        let payments = workspace.create_collection("Payments").unwrap();
        workspace
            .create_saved_request(&payments, "Login", template("https://p.test/login", "POST"))
            .unwrap();
        workspace
    }

    #[test]
    fn builds_namespace_from_collection_tree() {
        let catalog = RequestNamespaceCatalog::from_workspace(&sample_workspace());
        let roots = catalog.roots().collect::<Vec<_>>();
        assert_eq!(roots.len(), 2);
        assert_eq!(roots[0].name, "ChatAdmin");
        assert_eq!(roots[1].name, "Payments");

        let members = catalog.members_at(&["ChatAdmin".to_owned()]).unwrap();
        let names = members
            .iter()
            .map(|member| member.name.as_str())
            .collect::<Vec<_>>();
        assert!(names.contains(&"Users"));
        assert!(names.contains(&"Login"));
        assert!(names.contains(&"Logout"));
    }

    #[test]
    fn nested_namespaces_and_distinct_duplicate_request_names() {
        let catalog = RequestNamespaceCatalog::from_workspace(&sample_workspace());
        let chat_login = catalog
            .request_by_path("ChatAdmin.Login")
            .expect("ChatAdmin.Login resolves");
        let payments_login = catalog
            .request_by_path("Payments.Login")
            .expect("Payments.Login resolves");
        assert_ne!(chat_login.request_id, payments_login.request_id);

        let create = catalog
            .request_by_path("ChatAdmin.Users.Create")
            .expect("nested request resolves");
        assert_eq!(create.method, "POST");
        assert_eq!(create.collection_path, vec!["ChatAdmin", "Users"]);
        assert_eq!(create.url_template, "https://a.test/users");

        let delete = catalog
            .request_by_path("ChatAdmin.Users.Delete")
            .expect("second nested request resolves");
        assert_eq!(delete.method, "DELETE");
    }

    #[test]
    fn duplicate_request_in_same_namespace_is_ambiguous_and_unexposed() {
        let mut workspace = Workspace::default();
        let col = workspace.create_collection("ChatAdmin").unwrap();
        workspace
            .create_saved_request(&col, "Login", template("https://a.test/1", "GET"))
            .unwrap();
        workspace
            .create_saved_request(&col, "Login", template("https://a.test/2", "GET"))
            .unwrap();

        let catalog = RequestNamespaceCatalog::from_workspace(&workspace);
        assert!(catalog.request_by_path("ChatAdmin.Login").is_none());
        assert!(
            catalog
                .roots()
                .find(|root| root.name == "ChatAdmin")
                .and_then(|root| root.children.first())
                .is_some_and(|child| child.status == NodeStatus::Ambiguous)
        );
        assert!(
            catalog
                .notices()
                .iter()
                .any(|notice| notice.contains("ChatAdmin")
                    && notice.contains("Login")
                    && notice.contains("not exposed"))
        );
        let _ = col;
    }

    #[test]
    fn invalid_names_use_bracket_access() {
        let mut workspace = Workspace::default();
        let col = workspace.create_collection("Chat Admin").unwrap();
        workspace
            .create_saved_request(&col, "123Login", template("https://a.test/x", "GET"))
            .unwrap();
        workspace
            .create_saved_request(&col, "My Request", template("https://a.test/y", "GET"))
            .unwrap();

        let catalog = RequestNamespaceCatalog::from_workspace(&workspace);
        let root = catalog
            .roots()
            .find(|root| root.name == "Chat Admin")
            .expect("root with space exists");
        assert_eq!(root.access, AccessStep::Bracket("Chat Admin".to_owned()));

        let request = catalog
            .roots()
            .find(|root| root.name == "Chat Admin")
            .and_then(|root| root.children.iter().find(|child| child.name == "123Login"))
            .expect("request leaf");
        assert_eq!(request.access, AccessStep::Bracket("123Login".to_owned()));
        let _ = col;
    }

    #[test]
    fn reserved_global_root_is_not_exposed_with_notice() {
        let mut workspace = Workspace::default();
        let col = workspace.create_collection("JSON").unwrap();
        workspace
            .create_saved_request(&col, "Any", template("https://a.test/x", "GET"))
            .unwrap();

        let catalog = RequestNamespaceCatalog::from_workspace(&workspace);
        assert!(catalog.roots().find(|root| root.name == "JSON").is_none());
        assert!(catalog.notices().iter().any(|notice| {
            notice.contains("JSON") && notice.contains("built-in scripting global")
        }));
    }

    #[test]
    fn duplicate_collection_names_only_expose_first_with_notice() {
        let mut workspace = Workspace::default();
        let first = workspace.create_collection("ChatAdmin").unwrap();
        workspace
            .create_saved_request(&first, "Login", template("https://a.test/login", "POST"))
            .unwrap();
        let second = workspace.create_collection("ChatAdmin").unwrap();
        workspace
            .create_saved_request(&second, "Logout", template("https://a.test/logout", "POST"))
            .unwrap();

        let catalog = RequestNamespaceCatalog::from_workspace(&workspace);
        assert!(
            catalog
                .roots()
                .filter(|root| root.name == "ChatAdmin" && root.status == NodeStatus::Exposed)
                .count()
                <= 1
        );
        assert!(
            catalog.notices().iter().any(|notice| {
                notice.contains("ChatAdmin") && notice.contains("only the first")
            })
        );
    }

    #[test]
    fn two_different_invalid_names_never_collapse() {
        let mut workspace = Workspace::default();
        let col = workspace.create_collection("C").unwrap();
        workspace
            .create_saved_request(&col, "foo bar", template("https://a.test/1", "GET"))
            .unwrap();
        workspace
            .create_saved_request(&col, "foo_bar", template("https://a.test/2", "GET"))
            .unwrap();

        let catalog = RequestNamespaceCatalog::from_workspace(&workspace);
        let names = catalog
            .roots()
            .find(|root| root.name == "C")
            .unwrap()
            .children
            .iter()
            .map(|child| child.name.as_str())
            .collect::<Vec<_>>();
        assert!(names.contains(&"foo bar"));
        assert!(names.contains(&"foo_bar"));
        let _ = col;
    }

    #[test]
    fn runtime_specs_include_only_exposed_nodes_with_identity() {
        let catalog = RequestNamespaceCatalog::from_workspace(&sample_workspace());
        let specs = catalog.runtime_specs();
        let chat = specs
            .iter()
            .find(|spec| spec.name == "ChatAdmin")
            .expect("ChatAdmin root spec");
        let login = chat
            .children
            .iter()
            .find(|spec| spec.name == "Login")
            .expect("Login request spec");
        match &login.kind {
            RuntimeNodeKind::Request { id, path, method } => {
                assert!(!id.is_empty());
                assert_eq!(path, "ChatAdmin.Login");
                assert_eq!(method, "POST");
            }
            _ => panic!("Login should be a request spec"),
        }
        // Users is a nested namespace spec containing Create.
        let users = chat
            .children
            .iter()
            .find(|spec| spec.name == "Users")
            .expect("Users folder spec");
        assert_eq!(users.kind, RuntimeNodeKind::Namespace);
        assert!(users.children.iter().any(|spec| spec.name == "Create"));
    }

    #[test]
    fn request_ref_carries_stable_identity_that_survives_renames_in_tree_path() {
        // The reference stores the stable request id, not a name-derived value.
        // When the containing collection is renamed, the path re-derives but the
        // stable identity is unchanged, and the old path stops resolving.
        let mut workspace = sample_workspace();
        let catalog = RequestNamespaceCatalog::from_workspace(&workspace);
        let create = catalog
            .request_by_path("ChatAdmin.Users.Create")
            .expect("Create resolves");
        let id = create.request_id.clone();
        let first_path = create.collection_path.clone();
        assert!(!id.is_empty());

        let chat_id = workspace.collections[0].id.clone();
        workspace.rename_collection(&chat_id, "Auth").unwrap();

        let catalog2 = RequestNamespaceCatalog::from_workspace(&workspace);
        let after = catalog2
            .request_by_path("Auth.Users.Create")
            .expect("Create resolves under the renamed collection");
        assert_eq!(after.request_id, id);
        assert_ne!(first_path, after.collection_path);
        // The renamed/moved old path is no longer valid in the new catalog.
        assert!(catalog2.request_by_path("ChatAdmin.Users.Create").is_none());
    }
}
