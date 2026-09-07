#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControlToolDescriptor {
    pub name: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub read_only: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // The standalone MCP adapter shares this registry but does not render its UI.
pub struct ControlToolGroupDescriptor {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub tools: &'static [&'static str],
}

#[allow(dead_code)] // The standalone MCP adapter shares this registry but does not render its UI.
pub const CONTROL_TOOL_GROUPS: &[ControlToolGroupDescriptor] = &[
    ControlToolGroupDescriptor {
        id: "workspace",
        label: "Workspace",
        description: "Inspect the app context and move between local or connected workspaces.",
        tools: &[
            "status",
            "get_active_context",
            "list_workspaces",
            "switch_workspace",
        ],
    },
    ControlToolGroupDescriptor {
        id: "collections",
        label: "Collections and folders",
        description: "Organize collections and their folder trees.",
        tools: &[
            "list_collections",
            "create_collection",
            "rename_collection",
            "delete_collection",
            "create_folder",
            "rename_folder",
            "move_folder",
            "delete_folder",
        ],
    },
    ControlToolGroupDescriptor {
        id: "requests",
        label: "Requests",
        description: "Find, inspect, create, edit, move, duplicate, and delete saved requests.",
        tools: &[
            "search_requests",
            "get_request",
            "create_request",
            "save_request",
            "duplicate_request",
            "move_request",
            "delete_request",
            "set_request_scripts",
        ],
    },
    ControlToolGroupDescriptor {
        id: "http",
        label: "HTTP execution",
        description: "Run HTTP requests and sequences, poll their results, or cancel active work.",
        tools: &[
            "execute_http_request",
            "get_http_exchange",
            "query_http_response",
            "cancel_http_request",
            "run_request_sequence",
            "get_request_sequence",
        ],
    },
    ControlToolGroupDescriptor {
        id: "script_console",
        label: "Script console",
        description: "Run and inspect JavaScript against the latest HTTP exchange.",
        tools: &["run_script_console", "get_script_console"],
    },
    ControlToolGroupDescriptor {
        id: "websocket",
        label: "WebSocket",
        description: "Connect, exchange frames, inspect events, and run saved replays.",
        tools: &[
            "connect_websocket",
            "send_websocket_message",
            "get_websocket_events",
            "run_websocket_replay",
            "disconnect_websocket",
        ],
    },
    ControlToolGroupDescriptor {
        id: "environments",
        label: "Environments",
        description: "Inspect and manage environments, selection, and variables.",
        tools: &[
            "list_environments",
            "get_environment",
            "create_environment",
            "rename_environment",
            "set_active_environment",
            "set_environment_variable",
            "delete_environment",
            "delete_environment_variable",
        ],
    },
    ControlToolGroupDescriptor {
        id: "import_export",
        label: "Import and export",
        description: "Bring requests into Resolved or export them to supported formats.",
        tools: &["import_requests", "export_request"],
    },
    ControlToolGroupDescriptor {
        id: "snippets",
        label: "Snippets",
        description: "Inspect, manage, and run saved snippets.",
        tools: &[
            "list_snippets",
            "get_snippet",
            "create_snippet",
            "save_snippet",
            "delete_snippet",
            "run_snippet",
        ],
    },
    ControlToolGroupDescriptor {
        id: "history",
        label: "History",
        description: "Inspect, open, and replay sanitized request-history entries.",
        tools: &[
            "list_request_history",
            "get_history_entry",
            "open_history_entry",
            "replay_history_request",
        ],
    },
];

