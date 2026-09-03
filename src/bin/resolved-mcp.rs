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
                        "version": env!("CARGO_PKG_VERSION")
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

fn tool(name: &str, _description: &str, input_schema: Value, _read_only: bool) -> Value {
    let catalog = control_tools::tool(name).expect("every MCP definition must be in the catalog");
    let _label = catalog.label;
    json!({
        "name": name,
        "description": catalog.description,
        "inputSchema": input_schema,
        "annotations": {
            "readOnlyHint": catalog.read_only,
            "destructiveHint": false,
            "idempotentHint": catalog.read_only
        }
    })
}

fn tool_definitions() -> Vec<Value> {
    let empty = || object_schema(json!({}), &[]);
    let request_draft = json!({
        "type": "object",
        "description": "Resolved request draft. Fields omitted by Resolved defaults are optional.",
        "properties": {
            "method": { "type": "string" },
            "url": { "type": "string" },
            "query_params": { "type": "array", "items": { "type": "object" } },
            "headers": { "type": "array", "items": { "type": "object" } },
            "body": { "type": "string" },
            "body_mode": { "type": "string", "enum": ["none", "raw", "form_url_encoded", "multipart_form_data"] },
            "raw_body_language": { "type": "string" },
            "body_fields": { "type": "array", "items": { "type": "object" } }
        },
        "required": ["method", "url"],
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
                    "scripts": { "type": "object", "properties": { "pre_request": { "type": "string" }, "post_response": { "type": "string" } }, "additionalProperties": false }
                }),
                &["collection_id", "name", "request"],
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
    ]
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
}
