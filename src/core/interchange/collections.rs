use super::*;
use crate::core::{QueryParamEntry, url_with_query_params};

fn text(value: &Value, key: &str) -> String {
    value.get(key).and_then(value_as_text).unwrap_or_default()
}

pub(super) fn import_postman(source: &str) -> Result<Option<ImportBundle>, InterchangeError> {
    if !source.starts_with('{') {
        return Ok(None);
    }
    let Ok(document) = serde_json::from_str::<Value>(source) else {
        return Ok(None);
    };
    let root = document.get("collection").unwrap_or(&document);
    if root.get("info").is_none() || root.get("item").is_none() {
        return Ok(None);
    }
    let schema = root
        .pointer("/info/schema")
        .and_then(Value::as_str)
        .unwrap_or("");
    if !schema.contains("/v2.0.0/") && !schema.contains("/v2.1.0/") {
        return Err(InterchangeError::Unsupported(
            "expected a Postman collection v2.0 or v2.1 export".into(),
        ));
    }
    let mut bundle = ImportBundle {
        source_format: "Postman".into(),
        ..Default::default()
    };
    let mut collection = ImportedCollection {
        name: root
            .pointer("/info/name")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("Postman collection")
            .into(),
        ..Default::default()
    };
    visit_postman(root, &[], None, &mut collection, &mut bundle, 0)?;
    if bundle.requests.is_empty() {
        return Err(InterchangeError::Unsupported(
            "the Postman collection contains no requests".into(),
        ));
    }
    bundle.collections.push(collection);
    Ok(Some(bundle))
}

fn warn(bundle: &mut ImportBundle, message: &str) {
    if !bundle.warnings.iter().any(|v| v == message) {
        bundle.warnings.push(message.into());
    }
}

fn visit_postman<'a>(
    node: &'a Value,
    path: &[String],
    inherited_auth: Option<&'a Value>,
    collection: &mut ImportedCollection,
    bundle: &mut ImportBundle,
    depth: usize,
) -> Result<(), InterchangeError> {
    if depth > 32 {
        return Err(InterchangeError::Parse(
            "Postman folders exceed 32 levels".into(),
        ));
    }
    if node
        .get("event")
        .and_then(Value::as_array)
        .is_some_and(|v| !v.is_empty())
    {
        warn(
            bundle,
            "Postman scripts are not imported; their pm API is not supported.",
        );
    }
    if node
        .get("variable")
        .and_then(Value::as_array)
        .is_some_and(|v| !v.is_empty())
    {
        warn(
            bundle,
            "Variable placeholders are preserved. Add collection variable values to a Resolved environment.",
        );
    }
    let auth = node.get("auth").filter(|v| !v.is_null()).or(inherited_auth);
    let items = node
        .get("item")
        .and_then(Value::as_array)
        .ok_or_else(|| InterchangeError::Parse("Postman folder has no item array".into()))?;
    for item in items {
        if item.get("item").is_some() {
            let mut child = path.to_vec();
            let name = text(item, "name");
            child.push(if name.trim().is_empty() {
                "Untitled folder".into()
            } else {
                name
            });
            collection.folders.push(child.clone());
            visit_postman(item, &child, auth, collection, bundle, depth + 1)?;
            continue;
        }
        let request_value = item
            .get("request")
            .ok_or_else(|| InterchangeError::Parse("Postman item has no request".into()))?;
        if item
            .get("event")
            .and_then(Value::as_array)
            .is_some_and(|v| !v.is_empty())
        {
            warn(
                bundle,
                "Postman scripts are not imported; their pm API is not supported.",
            );
        }
        let mut request = postman_request(request_value, bundle)?;
        apply_postman_auth(
            request_value.get("auth").filter(|v| !v.is_null()).or(auth),
            &mut request,
            bundle,
        );
        let name = normalized_name(&text(item, "name"), &request);
        collection
            .requests
            .push((bundle.requests.len(), path.to_vec()));
        bundle.requests.push(ImportedRequest {
            name,
            template: RequestTemplate::new(request),
        });
        enforce_request_count(&bundle.requests)?;
    }
    Ok(())
}