pub const CONTROL_TOOLS: &[ControlToolDescriptor] = &[
    ControlToolDescriptor {
        name: "status",
        label: "Status",
        description: "Report the running Resolved instance and active workspace.",
        read_only: true,
    },
    ControlToolDescriptor {
        name: "get_active_context",
        label: "Get active context",
        description: "Read the active workspace, environment, request tab, and current response context.",
        read_only: true,
    },
    ControlToolDescriptor {
        name: "list_workspaces",
        label: "List workspaces",
        description: "List local and connected server workspaces.",
        read_only: true,
    },
    ControlToolDescriptor {
        name: "switch_workspace",
        label: "Switch workspace",
        description: "Switch Resolved to a named local or connected server workspace.",
        read_only: false,
    },
    ControlToolDescriptor {
        name: "list_collections",
        label: "List collections",
        description: "List collections, folders, and saved-request summaries.",
        read_only: true,
    },
    ControlToolDescriptor {
        name: "create_collection",
        label: "Create collection",
        description: "Create a collection in the active workspace.",
        read_only: false,
    },
    ControlToolDescriptor {
        name: "rename_collection",
        label: "Rename collection",
        description: "Rename a collection in the active workspace.",
        read_only: false,
    },
    ControlToolDescriptor {
        name: "delete_collection",
        label: "Delete collection",
        description: "Delete a collection and its folders and saved requests.",
        read_only: false,
    },
    ControlToolDescriptor {
        name: "create_folder",
        label: "Create folder",
        description: "Create a root or nested folder in a collection.",
        read_only: false,
    },
    ControlToolDescriptor {
        name: "rename_folder",
        label: "Rename folder",
        description: "Rename a folder in a collection.",
        read_only: false,
    },
    ControlToolDescriptor {
        name: "move_folder",
        label: "Move folder",
        description: "Move a collection folder to the root or beneath another folder.",
        read_only: false,
    },
    ControlToolDescriptor {
        name: "delete_folder",
        label: "Delete folder",
        description: "Delete a collection folder and its descendant folders and saved requests.",
        read_only: false,
    },
    ControlToolDescriptor {
        name: "search_requests",
        label: "Search requests",
        description: "Search saved requests by name or URL.",
        read_only: true,
    },
    ControlToolDescriptor {
        name: "get_request",
        label: "Get request",
        description: "Read a saved request and its scripts.",
        read_only: true,
    },
    ControlToolDescriptor {
        name: "create_request",
        label: "Create request",
        description: "Create a saved request, optionally with scripts.",
        read_only: false,
    },
    ControlToolDescriptor {
        name: "save_request",
        label: "Save request",
        description: "Update a saved request after checking its revision.",
        read_only: false,
    },
    ControlToolDescriptor {
        name: "duplicate_request",
        label: "Duplicate request",
        description: "Duplicate a saved request with a fresh identity.",
        read_only: false,
    },
    ControlToolDescriptor {
        name: "move_request",
        label: "Move request",
        description: "Move a saved request to a collection root or folder.",
        read_only: false,
    },
    ControlToolDescriptor {
        name: "delete_request",
        label: "Delete request",
        description: "Delete a saved request from the active workspace.",
        read_only: false,
    },
    ControlToolDescriptor {
        name: "set_request_scripts",
        label: "Set request scripts",
        description: "Replace the pre-request and post-response scripts on a saved request.",
        read_only: false,
    },
    ControlToolDescriptor {
        name: "execute_http_request",
        label: "Execute HTTP request",
        description: "Execute a saved HTTP request through Resolved with optional non-persistent overrides, including variables, pre/post scripts, request chaining, remote execution policy, and history.",
        read_only: false,
    },
    ControlToolDescriptor {
        name: "get_http_exchange",
        label: "Get HTTP exchange",
        description: "Poll a Resolved HTTP execution and read its response, errors, and script reports.",
        read_only: true,
    },
    ControlToolDescriptor {
        name: "query_http_response",
        label: "Query HTTP response",
        description: "Select and optionally project bounded JSON from the latest HTTP response without returning its full body.",
        read_only: true,
    },
    ControlToolDescriptor {
        name: "cancel_http_request",
        label: "Cancel HTTP request",
        description: "Cancel the HTTP request or script stage currently running through Resolved.",
        read_only: false,
    },
    ControlToolDescriptor {
        name: "list_request_history",
        label: "List request history",
        description: "List the active local request history with secret-redacted request data and response summaries.",
        read_only: true,
    },
    ControlToolDescriptor {
        name: "run_script_console",
        label: "Run script console",
        description: "Evaluate JavaScript against the latest HTTP exchange using api.response.text() or api.response.json() in Resolved's post-response console runtime.",
        read_only: false,
    },
    ControlToolDescriptor {
        name: "get_script_console",
        label: "Get script console",
        description: "Read the latest structured script reports and console output.",
        read_only: true,
    },
    ControlToolDescriptor {
        name: "connect_websocket",
        label: "Connect WebSocket",
        description: "Open a saved WebSocket request through Resolved, using the active environment and remote execution policy.",
        read_only: false,
    },
    ControlToolDescriptor {
        name: "send_websocket_message",
        label: "Send WebSocket message",
        description: "Send a text or base64-encoded binary message on the active MCP WebSocket connection.",
        read_only: false,
    },
    ControlToolDescriptor {
        name: "get_websocket_events",
        label: "Get WebSocket events",
        description: "Read connection, frame, error, and automation events from the active MCP WebSocket session.",
        read_only: true,
    },
    ControlToolDescriptor {
        name: "run_websocket_replay",
        label: "Run WebSocket replay",
        description: "Run a saved replay on the active MCP WebSocket connection with its recorded delays.",
        read_only: false,
    },
    ControlToolDescriptor {
        name: "disconnect_websocket",
        label: "Disconnect WebSocket",
        description: "Close the active MCP WebSocket connection.",
        read_only: false,
    },
    ControlToolDescriptor {
        name: "list_environments",
        label: "List environments",
        description: "List environments without returning secret values.",
        read_only: true,
    },
    ControlToolDescriptor {
        name: "get_environment",
        label: "Get environment",
        description: "Read one environment without returning secret values.",
        read_only: true,
    },
    ControlToolDescriptor {
        name: "create_environment",
        label: "Create environment",
        description: "Create an environment in the active workspace.",
        read_only: false,
    },
    ControlToolDescriptor {
        name: "rename_environment",
        label: "Rename environment",
        description: "Rename an environment.",
        read_only: false,
    },
    ControlToolDescriptor {
        name: "set_active_environment",
        label: "Set active environment",
        description: "Select or clear the active environment.",
        read_only: false,
    },
    ControlToolDescriptor {
        name: "set_environment_variable",
        label: "Set environment variable",
        description: "Create or update an environment variable without returning secret values; stored secrets cannot be marked non-secret through MCP.",
        read_only: false,
    },
    ControlToolDescriptor {
        name: "delete_environment",
        label: "Delete environment",
        description: "Delete an environment from the active workspace.",
        read_only: false,
    },
    ControlToolDescriptor {
        name: "delete_environment_variable",
        label: "Delete environment variable",
        description: "Delete a variable from an environment.",
        read_only: false,
    },
    ControlToolDescriptor {
        name: "import_requests",
        label: "Import requests",
        description: "Parse request text and import the discovered requests into a collection.",
        read_only: false,
    },
    ControlToolDescriptor {
        name: "export_request",
        label: "Export request",
        description: "Export a saved request in a supported command, specification, or code format.",
        read_only: true,
    },
    ControlToolDescriptor {
        name: "run_request_sequence",
        label: "Run request sequence",
        description: "Start an ordered asynchronous run of saved requests.",
        read_only: false,
    },
    ControlToolDescriptor {
        name: "get_request_sequence",
        label: "Get request sequence",
        description: "Read the progress and bounded results of a request-sequence operation.",
        read_only: true,
    },
    ControlToolDescriptor {
        name: "list_snippets",
        label: "List snippets",
        description: "List saved plain and executable snippets.",
        read_only: true,
    },
    ControlToolDescriptor {
        name: "get_snippet",
        label: "Get snippet",
        description: "Read one saved snippet.",
        read_only: true,
    },
    ControlToolDescriptor {
        name: "create_snippet",
        label: "Create snippet",
        description: "Create and persist a plain or executable snippet.",
        read_only: false,
    },
    ControlToolDescriptor {
        name: "save_snippet",
        label: "Save snippet",
        description: "Update a saved snippet after checking its revision.",
        read_only: false,
    },
    ControlToolDescriptor {
        name: "delete_snippet",
        label: "Delete snippet",
        description: "Delete a saved snippet.",
        read_only: false,
    },
    ControlToolDescriptor {
        name: "run_snippet",
        label: "Run snippet",
        description: "Generate text from a saved snippet against a saved HTTP request context.",
        read_only: false,
    },
    ControlToolDescriptor {
        name: "get_history_entry",
        label: "Get history entry",
        description: "Read one sanitized request-history entry.",
        read_only: true,
    },
    ControlToolDescriptor {
        name: "open_history_entry",
        label: "Open history entry",
        description: "Open a request-history entry as a request tab in Resolved.",
        read_only: false,
    },
    ControlToolDescriptor {
        name: "replay_history_request",
        label: "Replay history request",
        description: "Send the sanitized request stored in a history entry again.",
        read_only: false,
    },
];

