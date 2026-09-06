use std::{
    fs,
    io::{self, BufRead as _, BufReader, Write as _},
    path::PathBuf,
};

use serde::Deserialize;
use serde_json::{Value, json};

#[path = "../control_tools.rs"]
mod control_tools;

const MCP_PROTOCOL_VERSION: &str = "2025-06-18";

#[derive(Deserialize)]
struct ControlDescriptor {
    protocol_version: u32,
    transport: String,
    endpoint: String,
    token: String,
}

fn main() {
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let message: Value = match serde_json::from_str(&line) {
            Ok(message) => message,
            Err(error) => {
                write_message(
                    &mut stdout,
                    &rpc_error(Value::Null, -32700, error.to_string()),
                );
                continue;
            }
        };
        let Some(id) = message.get("id").cloned() else {
            continue;
        };
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let response = match method {
            "initialize" => rpc_result(
                id,
                json!({
                    "protocolVersion": message
                        .pointer("/params/protocolVersion")
                        .and_then(Value::as_str)
                        .unwrap_or(MCP_PROTOCOL_VERSION),
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": {
                        "name": "resolved-mcp",
                        "version": env!("RESOLVED_BUILD_VERSION")
                    }
                }),
            ),
            "ping" => rpc_result(id, json!({})),
            "tools/list" => rpc_result(id, json!({ "tools": enabled_tool_definitions() })),
            "tools/call" => call_tool(id, message.get("params").cloned().unwrap_or_default()),
            _ => rpc_error(id, -32601, format!("method '{method}' was not found")),
        };
        write_message(&mut stdout, &response);
    }
}

fn call_tool(id: Value, params: Value) -> Value {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    match invoke_control(name, arguments) {
        Ok(result) => rpc_result(
            id,
            json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string())
                }],
                "structuredContent": result,
                "isError": false
            }),
        ),
        Err(error) => rpc_result(
            id,
            json!({
                "content": [{ "type": "text", "text": error }],
                "isError": true
            }),
        ),
    }
}

fn invoke_control(method: &str, params: Value) -> Result<Value, String> {
    let descriptor: ControlDescriptor =
        serde_json::from_slice(&fs::read(descriptor_path()).map_err(|error| {
            format!("Resolved is not running or local control is unavailable: {error}")
        })?)
        .map_err(|error| format!("Resolved control descriptor is invalid: {error}"))?;
    if descriptor.protocol_version != 1 {
        return Err(format!(
            "Resolved control protocol {} is not supported by this adapter",
            descriptor.protocol_version
        ));
    }
    let request = json!({
        "token": descriptor.token,
        "method": method,
        "params": params
    });
    let response = send_control_request(&descriptor, &request)?;
    if response.get("ok").and_then(Value::as_bool) == Some(true) {
        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    } else {
        Err(response
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("Resolved rejected the control request")
            .to_owned())
    }
}

#[cfg(unix)]
fn send_control_request(descriptor: &ControlDescriptor, request: &Value) -> Result<Value, String> {
    use std::os::unix::net::UnixStream;

    if descriptor.transport != "unix" {
        return Err(format!(
            "unsupported local control transport '{}'",
            descriptor.transport
        ));
    }
    let mut stream = UnixStream::connect(&descriptor.endpoint)
        .map_err(|error| format!("could not connect to Resolved: {error}"))?;
    serde_json::to_writer(&mut stream, request).map_err(|error| error.to_string())?;
    stream.write_all(b"\n").map_err(|error| error.to_string())?;
    stream.flush().map_err(|error| error.to_string())?;
    let mut response = String::new();
    BufReader::new(stream)
        .read_line(&mut response)
        .map_err(|error| error.to_string())?;
    serde_json::from_str(&response).map_err(|error| error.to_string())
}

#[cfg(not(unix))]
fn send_control_request(descriptor: &ControlDescriptor, request: &Value) -> Result<Value, String> {
    use std::net::TcpStream;

    if descriptor.transport != "tcp" {
        return Err(format!(
            "unsupported local control transport '{}'",
            descriptor.transport
        ));
    }
    let mut stream = TcpStream::connect(&descriptor.endpoint)
        .map_err(|error| format!("could not connect to Resolved: {error}"))?;
    serde_json::to_writer(&mut stream, request).map_err(|error| error.to_string())?;
    stream.write_all(b"\n").map_err(|error| error.to_string())?;
    stream.flush().map_err(|error| error.to_string())?;
    let mut response = String::new();
    BufReader::new(stream)
        .read_line(&mut response)
        .map_err(|error| error.to_string())?;
    serde_json::from_str(&response).map_err(|error| error.to_string())
}

