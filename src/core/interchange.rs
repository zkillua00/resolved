//! Safe, text-only request interchange.
//!
//! Importers never execute pasted source. They recover request literals and
//! preserve supported dynamic URL expressions as Resolved placeholders. Every
//! generated command/snippet also carries a compact Resolved metadata comment,
//! which makes exporting and re-importing lossless without affecting execution.

use std::fmt::Write as _;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use thiserror::Error;
use url::Url;

use super::{
    BodyField, BodyFieldKind, BodyMode, HeaderEntry, RawBodyLanguage, RequestDraft, RequestTemplate,
};

pub const MAX_INTERCHANGE_BYTES: usize = 8 * 1024 * 1024;
const METADATA_MARKER: &str = "@resolved-request ";
const MULTIPART_BOUNDARY: &str = "resolved-boundary-7MA4YWxkTrZu0gW";

/// A concrete command, specification, language, or framework representation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InterchangeFormat {
    Curl,
    Wget,
    PowerShell,
    OpenApi,
    AsyncApi,
    IntelliJHttp,
    JavaScriptFetch,
    JavaScriptAxios,
    JavaScriptJquery,
    JavaHttpClient,
    JavaOkHttp,
    GoNetHttp,
    GoResty,
    CSharpHttpClient,
    CSharpRestSharp,
    RustReqwest,
    RustUreq,
    CppBoostBeast,
    CppLibcurl,
    PhpCurl,
    PhpGuzzle,
    KotlinKtor,
    KotlinOkHttp,
    KotlinJavaHttpClient,
}