fn postman_request(
    value: &Value,
    bundle: &mut ImportBundle,
) -> Result<RequestDraft, InterchangeError> {
    if let Some(url) = value.as_str() {
        return Ok(RequestDraft::new("GET", url));
    }
    let url_value = value
        .get("url")
        .ok_or_else(|| InterchangeError::Parse("Postman request has no URL".into()))?;
    let mut url = url_value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| text(url_value, "raw"));
    if url.is_empty() {
        let parts = |key: &str, separator: &str| {
            url_value
                .get(key)
                .map(|v| {
                    v.as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(Value::as_str)
                                .collect::<Vec<_>>()
                                .join(separator)
                        })
                        .unwrap_or_else(|| v.as_str().unwrap_or("").into())
                })
                .unwrap_or_default()
        };
        let host = parts("host", ".");
        if host.is_empty() {
            return Err(InterchangeError::Parse(
                "Postman request URL has no host".into(),
            ));
        }
        let protocol = text(url_value, "protocol");
        url = if protocol.is_empty() {
            host
        } else {
            format!("{protocol}://{host}")
        };
        let port = text(url_value, "port");
        if !port.is_empty() {
            url.push_str(&format!(":{port}"));
        }
        url.push('/');
        url.push_str(&parts("path", "/"));
        let hash = text(url_value, "hash");
        if !hash.is_empty() {
            url.push('#');
            url.push_str(&hash);
        }
    }
    // Postman's colon path parameters are placeholders, not literal wire paths.
    let mut segments = url.split('/').map(str::to_owned).collect::<Vec<_>>();
    for segment in &mut segments {
        if let Some(name) = segment.strip_prefix(':') {
            let end = name.find(['?', '#']).unwrap_or(name.len());
            *segment = format!("{{{{{}}}}}{}", &name[..end], &name[end..]);
        }
    }
    url = segments.join("/");
    let method = text(value, "method");
    let mut request = RequestDraft::new(if method.is_empty() { "GET" } else { &method }, url);
    if let Some(query) = url_value.get("query").and_then(Value::as_array) {
        if query.iter().any(|row| row.get("description").is_some_and(|value| !value.is_null())) {
            warn(
                bundle,
                "Postman query descriptions are not imported. Author explanations with @param annotations in Documentation.",
            );
        }
        request.query_params = query
            .iter()
            .map(|row| QueryParamEntry {
                enabled: row.get("disabled").and_then(Value::as_bool) != Some(true),
                key: text(row, "key"),
                value: text(row, "value"),
            })
            .collect();
        request.url = url_with_query_params(&request.url, &request.query_params);
    }
    if let Some(headers) = value.get("header") {
        if let Some(rows) = headers.as_array() {
            request.headers = rows
                .iter()
                .map(|row| {
                    let mut header = HeaderEntry::new(text(row, "key"), text(row, "value"));
                    header.enabled = row.get("disabled").and_then(Value::as_bool) != Some(true);
                    header
                })
                .collect();
        } else if let Some(lines) = headers.as_str() {
            request.headers = lines
                .lines()
                .filter_map(|line| line.split_once(':'))
                .map(|(k, v)| HeaderEntry::new(k.trim(), v.trim()))
                .collect();
        }
    }
    if let Some(body) = value
        .get("body")
        .filter(|body| body.get("disabled").and_then(Value::as_bool) != Some(true))
    {
        match body.get("mode").and_then(Value::as_str).unwrap_or("") {
            "raw" => {
                request.body = text(body, "raw");
                request.raw_body_language = match body
                    .pointer("/options/raw/language")
                    .and_then(Value::as_str)
                {
                    Some("json") => RawBodyLanguage::Json,
                    Some("xml") => raw_language_for_content_type("application/xml"),
                    Some("html") => raw_language_for_content_type("text/html"),
                    _ => raw_language_for_content_type("text/plain"),
                };
            }
            mode @ ("urlencoded" | "formdata") => {
                request.body_mode = if mode == "formdata" {
                    BodyMode::MultipartFormData
                } else {
                    BodyMode::FormUrlEncoded
                };
                if let Some(rows) = body.get(mode).and_then(Value::as_array) {
                    for row in rows {
                        let file = text(row, "type") == "file";
                        let values = if file {
                            row.get("src")
                                .and_then(Value::as_array)
                                .cloned()
                                .unwrap_or_else(|| {
                                    vec![
                                        row.get("src")
                                            .cloned()
                                            .unwrap_or(Value::String(String::new())),
                                    ]
                                })
                        } else {
                            vec![row.get("value").cloned().unwrap_or(Value::Null)]
                        };
                        for value in values {
                            let mut field = if file {
                                BodyField::file(
                                    text(row, "key"),
                                    value_as_text(&value).unwrap_or_default(),
                                )
                            } else {
                                BodyField::text(
                                    text(row, "key"),
                                    value_as_text(&value).unwrap_or_default(),
                                )
                            };
                            field.enabled =
                                row.get("disabled").and_then(Value::as_bool) != Some(true);
                            request.body_fields.push(field);
                        }
                    }
                }
            }
            "graphql" => {
                let graphql = &body["graphql"];
                let variables = graphql
                    .get("variables")
                    .and_then(Value::as_str)
                    .filter(|v| !v.trim().is_empty())
                    .map(serde_json::from_str::<Value>)
                    .transpose()
                    .map_err(|e| {
                        InterchangeError::Parse(format!("Invalid GraphQL variables: {e}"))
                    })?
                    .unwrap_or(json!({}));
                request.body = serde_json::to_string_pretty(
                    &json!({"query": text(graphql, "query"), "variables": variables}),
                )
                .unwrap();
            }
            "" => {}
            mode => {
                return Err(InterchangeError::Unsupported(format!(
                    "Postman body mode {mode} cannot be imported"
                )));
            }
        }
    }
    apply_content_type_language(&mut request);
    if url_value
        .get("variable")
        .and_then(Value::as_array)
        .is_some_and(|v| !v.is_empty())
    {
        warn(
            bundle,
            "URL variable placeholders are preserved. Add their values to a Resolved environment.",
        );
    }
    Ok(request)
}

