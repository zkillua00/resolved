use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::mpsc,
    thread,
    time::Duration,
};

use super::{
    DatabaseStore, Environment, EnvironmentMutation, HeaderEntry, HistoryEntry, RequestDraft,
    RequestHistory, RequestNamespaceCatalog, RequestScripts, RequestTemplate, ScriptCancellation,
    ScriptEnvironment, ScriptScope, Workspace, build_client, execute_post_response_with_chain,
    execute_pre_request_with_chain, resolve_request, spawn_request,
};

const INITIAL_SECRET: &str = "initial-test-secret";
const ROTATED_SECRET: &str = "rotated-test-secret";
const PRE_REQUEST_SCRIPT: &str = r#"
api.environment.set("token", "rotated-test-secret");
api.request.method = "POST";
api.request.url += "?source=pre";
api.request.headers.set("X-Pre-Script", "ran");
api.request.body = api.request.body.replace('"draft"', '"prepared"');
console.log("rotated", api.environment.get("token"));
"#;
const POST_RESPONSE_SCRIPT: &str = r#"
const payload = api.response.json();
api.test("loopback response is usable", () => {
  api.assert(api.response.status === 200, "unexpected status");
  api.assert(payload.ok === true, "response body was not decoded");
});
api.environment.set("session", payload.session);
console.info("post", api.response.status);
"#;