impl InterchangeFormat {
    pub const ALL: &'static [Self] = &[
        Self::Curl,
        Self::Wget,
        Self::PowerShell,
        Self::OpenApi,
        Self::AsyncApi,
        Self::IntelliJHttp,
        Self::JavaScriptFetch,
        Self::JavaScriptAxios,
        Self::JavaScriptJquery,
        Self::JavaHttpClient,
        Self::JavaOkHttp,
        Self::GoNetHttp,
        Self::GoResty,
        Self::CSharpHttpClient,
        Self::CSharpRestSharp,
        Self::RustReqwest,
        Self::RustUreq,
        Self::CppBoostBeast,
        Self::CppLibcurl,
        Self::PhpCurl,
        Self::PhpGuzzle,
        Self::KotlinKtor,
        Self::KotlinOkHttp,
        Self::KotlinJavaHttpClient,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Curl => "cURL",
            Self::Wget => "Wget",
            Self::PowerShell => "PowerShell Invoke-WebRequest",
            Self::OpenApi => "OpenAPI 3.1 (YAML)",
            Self::AsyncApi => "AsyncAPI 3.0 (YAML)",
            Self::IntelliJHttp => "IntelliJ HTTP Client",
            Self::JavaScriptFetch => "JavaScript · Fetch",
            Self::JavaScriptAxios => "JavaScript · Axios",
            Self::JavaScriptJquery => "JavaScript · jQuery",
            Self::JavaHttpClient => "Java · java.net.http",
            Self::JavaOkHttp => "Java · OkHttp",
            Self::GoNetHttp => "Go · net/http",
            Self::GoResty => "Go · Resty",
            Self::CSharpHttpClient => "C# · HttpClient",
            Self::CSharpRestSharp => "C# · RestSharp",
            Self::RustReqwest => "Rust · reqwest",
            Self::RustUreq => "Rust · ureq",
            Self::CppBoostBeast => "C++ · Boost.Beast",
            Self::CppLibcurl => "C++ · libcurl",
            Self::PhpCurl => "PHP · cURL",
            Self::PhpGuzzle => "PHP · Guzzle",
            Self::KotlinKtor => "Kotlin · Ktor",
            Self::KotlinOkHttp => "Kotlin · OkHttp",
            Self::KotlinJavaHttpClient => "Kotlin · java.net.http",
        }
    }

    pub const fn group(self) -> &'static str {
        match self {
            Self::Curl | Self::Wget | Self::PowerShell => "Command line",
            Self::OpenApi | Self::AsyncApi => "API specifications",
            Self::IntelliJHttp => "HTTP Client",
            Self::JavaScriptFetch | Self::JavaScriptAxios | Self::JavaScriptJquery => "JavaScript",
            Self::JavaHttpClient | Self::JavaOkHttp => "Java",
            Self::GoNetHttp | Self::GoResty => "Go",
            Self::CSharpHttpClient | Self::CSharpRestSharp => "C#",
            Self::RustReqwest | Self::RustUreq => "Rust",
            Self::CppBoostBeast | Self::CppLibcurl => "C++",
            Self::PhpCurl | Self::PhpGuzzle => "PHP",
            Self::KotlinKtor | Self::KotlinOkHttp | Self::KotlinJavaHttpClient => "Kotlin",
        }
    }

    pub const fn file_extension(self) -> &'static str {
        match self {
            Self::Curl | Self::Wget => "sh",
            Self::PowerShell => "ps1",
            Self::OpenApi | Self::AsyncApi => "yaml",
            Self::IntelliJHttp => "http",
            Self::JavaScriptFetch | Self::JavaScriptAxios | Self::JavaScriptJquery => "js",
            Self::JavaHttpClient | Self::JavaOkHttp => "java",
            Self::GoNetHttp | Self::GoResty => "go",
            Self::CSharpHttpClient | Self::CSharpRestSharp => "cs",
            Self::RustReqwest | Self::RustUreq => "rs",
            Self::CppBoostBeast | Self::CppLibcurl => "cpp",
            Self::PhpCurl | Self::PhpGuzzle => "php",
            Self::KotlinKtor | Self::KotlinOkHttp | Self::KotlinJavaHttpClient => "kt",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportedRequest {
    pub name: String,
    pub template: RequestTemplate,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportBundle {
    pub source_format: String,
    pub requests: Vec<ImportedRequest>,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum InterchangeError {
    #[error("the import text is empty")]
    Empty,
    #[error("the import is larger than the {MAX_INTERCHANGE_BYTES} byte safety limit")]
    TooLarge,
    #[error("no HTTP request could be found: {0}")]
    Unsupported(String),
    #[error("the request text could not be parsed: {0}")]
    Parse(String),
    #[error("the request could not be exported: {0}")]
    Export(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PortableRequest {
    name: String,
    template: RequestTemplate,
}

pub fn export_request(
    format: InterchangeFormat,
    name: &str,
    template: &RequestTemplate,
) -> Result<String, InterchangeError> {
    let name = normalized_name(name, &template.request);
    match format {
        InterchangeFormat::Curl => Ok(export_curl(&name, template)),
        InterchangeFormat::Wget => Ok(export_wget(&name, template)),
        InterchangeFormat::PowerShell => Ok(export_powershell(&name, template)),
        InterchangeFormat::OpenApi => export_openapi(&name, template),
        InterchangeFormat::AsyncApi => export_asyncapi(&name, template),
        InterchangeFormat::IntelliJHttp => Ok(export_intellij_http(&name, template)),
        InterchangeFormat::JavaScriptFetch => Ok(export_javascript_fetch(&name, template)),
        InterchangeFormat::JavaScriptAxios => Ok(export_javascript_axios(&name, template)),
        InterchangeFormat::JavaScriptJquery => Ok(export_javascript_jquery(&name, template)),
        InterchangeFormat::JavaHttpClient => Ok(export_java_http_client(&name, template)),
        InterchangeFormat::JavaOkHttp => Ok(export_java_okhttp(&name, template)),
        InterchangeFormat::GoNetHttp => Ok(export_go_net_http(&name, template)),
        InterchangeFormat::GoResty => Ok(export_go_resty(&name, template)),
        InterchangeFormat::CSharpHttpClient => Ok(export_csharp_http_client(&name, template)),
        InterchangeFormat::CSharpRestSharp => export_csharp_restsharp(&name, template),
        InterchangeFormat::RustReqwest => Ok(export_rust_reqwest(&name, template)),
        InterchangeFormat::RustUreq => Ok(export_rust_ureq(&name, template)),
        InterchangeFormat::CppBoostBeast => Ok(export_cpp_boost_beast(&name, template)),
        InterchangeFormat::CppLibcurl => Ok(export_cpp_libcurl(&name, template)),
        InterchangeFormat::PhpCurl => Ok(export_php_curl(&name, template)),
        InterchangeFormat::PhpGuzzle => Ok(export_php_guzzle(&name, template)),
        InterchangeFormat::KotlinKtor => Ok(export_kotlin_ktor(&name, template)),
        InterchangeFormat::KotlinOkHttp => Ok(export_kotlin_okhttp(&name, template)),
        InterchangeFormat::KotlinJavaHttpClient => {
            Ok(export_kotlin_java_http_client(&name, template))
        }
    }
}

/// Auto-detect and parse one or more requests without executing source code.
pub fn import_requests(source: &str) -> Result<ImportBundle, InterchangeError> {
    if source.len() > MAX_INTERCHANGE_BYTES {
        return Err(InterchangeError::TooLarge);
    }
    let source = source.trim_start_matches('\u{feff}').trim();
    if source.is_empty() {
        return Err(InterchangeError::Empty);
    }

    if let Some(portable) = decode_metadata(source)? {
        return Ok(ImportBundle {
            source_format: detect_source_label(source).to_owned(),
            requests: vec![ImportedRequest {
                name: portable.name,
                template: portable.template,
            }],
        });
    }

    if looks_like_openapi(source) {
        return import_openapi(source);
    }
    if looks_like_asyncapi(source) {
        return import_asyncapi(source);
    }
    if looks_like_intellij_http(source) {
        return import_intellij_http(source);
    }
    if starts_with_command(source, "curl") {
        return one_request("cURL", parse_curl(source)?);
    }
    if starts_with_command(source, "wget") {
        return one_request("Wget", parse_wget(source)?);
    }
    if contains_any_ignore_ascii_case(
        source,
        &[
            "invoke-webrequest",
            "invoke-restmethod",
            "invoke-getrequest",
        ],
    ) {
        return one_request("PowerShell", parse_powershell(source)?);
    }

    one_request(detect_source_label(source), parse_code_request(source)?)
}

fn one_request(
    source_format: &str,
    request: ImportedRequest,
) -> Result<ImportBundle, InterchangeError> {
    Ok(ImportBundle {
        source_format: source_format.to_owned(),
        requests: vec![request],
    })
}

fn metadata(name: &str, template: &RequestTemplate, prefix: &str) -> String {
    let payload = serde_json::to_vec(&PortableRequest {
        name: name.to_owned(),
        template: portable_template(template),
    })
    .expect("portable request serialization is infallible");
    format!(
        "{prefix} {METADATA_MARKER}{}",
        URL_SAFE_NO_PAD.encode(payload)
    )
}

/// Keep metadata faithful to the generated request without smuggling inactive
/// editor state (disabled secrets, hidden body variants, or scripts) into a
/// seemingly ordinary code sample.
fn portable_template(template: &RequestTemplate) -> RequestTemplate {
    let mut request = template.request.clone();
    request
        .headers
        .retain(|header| header.enabled && !header.name.trim().is_empty());
    match request.body_mode {
        BodyMode::None => {
            request.body.clear();
            request.body_fields.clear();
        }
        BodyMode::Raw => request.body_fields.clear(),
        BodyMode::FormUrlEncoded | BodyMode::MultipartFormData => {
            request.body.clear();
            request
                .body_fields
                .retain(|field| field.enabled && !field.name.trim().is_empty());
        }
    }
    RequestTemplate::new(request)
}

fn decode_metadata(source: &str) -> Result<Option<PortableRequest>, InterchangeError> {
    let Some(marker_offset) = source.find(METADATA_MARKER) else {
        return Ok(None);
    };
    let encoded = source[marker_offset + METADATA_MARKER.len()..]
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_end_matches("*/");
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|error| InterchangeError::Parse(format!("invalid Resolved metadata: {error}")))?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| InterchangeError::Parse(format!("invalid Resolved metadata: {error}")))
}

fn normalized_name(name: &str, request: &RequestDraft) -> String {
    let name = name.trim();
    if !name.is_empty() && name != "New Request" {
        return name.to_owned();
    }
    if request.url.trim().is_empty() {
        request.method.trim().to_ascii_uppercase()
    } else {
        format!(
            "{} {}",
            request.method.trim().to_ascii_uppercase(),
            compact_url(&request.url)
        )
    }
}

fn compact_url(value: &str) -> String {
    let trimmed = value.trim();
    Url::parse(trimmed)
        .ok()
        .map(|url| {
            let mut compact = url.path().to_owned();
            if compact.is_empty() || compact == "/" {
                compact = url.host_str().unwrap_or(trimmed).to_owned();
            }
            if let Some(query) = url.query() {
                compact.push('?');
                compact.push_str(query);
            }
            compact
        })
        .unwrap_or_else(|| trimmed.to_owned())
}

fn enabled_headers(request: &RequestDraft) -> impl Iterator<Item = &HeaderEntry> {
    request
        .headers
        .iter()
        .filter(|header| header.enabled && !header.name.trim().is_empty())
}

fn c_string(value: &str) -> String {
    serde_json::to_string(value).expect("string serialization is infallible")
}

fn shell_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn powershell_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn php_string(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}

fn request_body_bytes(request: &RequestDraft) -> Option<String> {
    match request.body_mode {
        BodyMode::None => None,
        BodyMode::Raw => (!request.body.is_empty()).then(|| request.body.clone()),
        BodyMode::FormUrlEncoded => {
            let mut serializer = url::form_urlencoded::Serializer::new(String::new());
            for field in request
                .body_fields
                .iter()
                .filter(|field| field.enabled && !field.name.trim().is_empty())
            {
                serializer.append_pair(&field.name, &field.value);
            }
            Some(serializer.finish())
        }
        BodyMode::MultipartFormData => Some(render_multipart_body(request)),
    }
}

fn render_multipart_body(request: &RequestDraft) -> String {
    let mut body = String::new();
    for field in request
        .body_fields
        .iter()
        .filter(|field| field.enabled && !field.name.trim().is_empty())
    {
        let _ = write!(
            body,
            "--{MULTIPART_BOUNDARY}\r\nContent-Disposition: form-data; name=\"{}\"",
            field.name.replace('"', "\\\"")
        );
        if field.kind == BodyFieldKind::File {
            let file_name = std::path::Path::new(&field.value)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("upload.bin");
            let _ = write!(
                body,
                "; filename=\"{}\"\r\nContent-Type: application/octet-stream\r\n\r\n{{{{file:{}}}}}\r\n",
                file_name.replace('"', "\\\""),
                field.value
            );
        } else {
            let _ = write!(body, "\r\n\r\n{}\r\n", field.value);
        }
    }
    let _ = write!(body, "--{MULTIPART_BOUNDARY}--\r\n");
    body
}

fn inferred_content_type(request: &RequestDraft) -> Option<&'static str> {
    if enabled_headers(request).any(|header| header.name.eq_ignore_ascii_case("content-type")) {
        return None;
    }
    match request.body_mode {
        BodyMode::None => None,
        BodyMode::Raw if request.body.is_empty() => None,
        BodyMode::Raw => Some(request.raw_body_language.content_type()),
        BodyMode::FormUrlEncoded => Some("application/x-www-form-urlencoded"),
        BodyMode::MultipartFormData => {
            Some("multipart/form-data; boundary=resolved-boundary-7MA4YWxkTrZu0gW")
        }
    }
}

fn all_headers(request: &RequestDraft) -> Vec<(String, String)> {
    let mut headers = enabled_headers(request)
        .map(|header| (header.name.clone(), header.value.clone()))
        .collect::<Vec<_>>();
    if let Some(content_type) = inferred_content_type(request) {
        headers.push(("Content-Type".to_owned(), content_type.to_owned()));
    }
    headers
}

fn method(request: &RequestDraft) -> String {
    let method = request.method.trim().to_ascii_uppercase();
    if method.is_empty() {
        "GET".to_owned()
    } else {
        method
    }
}

fn method_requires_body(method: &str) -> bool {
    matches!(method, "POST" | "PUT" | "PATCH" | "PROPPATCH" | "REPORT")
}

fn detect_source_label(source: &str) -> &'static str {
    let lower = source.to_ascii_lowercase();
    if lower.contains("axios") {
        "JavaScript · Axios"
    } else if lower.contains("$.ajax") || lower.contains("jquery") {
        "JavaScript · jQuery"
    } else if lower.contains("fetch(") {
        "JavaScript · Fetch"
    } else if lower.contains("boost/beast") || lower.contains("boost::beast") {
        "C++ · Boost.Beast"
    } else if lower.contains("curl_easy_") {
        "C++ · libcurl"
    } else if lower.contains("restsharp") {
        "C# · RestSharp"
    } else if lower.contains("httpclient") && lower.contains("system.net.http") {
        "C# · HttpClient"
    } else if lower.contains("io.ktor") || lower.contains("httpclient.request") {
        "Kotlin · Ktor"
    } else if lower.contains("okhttp") && lower.contains("fun main") {
        "Kotlin · OkHttp"
    } else if lower.contains("okhttp") {
        "Java · OkHttp"
    } else if lower.contains("java.net.http") && lower.contains("fun main") {
        "Kotlin · java.net.http"
    } else if lower.contains("java.net.http") {
        "Java · java.net.http"
    } else if lower.contains("resty.new") {
        "Go · Resty"
    } else if lower.contains("http.newrequest") {
        "Go · net/http"
    } else if lower.contains("reqwest") {
        "Rust · reqwest"
    } else if lower.contains("ureq") {
        "Rust · ureq"
    } else if lower.contains("guzzlehttp") {
        "PHP · Guzzle"
    } else if lower.contains("curl_init") {
        "PHP · cURL"
    } else {
        "source code"
    }
}

fn export_curl(name: &str, template: &RequestTemplate) -> String {
    let request = &template.request;
    let mut lines = vec![metadata(name, template, "#")];
    lines.push(format!("curl --request {} \\", method(request)));
    lines.push(format!("  --url {}", shell_string(request.url.trim())));
    for (name, value) in all_headers(request) {
        lines.last_mut().expect("URL line exists").push_str(" \\");
        lines.push(format!(
            "  --header {}",
            shell_string(&format!("{name}: {value}"))
        ));
    }
    match request.body_mode {
        BodyMode::None => {}
        BodyMode::Raw if request.body.is_empty() => {}
        BodyMode::Raw => {
            lines
                .last_mut()
                .expect("request line exists")
                .push_str(" \\");
            lines.push(format!("  --data-raw {}", shell_string(&request.body)));
        }
        BodyMode::FormUrlEncoded => {
            for field in request
                .body_fields
                .iter()
                .filter(|field| field.enabled && !field.name.trim().is_empty())
            {
                lines
                    .last_mut()
                    .expect("request line exists")
                    .push_str(" \\");
                lines.push(format!(
                    "  --data-urlencode {}",
                    shell_string(&format!("{}={}", field.name, field.value))
                ));
            }
        }
        BodyMode::MultipartFormData => {
            for field in request
                .body_fields
                .iter()
                .filter(|field| field.enabled && !field.name.trim().is_empty())
            {
                lines
                    .last_mut()
                    .expect("request line exists")
                    .push_str(" \\");
                let value = if field.kind == BodyFieldKind::File {
                    format!("{}=@{}", field.name, field.value)
                } else {
                    format!("{}={}", field.name, field.value)
                };
                lines.push(format!("  --form {}", shell_string(&value)));
            }
        }
    }
    lines.join("\n") + "\n"
}

fn export_wget(name: &str, template: &RequestTemplate) -> String {
    let request = &template.request;
    let mut lines = vec![metadata(name, template, "#")];
    lines.push(format!("wget --method={} \\", method(request)));
    for (name, value) in all_headers(request) {
        lines.push(format!(
            "  --header={} \\",
            shell_string(&format!("{name}: {value}"))
        ));
    }
    if let Some(body) = request_body_bytes(request) {
        lines.push(format!("  --body-data={} \\", shell_string(&body)));
    }
    lines.push("  --output-document=- \\".to_owned());
    lines.push(format!("  {}", shell_string(request.url.trim())));
    lines.join("\n") + "\n"
}

fn export_powershell(name: &str, template: &RequestTemplate) -> String {
    let request = &template.request;
    let mut output = metadata(name, template, "#");
    output.push('\n');
    let headers = managed_multipart_headers(request);
    if !headers.is_empty() {
        output.push_str("$headers = @{\n");
        for (name, value) in headers {
            let _ = writeln!(
                output,
                "    {} = {}",
                powershell_string(&name),
                powershell_string(&value)
            );
        }
        output.push_str("}\n\n");
    }
    if request.body_mode == BodyMode::MultipartFormData {
        output.push_str("$form = @{\n");
        for field in request
            .body_fields
            .iter()
            .filter(|field| field.enabled && !field.name.trim().is_empty())
        {
            let value = if field.kind == BodyFieldKind::File {
                format!("Get-Item {}", powershell_string(&field.value))
            } else {
                powershell_string(&field.value)
            };
            let _ = writeln!(output, "    {} = {value}", powershell_string(&field.name));
        }
        output.push_str("}\n\n");
    }
    output.push_str("Invoke-WebRequest `\n");
    let _ = writeln!(
        output,
        "    -Uri {} `",
        powershell_string(request.url.trim())
    );
    let _ = write!(
        output,
        "    -Method {}",
        powershell_string(&method(request))
    );
    if !managed_multipart_headers(request).is_empty() {
        output.push_str(" `\n    -Headers $headers");
    }
    if request.body_mode == BodyMode::MultipartFormData {
        output.push_str(" `\n    -Form $form");
    } else if let Some(body) = request_body_bytes(request) {
        let _ = write!(output, " `\n    -Body {}", powershell_string(&body));
    }
    output.push('\n');
    output
}

fn export_intellij_http(name: &str, template: &RequestTemplate) -> String {
    let request = &template.request;
    let mut output = metadata(name, template, "#");
    let _ = write!(
        output,
        "\n### {}\n{} {}\n",
        name,
        method(request),
        request.url.trim()
    );
    for (name, value) in all_headers(request) {
        let _ = writeln!(output, "{name}: {value}");
    }
    if let Some(body) = request_body_bytes(request) {
        output.push('\n');
        output.push_str(&body);
        output.push('\n');
    }
    output
}

fn export_openapi(name: &str, template: &RequestTemplate) -> Result<String, InterchangeError> {
    let request = &template.request;
    let request_method = method(request);
    if !matches!(
        request_method.as_str(),
        "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS" | "TRACE"
    ) {
        return Err(InterchangeError::Export(format!(
            "OpenAPI Path Items cannot represent the custom HTTP method {request_method}"
        )));
    }
    let (server, raw_path) = spec_url_parts(&request.url);
    let (raw_path, raw_query) = raw_path
        .split_once('?')
        .map_or((raw_path.as_str(), None), |(path, query)| {
            (path, Some(query))
        });
    let path = openapi_export_path(raw_path);
    let operation_key = request_method.to_ascii_lowercase();
    let mut operation = Map::new();
    operation.insert("summary".to_owned(), Value::String(name.to_owned()));
    operation.insert(
        "operationId".to_owned(),
        Value::String(identifier(name, "request")),
    );
    operation.insert(
        "x-resolved-request".to_owned(),
        serde_json::to_value(portable_template(template))
            .map_err(|error| InterchangeError::Export(error.to_string()))?,
    );

    let mut parameters = enabled_headers(request)
        .filter(|header| !header.name.eq_ignore_ascii_case("content-type"))
        .map(|header| {
            json!({
                "name": header.name,
                "in": "header",
                "required": true,
                "schema": { "type": "string" },
                "example": header.value,
            })
        })
        .collect::<Vec<_>>();
    if let Some(query) = raw_query {
        parameters.extend(
            url::form_urlencoded::parse(query.as_bytes())
                .filter(|(name, _)| !name.trim().is_empty())
                .map(|(name, value)| {
                    json!({
                        "name": name,
                        "in": "query",
                        "required": false,
                        "schema": { "type": "string" },
                        "example": value,
                    })
                }),
        );
    }
    parameters.extend(openapi_path_parameters(&path).into_iter().map(|name| {
        json!({
            "name": name,
            "in": "path",
            "required": true,
            "schema": { "type": "string" },
        })
    }));
    if !parameters.is_empty() {
        operation.insert("parameters".to_owned(), Value::Array(parameters));
    }
    if let Some((content_type, schema, example)) = spec_request_body(request) {
        operation.insert(
            "requestBody".to_owned(),
            json!({
                "required": true,
                "content": {
                    content_type: {
                        "schema": schema,
                        "example": example,
                    }
                }
            }),
        );
    }
    operation.insert(
        "responses".to_owned(),
        json!({
            "default": { "description": "Response" }
        }),
    );

    let document = json!({
        "openapi": "3.1.0",
        "info": {
            "title": format!("{name} API"),
            "version": "1.0.0",
        },
        "servers": [{ "url": server }],
        "paths": {
            path: {
                operation_key: Value::Object(operation),
            }
        }
    });
    serde_yaml::to_string(&document).map_err(|error| InterchangeError::Export(error.to_string()))
}

fn export_asyncapi(name: &str, template: &RequestTemplate) -> Result<String, InterchangeError> {
    let request = &template.request;
    let request_method = method(request);
    if !matches!(
        request_method.as_str(),
        "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS" | "CONNECT" | "TRACE"
    ) {
        return Err(InterchangeError::Export(format!(
            "the AsyncAPI HTTP binding cannot represent the custom method {request_method}"
        )));
    }
    let (server, raw_path) = spec_url_parts(&request.url);
    let (path, raw_query) = raw_path
        .split_once('?')
        .map_or((raw_path.as_str(), None), |(path, query)| {
            (path, Some(query))
        });
    let (server_host, server_protocol) = asyncapi_export_server(&server);
    let request_value = serde_json::to_value(portable_template(template))
        .map_err(|error| InterchangeError::Export(error.to_string()))?;
    let payload = spec_request_body(request)
        .map(|(_, schema, example)| json!({ "payload": schema, "examples": [{ "payload": example }] }))
        .unwrap_or_else(|| json!({ "payload": { "type": "null" } }));
    let channel_id = identifier(name, "request");
    let operation_id = format!("send_{channel_id}");
    let header_properties = managed_multipart_headers(request)
        .into_iter()
        .map(|(name, value)| {
            (
                name,
                json!({
                    "type": "string",
                    "examples": [value],
                }),
            )
        })
        .collect::<Map<_, _>>();
    let message_binding = (!header_properties.is_empty()).then(|| {
        json!({
            "http": {
                "headers": {
                    "type": "object",
                    "properties": header_properties,
                },
                "bindingVersion": "0.3.0",
            }
        })
    });
    let query_properties = raw_query
        .map(|query| {
            url::form_urlencoded::parse(query.as_bytes())
                .filter(|(name, _)| !name.trim().is_empty())
                .map(|(name, value)| {
                    (
                        name.into_owned(),
                        json!({ "type": "string", "examples": [value] }),
                    )
                })
                .collect::<Map<_, _>>()
        })
        .unwrap_or_default();
    let query_binding = (!query_properties.is_empty()).then(|| {
        json!({
            "type": "object",
            "properties": query_properties,
        })
    });
    let mut message = payload;
    if let Some(bindings) = message_binding {
        message
            .as_object_mut()
            .expect("message payload is an object")
            .insert("bindings".to_owned(), bindings);
    }
    let mut http_operation_binding = json!({
        "method": request_method,
        "bindingVersion": "0.3.0",
    });
    if let Some(query) = query_binding {
        http_operation_binding
            .as_object_mut()
            .expect("HTTP binding is an object")
            .insert("query".to_owned(), query);
    }
    let document = json!({
        "asyncapi": "3.0.0",
        "info": {
            "title": format!("{name} API"),
            "version": "1.0.0",
        },
        "servers": {
            "http": {
                "host": server_host,
                "protocol": server_protocol,
            }
        },
        "channels": {
            channel_id.clone(): {
                "address": path,
                "messages": {
                    "request": message,
                }
            }
        },
        "operations": {
            operation_id: {
                "action": "send",
                "channel": { "$ref": format!("#/channels/{channel_id}") },
                "messages": [{ "$ref": format!("#/channels/{channel_id}/messages/request") }],
                "bindings": {
                    "http": http_operation_binding,
                },
            }
        },
        "x-resolved-requests": [{
            "name": name,
            "template": request_value,
        }],
    });
    serde_yaml::to_string(&document).map_err(|error| InterchangeError::Export(error.to_string()))
}

fn spec_url_parts(value: &str) -> (String, String) {
    if let Ok(url) = Url::parse(value.trim()) {
        let mut server = format!(
            "{}://{}",
            url.scheme(),
            url.host_str().unwrap_or("localhost")
        );
        if let Some(port) = url.port() {
            let _ = write!(server, ":{port}");
        }
        let mut path = url.path().to_owned();
        if path.is_empty() {
            path.push('/');
        }
        if let Some(query) = url.query() {
            path.push('?');
            path.push_str(query);
        }
        (server, path)
    } else {
        ("https://example.com".to_owned(), value.trim().to_owned())
    }
}

fn openapi_export_path(path: &str) -> String {
    path.replace("{{", "{").replace("}}", "}")
}

fn openapi_path_parameters(path: &str) -> Vec<String> {
    let mut parameters = Vec::new();
    let mut remaining = path;
    while let Some(open) = remaining.find('{') {
        let after_open = &remaining[open + 1..];
        let Some(close) = after_open.find('}') else {
            break;
        };
        let name = &after_open[..close];
        if !name.trim().is_empty() && !parameters.iter().any(|current| current == name) {
            parameters.push(name.to_owned());
        }
        remaining = &after_open[close + 1..];
    }
    parameters
}

fn openapi_import_template(value: &str) -> String {
    let mut output = String::new();
    let mut remaining = value;
    while let Some(open) = remaining.find('{') {
        output.push_str(&remaining[..open]);
        let after_open = &remaining[open + 1..];
        if after_open.starts_with('{') {
            let Some(close) = after_open.find("}}") else {
                output.push_str(&remaining[open..]);
                return output;
            };
            output.push_str("{{");
            output.push_str(&after_open[1..close]);
            output.push_str("}}");
            remaining = &after_open[close + 2..];
            continue;
        }
        let Some(close) = after_open.find('}') else {
            output.push_str(&remaining[open..]);
            return output;
        };
        output.push_str("{{");
        output.push_str(&after_open[..close]);
        output.push_str("}}");
        remaining = &after_open[close + 1..];
    }
    output.push_str(remaining);
    output
}

fn asyncapi_export_server(server: &str) -> (String, String) {
    if let Ok(url) = Url::parse(server) {
        let mut host = url.host_str().unwrap_or("example.com").to_owned();
        if let Some(port) = url.port() {
            let _ = write!(host, ":{port}");
        }
        (host, url.scheme().to_owned())
    } else {
        (server.to_owned(), "https".to_owned())
    }
}

fn spec_request_body(request: &RequestDraft) -> Option<(String, Value, Value)> {
    match request.body_mode {
        BodyMode::None => None,
        BodyMode::Raw if request.body.is_empty() => None,
        BodyMode::Raw => {
            let content_type = enabled_headers(request)
                .find(|header| header.name.eq_ignore_ascii_case("content-type"))
                .map(|header| header.value.clone())
                .unwrap_or_else(|| request.raw_body_language.content_type().to_owned());
            let example = if request.raw_body_language == RawBodyLanguage::Json {
                serde_json::from_str(&request.body)
                    .unwrap_or_else(|_| Value::String(request.body.clone()))
            } else {
                Value::String(request.body.clone())
            };
            let schema = schema_for_example(&example);
            Some((content_type, schema, example))
        }
        BodyMode::FormUrlEncoded | BodyMode::MultipartFormData => {
            let fields = request
                .body_fields
                .iter()
                .filter(|field| field.enabled && !field.name.trim().is_empty())
                .collect::<Vec<_>>();
            let mut properties = Map::new();
            let mut example = Map::new();
            for field in fields {
                let schema = if field.kind == BodyFieldKind::File
                    && request.body_mode == BodyMode::MultipartFormData
                {
                    json!({ "type": "string", "format": "binary" })
                } else {
                    json!({ "type": "string" })
                };
                properties.insert(field.name.clone(), schema);
                example.insert(field.name.clone(), Value::String(field.value.clone()));
            }
            Some((
                if request.body_mode == BodyMode::FormUrlEncoded {
                    "application/x-www-form-urlencoded".to_owned()
                } else {
                    "multipart/form-data".to_owned()
                },
                json!({ "type": "object", "properties": properties }),
                Value::Object(example),
            ))
        }
    }
}

fn schema_for_example(example: &Value) -> Value {
    match example {
        Value::Null => json!({ "type": "null" }),
        Value::Bool(_) => json!({ "type": "boolean" }),
        Value::Number(number) if number.is_i64() || number.is_u64() => {
            json!({ "type": "integer" })
        }
        Value::Number(_) => json!({ "type": "number" }),
        Value::String(_) => json!({ "type": "string" }),
        Value::Array(values) => json!({
            "type": "array",
            "items": values.first().map(schema_for_example).unwrap_or_else(|| json!({}))
        }),
        Value::Object(values) => {
            let properties = values
                .iter()
                .map(|(name, value)| (name.clone(), schema_for_example(value)))
                .collect::<Map<_, _>>();
            json!({ "type": "object", "properties": properties })
        }
    }
}

fn identifier(value: &str, fallback: &str) -> String {
    let mut result = String::new();
    let mut previous_was_separator = false;
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            result.push(character.to_ascii_lowercase());
            previous_was_separator = false;
        } else if !previous_was_separator && !result.is_empty() {
            result.push('_');
            previous_was_separator = true;
        }
    }
    while result.ends_with('_') {
        result.pop();
    }
    if result.is_empty() {
        fallback.to_owned()
    } else if result.as_bytes()[0].is_ascii_digit() {
        format!("{fallback}_{result}")
    } else {
        result
    }
}