fn apply_postman_auth(auth: Option<&Value>, request: &mut RequestDraft, bundle: &mut ImportBundle) {
    let Some(auth) = auth else {
        return;
    };
    let kind = text(auth, "type");
    let rows = auth.get(&kind).and_then(Value::as_array);
    let field = |name: &str| {
        rows.and_then(|rows| {
            rows.iter()
                .find(|row| row.get("key").and_then(Value::as_str) == Some(name))
        })
        .map(|row| text(row, "value"))
        .unwrap_or_default()
    };
    match kind.as_str() {
        "" | "noauth" => {}
        "bearer" => request.headers.push(HeaderEntry::new(
            "Authorization",
            format!("Bearer {}", field("token")),
        )),
        "basic" => {
            let user = field("username");
            let password = field("password");
            if user.contains("{{") || password.contains("{{") {
                warn(
                    bundle,
                    "Basic auth contains variables. Set Authorization to a resolved Basic credential before sending.",
                );
                request
                    .headers
                    .push(HeaderEntry::new("Authorization", "Basic {{basicAuth}}"));
            } else {
                request.headers.push(HeaderEntry::new(
                    "Authorization",
                    format!(
                        "Basic {}",
                        base64::engine::general_purpose::STANDARD
                            .encode(format!("{user}:{password}"))
                    ),
                ));
            }
        }
        "apikey" => {
            let key = field("key");
            let value = field("value");
            if field("in") == "query" {
                request.query_params.push(QueryParamEntry::new(key, value));
                request.url = url_with_query_params(&request.url, &request.query_params);
            } else {
                request.headers.push(HeaderEntry::new(key, value));
            }
        }
        _ => warn(
            bundle,
            &format!("Postman {kind} authentication must be configured manually."),
        ),
    }
}

pub(super) fn operation_folder(operation: &Map<String, Value>, path: &str) -> Vec<String> {
    if let Some(tag) = operation
        .get("tags")
        .and_then(Value::as_array)
        .and_then(|v| v.first())
        .and_then(Value::as_str)
        .filter(|v| !v.trim().is_empty())
    {
        return vec![tag.to_owned()];
    }
    path.split('/')
        .filter(|s| !s.is_empty() && !s.starts_with('{'))
        .map(str::to_owned)
        .collect()
}

pub(super) fn spec_warnings(root: &Value) -> Vec<String> {
    let mut warnings = Vec::new();
    if root.get("security").is_some()
        || root.pointer("/components/securitySchemes").is_some()
        || root.get("securityDefinitions").is_some()
    {
        warnings.push("API authentication requirements must be configured before sending.".into());
    }
    warnings
}