fn descriptor_path() -> PathBuf {
    dirs::data_local_dir()
        .or_else(dirs::data_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("API Tester")
        .join("resolved-control.json")
}

fn rpc_result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn rpc_error(id: Value, code: i64, message: String) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    })
}

fn write_message(writer: &mut impl io::Write, value: &Value) {
    let _ = serde_json::to_writer(&mut *writer, value);
    let _ = writer.write_all(b"\n");
    let _ = writer.flush();
}

fn object_schema(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

fn tool(name: &str, _description: &str, mut input_schema: Value, _read_only: bool) -> Value {
    let catalog = control_tools::tool(name).expect("every MCP definition must be in the catalog");
    let _label = catalog.label;
    if control_tools::workspace_scoped_tool(name)
        && let Some(properties) = input_schema
            .get_mut("properties")
            .and_then(Value::as_object_mut)
    {
        properties.insert(
            "workspace_id".to_owned(),
            json!({
                "type": "string",
                "description": "Optional workspace ID from list_workspaces. Explicit targeting does not switch the user's active workspace."
            }),
        );
    }
    json!({
        "name": name,
        "description": catalog.description,
        "inputSchema": input_schema,
        "annotations": {
            "readOnlyHint": catalog.read_only,
            "destructiveHint": !catalog.read_only,
            "idempotentHint": catalog.read_only
        }
    })
}

fn tool_definitions() -> Vec<Value> {
    let empty = || object_schema(json!({}), &[]);
    let raw_body_languages = json!([
        "text",
        "json",
        "jsonl",
        "xml",
        "html",
        "javascript",
        "typescript",
        "css",
        "markdown",
        "graphql",
        "yaml",
        "toml",
        "sql",
        "shell",
        "rust",
        "python"
    ]);
    let headers = json!({
        "type": "array",
        "items": {
            "type": "object",
            "properties": {
                "enabled": { "type": "boolean", "default": true },
                "shared": { "type": "boolean", "default": true },
                "name": { "type": "string" },
                "value": { "type": "string" }
            },
            "required": ["name", "value"],
            "additionalProperties": false
        }
    });
    let query_parameters = json!({ "type": "array", "items": {
        "type": "object",
        "properties": {
            "enabled": { "type": "boolean", "default": true },
            "key": { "type": "string" },
            "value": { "type": "string" },
            "description": { "type": "string" }
        },
        "required": ["key", "value"],
        "additionalProperties": false
    } });
    let body_fields = json!({ "type": "array", "items": {
        "type": "object",
        "properties": {
            "enabled": { "type": "boolean", "default": true },
            "name": { "type": "string" },
            "value": { "type": "string", "description": "Text value or local file path when kind is file." },
            "kind": { "type": "string", "enum": ["text", "file"], "default": "text" }
        },
        "required": ["name", "value"],
        "additionalProperties": false
    } });
    let request_draft = json!({
        "type": "object",
        "description": "Resolved request draft. Fields omitted by Resolved defaults are optional.",
        "properties": {
            "method": { "type": "string" },
            "url": { "type": "string" },
            "query_params": query_parameters.clone(),
            "headers": headers.clone(),
            "body": { "type": "string" },
            "body_mode": { "type": "string", "enum": ["none", "raw", "form_url_encoded", "multipart_form_data"] },
            "raw_body_language": { "type": "string", "enum": raw_body_languages.clone() },
            "body_fields": body_fields.clone()
        },
        "required": ["method", "url"],
        "additionalProperties": false
    });
    let websocket_document = json!({
        "type": "object",
        "properties": {
            "url": { "type": "string" },
            "headers": headers,
            "composer": { "type": "string" },
            "composer_language": { "type": "string", "enum": raw_body_languages },
            "messages": { "type": "array", "items": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "name": { "type": "string" },
                    "payload": { "type": "string" },
                    "language": { "type": "string" }
                },
                "required": ["id", "name", "payload"],
                "additionalProperties": false
            } },
            "templates": { "type": "array", "items": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "name": { "type": "string" },
                    "payload": { "type": "string" }
                },
                "required": ["id", "name", "payload"],
                "additionalProperties": false
            } },
            "replays": { "type": "array", "items": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "name": { "type": "string" },
                    "frames": { "type": "array", "items": {
                        "type": "object",
                        "properties": {
                            "delay_ms": { "type": "integer", "minimum": 0 },
                            "payload": { "type": "string" }
                        },
                        "required": ["delay_ms", "payload"],
                        "additionalProperties": false
                    } }
                },
                "required": ["id", "name", "frames"],
                "additionalProperties": false
            } },
            "reset_input_after_send": { "type": "boolean" },
            "automation_enabled": { "type": "boolean" },
            "automation_source": { "type": "string" }
        },
        "required": ["url"],
        "additionalProperties": false
    });
    vec![
        tool(
            "status",
            "Report the running Resolved instance and active workspace.",
            empty(),
            true,
        ),
        tool(
            "list_workspaces",
            "List local Resolved workspaces.",
            empty(),
            true,
        ),
        tool(
            "list_collections",
            "List collections, folders, and saved requests in the active workspace.",
            empty(),
            true,
        ),
        tool(
            "create_collection",
            "Create a collection in the active workspace.",
            object_schema(json!({ "name": { "type": "string" } }), &["name"]),
            false,
        ),
        tool(
            "search_requests",
            "Search saved requests by name or URL.",
            object_schema(json!({ "query": { "type": "string", "default": "" } }), &[]),
            true,
        ),
        tool(
            "get_request",
            "Read a saved request including its pre-request and post-response scripts.",
            object_schema(
                json!({ "request_id": { "type": "string" } }),
                &["request_id"],
            ),
            true,
        ),
        tool(
            "create_request",
            "Create a saved request, optionally inside a collection folder and with scripts.",
            object_schema(
                json!({
                    "collection_id": { "type": "string" },
                    "folder_id": { "type": "string" },
                    "name": { "type": "string" },
                    "request": request_draft.clone(),
                    "websocket": websocket_document.clone(),
                    "scripts": { "type": "object", "properties": { "pre_request": { "type": "string" }, "post_response": { "type": "string" } }, "additionalProperties": false }
                }),
                &["collection_id", "name"],
            ),
            false,
        ),
        tool(
            "save_request",
            "Update a saved request after checking its updated_at revision.",
            object_schema(
                json!({
                    "request_id": { "type": "string" },
                    "expected_updated_at": { "type": "string" },
                    "name": { "type": "string" },
                    "request": request_draft,
                    "websocket": websocket_document,
                    "clear_websocket": { "type": "boolean", "default": false },
                    "scripts": { "type": "object", "properties": { "pre_request": { "type": "string" }, "post_response": { "type": "string" } }, "additionalProperties": false }
                }),
                &["request_id", "expected_updated_at"],
            ),
            false,
        ),
        tool(
            "set_request_scripts",
            "Replace both scripts on a saved request after checking its revision.",
            object_schema(
                json!({
                    "request_id": { "type": "string" },
                    "expected_updated_at": { "type": "string" },
                    "pre_request": { "type": "string" },
                    "post_response": { "type": "string" }
                }),
                &[
                    "request_id",
                    "expected_updated_at",
                    "pre_request",
                    "post_response",
                ],
            ),
            false,
        ),
        tool(
            "execute_http_request",
            "Execute a saved HTTP request through Resolved with optional non-persistent overrides.",
            object_schema(
                json!({
                    "request_id": { "type": "string" },
                    "overrides": {
                        "type": "object",
                        "description": "Ephemeral values used only for this execution. The saved request and revision are not changed.",
                        "properties": {
                            "method": { "type": "string" },
                            "url": { "type": "string" },
                            "query_parameters": query_parameters,
                            "headers": headers.clone(),
                            "body": { "type": "string" },
                            "body_mode": { "type": "string", "enum": ["none", "raw", "form_url_encoded", "multipart_form_data"] },
                            "raw_body_language": { "type": "string", "enum": raw_body_languages.clone() },
                            "body_fields": body_fields
                        },
                        "additionalProperties": false
                    }
                }),
                &["request_id"],
            ),
            false,
        ),
        tool(
            "get_http_exchange",
            "Poll an HTTP execution and read its response and script reports.",
            object_schema(
                json!({
                    "operation_id": { "type": "integer", "minimum": 0 },
                    "max_body_bytes": { "type": "integer", "minimum": 0, "maximum": 524288, "default": 262144 }
                }),
                &[],
            ),
            true,
        ),
        tool(
            "query_http_response",
            "Select and optionally project bounded JSON from an HTTP response without returning the full body.",
            object_schema(
                json!({
                    "operation_id": { "type": "integer", "minimum": 0 },
                    "json_pointer": { "type": "string", "description": "RFC 6901 JSON Pointer. Use an empty string for the response root." },
                    "projection": {
                        "type": "object",
                        "description": "Optional output field names mapped to relative JSON Pointers. When the selection is an array, the projection is applied to every returned item.",
                        "additionalProperties": { "type": "string" },
                        "maxProperties": 100
                    },
                    "limit": { "type": "integer", "minimum": 0, "maximum": 1000, "default": 100 },
                    "max_output_bytes": { "type": "integer", "minimum": 1, "maximum": 524288, "default": 262144 }
                }),
                &["json_pointer"],
            ),
            true,
        ),
        tool(
            "cancel_http_request",
            "Cancel the current HTTP request or script stage.",
            empty(),
            false,
        ),
        tool(
            "list_request_history",
            "List secret-redacted local request history.",
            object_schema(
                json!({
                    "limit": { "type": "integer", "minimum": 0, "maximum": 100, "default": 25 }
                }),
                &[],
            ),
            true,
        ),
        tool(
            "run_script_console",
            "Evaluate JavaScript against the latest HTTP exchange; response data is available through api.response.text() and api.response.json().",
            object_schema(json!({ "source": { "type": "string" } }), &["source"]),
            false,
        ),
        tool(
            "get_script_console",
            "Read the latest script-console output and reports.",
            object_schema(
                json!({
                    "operation_id": { "type": "integer", "minimum": 0 }
                }),
                &[],
            ),
            true,
        ),
        tool(
            "connect_websocket",
            "Open a saved WebSocket request through Resolved.",
            object_schema(
                json!({ "request_id": { "type": "string" } }),
                &["request_id"],
            ),
            false,
        ),
        tool(
            "send_websocket_message",
            "Send text or binary data on the active MCP WebSocket connection.",
            object_schema(
                json!({
                    "connection_id": { "type": "integer", "minimum": 1 },
                    "text": { "type": "string" },
                    "binary_base64": { "type": "string" },
                    "saved_message_id": { "type": "string" },
                    "template_id": { "type": "string" },
                    "template_values": {
                        "type": "object",
                        "additionalProperties": { "type": "string" }
                    }
                }),
                &["connection_id"],
            ),
            false,
        ),
        tool(
            "get_websocket_events",
            "Read new events from the active MCP WebSocket connection.",
            object_schema(
                json!({
                    "connection_id": { "type": "integer", "minimum": 1 },
                    "after_event_id": { "type": "integer", "minimum": 0, "default": 0 },
                    "limit": { "type": "integer", "minimum": 0, "maximum": 500, "default": 100 },
                    "max_payload_bytes": { "type": "integer", "minimum": 0, "maximum": 262144, "default": 65536 }
                }),
                &[],
            ),
            true,
        ),
        tool(
            "run_websocket_replay",
            "Run a saved replay on the active MCP WebSocket connection.",
            object_schema(
                json!({
                    "connection_id": { "type": "integer", "minimum": 1 },
                    "replay_id": { "type": "string" }
                }),
                &["connection_id", "replay_id"],
            ),
            false,
        ),
        tool(
            "disconnect_websocket",
            "Close the active MCP WebSocket connection.",
            object_schema(
                json!({
                    "connection_id": { "type": "integer", "minimum": 1 }
                }),
                &["connection_id"],
            ),
            false,
        ),
        tool(
            "list_environments",
            "List environments. Secret values are never returned.",
            empty(),
            true,
        ),
        tool(
            "get_environment",
            "Read one environment. Secret values are never returned.",
            object_schema(
                json!({ "environment_id": { "type": "string" } }),
                &["environment_id"],
            ),
            true,
        ),
        tool(
            "create_environment",
            "Create an environment in the active workspace.",
            object_schema(json!({ "name": { "type": "string" } }), &["name"]),
            false,
        ),
        tool(
            "rename_environment",
            "Rename an environment.",
            object_schema(
                json!({ "environment_id": { "type": "string" }, "name": { "type": "string" } }),
                &["environment_id", "name"],
            ),
            false,
        ),
        tool(
            "set_active_environment",
            "Select an environment, or pass null to clear the selection.",
            object_schema(
                json!({ "environment_id": { "type": ["string", "null"] } }),
                &["environment_id"],
            ),
            false,
        ),
        tool(
            "set_environment_variable",
            "Create or update an environment variable. Omitted fields are preserved on update; secret values are accepted but never returned.",
            object_schema(
                json!({
                    "environment_id": { "type": "string" },
                    "variable_id": { "type": "string" },
                    "key": { "type": "string" },
                    "value": { "type": "string" },
                    "enabled": { "type": "boolean" },
                    "secret": { "type": "boolean" }
                }),
                &["environment_id"],
            ),
            false,
        ),
        tool("get_active_context", "", empty(), true),
        tool(
            "switch_workspace",
            "",
            object_schema(
                json!({ "workspace_id": { "type": "string" } }),
                &["workspace_id"],
            ),
            false,
        ),
        tool(
            "rename_collection",
            "",
            object_schema(
                json!({
                    "collection_id": { "type": "string" },
                    "name": { "type": "string" }
                }),
                &["collection_id", "name"],
            ),
            false,
        ),
        tool(
            "delete_collection",
            "",
            object_schema(
                json!({ "collection_id": { "type": "string" } }),
                &["collection_id"],
            ),
            false,
        ),
        tool(
            "create_folder",
            "",
            object_schema(
                json!({
                    "collection_id": { "type": "string" },
                    "parent_folder_id": { "type": ["string", "null"] },
                    "name": { "type": "string" }
                }),
                &["collection_id", "name"],
            ),
            false,
        ),
        tool(
            "rename_folder",
            "",
            object_schema(
                json!({
                    "collection_id": { "type": "string" },
                    "folder_id": { "type": "string" },
                    "name": { "type": "string" }
                }),
                &["collection_id", "folder_id", "name"],
            ),
            false,
        ),
        tool(
            "move_folder",
            "",
            object_schema(
                json!({
                    "collection_id": { "type": "string" },
                    "folder_id": { "type": "string" },
                    "parent_folder_id": { "type": ["string", "null"] }
                }),
                &["collection_id", "folder_id", "parent_folder_id"],
            ),
            false,
        ),
        tool(
            "delete_folder",
            "",
            object_schema(
                json!({
                    "collection_id": { "type": "string" },
                    "folder_id": { "type": "string" }
                }),
                &["collection_id", "folder_id"],
            ),
            false,
        ),
        tool(
            "duplicate_request",
            "",
            object_schema(
                json!({
                    "request_id": { "type": "string" },
                    "name": { "type": "string" }
                }),
                &["request_id"],
            ),
            false,
        ),
        tool(
            "move_request",
            "",
            object_schema(
                json!({
                    "request_id": { "type": "string" },
                    "target_collection_id": { "type": "string" },
                    "target_folder_id": { "type": ["string", "null"] }
                }),
                &["request_id", "target_collection_id", "target_folder_id"],
            ),
            false,
        ),
        tool(
            "delete_request",
            "",
            object_schema(
                json!({ "request_id": { "type": "string" } }),
                &["request_id"],
            ),
            false,
        ),
        tool(
            "delete_environment",
            "",
            object_schema(
                json!({ "environment_id": { "type": "string" } }),
                &["environment_id"],
            ),
            false,
        ),
        tool(
            "delete_environment_variable",
            "",
            object_schema(
                json!({
                    "environment_id": { "type": "string" },
                    "variable_id": { "type": "string" }
                }),
                &["environment_id", "variable_id"],
            ),
            false,
        ),
        tool(
            "import_requests",
            "",
            object_schema(
                json!({
                    "collection_id": { "type": "string" },
                    "folder_id": { "type": ["string", "null"] },
                    "source": { "type": "string" }
                }),
                &["collection_id", "source"],
            ),
            false,
        ),
        tool(
            "export_request",
            "",
            object_schema(
                json!({
                    "request_id": { "type": "string" },
                    "format": {
                        "type": "string",
                        "enum": [
                            "curl", "wget", "powershell", "openapi", "asyncapi",
                            "intellij_http", "javascript_fetch", "javascript_axios",
                            "javascript_jquery", "java_http_client", "java_okhttp",
                            "go_net_http", "go_resty", "csharp_http_client",
                            "csharp_restsharp", "rust_reqwest", "rust_ureq",
                            "cpp_boost_beast", "cpp_libcurl", "php_curl", "php_guzzle",
                            "kotlin_ktor", "kotlin_okhttp", "kotlin_java_http_client"
                        ]
                    }
                }),
                &["request_id", "format"],
            ),
            true,
        ),
        tool(
            "run_request_sequence",
            "",
            object_schema(
                json!({
                    "request_ids": {
                        "type": "array",
                        "items": { "type": "string" },
                        "minItems": 1,
                        "maxItems": 25
                    }
                }),
                &["request_ids"],
            ),
            false,
        ),
        tool(
            "get_request_sequence",
            "",
            object_schema(
                json!({
                    "operation_id": { "type": "integer", "minimum": 0 },
                    "max_body_bytes": { "type": "integer", "minimum": 0, "maximum": 524288, "default": 262144 }
                }),
                &[],
            ),
            true,
        ),
        tool("list_snippets", "", empty(), true),
        tool(
            "get_snippet",
            "",
            object_schema(
                json!({ "snippet_id": { "type": "string" } }),
                &["snippet_id"],
            ),
            true,
        ),
        tool("create_snippet", "", snippet_write_schema(false), false),
        tool("save_snippet", "", snippet_write_schema(true), false),
        tool(
            "delete_snippet",
            "",
            object_schema(
                json!({
                    "snippet_id": { "type": "string" },
                    "expected_updated_at": { "type": "string" }
                }),
                &["snippet_id", "expected_updated_at"],
            ),
            false,
        ),
        tool(
            "run_snippet",
            "",
            object_schema(
                json!({
                    "snippet_id": { "type": "string" },
                    "request_id": { "type": "string" },
                    "operation_id": { "type": "integer", "minimum": 0 }
                }),
                &["snippet_id", "request_id"],
            ),
            false,
        ),
        tool(
            "get_history_entry",
            "",
            object_schema(
                json!({ "history_id": { "type": "string" } }),
                &["history_id"],
            ),
            true,
        ),
        tool(
            "open_history_entry",
            "",
            object_schema(
                json!({ "history_id": { "type": "string" } }),
                &["history_id"],
            ),
            false,
        ),
        tool(
            "replay_history_request",
            "",
            object_schema(
                json!({ "history_id": { "type": "string" } }),
                &["history_id"],
            ),
            false,
        ),
    ]
}