fn managed_multipart_headers(request: &RequestDraft) -> Vec<(String, String)> {
    if request.body_mode != BodyMode::MultipartFormData {
        return all_headers(request);
    }
    enabled_headers(request)
        .filter(|header| {
            !header.name.eq_ignore_ascii_case("content-type")
                && !header.name.eq_ignore_ascii_case("content-length")
        })
        .map(|header| (header.name.clone(), header.value.clone()))
        .collect()
}

fn media_type(request: &RequestDraft) -> String {
    enabled_headers(request)
        .find(|header| header.name.eq_ignore_ascii_case("content-type"))
        .map(|header| header.value.clone())
        .or_else(|| inferred_content_type(request).map(str::to_owned))
        .unwrap_or_else(|| "application/octet-stream".to_owned())
}

fn export_javascript_fetch(name: &str, template: &RequestTemplate) -> String {
    let request = &template.request;
    let mut output = metadata(name, template, "//");
    output.push('\n');
    if request.body_mode == BodyMode::MultipartFormData {
        output.push_str("const body = new FormData();\n");
        for field in request
            .body_fields
            .iter()
            .filter(|field| field.enabled && !field.name.trim().is_empty())
        {
            if field.kind == BodyFieldKind::File {
                let _ = writeln!(
                    output,
                    "body.append({}, file); // File for {}",
                    c_string(&field.name),
                    c_string(&field.value)
                );
            } else {
                let _ = writeln!(
                    output,
                    "body.append({}, {});",
                    c_string(&field.name),
                    c_string(&field.value)
                );
            }
        }
        output.push('\n');
    }
    let _ = writeln!(
        output,
        "const response = await fetch({}, {{",
        c_string(request.url.trim())
    );
    let _ = writeln!(output, "  method: {},", c_string(&method(request)));
    let headers = managed_multipart_headers(request);
    if !headers.is_empty() {
        output.push_str("  headers: {\n");
        for (name, value) in headers {
            let _ = writeln!(output, "    {}: {},", c_string(&name), c_string(&value));
        }
        output.push_str("  },\n");
    }
    if request.body_mode == BodyMode::MultipartFormData {
        output.push_str("  body,\n");
    } else if let Some(body) = request_body_bytes(request) {
        let _ = writeln!(output, "  body: {},", c_string(&body));
    }
    output.push_str("});\n\nconst data = await response.text();\n");
    output
}

fn export_javascript_axios(name: &str, template: &RequestTemplate) -> String {
    let request = &template.request;
    let mut output = metadata(name, template, "//");
    output.push_str("\nimport axios from \"axios\";\n\n");
    if request.body_mode == BodyMode::MultipartFormData {
        output.push_str("const data = new FormData();\n");
        for field in request
            .body_fields
            .iter()
            .filter(|field| field.enabled && !field.name.trim().is_empty())
        {
            let value = if field.kind == BodyFieldKind::File {
                "file".to_owned()
            } else {
                c_string(&field.value)
            };
            let _ = writeln!(output, "data.append({}, {value});", c_string(&field.name));
        }
        output.push('\n');
    }
    output.push_str("const response = await axios({\n");
    let _ = writeln!(
        output,
        "  method: {},",
        c_string(&method(request).to_ascii_lowercase())
    );
    let _ = writeln!(output, "  url: {},", c_string(request.url.trim()));
    let headers = managed_multipart_headers(request);
    if !headers.is_empty() {
        output.push_str("  headers: {\n");
        for (name, value) in headers {
            let _ = writeln!(output, "    {}: {},", c_string(&name), c_string(&value));
        }
        output.push_str("  },\n");
    }
    if request.body_mode == BodyMode::MultipartFormData {
        output.push_str("  data,\n");
    } else if let Some(body) = request_body_bytes(request) {
        let _ = writeln!(output, "  data: {},", c_string(&body));
    }
    output.push_str("});\n\nconsole.log(response.data);\n");
    output
}

fn export_javascript_jquery(name: &str, template: &RequestTemplate) -> String {
    let request = &template.request;
    let mut output = metadata(name, template, "//");
    output.push('\n');
    if request.body_mode == BodyMode::MultipartFormData {
        output.push_str("const data = new FormData();\n");
        for field in request
            .body_fields
            .iter()
            .filter(|field| field.enabled && !field.name.trim().is_empty())
        {
            let value = if field.kind == BodyFieldKind::File {
                "file".to_owned()
            } else {
                c_string(&field.value)
            };
            let _ = writeln!(output, "data.append({}, {value});", c_string(&field.name));
        }
        output.push('\n');
    }
    output.push_str("const response = await $.ajax({\n");
    let _ = writeln!(output, "  url: {},", c_string(request.url.trim()));
    let _ = writeln!(output, "  method: {},", c_string(&method(request)));
    let headers = managed_multipart_headers(request);
    if !headers.is_empty() {
        output.push_str("  headers: {\n");
        for (name, value) in headers {
            let _ = writeln!(output, "    {}: {},", c_string(&name), c_string(&value));
        }
        output.push_str("  },\n");
    }
    if request.body_mode == BodyMode::MultipartFormData {
        output.push_str("  data,\n  processData: false,\n  contentType: false,\n");
    } else if let Some(body) = request_body_bytes(request) {
        let _ = writeln!(output, "  data: {},", c_string(&body));
    }
    output.push_str("});\n\nconsole.log(response);\n");
    output
}

fn export_java_http_client(name: &str, template: &RequestTemplate) -> String {
    let request = &template.request;
    let body = request_body_bytes(request);
    let mut output = metadata(name, template, "//");
    output.push_str(
        "\nimport java.net.URI;\nimport java.net.http.HttpClient;\nimport java.net.http.HttpRequest;\nimport java.net.http.HttpResponse;\n\nclass Main {\n    public static void main(String[] args) throws Exception {\n",
    );
    output.push_str(
        "        var client = HttpClient.newHttpClient();\n        var request = HttpRequest.newBuilder()\n",
    );
    let _ = writeln!(
        output,
        "            .uri(URI.create({}))",
        c_string(request.url.trim())
    );
    for (name, value) in all_headers(request) {
        let _ = writeln!(
            output,
            "            .header({}, {})",
            c_string(&name),
            c_string(&value)
        );
    }
    match body {
        Some(body) => {
            let _ = writeln!(
                output,
                "            .method({}, HttpRequest.BodyPublishers.ofString({}))",
                c_string(&method(request)),
                c_string(&body)
            );
        }
        None => {
            let _ = writeln!(
                output,
                "            .method({}, HttpRequest.BodyPublishers.noBody())",
                c_string(&method(request))
            );
        }
    }
    output.push_str("            .build();\n\n        var response = client.send(request, HttpResponse.BodyHandlers.ofString());\n        System.out.println(response.body());\n    }\n}\n");
    output
}

fn export_java_okhttp(name: &str, template: &RequestTemplate) -> String {
    let request = &template.request;
    let mut output = metadata(name, template, "//");
    output.push_str(
        "\nimport java.io.File;\nimport okhttp3.*;\n\nclass Main {\n    public static void main(String[] args) throws Exception {\n        var client = new OkHttpClient();\n",
    );
    if request.body_mode == BodyMode::MultipartFormData {
        output.push_str(
            "        RequestBody body = new MultipartBody.Builder().setType(MultipartBody.FORM)\n",
        );
        for field in request
            .body_fields
            .iter()
            .filter(|field| field.enabled && !field.name.trim().is_empty())
        {
            if field.kind == BodyFieldKind::File {
                let _ = writeln!(
                    output,
                    "            .addFormDataPart({}, new File({}).getName(), RequestBody.create(new File({}), MediaType.parse(\"application/octet-stream\")))",
                    c_string(&field.name),
                    c_string(&field.value),
                    c_string(&field.value)
                );
            } else {
                let _ = writeln!(
                    output,
                    "            .addFormDataPart({}, {})",
                    c_string(&field.name),
                    c_string(&field.value)
                );
            }
        }
        output.push_str("            .build();\n");
    } else if let Some(body) = request_body_bytes(request) {
        let _ = writeln!(
            output,
            "        RequestBody body = RequestBody.create({}, MediaType.parse({}));",
            c_string(&body),
            c_string(&media_type(request))
        );
    } else if method_requires_body(&method(request)) {
        output.push_str("        RequestBody body = RequestBody.create(new byte[0], null);\n");
    } else {
        output.push_str("        RequestBody body = null;\n");
    }
    output.push_str("        var request = new Request.Builder()\n");
    let _ = writeln!(output, "            .url({})", c_string(request.url.trim()));
    for (name, value) in managed_multipart_headers(request) {
        let _ = writeln!(
            output,
            "            .addHeader({}, {})",
            c_string(&name),
            c_string(&value)
        );
    }
    let _ = writeln!(
        output,
        "            .method({}, body)",
        c_string(&method(request))
    );
    output.push_str("            .build();\n\n        try (var response = client.newCall(request).execute()) {\n            System.out.println(response.body().string());\n        }\n    }\n}\n");
    output
}

fn export_go_net_http(name: &str, template: &RequestTemplate) -> String {
    let request = &template.request;
    let body = request_body_bytes(request).unwrap_or_default();
    let mut output = metadata(name, template, "//");
    output.push_str("\npackage main\n\nimport (\n    \"fmt\"\n    \"io\"\n    \"net/http\"\n    \"strings\"\n)\n\nfunc main() {\n");
    let _ = writeln!(output, "    body := strings.NewReader({})", c_string(&body));
    let _ = writeln!(
        output,
        "    req, err := http.NewRequest({}, {}, body)",
        c_string(&method(request)),
        c_string(request.url.trim())
    );
    output.push_str("    if err != nil { panic(err) }\n");
    for (name, value) in all_headers(request) {
        let _ = writeln!(
            output,
            "    req.Header.Add({}, {})",
            c_string(&name),
            c_string(&value)
        );
    }
    output.push_str("    resp, err := http.DefaultClient.Do(req)\n    if err != nil { panic(err) }\n    defer resp.Body.Close()\n    data, err := io.ReadAll(resp.Body)\n    if err != nil { panic(err) }\n    fmt.Println(string(data))\n}\n");
    output
}

fn export_go_resty(name: &str, template: &RequestTemplate) -> String {
    let request = &template.request;
    let mut output = metadata(name, template, "//");
    output.push_str("\npackage main\n\nimport (\n    \"fmt\"\n    \"github.com/go-resty/resty/v2\"\n)\n\nfunc main() {\n    client := resty.New()\n    request := client.R()\n");
    for (name, value) in managed_multipart_headers(request) {
        let _ = writeln!(
            output,
            "    request.SetHeader({}, {})",
            c_string(&name),
            c_string(&value)
        );
    }
    if request.body_mode == BodyMode::MultipartFormData {
        for field in request
            .body_fields
            .iter()
            .filter(|field| field.enabled && !field.name.trim().is_empty())
        {
            if field.kind == BodyFieldKind::File {
                let _ = writeln!(
                    output,
                    "    request.SetFile({}, {})",
                    c_string(&field.name),
                    c_string(&field.value)
                );
            } else {
                let _ = writeln!(
                    output,
                    "    request.SetFormData(map[string]string{{{}: {}}})",
                    c_string(&field.name),
                    c_string(&field.value)
                );
            }
        }
    } else if let Some(body) = request_body_bytes(request) {
        let _ = writeln!(output, "    request.SetBody({})", c_string(&body));
    }
    let _ = writeln!(
        output,
        "    response, err := request.Execute({}, {})",
        c_string(&method(request)),
        c_string(request.url.trim())
    );
    output.push_str("    if err != nil { panic(err) }\n    fmt.Println(response.String())\n}\n");
    output
}

fn export_csharp_http_client(name: &str, template: &RequestTemplate) -> String {
    let request = &template.request;
    let mut output = metadata(name, template, "//");
    output.push_str(
        "\nusing System;\nusing System.IO;\nusing System.Net.Http;\nusing System.Text;\n\nusing var client = new HttpClient();\n",
    );
    let _ = writeln!(
        output,
        "using var request = new HttpRequestMessage(new HttpMethod({}), {});",
        c_string(&method(request)),
        c_string(request.url.trim())
    );
    if request.body_mode == BodyMode::MultipartFormData {
        output.push_str("var content = new MultipartFormDataContent();\n");
        for field in request
            .body_fields
            .iter()
            .filter(|field| field.enabled && !field.name.trim().is_empty())
        {
            if field.kind == BodyFieldKind::File {
                let _ = writeln!(
                    output,
                    "content.Add(new StreamContent(File.OpenRead({})), {}, Path.GetFileName({}));",
                    c_string(&field.value),
                    c_string(&field.name),
                    c_string(&field.value)
                );
            } else {
                let _ = writeln!(
                    output,
                    "content.Add(new StringContent({}), {});",
                    c_string(&field.value),
                    c_string(&field.name)
                );
            }
        }
        output.push_str("request.Content = content;\n");
    } else if let Some(body) = request_body_bytes(request) {
        let _ = writeln!(
            output,
            "request.Content = new StringContent({}, Encoding.UTF8);",
            c_string(&body)
        );
        let _ = writeln!(
            output,
            "request.Content.Headers.ContentType = System.Net.Http.Headers.MediaTypeHeaderValue.Parse({});",
            c_string(&media_type(request))
        );
    }
    for (header_name, value) in managed_multipart_headers(request) {
        if header_name.eq_ignore_ascii_case("content-type") {
            continue;
        }
        let _ = writeln!(
            output,
            "request.Headers.TryAddWithoutValidation({}, {});",
            c_string(&header_name),
            c_string(&value)
        );
    }
    output.push_str("\nusing var response = await client.SendAsync(request);\nConsole.WriteLine(await response.Content.ReadAsStringAsync());\n");
    output
}

fn export_csharp_restsharp(
    name: &str,
    template: &RequestTemplate,
) -> Result<String, InterchangeError> {
    let request = &template.request;
    let request_method = method(request);
    let restsharp_method = restsharp_method(&request_method).ok_or_else(|| {
        InterchangeError::Export(format!(
            "RestSharp cannot represent the custom HTTP method {request_method}"
        ))
    })?;
    let mut output = metadata(name, template, "//");
    output.push_str("\nusing System;\nusing RestSharp;\n\n");
    let _ = writeln!(
        output,
        "var client = new RestClient({});",
        c_string(request.url.trim())
    );
    let _ = writeln!(
        output,
        "var request = new RestRequest(\"\", Method.{});",
        restsharp_method
    );
    for (header_name, value) in managed_multipart_headers(request) {
        let _ = writeln!(
            output,
            "request.AddHeader({}, {});",
            c_string(&header_name),
            c_string(&value)
        );
    }
    if request.body_mode == BodyMode::MultipartFormData {
        output.push_str("request.AlwaysMultipartFormData = true;\n");
        for field in request
            .body_fields
            .iter()
            .filter(|field| field.enabled && !field.name.trim().is_empty())
        {
            if field.kind == BodyFieldKind::File {
                let _ = writeln!(
                    output,
                    "request.AddFile({}, {}, \"application/octet-stream\");",
                    c_string(&field.name),
                    c_string(&field.value)
                );
            } else {
                let _ = writeln!(
                    output,
                    "request.AddParameter({}, {});",
                    c_string(&field.name),
                    c_string(&field.value)
                );
            }
        }
    } else if let Some(body) = request_body_bytes(request) {
        let _ = writeln!(
            output,
            "request.AddStringBody({}, {});",
            c_string(&body),
            c_string(&media_type(request))
        );
    }
    output.push_str("\nvar response = await client.ExecuteAsync(request);\nConsole.WriteLine(response.Content);\n");
    Ok(output)
}

fn restsharp_method(method: &str) -> Option<&'static str> {
    match method {
        "GET" => Some("Get"),
        "POST" => Some("Post"),
        "PUT" => Some("Put"),
        "PATCH" => Some("Patch"),
        "DELETE" => Some("Delete"),
        "HEAD" => Some("Head"),
        "OPTIONS" => Some("Options"),
        "MERGE" => Some("Merge"),
        "COPY" => Some("Copy"),
        "SEARCH" => Some("Search"),
        _ => None,
    }
}