// Normalize Swagger 2's wire fields into the existing OpenAPI request importer.
// References remain local; no documents or code are fetched or executed.
pub(super) fn swagger_to_openapi(source: &Value) -> Result<Value, InterchangeError> {
    let mut root = source.clone();
    let scheme = source
        .get("schemes")
        .and_then(Value::as_array)
        .and_then(|v| v.first())
        .and_then(Value::as_str)
        .unwrap_or("https");
    let host = source
        .get("host")
        .and_then(Value::as_str)
        .unwrap_or("{{host}}");
    root["servers"] = json!([{"url": format!("{scheme}://{host}{}", text(source,"basePath"))}]);
    let paths = root
        .get_mut("paths")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| InterchangeError::Parse("Swagger document has no paths object".into()))?;
    for path in paths.values_mut() {
        *path = resolve_local_reference(source, path).clone();
        let Some(path) = path.as_object_mut() else {
            continue;
        };
        let inherited = path
            .get("parameters")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for method in ["get", "post", "put", "patch", "delete", "head", "options"] {
            let Some(operation) = path.get_mut(method).and_then(Value::as_object_mut) else {
                continue;
            };
            if let Some(scheme) = operation
                .get("schemes")
                .and_then(Value::as_array)
                .and_then(|v| v.first())
                .and_then(Value::as_str)
            {
                operation.insert(
                    "servers".into(),
                    json!([{"url":format!("{scheme}://{host}{}",text(source,"basePath"))}]),
                );
            }
            let content_type = operation
                .get("consumes")
                .or_else(|| source.get("consumes"))
                .and_then(Value::as_array)
                .and_then(|v| v.first())
                .and_then(Value::as_str)
                .unwrap_or("application/json")
                .to_owned();
            let mut parameters = inherited.clone();
            parameters.extend(
                operation
                    .get("parameters")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
            );
            let mut seen = std::collections::HashSet::new();
            let mut forms = Map::new();
            let mut body = None;
            for parameter in parameters.iter().rev() {
                let parameter = resolve_local_reference(source, parameter);
                let location = text(parameter, "in");
                let name = text(parameter, "name");
                if !seen.insert((location.clone(), name.clone())) {
                    continue;
                }
                if location == "body" {
                    let schema = parameter.get("schema").cloned().unwrap_or(json!({}));
                    body = Some(
                        json!({"content":{&content_type:{"schema":schema,"example":schema_example(source, &parameter["schema"], 0)}}}),
                    );
                } else if location == "formData" {
                    let mut schema = parameter.clone();
                    if text(parameter, "type") == "file" {
                        schema["format"] = json!("binary");
                    }
                    forms.insert(name, schema);
                }
            }
            if !forms.is_empty() {
                let mime = if content_type == "application/json" {
                    "application/x-www-form-urlencoded"
                } else {
                    &content_type
                };
                body =
                    Some(json!({"content":{mime:{"schema":{"type":"object","properties":forms}}}}));
            }
            if let Some(body) = body {
                operation.insert("requestBody".into(), body);
            }
        }
    }
    Ok(root)
}

