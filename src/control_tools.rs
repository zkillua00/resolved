#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControlToolDescriptor {
    pub name: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub read_only: bool,
}

pub const CONTROL_TOOLS: &[ControlToolDescriptor] = &[
    ControlToolDescriptor {
        name: "status",
        label: "Status",
        description: "Report the running Resolved instance and active workspace.",
        read_only: true,
    },
    ControlToolDescriptor {
        name: "list_workspaces",
        label: "List workspaces",
        description: "List local and connected server workspaces.",
        read_only: true,
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
        name: "set_request_scripts",
        label: "Set request scripts",
        description: "Replace the pre-request and post-response scripts on a saved request.",
        read_only: false,
    },
    ControlToolDescriptor {
        name: "execute_http_request",
        label: "Execute HTTP request",
        description: "Execute a saved HTTP request through Resolved, including variables, pre/post scripts, request chaining, remote execution policy, and history.",
        read_only: false,
    },
    ControlToolDescriptor {
        name: "get_http_exchange",
        label: "Get HTTP exchange",
        description: "Poll a Resolved HTTP execution and read its response, errors, and script reports.",
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
        description: "Evaluate JavaScript against the latest HTTP request and response using Resolved's post-response console runtime.",
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
];

pub fn tool(name: &str) -> Option<&'static ControlToolDescriptor> {
    CONTROL_TOOLS.iter().find(|tool| tool.name == name)
}