fn export_rust_reqwest(name: &str, template: &RequestTemplate) -> String {
    let request = &template.request;
    let mut output = metadata(name, template, "//");
    output.push_str("\nuse reqwest::{Client, Method};\nuse std::str::FromStr;\n\n#[tokio::main]\nasync fn main() -> Result<(), Box<dyn std::error::Error>> {\n    let client = Client::new();\n");
    let _ = writeln!(
        output,
        "    let mut request = client.request(Method::from_str({})?, {});",
        c_string(&method(request)),
        c_string(request.url.trim())
    );
    for (header_name, value) in managed_multipart_headers(request) {
        let _ = writeln!(
            output,
            "    request = request.header({}, {});",
            c_string(&header_name),
            c_string(&value)
        );
    }
    if request.body_mode == BodyMode::MultipartFormData {
        output.push_str("    let mut form = reqwest::multipart::Form::new();\n");
        for field in request
            .body_fields
            .iter()
            .filter(|field| field.enabled && !field.name.trim().is_empty())
        {
            if field.kind == BodyFieldKind::File {
                let _ = writeln!(
                    output,
                    "    form = form.file({}, {}).await?;",
                    c_string(&field.name),
                    c_string(&field.value)
                );
            } else {
                let _ = writeln!(
                    output,
                    "    form = form.text({}, {});",
                    c_string(&field.name),
                    c_string(&field.value)
                );
            }
        }
        output.push_str("    request = request.multipart(form);\n");
    } else if request.body_mode == BodyMode::FormUrlEncoded {
        output.push_str("    request = request.form(&[\n");
        for field in request
            .body_fields
            .iter()
            .filter(|field| field.enabled && !field.name.trim().is_empty())
        {
            let _ = writeln!(
                output,
                "        ({}, {}),",
                c_string(&field.name),
                c_string(&field.value)
            );
        }
        output.push_str("    ]);\n");
    } else if let Some(body) = request_body_bytes(request) {
        let _ = writeln!(output, "    request = request.body({});", c_string(&body));
    }
    output.push_str("    let response = request.send().await?;\n    println!(\"{}\", response.text().await?);\n    Ok(())\n}\n");
    output
}

fn export_rust_ureq(name: &str, template: &RequestTemplate) -> String {
    let request = &template.request;
    let mut output = metadata(name, template, "//");
    output.push_str("\nuse ureq::http::{Method, Request};\n\nfn main() -> Result<(), Box<dyn std::error::Error>> {\n    let request = Request::builder()\n");
    let _ = writeln!(
        output,
        "        .method(Method::from_bytes({}.as_bytes())?)\n        .uri({})",
        c_string(&method(request)),
        c_string(request.url.trim())
    );
    for (header_name, value) in all_headers(request) {
        let _ = writeln!(
            output,
            "        .header({}, {})",
            c_string(&header_name),
            c_string(&value)
        );
    }
    if let Some(body) = request_body_bytes(request) {
        let _ = writeln!(output, "        .body({})?;", c_string(&body));
    } else {
        output.push_str("        .body(())?;\n");
    }
    output.push_str("    let mut response = ureq::run(request)?;\n    println!(\"{}\", response.body_mut().read_to_string()?);\n    Ok(())\n}\n");
    output
}

fn export_cpp_boost_beast(name: &str, template: &RequestTemplate) -> String {
    let request = &template.request;
    let (server, target) = spec_url_parts(&request.url);
    let parsed_server = Url::parse(&server).ok();
    let https = parsed_server
        .as_ref()
        .is_some_and(|url| url.scheme() == "https");
    let host = parsed_server
        .as_ref()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_else(|| "localhost".to_owned());
    let port = parsed_server
        .as_ref()
        .and_then(|url| url.port_or_known_default())
        .unwrap_or(80);
    let mut output = metadata(name, template, "//");
    output.push_str("\n#include <boost/asio/connect.hpp>\n#include <boost/asio/ip/tcp.hpp>\n#include <boost/beast/core.hpp>\n#include <boost/beast/http.hpp>\n");
    if https {
        output.push_str("#include <boost/asio/ssl.hpp>\n#include <boost/beast/ssl.hpp>\n#include <openssl/ssl.h>\n");
    }
    output.push_str("#include <iostream>\n#include <string>\n\nint main() {\n    namespace asio = boost::asio;\n    namespace beast = boost::beast;\n    namespace http = beast::http;\n    asio::io_context context;\n    asio::ip::tcp::resolver resolver(context);\n");
    let _ = writeln!(
        output,
        "    std::string const host = {};\n    auto const endpoints = resolver.resolve(host, {});",
        c_string(&host),
        c_string(&port.to_string())
    );
    if https {
        output.push_str("    asio::ssl::context tls_context(asio::ssl::context::tls_client);\n    tls_context.set_default_verify_paths();\n    beast::ssl_stream<beast::tcp_stream> stream(context, tls_context);\n    if (!SSL_set_tlsext_host_name(stream.native_handle(), host.c_str())) return 1;\n    stream.set_verify_mode(asio::ssl::verify_peer);\n    stream.set_verify_callback(asio::ssl::host_name_verification(host));\n    beast::get_lowest_layer(stream).connect(endpoints);\n    stream.handshake(asio::ssl::stream_base::client);\n");
    } else {
        output.push_str("    beast::tcp_stream stream(context);\n    stream.connect(endpoints);\n");
    }
    let _ = writeln!(
        output,
        "    http::request<http::string_body> request{{http::string_to_verb({}), {}, 11}};",
        c_string(&method(request)),
        c_string(&target)
    );
    let _ = writeln!(output, "    request.set(http::field::host, host);");
    for (header_name, value) in all_headers(request) {
        let _ = writeln!(
            output,
            "    request.set({}, {});",
            c_string(&header_name),
            c_string(&value)
        );
    }
    if let Some(body) = request_body_bytes(request) {
        let _ = writeln!(output, "    request.body() = {};", c_string(&body));
        output.push_str("    request.prepare_payload();\n");
    }
    output.push_str("    http::write(stream, request);\n    beast::flat_buffer buffer;\n    http::response<http::dynamic_body> response;\n    http::read(stream, buffer, response);\n    std::cout << response << std::endl;\n");
    if https {
        output.push_str("    beast::error_code error;\n    stream.shutdown(error);\n");
    }
    output.push_str("}\n");
    output
}

fn export_cpp_libcurl(name: &str, template: &RequestTemplate) -> String {
    let request = &template.request;
    let mut output = metadata(name, template, "//");
    output.push_str("\n#include <curl/curl.h>\n\nint main() {\n    CURL* curl = curl_easy_init();\n    if (!curl) return 1;\n    struct curl_slist* headers = nullptr;\n");
    for (header_name, value) in all_headers(request) {
        let _ = writeln!(
            output,
            "    headers = curl_slist_append(headers, {});",
            c_string(&format!("{header_name}: {value}"))
        );
    }
    let _ = writeln!(
        output,
        "    curl_easy_setopt(curl, CURLOPT_URL, {});",
        c_string(request.url.trim())
    );
    let _ = writeln!(
        output,
        "    curl_easy_setopt(curl, CURLOPT_CUSTOMREQUEST, {});",
        c_string(&method(request))
    );
    output.push_str("    curl_easy_setopt(curl, CURLOPT_HTTPHEADER, headers);\n");
    if let Some(body) = request_body_bytes(request) {
        let _ = writeln!(output, "    const char* body = {};", c_string(&body));
        output.push_str("    curl_easy_setopt(curl, CURLOPT_POSTFIELDS, body);\n");
    }
    output.push_str("    CURLcode result = curl_easy_perform(curl);\n    curl_slist_free_all(headers);\n    curl_easy_cleanup(curl);\n    return result == CURLE_OK ? 0 : 1;\n}\n");
    output
}

fn export_php_curl(name: &str, template: &RequestTemplate) -> String {
    let request = &template.request;
    let mut output = String::from("<?php\n");
    output.push_str(&metadata(name, template, "//"));
    output.push_str("\n$curl = curl_init();\n$headers = [\n");
    for (header_name, value) in managed_multipart_headers(request) {
        let _ = writeln!(
            output,
            "    {},",
            php_string(&format!("{header_name}: {value}"))
        );
    }
    output.push_str("];\n");
    if request.body_mode == BodyMode::MultipartFormData {
        output.push_str("$body = [\n");
        for field in request
            .body_fields
            .iter()
            .filter(|field| field.enabled && !field.name.trim().is_empty())
        {
            let value = if field.kind == BodyFieldKind::File {
                format!("new CURLFile({})", php_string(&field.value))
            } else {
                php_string(&field.value)
            };
            let _ = writeln!(output, "    {} => {value},", php_string(&field.name));
        }
        output.push_str("];\n");
    } else if let Some(body) = request_body_bytes(request) {
        let _ = writeln!(output, "$body = {};", php_string(&body));
    }
    output.push_str("curl_setopt_array($curl, [\n");
    let _ = writeln!(
        output,
        "    CURLOPT_URL => {},",
        php_string(request.url.trim())
    );
    let _ = writeln!(
        output,
        "    CURLOPT_CUSTOMREQUEST => {},",
        php_string(&method(request))
    );
    output.push_str("    CURLOPT_RETURNTRANSFER => true,\n    CURLOPT_HTTPHEADER => $headers,\n");
    if request_body_bytes(request).is_some() {
        output.push_str("    CURLOPT_POSTFIELDS => $body,\n");
    }
    output.push_str("]);\n$response = curl_exec($curl);\nif ($response === false) { throw new RuntimeException(curl_error($curl)); }\ncurl_close($curl);\necho $response;\n");
    output
}

fn export_php_guzzle(name: &str, template: &RequestTemplate) -> String {
    let request = &template.request;
    let mut output = String::from("<?php\n");
    output.push_str(&metadata(name, template, "//"));
    output.push_str("\nrequire 'vendor/autoload.php';\n\n$client = new GuzzleHttp\\Client();\n$options = [\n    'headers' => [\n");
    for (header_name, value) in managed_multipart_headers(request) {
        let _ = writeln!(
            output,
            "        {} => {},",
            php_string(&header_name),
            php_string(&value)
        );
    }
    output.push_str("    ],\n");
    match request.body_mode {
        BodyMode::MultipartFormData => {
            output.push_str("    'multipart' => [\n");
            for field in request
                .body_fields
                .iter()
                .filter(|field| field.enabled && !field.name.trim().is_empty())
            {
                let contents = if field.kind == BodyFieldKind::File {
                    format!("fopen({}, 'r')", php_string(&field.value))
                } else {
                    php_string(&field.value)
                };
                let _ = writeln!(
                    output,
                    "        ['name' => {}, 'contents' => {contents}],",
                    php_string(&field.name)
                );
            }
            output.push_str("    ],\n");
        }
        BodyMode::FormUrlEncoded => {
            output.push_str("    'form_params' => [\n");
            for field in request
                .body_fields
                .iter()
                .filter(|field| field.enabled && !field.name.trim().is_empty())
            {
                let _ = writeln!(
                    output,
                    "        {} => {},",
                    php_string(&field.name),
                    php_string(&field.value)
                );
            }
            output.push_str("    ],\n");
        }
        BodyMode::Raw if !request.body.is_empty() => {
            let _ = writeln!(output, "    'body' => {},", php_string(&request.body));
        }
        BodyMode::None | BodyMode::Raw => {}
    }
    output.push_str("];\n");
    let _ = writeln!(
        output,
        "$response = $client->request({}, {}, $options);",
        php_string(&method(request)),
        php_string(request.url.trim())
    );
    output.push_str("echo $response->getBody();\n");
    output
}

fn export_kotlin_ktor(name: &str, template: &RequestTemplate) -> String {
    let request = &template.request;
    let mut output = metadata(name, template, "//");
    output.push_str("\nimport io.ktor.client.*\nimport io.ktor.client.engine.cio.*\nimport io.ktor.client.request.*\nimport io.ktor.client.statement.*\nimport io.ktor.http.*\nimport io.ktor.client.request.forms.*\nimport java.io.File\n\nsuspend fun main() {\n    val client = HttpClient(CIO)\n    val response = client.request(");
    output.push_str(&c_string(request.url.trim()));
    output.push_str(") {\n");
    let _ = writeln!(
        output,
        "        method = HttpMethod.parse({})",
        c_string(&method(request))
    );
    for (header_name, value) in managed_multipart_headers(request) {
        let _ = writeln!(
            output,
            "        header({}, {})",
            c_string(&header_name),
            c_string(&value)
        );
    }
    if request.body_mode == BodyMode::MultipartFormData {
        output.push_str("        setBody(MultiPartFormDataContent(formData {\n");
        for field in request
            .body_fields
            .iter()
            .filter(|field| field.enabled && !field.name.trim().is_empty())
        {
            if field.kind == BodyFieldKind::File {
                let _ = writeln!(
                    output,
                    "            append({}, File({}).readBytes(), Headers.build {{ append(HttpHeaders.ContentDisposition, {}) }})",
                    c_string(&field.name),
                    c_string(&field.value),
                    c_string(&format!(
                        "filename=\"{}\"",
                        std::path::Path::new(&field.value)
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("upload.bin")
                    ))
                );
            } else {
                let _ = writeln!(
                    output,
                    "            append({}, {})",
                    c_string(&field.name),
                    c_string(&field.value)
                );
            }
        }
        output.push_str("        }))\n");
    } else if let Some(body) = request_body_bytes(request) {
        let _ = writeln!(output, "        setBody({})", c_string(&body));
    }
    output.push_str("    }\n    println(response.bodyAsText())\n    client.close()\n}\n");
    output
}

fn export_kotlin_okhttp(name: &str, template: &RequestTemplate) -> String {
    let request = &template.request;
    let mut output = metadata(name, template, "//");
    output.push_str("\nimport okhttp3.*\nimport okhttp3.MediaType.Companion.toMediaType\nimport okhttp3.RequestBody.Companion.asRequestBody\nimport okhttp3.RequestBody.Companion.toRequestBody\nimport java.io.File\n\nfun main() {\n    val client = OkHttpClient()\n");
    if request.body_mode == BodyMode::MultipartFormData {
        output.push_str(
            "    val body: RequestBody = MultipartBody.Builder().setType(MultipartBody.FORM)\n",
        );
        for field in request
            .body_fields
            .iter()
            .filter(|field| field.enabled && !field.name.trim().is_empty())
        {
            if field.kind == BodyFieldKind::File {
                let _ = writeln!(
                    output,
                    "        .addFormDataPart({}, File({}).name, File({}).asRequestBody(\"application/octet-stream\".toMediaType()))",
                    c_string(&field.name),
                    c_string(&field.value),
                    c_string(&field.value)
                );
            } else {
                let _ = writeln!(
                    output,
                    "        .addFormDataPart({}, {})",
                    c_string(&field.name),
                    c_string(&field.value)
                );
            }
        }
        output.push_str("        .build()\n");
    } else if let Some(body) = request_body_bytes(request) {
        let _ = writeln!(
            output,
            "    val body: RequestBody? = {}.toRequestBody({}.toMediaType())",
            c_string(&body),
            c_string(&media_type(request))
        );
    } else if method_requires_body(&method(request)) {
        output.push_str("    val body: RequestBody? = ByteArray(0).toRequestBody()\n");
    } else {
        output.push_str("    val body: RequestBody? = null\n");
    }
    output.push_str("    val request = Request.Builder()\n");
    let _ = writeln!(output, "        .url({})", c_string(request.url.trim()));
    for (header_name, value) in managed_multipart_headers(request) {
        let _ = writeln!(
            output,
            "        .addHeader({}, {})",
            c_string(&header_name),
            c_string(&value)
        );
    }
    let _ = writeln!(
        output,
        "        .method({}, body)",
        c_string(&method(request))
    );
    output.push_str("        .build()\n    client.newCall(request).execute().use { response ->\n        println(response.body?.string())\n    }\n}\n");
    output
}