pub fn tool(name: &str) -> Option<&'static ControlToolDescriptor> {
    CONTROL_TOOLS.iter().find(|tool| tool.name == name)
}

#[allow(dead_code)] // The standalone MCP adapter shares this registry but does not render its UI.
pub fn tool_group(id: &str) -> Option<&'static ControlToolGroupDescriptor> {
    CONTROL_TOOL_GROUPS.iter().find(|group| group.id == id)
}

pub fn workspace_scoped_tool(name: &str) -> bool {
    matches!(
        name,
        "list_collections"
            | "create_collection"
            | "rename_collection"
            | "delete_collection"
            | "create_folder"
            | "rename_folder"
            | "move_folder"
            | "delete_folder"
            | "search_requests"
            | "get_request"
            | "create_request"
            | "save_request"
            | "duplicate_request"
            | "move_request"
            | "delete_request"
            | "set_request_scripts"
            | "list_environments"
            | "get_environment"
            | "create_environment"
            | "rename_environment"
            | "set_environment_variable"
            | "delete_environment"
            | "delete_environment_variable"
            | "import_requests"
            | "export_request"
    )
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn tool_groups_partition_the_tool_registry() {
        let mut grouped = HashSet::new();
        let mut group_ids = HashSet::new();

        for group in CONTROL_TOOL_GROUPS {
            assert!(
                group_ids.insert(group.id),
                "duplicate tool group {}",
                group.id
            );
            assert!(!group.tools.is_empty(), "empty tool group {}", group.id);
            for tool_name in group.tools {
                assert!(
                    tool(tool_name).is_some(),
                    "unknown grouped tool {tool_name}"
                );
                assert!(
                    grouped.insert(*tool_name),
                    "tool grouped more than once: {tool_name}"
                );
            }
        }

        let registered = CONTROL_TOOLS
            .iter()
            .map(|tool| tool.name)
            .collect::<HashSet<_>>();
        assert_eq!(grouped, registered);
    }
}