fn schema_example(root: &Value, schema: &Value, depth: usize) -> Value {
    if depth > 12 {
        return json!("{{value}}");
    }
    let schema = resolve_local_reference(root, schema);
    if let Some(value) = schema.get("example").or_else(|| schema.get("default")) {
        return value.clone();
    }
    if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
        return Value::Object(
            properties
                .iter()
                .map(|(name, schema)| {
                    let value = if schema.get("example").is_none()
                        && schema.get("default").is_none()
                        && schema.get("properties").is_none()
                        && schema.get("$ref").is_none()
                        && schema.get("items").is_none()
                    {
                        json!(format!("{{{{{name}}}}}"))
                    } else {
                        schema_example(root, schema, depth + 1)
                    };
                    (name.clone(), value)
                })
                .collect(),
        );
    }
    if let Some(items) = schema.get("items") {
        return json!([schema_example(root, items, depth + 1)]);
    }
    json!("{{value}}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collection(items: Value) -> String {
        json!({"info":{"name":"Store","schema":"https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},"auth":{"type":"bearer","bearer":[{"key":"token","value":"{{token}}"}]},"item":items}).to_string()
    }

    #[test]
    fn postman_preserves_folders_disabled_fields_and_inherited_auth() {
        let source = collection(
            json!([{"name":"Users","item":[{"name":"Empty","item":[]},{"name":"Create","request":{"method":"POST","url":{"raw":"https://example.test/users/:id?off=1","query":[{"key":"off","value":"1","disabled":true},{"key":"q","value":"a&b"}]},"header":[{"key":"X-Off","value":"secret","disabled":true}],"body":{"mode":"raw","raw":"{\"name\":\"Ada\"}","options":{"raw":{"language":"json"}}}}}]}]),
        );
        let bundle = import_requests(&source).unwrap();
        assert_eq!(bundle.collections[0].name, "Store");
        assert_eq!(
            bundle.collections[0].folders,
            vec![vec!["Users"], vec!["Users", "Empty"]]
        );
        let request = &bundle.requests[0].template.request;
        assert_eq!(request.url, "https://example.test/users/{{id}}?q=a%26b");
        assert!(!request.query_params[0].enabled);
        assert!(!request.headers[0].enabled);
        assert_eq!(request.headers[1].value, "Bearer {{token}}");
        assert_eq!(request.body, "{\"name\":\"Ada\"}");
    }

    #[test]
    fn postman_warns_when_separate_query_descriptions_are_omitted() {
        let source = collection(json!([{
            "name": "Search",
            "request": {
                "url": {
                    "raw": "https://example.test?limit=25",
                    "query": [{"key": "limit", "value": "25", "description": "Page size"}]
                }
            }
        }]));
        let bundle = import_requests(&source).unwrap();
        assert!(bundle.warnings.iter().any(|warning| warning.contains("query descriptions")));
        assert_eq!(bundle.requests[0].template.request.query_params[0], QueryParamEntry::new("limit", "25"));
    }

    #[test]
    fn postman_v2_noauth_overrides_parent_and_form_files_stay_files() {
        let source = collection(json!([{"name":"Upload","request":{"method":"POST","url":"https://example.test/upload","auth":{"type":"noauth"},"body":{"mode":"formdata","formdata":[{"key":"file","type":"file","src":["a.txt","b.txt"]},{"key":"hidden","value":"x","disabled":true}]}}}])).replace("v2.1.0","v2.0.0");
        let bundle = import_requests(&source).unwrap();
        let request = &bundle.requests[0].template.request;
        assert!(request.headers.is_empty());
        assert_eq!(request.body_mode, BodyMode::MultipartFormData);
        assert_eq!(request.body_fields.len(), 3);
        assert_eq!(request.body_fields[0].kind, BodyFieldKind::File);
        assert!(!request.body_fields[2].enabled);
    }

    #[test]
    fn swagger_imports_refs_body_and_parameter_overrides() {
        let source = r##"swagger: '2.0'
info: {title: Store, version: '1'}
host: example.test
basePath: /v1
schemes: [https]
consumes: [application/json]
definitions:
  User:
    type: object
    properties:
      name: {type: string, example: Ada}
paths:
  /users/{id}:
    parameters:
      - {in: query, name: q, type: string, default: old}
    post:
      tags: [Users]
      parameters:
        - {in: query, name: q, type: string, default: 'a&b'}
        - in: body
          name: user
          schema: {$ref: '#/definitions/User'}
      responses: {'200': {description: ok}}
"##;
        let bundle = import_requests(source).unwrap();
        assert_eq!(bundle.source_format, "Swagger 2.0");
        assert_eq!(bundle.collections[0].requests[0].1, vec!["Users"]);
        let request = &bundle.requests[0].template.request;
        assert_eq!(request.url, "https://example.test/v1/users/{{id}}?q=a%26b");
        assert_eq!(
            serde_json::from_str::<Value>(&request.body).unwrap(),
            json!({"name":"Ada"})
        );
        assert_eq!(request.query_params.len(), 1);
    }

    #[test]
    fn swagger_multipart_and_untagged_paths() {
        let bundle = import_requests(r#"{"swagger":"2.0","info":{"title":"Files"},"host":"example.test","paths":{"/v1/files":{"post":{"schemes":["http"],"consumes":["multipart/form-data"],"parameters":[{"in":"formData","name":"upload","type":"file"}]}}}}"#).unwrap();
        assert_eq!(bundle.collections[0].requests[0].1, vec!["v1", "files"]);
        let request = &bundle.requests[0].template.request;
        assert_eq!(request.url, "http://example.test/v1/files");
        assert_eq!(request.body_mode, BodyMode::MultipartFormData);
        assert_eq!(request.body_fields[0].kind, BodyFieldKind::File);
    }

    #[test]
    fn unsupported_postman_body_is_explicit_and_scripts_are_not_executed() {
        assert!(import_requests(&collection(json!([{"request":{"url":"https://example.test","body":{"mode":"file","file":{"src":"a"}}}}]))).is_err());
        let bundle = import_requests(&collection(json!([{"event":[{"script":{"exec":["throw new Error('never run')"]}}],"request":"https://example.test"}]))).unwrap();
        assert_eq!(bundle.warnings.len(), 1);
        assert!(bundle.requests[0].template.scripts.pre_request.is_empty());
    }
}