fn export_kotlin_java_http_client(name: &str, template: &RequestTemplate) -> String {
    let request = &template.request;
    let body = request_body_bytes(request);
    let mut output = metadata(name, template, "//");
    output.push_str("\nimport java.net.URI\nimport java.net.http.HttpClient\nimport java.net.http.HttpRequest\nimport java.net.http.HttpResponse\n\nfun main() {\n    val client = HttpClient.newHttpClient()\n    val request = HttpRequest.newBuilder()\n");
    let _ = writeln!(
        output,
        "        .uri(URI.create({}))",
        c_string(request.url.trim())
    );
    for (header_name, value) in all_headers(request) {
        let _ = writeln!(
            output,
            "        .header({}, {})",
            c_string(&header_name),
            c_string(&value)
        );
    }
    if let Some(body) = body {
        let _ = writeln!(
            output,
            "        .method({}, HttpRequest.BodyPublishers.ofString({}))",
            c_string(&method(request)),
            c_string(&body)
        );
    } else {
        let _ = writeln!(
            output,
            "        .method({}, HttpRequest.BodyPublishers.noBody())",
            c_string(&method(request))
        );
    }
    output.push_str("        .build()\n    val response = client.send(request, HttpResponse.BodyHandlers.ofString())\n    println(response.body())\n}\n");
    output
}

fn looks_like_openapi(source: &str) -> bool {
    let lower = source.to_ascii_lowercase();
    (lower.contains("\"openapi\"")
        || lower
            .lines()
            .any(|line| line.trim_start().starts_with("openapi:")))
        && (lower.contains("\"paths\"")
            || lower
                .lines()
                .any(|line| line.trim_start().starts_with("paths:")))
}

fn looks_like_asyncapi(source: &str) -> bool {
    let lower = source.to_ascii_lowercase();
    lower.contains("\"asyncapi\"")
        || lower
            .lines()
            .any(|line| line.trim_start().starts_with("asyncapi:"))
}

fn parse_yaml_or_json(source: &str, label: &str) -> Result<Value, InterchangeError> {
    serde_yaml::from_str(source)
        .map_err(|error| InterchangeError::Parse(format!("invalid {label}: {error}")))
}

fn resolve_local_reference<'a>(root: &'a Value, mut value: &'a Value) -> &'a Value {
    for _ in 0..16 {
        let Some(reference) = value.get("$ref").and_then(Value::as_str) else {
            break;
        };
        let Some(pointer) = reference.strip_prefix('#') else {
            break;
        };
        let Some(resolved) = root.pointer(pointer) else {
            break;
        };
        if std::ptr::eq(resolved, value) {
            break;
        }
        value = resolved;
    }
    value
}

fn import_openapi(source: &str) -> Result<ImportBundle, InterchangeError> {
    let root = parse_yaml_or_json(source, "OpenAPI document")?;
    let root_object = root
        .as_object()
        .ok_or_else(|| InterchangeError::Parse("OpenAPI document must be an object".to_owned()))?;
    let server = root_object
        .get("servers")
        .and_then(Value::as_array)
        .and_then(|servers| servers.first())
        .map(|server| resolve_local_reference(&root, server))
        .and_then(|server| server.get("url"))
        .and_then(Value::as_str)
        .unwrap_or("https://example.com");
    let paths = root_object
        .get("paths")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            InterchangeError::Parse("OpenAPI document has no paths object".to_owned())
        })?;
    let mut requests = Vec::new();
    for (path, path_item) in paths {
        let path_item = resolve_local_reference(&root, path_item);
        let Some(path_item) = path_item.as_object() else {
            continue;
        };
        for method_name in [
            "get", "post", "put", "patch", "delete", "head", "options", "trace",
        ] {
            let Some(operation) = path_item.get(method_name).and_then(Value::as_object) else {
                continue;
            };
            if let Some(template) = operation
                .get("x-resolved-request")
                .and_then(|value| serde_json::from_value::<RequestTemplate>(value.clone()).ok())
            {
                requests.push(ImportedRequest {
                    name: operation_name(operation, method_name, path),
                    template,
                });
                enforce_request_count(&requests)?;
                continue;
            }

            let mut request = RequestDraft::new(
                method_name.to_ascii_uppercase(),
                join_server_path(
                    &openapi_import_template(server),
                    &openapi_import_template(path),
                ),
            );
            let mut parameters = Vec::new();
            if let Some(values) = path_item.get("parameters").and_then(Value::as_array) {
                parameters.extend(values.iter());
            }
            if let Some(values) = operation.get("parameters").and_then(Value::as_array) {
                parameters.extend(values.iter());
            }
            let mut query = Vec::new();
            for parameter in parameters {
                let parameter = resolve_local_reference(&root, parameter);
                let Some(parameter) = parameter.as_object() else {
                    continue;
                };
                let Some(name) = parameter.get("name").and_then(Value::as_str) else {
                    continue;
                };
                let value = openapi_parameter_value(parameter, name);
                match parameter.get("in").and_then(Value::as_str) {
                    Some("header") => request.headers.push(HeaderEntry::new(name, value)),
                    Some("query") => query.push((name.to_owned(), value)),
                    _ => {}
                }
            }
            append_query_parameters(&mut request.url, &query);
            if let Some(request_body) = operation.get("requestBody") {
                import_openapi_body(&root, &mut request, request_body);
            }
            requests.push(ImportedRequest {
                name: operation_name(operation, method_name, path),
                template: RequestTemplate::new(request),
            });
            enforce_request_count(&requests)?;
        }
    }
    if requests.is_empty() {
        return Err(InterchangeError::Unsupported(
            "the OpenAPI document contains no supported HTTP operations".to_owned(),
        ));
    }
    Ok(ImportBundle {
        source_format: "OpenAPI".to_owned(),
        requests,
    })
}

fn operation_name(operation: &Map<String, Value>, method: &str, path: &str) -> String {
    operation
        .get("summary")
        .or_else(|| operation.get("operationId"))
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{} {path}", method.to_ascii_uppercase()))
}

fn openapi_parameter_value(parameter: &Map<String, Value>, name: &str) -> String {
    parameter
        .get("example")
        .or_else(|| {
            parameter
                .get("schema")
                .and_then(|schema| schema.get("example"))
        })
        .or_else(|| {
            parameter
                .get("schema")
                .and_then(|schema| schema.get("default"))
        })
        .and_then(value_as_text)
        .unwrap_or_else(|| format!("{{{{{name}}}}}"))
}

fn append_query_parameters(url: &mut String, parameters: &[(String, String)]) {
    if parameters.is_empty() {
        return;
    }
    let separator = if url.contains('?') { '&' } else { '?' };
    url.push(separator);
    for (index, (name, value)) in parameters.iter().enumerate() {
        if index > 0 {
            url.push('&');
        }
        url.push_str(name);
        url.push('=');
        url.push_str(value);
    }
}

fn import_openapi_body(root: &Value, request: &mut RequestDraft, request_body: &Value) {
    let request_body = resolve_local_reference(root, request_body);
    let Some(content) = request_body.get("content").and_then(Value::as_object) else {
        return;
    };
    let preferred = [
        "application/json",
        "application/x-www-form-urlencoded",
        "multipart/form-data",
        "text/plain",
    ]
    .into_iter()
    .find_map(|content_type| content.get_key_value(content_type))
    .or_else(|| content.iter().next());
    let Some((content_type, media)) = preferred else {
        return;
    };
    let media = resolve_local_reference(root, media);
    let example = media
        .get("example")
        .or_else(|| {
            media
                .get("schema")
                .map(|schema| resolve_local_reference(root, schema))
                .and_then(|schema| schema.get("example"))
        })
        .map(|example| resolve_local_reference(root, example));
    if content_type == "application/x-www-form-urlencoded" || content_type == "multipart/form-data"
    {
        request.body_mode = if content_type == "multipart/form-data" {
            BodyMode::MultipartFormData
        } else {
            BodyMode::FormUrlEncoded
        };
        let properties = media
            .get("schema")
            .map(|schema| resolve_local_reference(root, schema))
            .and_then(|schema| schema.get("properties"))
            .and_then(Value::as_object);
        let examples = example.and_then(Value::as_object);
        if let Some(properties) = properties {
            for (name, schema) in properties {
                let schema = resolve_local_reference(root, schema);
                let value = examples
                    .and_then(|examples| examples.get(name))
                    .and_then(value_as_text)
                    .or_else(|| schema.get("example").and_then(value_as_text))
                    .or_else(|| schema.get("default").and_then(value_as_text))
                    .unwrap_or_else(|| format!("{{{{{name}}}}}"));
                request.body_fields.push(BodyField {
                    enabled: true,
                    name: name.clone(),
                    value,
                    kind: if schema.get("format").and_then(Value::as_str) == Some("binary") {
                        BodyFieldKind::File
                    } else {
                        BodyFieldKind::Text
                    },
                });
            }
        } else if let Some(examples) = examples {
            for (name, value) in examples {
                request.body_fields.push(BodyField::text(
                    name,
                    value_as_text(value).unwrap_or_default(),
                ));
            }
        }
        return;
    }
    request.body_mode = BodyMode::Raw;
    request.raw_body_language = raw_language_for_content_type(content_type);
    if let Some(example) = example {
        request.body = value_to_body(example, content_type);
    }
    if !content_type.eq_ignore_ascii_case(request.raw_body_language.content_type()) {
        request
            .headers
            .push(HeaderEntry::new("Content-Type", content_type));
    }
}

fn import_asyncapi(source: &str) -> Result<ImportBundle, InterchangeError> {
    let root = parse_yaml_or_json(source, "AsyncAPI document")?;
    if let Some(values) = root.get("x-resolved-requests").and_then(Value::as_array) {
        let requests = values
            .iter()
            .filter_map(|value| {
                let name = value.get("name")?.as_str()?.to_owned();
                let template = serde_json::from_value(value.get("template")?.clone()).ok()?;
                Some(ImportedRequest { name, template })
            })
            .collect::<Vec<_>>();
        if !requests.is_empty() {
            return Ok(ImportBundle {
                source_format: "AsyncAPI".to_owned(),
                requests,
            });
        }
    }
    let server = asyncapi_server(&root);
    let channels = root
        .get("channels")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            InterchangeError::Parse("AsyncAPI document has no channels object".to_owned())
        })?;
    let mut requests = Vec::new();
    for (channel_name, channel) in channels {
        let channel = resolve_local_reference(&root, channel);
        let address = channel
            .get("address")
            .and_then(Value::as_str)
            .unwrap_or(channel_name);
        let channel_http_binding = channel
            .get("bindings")
            .and_then(|bindings| bindings.get("http"));
        let operation = asyncapi_operation_for_channel(&root, channel_name);
        let operation_http_binding = operation
            .and_then(|operation| operation.get("bindings"))
            .and_then(|bindings| bindings.get("http"));
        let legacy_operation_http_binding = channel
            .get("publish")
            .or_else(|| channel.get("subscribe"))
            .and_then(|operation| operation.get("bindings"))
            .and_then(|bindings| bindings.get("http"));
        let method = operation_http_binding
            .or(legacy_operation_http_binding)
            .or(channel_http_binding)
            .and_then(|binding| binding.get("method"))
            .and_then(Value::as_str)
            .unwrap_or("POST");
        let has_http_transport = operation_http_binding.is_some()
            || legacy_operation_http_binding.is_some()
            || channel_http_binding.is_some()
            || server.starts_with("http://")
            || server.starts_with("https://");
        if !has_http_transport {
            continue;
        }
        let mut request = RequestDraft::new(
            method.to_ascii_uppercase(),
            join_server_path(
                &openapi_import_template(&server),
                &openapi_import_template(address),
            ),
        );
        if let Some(properties) = operation_http_binding
            .or(legacy_operation_http_binding)
            .and_then(|binding| binding.get("query"))
            .and_then(|query| query.get("properties"))
            .and_then(Value::as_object)
        {
            let query = properties
                .iter()
                .map(|(name, schema)| {
                    let value = schema
                        .get("examples")
                        .and_then(Value::as_array)
                        .and_then(|examples| examples.first())
                        .or_else(|| schema.get("example"))
                        .or_else(|| schema.get("default"))
                        .and_then(value_as_text)
                        .unwrap_or_else(|| format!("{{{{{name}}}}}"));
                    (name.clone(), value)
                })
                .collect::<Vec<_>>();
            append_query_parameters(&mut request.url, &query);
        }
        let message = channel
            .get("messages")
            .and_then(Value::as_object)
            .and_then(|messages| messages.values().next())
            .or_else(|| {
                channel
                    .get("publish")
                    .and_then(|operation| operation.get("message"))
            })
            .or_else(|| {
                channel
                    .get("subscribe")
                    .and_then(|operation| operation.get("message"))
            })
            .map(|message| resolve_local_reference(&root, message));
        if let Some(message) = message {
            if let Some(properties) = message
                .get("bindings")
                .and_then(|bindings| bindings.get("http"))
                .and_then(|binding| binding.get("headers"))
                .or_else(|| message.get("headers"))
                .and_then(|headers| headers.get("properties"))
                .and_then(Value::as_object)
            {
                for (name, schema) in properties {
                    let value = schema
                        .get("examples")
                        .and_then(Value::as_array)
                        .and_then(|examples| examples.first())
                        .or_else(|| schema.get("example"))
                        .or_else(|| schema.get("default"))
                        .and_then(value_as_text)
                        .unwrap_or_else(|| format!("{{{{{name}}}}}"));
                    request.headers.push(HeaderEntry::new(name, value));
                }
            }
            let example = message
                .get("examples")
                .and_then(Value::as_array)
                .and_then(|examples| examples.first())
                .and_then(|example| example.get("payload"))
                .or_else(|| {
                    message
                        .get("payload")
                        .and_then(|payload| payload.get("example"))
                });
            if let Some(example) = example {
                request.body_mode = BodyMode::Raw;
                request.raw_body_language = RawBodyLanguage::Json;
                request.body = value_to_body(example, "application/json");
            }
        }
        requests.push(ImportedRequest {
            name: channel
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or(channel_name)
                .to_owned(),
            template: RequestTemplate::new(request),
        });
        enforce_request_count(&requests)?;
    }
    if requests.is_empty() {
        return Err(InterchangeError::Unsupported(
            "the AsyncAPI document contains no importable HTTP channels".to_owned(),
        ));
    }
    Ok(ImportBundle {
        source_format: "AsyncAPI".to_owned(),
        requests,
    })
}

fn asyncapi_operation_for_channel<'a>(root: &'a Value, channel_name: &str) -> Option<&'a Value> {
    root.get("operations")
        .and_then(Value::as_object)?
        .values()
        .find(|operation| {
            operation
                .get("channel")
                .and_then(|channel| channel.get("$ref"))
                .and_then(Value::as_str)
                .is_some_and(|reference| reference.rsplit('/').next() == Some(channel_name))
        })
}

fn asyncapi_server(root: &Value) -> String {
    let Some(server) = root
        .get("servers")
        .and_then(Value::as_object)
        .and_then(|servers| servers.values().next())
        .map(|server| resolve_local_reference(root, server))
    else {
        return "https://example.com".to_owned();
    };
    if let Some(url) = server.get("url").and_then(Value::as_str) {
        return url.to_owned();
    }
    let host = server
        .get("host")
        .and_then(Value::as_str)
        .unwrap_or("example.com");
    if host.starts_with("http://") || host.starts_with("https://") {
        host.to_owned()
    } else {
        let protocol = server
            .get("protocol")
            .and_then(Value::as_str)
            .unwrap_or("https");
        format!("{protocol}://{host}")
    }
}