#[test]
fn sqlite_backed_mvp_request_flow_persists_scripts_mutations_and_redacted_history() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback test server");
    let address = listener.local_addr().expect("read loopback address");
    let (captured_request_tx, captured_request_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept loopback request");
        let request = read_http_request(&mut stream);
        captured_request_tx
            .send(request)
            .expect("return captured request");

        let body = br#"{"ok":true,"session":"server-session"}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .expect("write response headers");
        stream.write_all(body).expect("write response body");
    });

    let directory = tempfile::tempdir().expect("create temporary database directory");
    let store =
        DatabaseStore::new(directory.path().join("api-tester.sqlite3")).with_history_limit(10);

    let mut workspace = Workspace::default();
    let environment_id = workspace
        .create_environment("Loopback")
        .expect("create environment");
    workspace
        .add_environment_variable(
            &environment_id,
            "base_url",
            format!("http://{address}"),
            true,
            false,
        )
        .expect("add base URL");
    workspace
        .add_environment_variable(&environment_id, "token", INITIAL_SECRET, true, true)
        .expect("add secret token");
    workspace
        .set_active_environment(Some(&environment_id))
        .expect("activate environment");

    let collection_id = workspace
        .create_collection("MVP smoke")
        .expect("create collection");
    let request = RequestDraft {
        method: "GET".to_owned(),
        url: "{{base_url}}/v1/ping".to_owned(),
        headers: vec![
            HeaderEntry::new("Content-Type", "application/json"),
            HeaderEntry::new("Authorization", "Bearer {{token}}"),
        ],
        body: r#"{"phase":"draft","token":"{{token}}"}"#.to_owned(),
        ..RequestDraft::default()
    };
    let request_id = workspace
        .create_saved_request(
            &collection_id,
            "Loopback request",
            RequestTemplate::new(request).with_scripts(RequestScripts {
                pre_request: PRE_REQUEST_SCRIPT.to_owned(),
                post_response: POST_RESPONSE_SCRIPT.to_owned(),
            }),
        )
        .expect("save request");

    store
        .save_state(&workspace, &RequestHistory::new(10))
        .expect("persist initial SQLite state");
    store.quick_check().expect("initial database is healthy");

    let initial_state = store.load_state().expect("reload initial SQLite state");
    let mut workspace = initial_state.workspace;
    let mut history = initial_state.history;
    let persisted_template = workspace
        .saved_request(&request_id)
        .expect("saved request survived reload")
        .1
        .definition
        .clone();
    assert_eq!(persisted_template.scripts.pre_request, PRE_REQUEST_SCRIPT);
    assert_eq!(
        persisted_template.scripts.post_response,
        POST_RESPONSE_SCRIPT
    );
    assert_eq!(
        workspace.active_environment_id.as_deref(),
        Some(environment_id.as_str())
    );

    let pre_scope = script_scope(workspace.active_environment());
    let pre_result = execute_pre_request_with_chain(
        &persisted_template.scripts.pre_request,
        &persisted_template.request,
        &pre_scope,
        &RequestNamespaceCatalog::default(),
        &ScriptCancellation::new(),
        None,
    )
    .expect("pre-request script succeeds");
    assert_eq!(pre_result.request.method, "POST");
    assert_eq!(pre_result.report.logs[0].message, "rotated [REDACTED]");
    apply_environment_mutations(
        &mut workspace,
        &environment_id,
        &pre_result.environment_mutations,
    );
    store
        .save_workspace(&workspace)
        .expect("persist pre-request environment mutation");
    workspace = store
        .load_workspace()
        .expect("reload pre-request environment mutation");

    let resolved = resolve_request(&pre_result.request, workspace.active_environment())
        .expect("resolve active environment variables");
    assert_eq!(
        resolved.request.url,
        format!("http://{address}/v1/ping?source=pre")
    );
    assert_eq!(
        resolved
            .request
            .headers
            .iter()
            .find(|header| header.name == "Authorization")
            .map(|header| header.value.as_str()),
        Some("Bearer rotated-test-secret")
    );
    assert!(resolved.request.body.contains(r#""phase":"prepared""#));
    assert!(resolved.request.body.contains(ROTATED_SECRET));
    assert!(
        resolved
            .sensitive_values
            .contains(&ROTATED_SECRET.to_owned())
    );

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("build Tokio runtime");
    let request_task = spawn_request(
        runtime.handle(),
        build_client().expect("build HTTP client"),
        resolved.request.clone(),
    );
    let response = runtime
        .block_on(request_task.wait())
        .expect("execute loopback request");
    assert_eq!(response.status, 200);
    assert_eq!(
        response.body.as_ref(),
        br#"{"ok":true,"session":"server-session"}"#
    );

    let captured_request = captured_request_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("receive captured loopback request");
    server.join().expect("join loopback server");
    let captured_request = String::from_utf8_lossy(&captured_request);
    assert!(captured_request.starts_with("POST /v1/ping?source=pre HTTP/1.1\r\n"));
    assert!(captured_request.contains("authorization: Bearer rotated-test-secret\r\n"));
    assert!(captured_request.contains("x-pre-script: ran\r\n"));
    assert!(captured_request.ends_with(&resolved.request.body));

    let post_scope = script_scope(workspace.active_environment());
    let post_result = execute_post_response_with_chain(
        &persisted_template.scripts.post_response,
        &resolved.request,
        &response,
        &post_scope,
        &RequestNamespaceCatalog::default(),
        &ScriptCancellation::new(),
        None,
    )
    .expect("post-response script succeeds");
    assert_eq!(post_result.report.tests.len(), 1);
    assert!(post_result.report.tests[0].passed);
    assert_eq!(
        post_result.report.tests[0].name,
        "loopback response is usable"
    );
    apply_environment_mutations(
        &mut workspace,
        &environment_id,
        &post_result.environment_mutations,
    );

    history.push(HistoryEntry::completed_with_secrets(
        &resolved.request,
        &response,
        &resolved.sensitive_values,
    ));
    store
        .save_state(&workspace, &history)
        .expect("persist final workspace and sanitized history");
    store.quick_check().expect("final database is healthy");

    let final_state = store.load_state().expect("reload final SQLite state");
    let final_request = final_state
        .workspace
        .saved_request(&request_id)
        .expect("saved request and scripts survive final reload")
        .1;
    assert_eq!(
        final_request.definition.scripts.pre_request,
        PRE_REQUEST_SCRIPT
    );
    assert_eq!(
        final_request.definition.scripts.post_response,
        POST_RESPONSE_SCRIPT
    );
    let final_environment = final_state
        .workspace
        .active_environment()
        .expect("active environment survives final reload");
    let rotated_token = final_environment
        .variables
        .iter()
        .find(|variable| variable.key == "token")
        .expect("rotated token survives final reload");
    assert_eq!(rotated_token.value, ROTATED_SECRET);
    assert!(rotated_token.secret);
    assert_eq!(
        final_environment
            .variables
            .iter()
            .find(|variable| variable.key == "session")
            .map(|variable| variable.value.as_str()),
        Some("server-session")
    );

    assert_eq!(final_state.history.len(), 1);
    let history_entry = &final_state.history.entries()[0];
    assert_eq!(
        history_entry
            .response
            .as_ref()
            .map(|summary| summary.status),
        Some(200)
    );
    assert_eq!(
        history_entry
            .request
            .headers
            .iter()
            .find(|header| header.name == "Authorization")
            .map(|header| header.value.as_str()),
        Some("[REDACTED]")
    );
    assert!(history_entry.request.body.contains("[REDACTED]"));
    let serialized_history =
        serde_json::to_string(final_state.history.entries()).expect("serialize reloaded history");
    assert!(!serialized_history.contains(INITIAL_SECRET));
    assert!(!serialized_history.contains(ROTATED_SECRET));
}

fn script_scope(environment: Option<&Environment>) -> ScriptScope {
    let mut script_environment = ScriptEnvironment::default();
    if let Some(environment) = environment {
        for variable in environment
            .variables
            .iter()
            .filter(|variable| variable.enabled)
        {
            if variable.secret {
                script_environment.insert_secret(variable.key.clone(), variable.value.clone());
            } else {
                script_environment.insert(variable.key.clone(), variable.value.clone());
            }
        }
    }
    ScriptScope {
        environment: script_environment,
        ..ScriptScope::default()
    }
}

fn apply_environment_mutations(
    workspace: &mut Workspace,
    environment_id: &str,
    mutations: &[EnvironmentMutation],
) {
    for mutation in mutations {
        match mutation {
            EnvironmentMutation::Set { key, value } => {
                let existing = workspace
                    .environment(environment_id)
                    .expect("active environment exists")
                    .variables
                    .iter()
                    .find(|variable| variable.key == *key)
                    .map(|variable| (variable.id.clone(), variable.enabled, variable.secret));
                if let Some((id, enabled, secret)) = existing {
                    workspace
                        .update_environment_variable(
                            environment_id,
                            &id,
                            key,
                            value,
                            enabled,
                            secret,
                        )
                        .expect("update environment variable");
                } else {
                    workspace
                        .add_environment_variable(environment_id, key, value, true, false)
                        .expect("add environment variable");
                }
            }
            EnvironmentMutation::Unset { key } => {
                let variable_id = workspace
                    .environment(environment_id)
                    .expect("active environment exists")
                    .variables
                    .iter()
                    .find(|variable| variable.key == *key)
                    .map(|variable| variable.id.clone());
                if let Some(variable_id) = variable_id {
                    workspace
                        .remove_environment_variable(environment_id, &variable_id)
                        .expect("remove environment variable");
                }
            }
        }
    }
}

fn read_http_request(stream: &mut impl Read) -> Vec<u8> {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let read = stream.read(&mut buffer).expect("read loopback request");
        if read == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..read]);

        let Some(headers_end) = request
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|position| position + 4)
        else {
            continue;
        };
        let headers = String::from_utf8_lossy(&request[..headers_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or_default();
        if request.len() >= headers_end + content_length {
            break;
        }
    }
    request
}