fn snippet_write_schema(update: bool) -> Value {
    let mut properties = serde_json::Map::from_iter([
        ("name".to_owned(), json!({ "type": "string" })),
        ("description".to_owned(), json!({ "type": "string" })),
        (
            "category".to_owned(),
            json!({ "type": "string", "enum": ["pre_request", "post_response"] }),
        ),
        (
            "kind".to_owned(),
            json!({ "type": "string", "enum": ["plain", "executable"] }),
        ),
        ("source".to_owned(), json!({ "type": "string" })),
        (
            "requirements".to_owned(),
            json!({
                "type": "array",
                "items": {
                    "type": "string",
                    "enum": [
                        "always", "has_request", "has_response", "has_selection",
                        "has_request_selection", "has_response_selection", "has_json_selection"
                    ]
                },
                "uniqueItems": true
            }),
        ),
    ]);
    let required = if update {
        properties.insert("snippet_id".to_owned(), json!({ "type": "string" }));
        properties.insert(
            "expected_updated_at".to_owned(),
            json!({ "type": "string" }),
        );
        vec!["snippet_id", "expected_updated_at"]
    } else {
        vec!["name", "category"]
    };
    object_schema(Value::Object(properties), &required)
}

fn enabled_tool_definitions() -> Vec<Value> {
    let enabled = invoke_control("__list_enabled_tools", json!({}))
        .ok()
        .and_then(|result| result.get("tools").and_then(Value::as_array).cloned())
        .unwrap_or_default();
    let enabled = enabled
        .iter()
        .filter_map(Value::as_str)
        .collect::<std::collections::HashSet<_>>();
    tool_definitions()
        .into_iter()
        .filter(|definition| {
            definition
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| enabled.contains(name))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_definitions_match_the_desktop_tool_catalog() {
        let definitions = tool_definitions();
        assert_eq!(definitions.len(), control_tools::CONTROL_TOOLS.len());
        for tool in control_tools::CONTROL_TOOLS {
            let definition = definitions
                .iter()
                .find(|definition| definition["name"] == tool.name)
                .unwrap_or_else(|| panic!("missing MCP definition for {}", tool.name));
            assert_eq!(definition["description"], tool.description);
            assert_eq!(definition["annotations"]["readOnlyHint"], tool.read_only);
        }
    }

    #[test]
    fn workspace_scoped_tools_advertise_an_optional_workspace_id() {
        let definitions = tool_definitions();
        for definition in &definitions {
            let name = definition["name"].as_str().expect("tool name");
            let properties = definition["inputSchema"]["properties"]
                .as_object()
                .expect("tool properties");
            if control_tools::workspace_scoped_tool(name) {
                assert_eq!(
                    properties["workspace_id"]["type"], "string",
                    "{name} must advertise workspace_id"
                );
                assert!(
                    !definition["inputSchema"]["required"]
                        .as_array()
                        .expect("required fields")
                        .iter()
                        .any(|field| field == "workspace_id"),
                    "{name} must keep workspace_id optional"
                );
            }
        }

        for name in ["status", "get_active_context", "list_snippets"] {
            let definition = definitions
                .iter()
                .find(|definition| definition["name"] == name)
                .unwrap_or_else(|| panic!("missing MCP definition for {name}"));
            assert!(
                definition["inputSchema"]["properties"]
                    .get("workspace_id")
                    .is_none(),
                "{name} must not advertise workspace_id"
            );
        }
    }

    #[test]
    fn expanded_tools_publish_the_expected_async_and_revision_schemas() {
        let definitions = tool_definitions();
        let definition = |name: &str| {
            definitions
                .iter()
                .find(|definition| definition["name"] == name)
                .unwrap_or_else(|| panic!("missing MCP definition for {name}"))
        };

        assert_eq!(
            definition("run_request_sequence")["inputSchema"]["properties"]["request_ids"]["maxItems"],
            25
        );
        assert_eq!(
            definition("get_request_sequence")["inputSchema"]["required"],
            json!([])
        );
        assert_eq!(
            definition("run_snippet")["inputSchema"]["required"],
            json!(["snippet_id", "request_id"])
        );
        assert_eq!(
            definition("save_snippet")["inputSchema"]["required"],
            json!(["snippet_id", "expected_updated_at"])
        );
        assert_eq!(
            definition("delete_snippet")["inputSchema"]["required"],
            json!(["snippet_id", "expected_updated_at"])
        );
        assert_eq!(
            definition("execute_http_request")["inputSchema"]["properties"]["overrides"]["additionalProperties"],
            false
        );
        assert_eq!(
            definition("query_http_response")["inputSchema"]["required"],
            json!(["json_pointer"])
        );
        assert_eq!(
            definition("query_http_response")["inputSchema"]["properties"]["limit"]["maximum"],
            1000
        );
        for name in [
            "get_active_context",
            "switch_workspace",
            "rename_collection",
            "delete_collection",
            "create_folder",
            "rename_folder",
            "move_folder",
            "delete_folder",
            "duplicate_request",
            "move_request",
            "delete_request",
            "delete_environment",
            "delete_environment_variable",
            "import_requests",
            "export_request",
            "run_request_sequence",
            "get_request_sequence",
            "list_snippets",
            "get_snippet",
            "create_snippet",
            "save_snippet",
            "delete_snippet",
            "run_snippet",
            "get_history_entry",
            "open_history_entry",
            "replay_history_request",
        ] {
            let _ = definition(name);
        }
    }
}