fn join_server_path(server: &str, path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        return path.to_owned();
    }
    format!(
        "{}/{}",
        server.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

fn value_as_text(value: &Value) -> Option<String> {
    match value {
        Value::Null => Some("null".to_owned()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Number(value) => Some(value.to_string()),
        Value::String(value) => Some(value.clone()),
        Value::Array(_) | Value::Object(_) => serde_json::to_string(value).ok(),
    }
}

fn value_to_body(value: &Value, content_type: &str) -> String {
    if content_type.to_ascii_lowercase().contains("json") {
        serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
    } else {
        value_as_text(value).unwrap_or_default()
    }
}

fn raw_language_for_content_type(content_type: &str) -> RawBodyLanguage {
    let lower = content_type.to_ascii_lowercase();
    if lower.contains("json") {
        RawBodyLanguage::Json
    } else if lower.contains("xml") {
        RawBodyLanguage::Xml
    } else if lower.contains("html") {
        RawBodyLanguage::Html
    } else if lower.contains("javascript") {
        RawBodyLanguage::JavaScript
    } else if lower.contains("yaml") || lower.contains("yml") {
        RawBodyLanguage::Yaml
    } else if lower.contains("graphql") {
        RawBodyLanguage::GraphQl
    } else {
        RawBodyLanguage::Text
    }
}

fn enforce_request_count(requests: &[ImportedRequest]) -> Result<(), InterchangeError> {
    if requests.len() > 256 {
        Err(InterchangeError::TooLarge)
    } else {
        Ok(())
    }
}

fn starts_with_command(source: &str, command: &str) -> bool {
    source
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .is_some_and(|line| {
            line.split_whitespace()
                .next()
                .is_some_and(|word| word.rsplit('/').next() == Some(command))
        })
}

fn normalized_command_source(source: &str) -> String {
    source
        .replace("\\\r\n", " ")
        .replace("\\\n", " ")
        .replace("`\r\n", " ")
        .replace("`\n", " ")
        .replace("^\r\n", " ")
        .replace("^\n", " ")
}

fn shell_tokens(source: &str) -> Result<Vec<String>, InterchangeError> {
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut chars = source.chars().peekable();
    let mut quote = None;
    while let Some(character) = chars.next() {
        match quote {
            Some('\'') => {
                if character == '\'' {
                    quote = None;
                } else {
                    token.push(character);
                }
            }
            Some('"') => match character {
                '"' => quote = None,
                '\\' => {
                    if let Some(next) = chars.next() {
                        token.push(match next {
                            'n' => '\n',
                            'r' => '\r',
                            't' => '\t',
                            other => other,
                        });
                    }
                }
                other => token.push(other),
            },
            Some(_) => unreachable!(),
            None => match character {
                '\'' | '"' => quote = Some(character),
                '\\' => {
                    if let Some(next) = chars.next() {
                        token.push(next);
                    }
                }
                character if character.is_whitespace() => {
                    if !token.is_empty() {
                        tokens.push(std::mem::take(&mut token));
                    }
                }
                other => token.push(other),
            },
        }
    }
    if quote.is_some() {
        return Err(InterchangeError::Parse(
            "an imported command contains an unterminated quote".to_owned(),
        ));
    }
    if !token.is_empty() {
        tokens.push(token);
    }
    Ok(tokens)
}

fn parse_curl(source: &str) -> Result<ImportedRequest, InterchangeError> {
    let normalized = normalized_command_source(source);
    let tokens = shell_tokens(&normalized)?;
    let command_index = tokens
        .iter()
        .position(|token| token.rsplit('/').next() == Some("curl"))
        .ok_or_else(|| InterchangeError::Parse("cURL command is missing `curl`".to_owned()))?;
    let mut request = RequestDraft {
        body_mode: BodyMode::None,
        ..RequestDraft::default()
    };
    let mut explicit_method = false;
    let mut data = Vec::new();
    let mut query_mode = false;
    let mut index = command_index + 1;
    while index < tokens.len() {
        let token = &tokens[index];
        let (flag, inline_value) = split_long_option(token);
        match flag {
            "-X" | "--request" => {
                let (value, consumed) = option_value(&tokens, index, inline_value)?;
                request.method = value.to_ascii_uppercase();
                explicit_method = true;
                index += consumed;
            }
            value if value.starts_with("-X") && value.len() > 2 => {
                request.method = value[2..].to_ascii_uppercase();
                explicit_method = true;
            }
            "--url" => {
                let (value, consumed) = option_value(&tokens, index, inline_value)?;
                request.url = value.to_owned();
                index += consumed;
            }
            "-H" | "--header" => {
                let (value, consumed) = option_value(&tokens, index, inline_value)?;
                if let Some(header) = parse_header(value) {
                    request.headers.push(header);
                }
                index += consumed;
            }
            value if value.starts_with("-H") && value.len() > 2 => {
                if let Some(header) = parse_header(&value[2..]) {
                    request.headers.push(header);
                }
            }
            "-d" | "--data" | "--data-raw" | "--data-binary" | "--data-ascii" | "--json" => {
                let (value, consumed) = option_value(&tokens, index, inline_value)?;
                data.push(value.to_owned());
                if flag == "--json" {
                    ensure_header(&mut request, "Content-Type", "application/json");
                    ensure_header(&mut request, "Accept", "application/json");
                }
                index += consumed;
            }
            "--data-urlencode" => {
                let (value, consumed) = option_value(&tokens, index, inline_value)?;
                request.body_mode = BodyMode::FormUrlEncoded;
                let (name, value) = value.split_once('=').unwrap_or((value, ""));
                request.body_fields.push(BodyField::text(name, value));
                index += consumed;
            }
            "-F" | "--form" | "--form-string" => {
                let (value, consumed) = option_value(&tokens, index, inline_value)?;
                request.body_mode = BodyMode::MultipartFormData;
                let (name, value) = value.split_once('=').unwrap_or((value, ""));
                let field = if let Some(path) = value.strip_prefix('@') {
                    BodyField::file(name, path)
                } else {
                    BodyField::text(name, value)
                };
                request.body_fields.push(field);
                index += consumed;
            }
            "-G" | "--get" => {
                query_mode = true;
                request.method = "GET".to_owned();
                explicit_method = true;
            }
            "-I" | "--head" => {
                request.method = "HEAD".to_owned();
                explicit_method = true;
            }
            "-u" | "--user" => {
                let (value, consumed) = option_value(&tokens, index, inline_value)?;
                ensure_header(
                    &mut request,
                    "Authorization",
                    &format!(
                        "Basic {}",
                        base64::engine::general_purpose::STANDARD.encode(value)
                    ),
                );
                index += consumed;
            }
            value
                if !value.starts_with('-')
                    && request.url.is_empty()
                    && (value.starts_with("http://")
                        || value.starts_with("https://")
                        || value.contains("{{")) =>
            {
                request.url = value.to_owned();
            }
            _ => {}
        }
        index += 1;
    }
    if request.url.trim().is_empty() {
        return Err(InterchangeError::Unsupported(
            "the cURL command has no static URL".to_owned(),
        ));
    }
    if !data.is_empty() {
        let joined = data.join("&");
        if query_mode {
            let separator = if request.url.contains('?') { '&' } else { '?' };
            request.url.push(separator);
            request.url.push_str(&joined);
        } else if request.body_mode != BodyMode::FormUrlEncoded {
            request.body_mode = BodyMode::Raw;
            request.body = joined;
            apply_content_type_language(&mut request);
            if !explicit_method {
                request.method = "POST".to_owned();
            }
        }
    }
    imported_from_draft(request)
}

fn parse_wget(source: &str) -> Result<ImportedRequest, InterchangeError> {
    let normalized = normalized_command_source(source);
    let tokens = shell_tokens(&normalized)?;
    let command_index = tokens
        .iter()
        .position(|token| token.rsplit('/').next() == Some("wget"))
        .ok_or_else(|| InterchangeError::Parse("Wget command is missing `wget`".to_owned()))?;
    let mut request = RequestDraft {
        body_mode: BodyMode::None,
        ..RequestDraft::default()
    };
    let mut index = command_index + 1;
    while index < tokens.len() {
        let token = &tokens[index];
        let (flag, inline_value) = split_long_option(token);
        match flag {
            "--method" => {
                let (value, consumed) = option_value(&tokens, index, inline_value)?;
                request.method = value.to_ascii_uppercase();
                index += consumed;
            }
            "--header" => {
                let (value, consumed) = option_value(&tokens, index, inline_value)?;
                if let Some(header) = parse_header(value) {
                    request.headers.push(header);
                }
                index += consumed;
            }
            "--body-data" | "--post-data" => {
                let (value, consumed) = option_value(&tokens, index, inline_value)?;
                request.body_mode = BodyMode::Raw;
                request.body = value.to_owned();
                if request.method == "GET" {
                    request.method = "POST".to_owned();
                }
                index += consumed;
            }
            "--body-file" | "--post-file" => {
                let (value, consumed) = option_value(&tokens, index, inline_value)?;
                request.body_mode = BodyMode::Raw;
                request.body = format!("{{{{file:{value}}}}}");
                if request.method == "GET" {
                    request.method = "POST".to_owned();
                }
                index += consumed;
            }
            value
                if !value.starts_with('-')
                    && (value.starts_with("http://")
                        || value.starts_with("https://")
                        || value.contains("{{")) =>
            {
                request.url = value.to_owned();
            }
            _ => {}
        }
        index += 1;
    }
    if request.url.trim().is_empty() {
        return Err(InterchangeError::Unsupported(
            "the Wget command has no static URL".to_owned(),
        ));
    }
    apply_content_type_language(&mut request);
    imported_from_draft(request)
}

fn parse_powershell(source: &str) -> Result<ImportedRequest, InterchangeError> {
    let normalized = normalized_command_source(source);
    let tokens = shell_tokens(&normalized)?;
    let mut request = RequestDraft {
        body_mode: BodyMode::None,
        url: powershell_argument(&tokens, &["-uri", "-url"]).unwrap_or_default(),
        method: powershell_argument(&tokens, &["-method"])
            .unwrap_or_else(|| "GET".to_owned())
            .to_ascii_uppercase(),
        ..RequestDraft::default()
    };
    if let Some(body) = powershell_argument(&tokens, &["-body"]) {
        request.body_mode = BodyMode::Raw;
        request.body = body;
        if request.method == "GET" {
            request.method = "POST".to_owned();
        }
    }
    if let Some(content_type) = powershell_argument(&tokens, &["-contenttype"]) {
        request
            .headers
            .push(HeaderEntry::new("Content-Type", content_type));
    }
    if let Some(map) = powershell_map_argument(source, "-Headers") {
        for (name, value) in parse_powershell_hashtable(map) {
            request.headers.push(HeaderEntry::new(name, value));
        }
    }
    if let Some(map) = powershell_map_argument(source, "-Form") {
        request.body_mode = BodyMode::MultipartFormData;
        request.body_fields = parse_powershell_form(map);
        if request.method == "GET" {
            request.method = "POST".to_owned();
        }
    }
    if request.url.trim().is_empty() {
        request.url = string_literals(source)
            .into_iter()
            .map(|literal| literal.value)
            .find(|value| value.starts_with("http://") || value.starts_with("https://"))
            .unwrap_or_default();
    }
    if request.url.trim().is_empty() {
        return Err(InterchangeError::Unsupported(
            "the PowerShell command has no static -Uri value".to_owned(),
        ));
    }
    apply_content_type_language(&mut request);
    imported_from_draft(request)
}

fn split_long_option(token: &str) -> (&str, Option<&str>) {
    if token.starts_with("--")
        && let Some((flag, value)) = token.split_once('=')
    {
        (flag, Some(value))
    } else {
        (token, None)
    }
}

fn option_value<'a>(
    tokens: &'a [String],
    index: usize,
    inline: Option<&'a str>,
) -> Result<(&'a str, usize), InterchangeError> {
    if let Some(value) = inline {
        return Ok((value, 0));
    }
    tokens
        .get(index + 1)
        .map(|value| (value.as_str(), 1))
        .ok_or_else(|| InterchangeError::Parse(format!("{} is missing its value", tokens[index])))
}

fn parse_header(value: &str) -> Option<HeaderEntry> {
    let (name, value) = value.split_once(':')?;
    (!name.trim().is_empty()).then(|| HeaderEntry::new(name.trim(), value.trim()))
}

fn ensure_header(request: &mut RequestDraft, name: &str, value: &str) {
    if !request
        .headers
        .iter()
        .any(|header| header.enabled && header.name.eq_ignore_ascii_case(name))
    {
        request.headers.push(HeaderEntry::new(name, value));
    }
}

fn powershell_argument(tokens: &[String], names: &[&str]) -> Option<String> {
    tokens.iter().enumerate().find_map(|(index, token)| {
        names
            .iter()
            .any(|name| token.eq_ignore_ascii_case(name))
            .then(|| tokens.get(index + 1).cloned())
            .flatten()
    })
}

fn powershell_map_argument<'a>(source: &'a str, parameter: &str) -> Option<&'a str> {
    let lower = source.to_ascii_lowercase();
    let offset = lower.rfind(&parameter.to_ascii_lowercase())?;
    let argument = source[offset + parameter.len()..].trim_start();
    if argument.starts_with("@{") {
        return Some(argument);
    }
    let variable = argument.split_whitespace().next()?;
    if !variable.starts_with('$') {
        return Some(argument);
    }
    let definition_offset = lower[..offset].rfind(&variable.to_ascii_lowercase())?;
    Some(&source[definition_offset + variable.len()..offset])
}

fn parse_powershell_hashtable(source: &str) -> Vec<(String, String)> {
    let block = source
        .find("@{")
        .and_then(|start| {
            source[start + 2..]
                .find('}')
                .map(|end| &source[start + 2..start + 2 + end])
        })
        .unwrap_or_default();
    block
        .split([';', '\n'])
        .filter_map(|entry| {
            let (name, value) = entry.split_once('=')?;
            Some((trim_quotes(name.trim()), trim_quotes(value.trim())))
        })
        .filter(|(name, _)| !name.is_empty())
        .collect()
}

fn parse_powershell_form(source: &str) -> Vec<BodyField> {
    parse_powershell_hashtable(source)
        .into_iter()
        .map(|(name, value)| {
            let value = value.trim();
            if let Some(path) = value
                .strip_prefix("Get-Item ")
                .or_else(|| value.strip_prefix("get-item "))
            {
                BodyField::file(name, trim_quotes(path.trim()))
            } else {
                BodyField::text(name, value)
            }
        })
        .collect()
}

fn trim_quotes(value: &str) -> String {
    if value.len() >= 2 {
        let first = value.as_bytes()[0] as char;
        let last = value.as_bytes()[value.len() - 1] as char;
        if (first == '\'' && last == '\'') || (first == '"' && last == '"') {
            return value[1..value.len() - 1]
                .replace("''", "'")
                .replace("\\\"", "\"");
        }
    }
    value.to_owned()
}

fn imported_from_draft(request: RequestDraft) -> Result<ImportedRequest, InterchangeError> {
    if request.url.trim().is_empty() {
        return Err(InterchangeError::Unsupported(
            "the request URL is dynamic or missing".to_owned(),
        ));
    }
    Ok(ImportedRequest {
        name: normalized_name("", &request),
        template: RequestTemplate::new(request),
    })
}

fn apply_content_type_language(request: &mut RequestDraft) {
    if let Some(content_type) = request
        .headers
        .iter()
        .find(|header| header.enabled && header.name.eq_ignore_ascii_case("content-type"))
        .map(|header| header.value.clone())
    {
        request.raw_body_language = raw_language_for_content_type(&content_type);
    }
}

fn looks_like_intellij_http(source: &str) -> bool {
    source.split("###").any(|section| {
        section
            .lines()
            .map(str::trim)
            .filter(|line| {
                !line.is_empty()
                    && !line.starts_with('#')
                    && !line.starts_with("//")
                    && !line.starts_with('@')
            })
            .any(is_http_request_line)
    })
}

fn is_http_request_line(line: &str) -> bool {
    let mut parts = line.split_whitespace();
    let Some(method) = parts.next() else {
        return false;
    };
    let Some(url) = parts.next() else {
        return false;
    };
    is_method(method)
        && (url.starts_with("http://")
            || url.starts_with("https://")
            || url.starts_with('/')
            || url.contains("{{"))
}

fn import_intellij_http(source: &str) -> Result<ImportBundle, InterchangeError> {
    let mut requests = Vec::new();
    for section in source.split("###") {
        let lines = section.lines().collect::<Vec<_>>();
        let Some(request_line_index) = lines
            .iter()
            .position(|line| is_http_request_line(line.trim()))
        else {
            continue;
        };
        let request_line = lines[request_line_index].trim();
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or("GET");
        let url = parts.next().unwrap_or_default();
        let mut request = RequestDraft::new(method.to_ascii_uppercase(), url);
        request.body_mode = BodyMode::None;
        let mut body_start = None;
        for (offset, line) in lines[request_line_index + 1..].iter().enumerate() {
            let line_index = request_line_index + 1 + offset;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                body_start = Some(line_index + 1);
                break;
            }
            if trimmed.starts_with('#') || trimmed.starts_with("//") {
                continue;
            }
            if let Some(header) = parse_header(trimmed) {
                request.headers.push(header);
            } else {
                body_start = Some(line_index);
                break;
            }
        }
        if let Some(body_start) = body_start {
            let body = lines[body_start..]
                .iter()
                .take_while(|line| {
                    let trimmed = line.trim_start();
                    !trimmed.starts_with("> {%")
                        && !trimmed.starts_with("< {%")
                        && !trimmed.starts_with("client.")
                })
                .copied()
                .collect::<Vec<_>>()
                .join("\n")
                .trim()
                .to_owned();
            if !body.is_empty() {
                request.body_mode = BodyMode::Raw;
                request.body = body;
                apply_content_type_language(&mut request);
            }
        }
        let name = lines[..request_line_index]
            .iter()
            .rev()
            .map(|line| line.trim().trim_start_matches('#').trim())
            .find(|line| !line.is_empty() && !line.starts_with('@'))
            .map(str::to_owned)
            .unwrap_or_else(|| normalized_name("", &request));
        requests.push(ImportedRequest {
            name,
            template: RequestTemplate::new(request),
        });
        enforce_request_count(&requests)?;
    }
    if requests.is_empty() {
        return Err(InterchangeError::Unsupported(
            "the HTTP Client file contains no request lines".to_owned(),
        ));
    }
    Ok(ImportBundle {
        source_format: "IntelliJ HTTP Client".to_owned(),
        requests,
    })
}

#[derive(Clone, Debug)]
struct StringLiteral {
    start: usize,
    quote: u8,
    value: String,
}

fn string_literals(source: &str) -> Vec<StringLiteral> {
    let bytes = source.as_bytes();
    let mut literals = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let quote = bytes[index];
        if quote != b'\'' && quote != b'"' && quote != b'`' {
            index += 1;
            continue;
        }
        let start = index;
        index += 1;
        let mut value = String::new();
        while index < bytes.len() {
            let byte = bytes[index];
            if byte == quote {
                index += 1;
                if quote == b'\'' && index < bytes.len() && bytes[index] == b'\'' {
                    value.push('\'');
                    index += 1;
                    continue;
                }
                break;
            }
            if byte == b'\\' && index + 1 < bytes.len() {
                index += 1;
                let escaped = bytes[index];
                match escaped {
                    b'n' => {
                        value.push('\n');
                        index += 1;
                    }
                    b'r' => {
                        value.push('\r');
                        index += 1;
                    }
                    b't' => {
                        value.push('\t');
                        index += 1;
                    }
                    b'b' => {
                        value.push('\u{8}');
                        index += 1;
                    }
                    b'f' => {
                        value.push('\u{c}');
                        index += 1;
                    }
                    b'\\' => {
                        value.push('\\');
                        index += 1;
                    }
                    b'\'' => {
                        value.push('\'');
                        index += 1;
                    }
                    b'"' => {
                        value.push('"');
                        index += 1;
                    }
                    b'u' if index + 4 < bytes.len() => {
                        let hex = &source[index + 1..index + 5];
                        if let Ok(code) = u32::from_str_radix(hex, 16)
                            && let Some(character) = char::from_u32(code)
                        {
                            value.push(character);
                            index += 5;
                        } else {
                            index += 1;
                        }
                    }
                    // An escaped non-ASCII byte is the leading byte of a multi-byte
                    // UTF-8 sequence. Decode the whole character instead of pushing
                    // the raw byte (which corrupted the value) and then stepping onto
                    // a continuation byte (which used to panic on the next slice).
                    other if !other.is_ascii() => {
                        if let Some(character) = source[index..].chars().next() {
                            value.push(character);
                            index += character.len_utf8();
                        } else {
                            index += 1;
                        }
                    }
                    other => {
                        value.push(other as char);
                        index += 1;
                    }
                }
                continue;
            }
            let Some(character) = source[index..].chars().next() else {
                break;
            };
            value.push(character);
            index += character.len_utf8();
        }
        literals.push(StringLiteral {
            start,
            quote,
            value,
        });
    }
    literals
}

fn parse_code_request(source: &str) -> Result<ImportedRequest, InterchangeError> {
    let literals = string_literals(source);
    let url = find_code_url(source, &literals).ok_or_else(|| {
        InterchangeError::Unsupported(
            "the source has no recognizable URL literal or expression".to_owned(),
        )
    })?;
    let mut request = RequestDraft::new(detect_code_method(source), url);
    request.body_mode = BodyMode::None;
    request.headers = extract_code_headers(source, &literals);
    if let Some(body) = extract_code_body(source, &literals, &request.url, &request.headers) {
        request.body_mode = BodyMode::Raw;
        request.body = body;
        apply_content_type_language(&mut request);
    }
    imported_from_draft(request)
}

fn find_code_url(source: &str, literals: &[StringLiteral]) -> Option<String> {
    let javascript = looks_like_javascript_request(source);
    if javascript
        && let Some(url) = literals
            .iter()
            .filter(|literal| literal.quote == b'`' && literal.value.contains("${"))
            .find(|literal| javascript_template_is_url_context(source, literal.start))
            .and_then(|literal| javascript_template_to_resolved(&literal.value))
    {
        return Some(url);
    }
    if javascript && let Some(url) = find_javascript_bare_url_argument(source) {
        return Some(url);
    }

    literals
        .iter()
        .find(|literal| {
            literal.value.starts_with("http://") || literal.value.starts_with("https://")
        })
        .map(|literal| imported_code_literal(literal, javascript))
        .or_else(|| {
            for keyword in ["url", "uri", "baseurl", "endpoint"] {
                let mut search_start = 0;
                let lower = source.to_ascii_lowercase();
                while let Some(relative) = lower[search_start..].find(keyword) {
                    let offset = search_start + relative;
                    if let Some(literal) = literals
                        .iter()
                        .find(|literal| literal.start > offset && literal.start < offset + 160)
                    {
                        return Some(imported_code_literal(literal, javascript));
                    }
                    search_start = offset + keyword.len();
                }
            }
            None
        })
        .or_else(|| {
            let lower = source.to_ascii_lowercase();
            if !lower.contains("boost::beast")
                && !lower.contains("boost/beast")
                && !lower.contains("http::request<")
                && !lower.contains("http::verb::")
            {
                return None;
            }
            let host = literals.iter().find(|literal| {
                !literal.value.contains(char::is_whitespace)
                    && literal.value.contains('.')
                    && !literal.value.contains(':')
                    && !literal.value.starts_with('/')
            })?;
            let target = literals
                .iter()
                .find(|literal| literal.value.starts_with('/'))
                .map(|literal| literal.value.as_str())
                .unwrap_or("/");
            let scheme =
                if lower.contains("ssl") || literals.iter().any(|literal| literal.value == "443") {
                    "https"
                } else {
                    "http"
                };
            Some(format!("{scheme}://{}{target}", host.value))
        })
}

fn looks_like_javascript_request(source: &str) -> bool {
    let lower = source.to_ascii_lowercase();
    lower.contains("fetch")
        || lower.contains("axios")
        || lower.contains("$.ajax")
        || lower.contains("jquery")
}

fn imported_code_literal(literal: &StringLiteral, javascript: bool) -> String {
    if javascript && literal.quote == b'`' {
        javascript_template_to_resolved(&literal.value).unwrap_or_else(|| literal.value.clone())
    } else {
        literal.value.clone()
    }
}

fn javascript_template_is_url_context(source: &str, literal_start: usize) -> bool {
    let context = source[..literal_start]
        .chars()
        .rev()
        .take(256)
        .collect::<String>()
        .chars()
        .rev()
        .filter(|character| !character.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    [
        "fetch(",
        "newrequest(",
        "axios.get(",
        "axios.post(",
        "axios.put(",
        "axios.patch(",
        "axios.delete(",
        "axios.head(",
        "axios.options(",
        "url:",
        "url=",
        "uri:",
        "uri=",
        "endpoint:",
        "endpoint=",
    ]
    .iter()
    .any(|suffix| context.ends_with(suffix))
}

fn find_javascript_bare_url_argument(source: &str) -> Option<String> {
    let lower = source.to_ascii_lowercase();
    for call in [
        "fetch(",
        "axios.get(",
        "axios.post(",
        "axios.put(",
        "axios.patch(",
        "axios.delete(",
        "axios.head(",
        "axios.options(",
    ] {
        let Some(offset) = lower.find(call).map(|offset| offset + call.len()) else {
            continue;
        };
        let argument = source[offset..].trim_start();
        let end = argument
            .find(|character: char| {
                !character.is_ascii_alphanumeric() && !"_$.".contains(character)
            })
            .unwrap_or(argument.len());
        let expression = &argument[..end];
        let terminator = argument[end..].trim_start().as_bytes().first().copied();
        if !expression.is_empty()
            && matches!(terminator, Some(b',' | b')'))
            && expression
                .bytes()
                .next()
                .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_' || byte == b'$')
        {
            return Some(format!("{{{{{expression}}}}}"));
        }
    }
    None
}

fn javascript_template_to_resolved(template: &str) -> Option<String> {
    let bytes = template.as_bytes();
    let mut output = String::with_capacity(template.len());
    let mut cursor = 0;
    let mut found_interpolation = false;
    while cursor < bytes.len() {
        let Some(relative) = template[cursor..].find("${") else {
            output.push_str(&template[cursor..]);
            break;
        };
        let opening = cursor + relative;
        output.push_str(&template[cursor..opening]);
        let mut index = opening + 2;
        let mut depth = 1usize;
        let mut quote = None;
        let mut escaped = false;
        while index < bytes.len() {
            let byte = bytes[index];
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if let Some(active_quote) = quote {
                if byte == active_quote {
                    quote = None;
                }
            } else if matches!(byte, b'\'' | b'"' | b'`') {
                quote = Some(byte);
            } else if byte == b'{' {
                depth += 1;
            } else if byte == b'}' {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            index += 1;
        }
        if depth != 0 {
            return None;
        }
        let expression = template[opening + 2..index].trim();
        if expression.is_empty()
            || expression.contains('{')
            || expression.contains('}')
            || expression.contains("{{")
            || expression.contains("}}")
        {
            return None;
        }
        let _ = write!(output, "{{{{{expression}}}}}");
        found_interpolation = true;
        cursor = index + 1;
    }
    found_interpolation.then_some(output)
}

fn detect_code_method(source: &str) -> String {
    let lower = source.to_ascii_lowercase();
    let patterns = [
        ("verb::patch", "PATCH"),
        ("::patch(", "PATCH"),
        ("method.patch", "PATCH"),
        ("httpmethod.patch", "PATCH"),
        ("method::patch", "PATCH"),
        (".patch(", "PATCH"),
        ("verb::delete", "DELETE"),
        ("::delete(", "DELETE"),
        ("method.delete", "DELETE"),
        ("httpmethod.delete", "DELETE"),
        ("method::delete", "DELETE"),
        (".delete(", "DELETE"),
        ("verb::put", "PUT"),
        ("::put(", "PUT"),
        ("method.put", "PUT"),
        ("httpmethod.put", "PUT"),
        ("method::put", "PUT"),
        (".put(", "PUT"),
        ("verb::post", "POST"),
        ("::post(", "POST"),
        ("method.post", "POST"),
        ("httpmethod.post", "POST"),
        ("method::post", "POST"),
        (".post(", "POST"),
        ("verb::head", "HEAD"),
        ("::head(", "HEAD"),
        ("method.head", "HEAD"),
        ("httpmethod.head", "HEAD"),
        ("method::head", "HEAD"),
        (".head(", "HEAD"),
        ("verb::options", "OPTIONS"),
        ("::options(", "OPTIONS"),
        ("method.options", "OPTIONS"),
        ("httpmethod.options", "OPTIONS"),
        ("method::options", "OPTIONS"),
        (".options(", "OPTIONS"),
        ("verb::get", "GET"),
        ("::get(", "GET"),
        ("method.get", "GET"),
        ("httpmethod.get", "GET"),
        ("method::get", "GET"),
        (".get(", "GET"),
    ];
    for (pattern, method) in patterns {
        if lower.contains(pattern) {
            return method.to_owned();
        }
    }
    for method in ["PATCH", "DELETE", "POST", "PUT", "HEAD", "OPTIONS", "GET"] {
        let quoted_double = format!("\"{method}\"");
        let quoted_single = format!("'{method}'");
        if lower.contains(&quoted_double.to_ascii_lowercase())
            || lower.contains(&quoted_single.to_ascii_lowercase())
        {
            return method.to_owned();
        }
    }
    if contains_any_ignore_ascii_case(
        source,
        &[
            "postfields",
            "stringcontent",
            "setbody",
            "requestbody.create",
        ],
    ) {
        "POST".to_owned()
    } else {
        "GET".to_owned()
    }
}

fn extract_code_headers(source: &str, literals: &[StringLiteral]) -> Vec<HeaderEntry> {
    let lower = source.to_ascii_lowercase();
    let mut headers = Vec::new();
    for pattern in [
        ".header(",
        ".addheader(",
        ".setheader(",
        ".tryaddwithoutvalidation(",
        "header.add(",
        "headers.add(",
        "header(",
    ] {
        let mut offset = 0;
        while let Some(relative) = lower[offset..].find(pattern) {
            let start = offset + relative;
            let values = literals
                .iter()
                .filter(|literal| literal.start > start && literal.start < start + 500)
                .take(2)
                .collect::<Vec<_>>();
            if let [name, value] = values.as_slice()
                && is_probable_header_name(&name.value)
                && !headers.iter().any(|header: &HeaderEntry| {
                    header.name == name.value && header.value == value.value
                })
            {
                headers.push(HeaderEntry::new(&name.value, &value.value));
            }
            offset = start + pattern.len();
        }
    }
    if lower.contains("ureq") {
        extract_literal_header_calls(&lower, literals, ".set(", &mut headers);
    }
    let mut offset = 0;
    while let Some(relative) = lower[offset..].find("curl_slist_append") {
        let start = offset + relative;
        if let Some(value) = literals
            .iter()
            .find(|literal| literal.start > start && literal.start < start + 500)
            .and_then(|literal| parse_header(&literal.value))
            && !headers
                .iter()
                .any(|header| header.name == value.name && header.value == value.value)
        {
            headers.push(value);
        }
        offset = start + "curl_slist_append".len();
    }
    if let Some(headers_offset) = lower.find("headers")
        && let Some(open_offset) = source[headers_offset..].find('{')
        && let Some(close_offset) = source[headers_offset + open_offset + 1..].find('}')
    {
        let block_start = headers_offset + open_offset + 1;
        let block_end = block_start + close_offset;
        let values = literals
            .iter()
            .filter(|literal| literal.start >= block_start && literal.start < block_end)
            .collect::<Vec<_>>();
        for pair in values.chunks_exact(2) {
            if is_probable_header_name(&pair[0].value)
                && !headers
                    .iter()
                    .any(|header| header.name == pair[0].value && header.value == pair[1].value)
            {
                headers.push(HeaderEntry::new(&pair[0].value, &pair[1].value));
            }
        }
    }
    if let Some(headers_offset) = lower.find("headers")
        && let Some(open_offset) = source[headers_offset..].find('[')
        && let Some(close_offset) = source[headers_offset + open_offset + 1..].find(']')
    {
        let block_start = headers_offset + open_offset + 1;
        let block_end = block_start + close_offset;
        let values = literals
            .iter()
            .filter(|literal| literal.start >= block_start && literal.start < block_end)
            .collect::<Vec<_>>();
        for pair in values.chunks_exact(2) {
            if is_probable_header_name(&pair[0].value)
                && !headers
                    .iter()
                    .any(|header| header.name == pair[0].value && header.value == pair[1].value)
            {
                headers.push(HeaderEntry::new(&pair[0].value, &pair[1].value));
            }
        }
    }
    headers
}

fn extract_literal_header_calls(
    lower: &str,
    literals: &[StringLiteral],
    pattern: &str,
    headers: &mut Vec<HeaderEntry>,
) {
    let mut offset = 0;
    while let Some(relative) = lower[offset..].find(pattern) {
        let start = offset + relative;
        let values = literals
            .iter()
            .filter(|literal| literal.start > start && literal.start < start + 500)
            .take(2)
            .collect::<Vec<_>>();
        if let [name, value] = values.as_slice()
            && is_probable_header_name(&name.value)
            && !headers
                .iter()
                .any(|header| header.name == name.value && header.value == value.value)
        {
            headers.push(HeaderEntry::new(&name.value, &value.value));
        }
        offset = start + pattern.len();
    }
}

fn is_probable_header_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

fn extract_code_body(
    source: &str,
    literals: &[StringLiteral],
    url: &str,
    headers: &[HeaderEntry],
) -> Option<String> {
    let lower = source.to_ascii_lowercase();
    let javascript = looks_like_javascript_request(source);
    for pattern in [
        "postfields",
        "addstringbody(",
        "stringcontent(",
        "requestbody.create(",
        "ofstring(",
        "newreader(",
        "send_string(",
        "sendstring(",
        "axios.post(",
        "axios.put(",
        "axios.patch(",
        ".setbody(",
        ".body(",
        "body:",
        "data:",
        "'body' =>",
    ] {
        let Some(offset) = lower.find(pattern) else {
            continue;
        };
        if let Some(literal) = literals.iter().find(|literal| {
            literal.start > offset
                && literal.start < offset + 1200
                && imported_code_literal(literal, javascript) != url
                && !headers
                    .iter()
                    .any(|header| literal.value == header.name || literal.value == header.value)
                && !is_method(&literal.value)
        }) {
            return Some(literal.value.clone());
        }
    }
    None
}

fn is_method(value: &str) -> bool {
    matches!(
        value.to_ascii_uppercase().as_str(),
        "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS" | "TRACE"
    )
}

fn contains_any_ignore_ascii_case(source: &str, needles: &[&str]) -> bool {
    let lower = source.to_ascii_lowercase();
    needles
        .iter()
        .any(|needle| lower.contains(&needle.to_ascii_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::RequestScripts;

    fn sample_template() -> RequestTemplate {
        RequestTemplate {
            request: RequestDraft {
                method: "POST".to_owned(),
                url: "https://api.example.com/v1/users?dry_run=true".to_owned(),
                headers: vec![
                    HeaderEntry::new("Authorization", "Bearer {{token}}"),
                    HeaderEntry {
                        enabled: false,
                        shared: true,
                        name: "X-Disabled".to_owned(),
                        value: "kept".to_owned(),
                    },
                ],
                body: "{\n  \"name\": \"Ada\",\n  \"active\": true\n}".to_owned(),
                body_mode: BodyMode::Raw,
                raw_body_language: RawBodyLanguage::Json,
                body_fields: Vec::new(),
            },
            scripts: RequestScripts {
                pre_request: "api.variables.set('nonce', '1');".to_owned(),
                post_response: "api.test('ok', () => true);".to_owned(),
            },
        }
    }

    #[test]
    fn every_export_format_round_trips_losslessly() {
        let template = sample_template();
        let expected = portable_template(&template);
        for format in InterchangeFormat::ALL {
            let source = export_request(*format, "Create user", &template)
                .unwrap_or_else(|error| panic!("{} export failed: {error}", format.label()));
            assert!(
                !source.trim().is_empty(),
                "{} produced empty output",
                format.label()
            );
            let imported = import_requests(&source).unwrap_or_else(|error| {
                panic!("{} import failed: {error}\n{source}", format.label())
            });
            assert_eq!(imported.requests.len(), 1, "{}", format.label());
            assert_eq!(
                imported.requests[0].name,
                "Create user",
                "{}",
                format.label()
            );
            assert_eq!(
                imported.requests[0].template,
                expected,
                "{}",
                format.label()
            );
        }
    }

    #[test]
    fn generated_metadata_never_contains_inactive_or_script_state() {
        let template = sample_template();
        let source = export_request(InterchangeFormat::Curl, "Create user", &template).unwrap();
        let imported = import_requests(&source).unwrap();
        let imported = &imported.requests[0].template;
        assert_eq!(imported.request.headers.len(), 1);
        assert_eq!(imported.request.headers[0].name, "Authorization");
        assert!(imported.scripts.is_empty());
        assert!(!source.contains("X-Disabled"));
        assert!(!source.contains("api.test"));
    }

    #[test]
    fn multipart_round_trips_in_every_generated_representation() {
        let mut request = RequestDraft::new("POST", "https://upload.example.com/files");
        request.body_mode = BodyMode::MultipartFormData;
        request.body_fields = vec![
            BodyField::text("caption", "summer"),
            BodyField::file("asset", "/tmp/photo.jpg"),
        ];
        let template = RequestTemplate::new(request);
        for format in InterchangeFormat::ALL {
            let source = export_request(*format, "Upload", &template).unwrap();
            let imported = import_requests(&source).unwrap();
            assert_eq!(
                imported.requests[0].template,
                template,
                "{}",
                format.label()
            );
        }
    }

    #[test]
    fn imports_external_curl_wget_and_powershell_commands() {
        let curl = import_requests(
            r#"curl -X POST 'https://api.example.com/items' -H 'Content-Type: application/json' -H 'X-Trace: one' --data-raw '{"name":"Ada"}'"#,
        )
        .unwrap();
        let curl_request = &curl.requests[0].template.request;
        assert_eq!(curl_request.method, "POST");
        assert_eq!(curl_request.body, r#"{"name":"Ada"}"#);
        assert_eq!(curl_request.raw_body_language, RawBodyLanguage::Json);
        assert_eq!(curl_request.headers.len(), 2);

        let wget = import_requests(
            "wget --method=PATCH --header='X-Token: abc' --body-data='enabled=true' https://api.example.com/items/1",
        )
        .unwrap();
        assert_eq!(wget.requests[0].template.request.method, "PATCH");
        assert_eq!(wget.requests[0].template.request.body, "enabled=true");

        let powershell = import_requests(
            "Invoke-GetRequest -Uri 'https://api.example.com/items/1' -Method 'DELETE' -Headers @{ 'X-Token' = 'abc' }",
        )
        .unwrap();
        assert_eq!(powershell.requests[0].template.request.method, "DELETE");
        assert_eq!(
            powershell.requests[0].template.request.headers[0].name,
            "X-Token"
        );
    }

    #[test]
    fn imports_curl_form_and_multipart_fields() {
        let form = import_requests(
            "curl https://example.com/login --data-urlencode 'email=a@example.com' --data-urlencode 'code=1 2'",
        )
        .unwrap();
        let request = &form.requests[0].template.request;
        assert_eq!(request.body_mode, BodyMode::FormUrlEncoded);
        assert_eq!(request.body_fields.len(), 2);

        let multipart = import_requests(
            "curl -X POST https://example.com/upload -F 'caption=summer' -F 'file=@/tmp/a.png'",
        )
        .unwrap();
        let request = &multipart.requests[0].template.request;
        assert_eq!(request.body_mode, BodyMode::MultipartFormData);
        assert_eq!(request.body_fields[1].kind, BodyFieldKind::File);
        assert_eq!(request.body_fields[1].value, "/tmp/a.png");
    }

    #[test]
    fn imports_multiple_intellij_http_requests() {
        let bundle = import_requests(
            r#"
### List users
GET https://api.example.com/users
Accept: application/json

### Create user
POST https://api.example.com/users
Content-Type: application/json

{"name":"Ada"}
"#,
        )
        .unwrap();
        assert_eq!(bundle.source_format, "IntelliJ HTTP Client");
        assert_eq!(bundle.requests.len(), 2);
        assert_eq!(bundle.requests[0].name, "List users");
        assert_eq!(bundle.requests[1].template.request.method, "POST");
        assert_eq!(
            bundle.requests[1].template.request.raw_body_language,
            RawBodyLanguage::Json
        );
    }

    #[test]
    fn imports_all_openapi_operations_with_parameters_and_examples() {
        let bundle = import_requests(
            r#"
openapi: 3.1.0
info:
  title: Users
  version: 1.0.0
servers:
  - url: https://api.example.com
paths:
  /users/{userId}:
    get:
      operationId: listUsers
      parameters:
        - name: limit
          in: query
          schema:
            type: integer
            default: 25
    post:
      summary: Create user
      parameters:
        - name: X-Trace
          in: header
          example: trace-1
      requestBody:
        content:
          application/json:
            example:
              name: Ada
      responses:
        default:
          description: ok
"#,
        )
        .unwrap();
        assert_eq!(bundle.requests.len(), 2);
        assert_eq!(
            bundle.requests[0].template.request.url,
            "https://api.example.com/users/{{userId}}?limit=25"
        );
        assert_eq!(bundle.requests[1].name, "Create user");
        assert_eq!(
            bundle.requests[1].template.request.body,
            "{\n  \"name\": \"Ada\"\n}"
        );
        assert_eq!(
            bundle.requests[1].template.request.headers[0].name,
            "X-Trace"
        );
    }

    #[test]
    fn imports_local_openapi_component_references() {
        let bundle = import_requests(
            r#"
openapi: 3.1.0
info: { title: Users, version: 1.0.0 }
servers:
  - url: https://api.example.com
paths:
  /users:
    post:
      parameters:
        - $ref: '#/components/parameters/Trace'
      requestBody:
        $ref: '#/components/requestBodies/User'
      responses:
        default: { description: ok }
components:
  parameters:
    Trace:
      name: X-Trace
      in: header
      example: trace-1
  requestBodies:
    User:
      content:
        application/json:
          example: { name: Ada }
"#,
        )
        .unwrap();
        let request = &bundle.requests[0].template.request;
        assert_eq!(request.headers[0], HeaderEntry::new("X-Trace", "trace-1"));
        assert_eq!(request.body, "{\n  \"name\": \"Ada\"\n}");
    }

    #[test]
    fn imports_external_asyncapi_http_channel() {
        let bundle = import_requests(
            r#"
asyncapi: 3.0.0
info:
  title: Webhook
  version: 1.0.0
servers:
  production:
    host: hooks.example.com
    protocol: https
channels:
  receiveEvent:
    address: /events
    messages:
      event:
        $ref: '#/components/messages/Event'
components:
  messages:
    Event:
      bindings:
        http:
          headers:
            type: object
            properties:
              X-Event-Key:
                type: string
                example: event-key
      payload:
        type: object
      examples:
        - payload:
            type: created
operations:
  sendEvent:
    action: send
    channel:
      $ref: '#/channels/receiveEvent'
    bindings:
      http:
        method: POST
        query:
          type: object
          properties:
            source:
              type: string
              default: webhook
        bindingVersion: '0.3.0'
"#,
        )
        .unwrap();
        let request = &bundle.requests[0].template.request;
        assert_eq!(
            request.url,
            "https://hooks.example.com/events?source=webhook"
        );
        assert_eq!(request.method, "POST");
        assert_eq!(request.body, "{\n  \"type\": \"created\"\n}");
        assert_eq!(
            request.headers[0],
            HeaderEntry::new("X-Event-Key", "event-key")
        );
    }

    #[test]
    fn imports_static_framework_code_without_running_it() {
        let samples = [
            r#"fetch("https://api.example.com/users", { method: "POST", headers: { "X-Key": "one" }, body: "{}" });"#,
            r#"axios.post("https://api.example.com/users", "{}", { headers: { "X-Key": "one" } });"#,
            r#"$.ajax({ url: "https://api.example.com/users", type: "POST", headers: { "X-Key": "one" }, data: "{}" });"#,
            r#"HttpRequest.newBuilder().uri(URI.create("https://api.example.com/users")).header("X-Key", "one").POST(HttpRequest.BodyPublishers.ofString("{}"));"#,
            r#"new Request.Builder().url("https://api.example.com/users").addHeader("X-Key", "one").post(RequestBody.create("{}", JSON)).build();"#,
            r#"req, _ := http.NewRequest("PUT", "https://api.example.com/users/1", strings.NewReader("{}")); req.Header.Add("X-Key", "one")"#,
            r#"resty.New().R().SetHeader("X-Key", "one").SetBody("{}").Post("https://api.example.com/users")"#,
            r#"new HttpRequestMessage(HttpMethod.Post, "https://api.example.com/users") { Content = new StringContent("{}") };"#,
            r#"new RestRequest("https://api.example.com/users", Method.Post).AddHeader("X-Key", "one").AddStringBody("{}", ContentType.Json);"#,
            r#"let request = client.request(Method::PATCH, "https://api.example.com/users/1").header("X-Key", "one").body("{}");"#,
            r#"ureq::post("https://api.example.com/users").set("X-Key", "one").send_string("{}");"#,
            r#"auto host = "api.example.com"; auto target = "/users"; http::request<http::string_body> req{http::verb::post, target, 11}; req.body() = "{}";"#,
            r#"curl_easy_setopt(curl, CURLOPT_URL, "https://api.example.com/users"); curl_easy_setopt(curl, CURLOPT_CUSTOMREQUEST, "DELETE");"#,
            r#"curl_setopt($curl, CURLOPT_URL, 'https://api.example.com/users'); curl_setopt($curl, CURLOPT_CUSTOMREQUEST, 'POST'); curl_setopt($curl, CURLOPT_POSTFIELDS, '{}');"#,
            r#"$client->request('POST', 'https://api.example.com/users', ['body' => '{}']);"#,
            r#"client.request("https://api.example.com/users") { method = HttpMethod.Post; setBody("{}") }"#,
            r#"Request.Builder().url("https://api.example.com/users").post("{}".toRequestBody()).build()"#,
            r#"HttpRequest.newBuilder().uri(URI.create("https://api.example.com/users")).POST(HttpRequest.BodyPublishers.ofString("{}"))"#,
        ];
        for source in samples {
            let imported = import_requests(source)
                .unwrap_or_else(|error| panic!("failed to import {source}: {error}"));
            let url = &imported.requests[0].template.request.url;
            assert!(url.starts_with("https://") || url.starts_with("http://"));
            assert_ne!(imported.requests[0].template.request.method, "GET");
        }
    }

    #[test]
    fn imports_javascript_dynamic_urls_as_request_variables() {
        let fetch = import_requests(
            r#"fetch(`${baseUrl}/users/${userId}?expand=${includeDetails}`, { method: "PATCH", body: "{}" });"#,
        )
        .unwrap();
        let request = &fetch.requests[0].template.request;
        assert_eq!(
            request.url,
            "{{baseUrl}}/users/{{userId}}?expand={{includeDetails}}"
        );
        assert_eq!(request.method, "PATCH");
        assert_eq!(request.body, "{}");

        let alias = import_requests(
            r#"const endpoint = `${apiBase}/v1/items/${itemId}`;
axios.get(endpoint);"#,
        )
        .unwrap();
        assert_eq!(
            alias.requests[0].template.request.url,
            "{{apiBase}}/v1/items/{{itemId}}"
        );

        let config = import_requests(
            r#"axios({ method: "GET", url: `https://${tenant}.example.com/items/${itemId}` });"#,
        )
        .unwrap();
        assert_eq!(
            config.requests[0].template.request.url,
            "https://{{tenant}}.example.com/items/{{itemId}}"
        );

        let bare = import_requests("fetch(config.apiUrl, options)").unwrap();
        assert_eq!(bare.requests[0].template.request.url, "{{config.apiUrl}}");
    }

    #[test]
    fn rejects_executable_url_expressions_or_oversized_imports() {
        assert!(matches!(
            import_requests("fetch(buildUrl(), options)"),
            Err(InterchangeError::Unsupported(_))
        ));
        let oversized = "x".repeat(MAX_INTERCHANGE_BYTES + 1);
        assert_eq!(import_requests(&oversized), Err(InterchangeError::TooLarge));
    }

    #[test]
    fn escaped_non_ascii_characters_do_not_panic_or_corrupt_the_literal() {
        // `\é` used to step one byte past the leading byte of a multi-byte
        // character, panicking on the next non-char-boundary slice (and pushing
        // mojibake before that). The full character must be preserved instead,
        // with the backslash dropped like any other non-special escape.
        let literals = string_literals(r#"curl "https://e.test/\é?x=\😀" -H 'a=\中'"#);
        assert_eq!(literals.len(), 2);
        assert_eq!(literals[0].value, "https://e.test/é?x=😀");
        assert_eq!(literals[1].value, "a=中");
    }

    #[test]
    fn generated_java_files_are_complete_and_restsharp_never_changes_the_method() {
        let template = sample_template();
        for format in [
            InterchangeFormat::JavaHttpClient,
            InterchangeFormat::JavaOkHttp,
        ] {
            let source = export_request(format, "Create user", &template).unwrap();
            assert!(source.contains("class Main"));
            assert!(source.contains("public static void main"));
        }

        let mut custom = template;
        custom.request.method = "TRACE".to_owned();
        assert!(matches!(
            export_request(InterchangeFormat::CSharpRestSharp, "Trace", &custom),
            Err(InterchangeError::Export(_))
        ));
    }

    #[test]
    fn exported_specs_keep_standard_documents_alongside_lossless_extension() {
        let template = sample_template();
        let openapi = export_request(InterchangeFormat::OpenApi, "Create user", &template).unwrap();
        assert!(openapi.contains("openapi: 3.1.0"));
        assert!(openapi.contains("paths:"));
        assert!(openapi.contains("x-resolved-request:"));
        let document: Value = serde_yaml::from_str(&openapi).unwrap();
        let paths = document.get("paths").and_then(Value::as_object).unwrap();
        assert!(paths.contains_key("/v1/users"));
        let parameters = paths["/v1/users"]["post"]["parameters"].as_array().unwrap();
        assert!(parameters.iter().any(|parameter| {
            parameter.get("name").and_then(Value::as_str) == Some("dry_run")
                && parameter.get("in").and_then(Value::as_str) == Some("query")
        }));

        let asyncapi =
            export_request(InterchangeFormat::AsyncApi, "Create user", &template).unwrap();
        assert!(asyncapi.contains("asyncapi: 3.0.0"));
        assert!(asyncapi.contains("channels:"));
        assert!(asyncapi.contains("x-resolved-requests:"));
        let document: Value = serde_yaml::from_str(&asyncapi).unwrap();
        let operation = document["operations"]
            .as_object()
            .and_then(|operations| operations.values().next())
            .unwrap();
        assert_eq!(operation["bindings"]["http"]["method"], "POST");
        assert_eq!(operation["bindings"]["http"]["bindingVersion"], "0.3.0");
    }
}
