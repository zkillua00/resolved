//! Local, authenticated HTTP/1 preview bridge. There is deliberately no browser
//! CORS support or implicit routing. One exchange per connection bounds lifetime
//! and prevents rejected request bodies from becoming a subsequent request.
//!
//! Preview limits: 32 connections, 32 KiB/96 headers, 8 MiB request bodies,
//! 32 MiB response bodies, 10s headers, 30s body, 120s coordinator reply, and
//! 180s total connection lifetime (including slow response readers).
use std::{convert::Infallible, net::Ipv4Addr, sync::Arc, time::Duration};

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{
    Request, Response, StatusCode, body::Incoming, server::conn::http1, service::service_fn,
};
use hyper_util::rt::{TokioIo, TokioTimer};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use tokio::{
    net::TcpListener,
    sync::{mpsc, oneshot},
    task::{JoinHandle, JoinSet},
    time::timeout,
};

pub(crate) const MAX_REQUEST_BODY_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MAX_RESPONSE_BODY_BYTES: usize = 32 * 1024 * 1024;
const TOKEN: &str = "x-resolved-gateway-token";
const TARGET: &str = "x-resolved-target";

pub(crate) struct GatewayRequest {
    pub method: String,
    /// Absolute target URL, with the incoming origin-form path/query unchanged.
    pub target: String,
    pub headers: HeaderMap,
    pub body: Bytes,
}

pub(crate) struct GatewayReply {
    pub status: u16,
    pub headers: Vec<(String, Vec<u8>)>,
    pub body: crate::core::ResponseBody,
}

impl GatewayReply {
    /// Safe coordinator rejections use fixed codes, never raw execution errors.
    pub(crate) fn rejected(status: u16, code: &'static str) -> Self {
        let body = serde_json::json!({
            "status": status,
            "code": code,
            "id": uuid::Uuid::new_v4().to_string(),
        });
        Self {
            status,
            headers: vec![("content-type".into(), b"application/json".to_vec())],
            body: body.to_string().into_bytes().into(),
        }
    }
}

pub(crate) struct GatewayExchange {
    pub request: GatewayRequest,
    /// Check closure before dispatch. Already-dispatched work may finish under
    /// a hard deadline so its actual outcome can still be recorded in history.
    pub reply: oneshot::Sender<Result<GatewayReply, String>>,
}

pub(crate) struct GatewayServer {
    port: u16,
    task: JoinHandle<()>,
}

impl GatewayServer {
    pub(crate) async fn start(
        port: u16,
        token: String,
        sender: mpsc::Sender<GatewayExchange>,
    ) -> Result<Self, String> {
        Self::bind(port, token, sender).await
    }

    pub(crate) async fn bind(
        port: u16,
        token: String,
        sender: mpsc::Sender<GatewayExchange>,
    ) -> Result<Self, String> {
        if token.is_empty() || HeaderValue::from_str(&token).is_err() {
            return Err("invalid gateway token".into());
        }
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))
            .await
            .map_err(|_| "could not bind gateway listener")?;
        let port = listener
            .local_addr()
            .map_err(|_| "could not inspect gateway listener")?
            .port();
        let token = Arc::new(token);
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    Some(_) = connections.join_next(), if !connections.is_empty() => {}
                    accepted = listener.accept(), if connections.len() < 32 => {
                        let Ok((socket, _)) = accepted else { break };
                        let sender = sender.clone();
                        let token = token.clone();
                        connections.spawn(async move {
                            let service = service_fn(move |request| {
                                let sender = sender.clone();
                                let token = token.clone();
                                async move {
                                    Ok::<_, Infallible>(handle(request, &token, port, &sender).await)
                                }
                            });
                            let mut builder = http1::Builder::new();
                            builder.keep_alive(false).max_headers(96).max_buf_size(32 * 1024)
                                .timer(TokioTimer::new())
                                .header_read_timeout(Duration::from_secs(10));
                            let _ = timeout(Duration::from_secs(180),
                                builder.serve_connection(TokioIo::new(socket), service)).await;
                        });
                    }
                }
            }
            // Dropping JoinSet aborts all connections and their pending replies.
        });
        Ok(Self { port, task })
    }

    pub(crate) fn port(&self) -> u16 {
        self.port
    }

    pub(crate) fn stop(&self) {
        self.task.abort();
    }
}

impl Drop for GatewayServer {
    fn drop(&mut self) {
        self.stop();
    }
}

type Reply = Response<Full<Bytes>>;

fn error(status: u16, code: &'static str) -> Reply {
    let body = serde_json::json!({
        "status": status, "code": code, "id": uuid::Uuid::new_v4().to_string()
    })
    .to_string();
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .header("cache-control", "no-store")
        .header("connection", "close")
        .body(Full::new(Bytes::from(body)))
        .expect("constant error response")
}

fn unique<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a HeaderValue> {
    let mut values = headers.get_all(name).iter();
    let first = values.next()?;
    values.next().is_none().then_some(first)
}

fn strip_headers(headers: &mut HeaderMap) {
    let nominated: Vec<HeaderName> = headers
        .get_all("connection")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(|name| HeaderName::from_bytes(name.trim().as_bytes()).ok())
        .collect();
    for name in nominated {
        headers.remove(name);
    }
    let controls: Vec<_> = headers
        .keys()
        .filter(|name| name.as_str().starts_with("x-resolved-"))
        .cloned()
        .collect();
    for name in controls {
        headers.remove(name);
    }
    for name in [
        "connection",
        "proxy-connection",
        "keep-alive",
        "transfer-encoding",
        "te",
        "trailer",
        "upgrade",
        "proxy-authorization",
        "proxy-authenticate",
        "content-length",
        "host",
        "expect",
    ] {
        headers.remove(name);
    }
}

fn target(origin: &str, path: &str, port: u16) -> Option<String> {
    let authority = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))?;
    if authority.is_empty()
        || authority.contains(['/', '?', '#', '@', '\\'])
        || authority.chars().any(|c| c.is_whitespace())
    {
        return None;
    }
    let _: hyper::http::uri::Authority = authority.parse().ok()?;
    let url = url::Url::parse(origin).ok()?;
    // Conservatively exclude this port on every host, including aliases and
    // DNS rebinding. Preview does not support upstreams on the listener port.
    if url.port_or_known_default()? == port {
        return None;
    }
    Some(format!("{origin}{path}"))
}

async fn handle(
    request: Request<Incoming>,
    token: &str,
    port: u16,
    sender: &mpsc::Sender<GatewayExchange>,
) -> Reply {
    let (parts, body) = request.into_parts();
    // Authenticate before stripping Connection-nominated headers.
    if !unique(&parts.headers, TOKEN).is_some_and(|value| value.as_bytes() == token.as_bytes()) {
        return error(401, "gateway_unauthorized");
    }
    if parts.headers.contains_key("origin") {
        return error(403, "gateway_browser_forbidden");
    }
    if parts.headers.contains_key("x-resolved-workspace")
        || parts.headers.contains_key("x-resolved-environment")
    {
        return error(400, "gateway_scope_unsupported");
    }
    if parts.method == hyper::Method::CONNECT
        || parts.headers.contains_key("upgrade")
        || parts.headers.get_all("connection").iter().any(|value| {
            value.to_str().is_ok_and(|value| {
                value
                    .split(',')
                    .any(|part| part.trim().eq_ignore_ascii_case("upgrade"))
            })
        })
    {
        return error(400, "gateway_protocol_unsupported");
    }
    // Reject Expect without polling Incoming: Hyper must not emit 100 Continue
    // before authentication, validation, and the size checks.
    if parts.headers.contains_key("expect") {
        return error(417, "gateway_expect_unsupported");
    }
    if parts.uri.scheme().is_some() || parts.uri.authority().is_some() {
        return error(400, "gateway_invalid_path");
    }
    let Some(path) = parts
        .uri
        .path_and_query()
        .map(|p| p.as_str())
        .filter(|p| p.starts_with('/'))
    else {
        return error(400, "gateway_invalid_path");
    };
    let Some(target) = unique(&parts.headers, TARGET)
        .and_then(|v| v.to_str().ok())
        .and_then(|origin| target(origin, path, port))
    else {
        return error(400, "gateway_invalid_target");
    };
    if parts
        .headers
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .is_some_and(|n| n > MAX_REQUEST_BODY_BYTES as u64)
    {
        return error(413, "gateway_body_too_large");
    }
    let body = match timeout(
        Duration::from_secs(30),
        Limited::new(body, MAX_REQUEST_BODY_BYTES).collect(),
    )
    .await
    {
        Ok(Ok(body)) => body.to_bytes(),
        Ok(Err(error_value)) => {
            return if error_value.is::<http_body_util::LengthLimitError>() {
                error(413, "gateway_body_too_large")
            } else {
                error(400, "gateway_invalid_body")
            };
        }
        Err(_) => return error(408, "gateway_body_timeout"),
    };
    let is_head = parts.method == hyper::Method::HEAD;
    let mut headers = parts.headers;
    strip_headers(&mut headers);
    let (reply, receive) = oneshot::channel();
    let exchange = GatewayExchange {
        request: GatewayRequest {
            method: parts.method.to_string(),
            target,
            headers,
            body,
        },
        reply,
    };
    if let Err(failure) = sender.try_send(exchange) {
        return match failure {
            mpsc::error::TrySendError::Full(_) => error(429, "gateway_busy"),
            mpsc::error::TrySendError::Closed(_) => error(503, "gateway_unavailable"),
        };
    }
    match timeout(Duration::from_secs(120), receive).await {
        Ok(Ok(Ok(reply))) => normalize_reply(reply, is_head),
        Ok(_) => error(502, "gateway_execution_failed"),
        Err(_) => error(504, "gateway_upstream_timeout"),
    }
}

fn normalize_reply(reply: GatewayReply, is_head: bool) -> Reply {
    let Ok(status) = StatusCode::from_u16(reply.status) else {
        return error(502, "gateway_invalid_reply");
    };
    if status.is_informational() {
        return error(502, "gateway_invalid_reply");
    }
    if reply.body.len() > MAX_RESPONSE_BODY_BYTES {
        return error(502, "gateway_reply_too_large");
    }
    let mut headers = HeaderMap::new();
    let mut header_bytes = 0usize;
    for (name, value) in reply.headers {
        header_bytes = header_bytes
            .saturating_add(name.len())
            .saturating_add(value.len());
        if headers.len() >= 96 || header_bytes > 32 * 1024 {
            return error(502, "gateway_invalid_reply");
        }
        let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_bytes(&value),
        ) else {
            return error(502, "gateway_invalid_reply");
        };
        headers.append(name, value);
    }
    strip_headers(&mut headers);
    headers.insert("connection", HeaderValue::from_static("close"));
    let body = if is_head
        || status == StatusCode::NO_CONTENT
        || status == StatusCode::NOT_MODIFIED
        || status == StatusCode::RESET_CONTENT
    {
        Bytes::new()
    } else {
        Bytes::from_owner(reply.body)
    };
    let mut response = Response::new(Full::new(body));
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpStream,
    };

    async fn server() -> (GatewayServer, mpsc::Receiver<GatewayExchange>) {
        let (send, receive) = mpsc::channel(8);
        (
            GatewayServer::start(0, "test-secret".into(), send)
                .await
                .unwrap(),
            receive,
        )
    }

    async fn raw(port: u16, bytes: &[u8]) -> Vec<u8> {
        let mut socket = TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .await
            .unwrap();
        socket.write_all(bytes).await.unwrap();
        let mut output = Vec::new();
        timeout(Duration::from_secs(5), socket.read_to_end(&mut output))
            .await
            .unwrap()
            .unwrap();
        output
    }

    fn request(extra: &str) -> String {
        format!(
            "GET /raw%2Fpath?q=%ff HTTP/1.1\r\nHost: ignored\r\nX-Resolved-Gateway-Token: test-secret\r\n{extra}\r\n"
        )
    }

    #[tokio::test]
    async fn rejected_requests_never_execute() {
        let (server, mut receive) = server().await;
        let cases = [
            ("GET / HTTP/1.1\r\nHost: localhost\r\n\r\n".into(), 401),
            (
                request(
                    "X-Resolved-Gateway-Token: test-secret\r\nX-Resolved-Target: https://example.com\r\n",
                ),
                401,
            ),
            (
                request(
                    "X-Resolved-Target: https://example.com\r\nX-Resolved-Target: https://example.com\r\n",
                ),
                400,
            ),
            (
                request("X-Resolved-Target: https://user@example.com\r\n"),
                400,
            ),
            (
                request("X-Resolved-Target: https://example.com/path\r\n"),
                400,
            ),
            (
                request("X-Resolved-Target: https://example.com?query\r\n"),
                400,
            ),
            (
                request("X-Resolved-Target: https://example.com#fragment\r\n"),
                400,
            ),
            (
                request(&format!(
                    "X-Resolved-Target: http://alias.resolved:{}\r\n",
                    server.port()
                )),
                400,
            ),
            (
                request(
                    "X-Resolved-Target: https://example.com\r\nOrigin: https://browser.example\r\n",
                ),
                403,
            ),
            (
                request("X-Resolved-Target: https://example.com\r\nOrigin: null\r\n")
                    .replacen("GET ", "OPTIONS ", 1),
                403,
            ),
            (
                request(
                    "X-Resolved-Target: https://example.com\r\nX-Resolved-Environment: production\r\n",
                ),
                400,
            ),
            (
                request("X-Resolved-Target: https://example.com\r\nUpgrade: websocket\r\n"),
                400,
            ),
            (
                request("X-Resolved-Target: https://example.com\r\nExpect: 100-continue\r\n"),
                417,
            ),
            (
                request("X-Resolved-Target: https://example.com\r\nContent-Length: 8388609\r\n"),
                413,
            ),
        ];
        for (input, status) in cases {
            let output = raw(server.port(), input.as_bytes()).await;
            let output = String::from_utf8(output).unwrap();
            assert!(
                output.starts_with(&format!("HTTP/1.1 {status} ")),
                "{output}"
            );
            assert!(!output.contains("100 Continue"));
            assert!(!output.contains("test-secret"));
            assert!(receive.try_recv().is_err());
        }
    }

    #[tokio::test]
    async fn binary_contract_and_header_sanitization() {
        let (server, mut receive) = server().await;
        let port = server.port();
        let client = tokio::spawn(async move {
            raw(port, b"POST /raw%2Fpath?q=%ff HTTP/1.1\r\nHost: ignored\r\nX-Resolved-Gateway-Token: test-secret\r\nX-Resolved-Target: https://example.com\r\nConnection: X-Remove, X-Resolved-Gateway-Token\r\nX-Remove: secret\r\nX-Resolved-Other: secret\r\nX-Keep: a\r\nX-Keep: b\r\nContent-Length: 3\r\n\r\n\x00\xff\x01").await
        });
        let exchange = receive.recv().await.unwrap();
        assert_eq!(exchange.request.method, "POST");
        assert_eq!(
            exchange.request.target,
            "https://example.com/raw%2Fpath?q=%ff"
        );
        assert_eq!(&exchange.request.body[..], b"\x00\xff\x01");
        assert_eq!(exchange.request.headers.get_all("x-keep").iter().count(), 2);
        for name in [
            TOKEN,
            TARGET,
            "x-remove",
            "host",
            "content-length",
            "connection",
            "x-resolved-other",
        ] {
            assert!(!exchange.request.headers.contains_key(name), "{name}");
        }
        assert!(
            exchange
                .reply
                .send(Ok(GatewayReply {
                    status: 201,
                    headers: vec![
                        ("set-cookie".into(), b"a=1".to_vec()),
                        ("set-cookie".into(), b"b=2".to_vec()),
                        ("connection".into(), b"x-private".to_vec()),
                        ("x-private".into(), b"hidden".to_vec()),
                        ("content-length".into(), b"999".to_vec()),
                        ("x-resolved-token".into(), b"hidden".to_vec()),
                    ],
                    body: vec![0, 255, 2].into(),
                }))
                .is_ok()
        );
        let response = client.await.unwrap();
        assert!(response.starts_with(b"HTTP/1.1 201 "));
        assert!(response.ends_with(b"\x00\xff\x02"));
        let header_end = response.windows(4).position(|v| v == b"\r\n\r\n").unwrap();
        let headers = std::str::from_utf8(&response[..header_end]).unwrap();
        assert_eq!(headers.matches("set-cookie:").count(), 2);
        assert!(!headers.contains("hidden"));
        assert!(headers.contains("content-length: 3"));
    }

    #[tokio::test]
    async fn disconnect_and_stop_cancel_pending_work() {
        let (server, mut receive) = server().await;
        let mut socket = TcpStream::connect((Ipv4Addr::LOCALHOST, server.port()))
            .await
            .unwrap();
        socket
            .write_all(request("X-Resolved-Target: https://example.com\r\n").as_bytes())
            .await
            .unwrap();
        let mut exchange = receive.recv().await.unwrap();
        drop(socket);
        timeout(Duration::from_secs(2), exchange.reply.closed())
            .await
            .expect("disconnect cancels");
        let mut socket = TcpStream::connect((Ipv4Addr::LOCALHOST, server.port()))
            .await
            .unwrap();
        socket
            .write_all(request("X-Resolved-Target: https://example.com\r\n").as_bytes())
            .await
            .unwrap();
        let mut exchange = receive.recv().await.unwrap();
        let port = server.port();
        drop(server);
        timeout(Duration::from_secs(2), exchange.reply.closed())
            .await
            .expect("stop cancels");
        assert!(
            TcpStream::connect((Ipv4Addr::LOCALHOST, port))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn full_queue_and_failed_execution_have_stable_errors() {
        let (sender, mut receive) = mpsc::channel(1);
        let server = GatewayServer::bind(0, "test-secret".into(), sender)
            .await
            .unwrap();
        let port = server.port();
        let first = tokio::spawn(async move {
            raw(
                port,
                request("X-Resolved-Target: https://example.com\r\n").as_bytes(),
            )
            .await
        });
        timeout(Duration::from_secs(2), async {
            while receive.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let rejected = raw(
            port,
            request("X-Resolved-Target: https://example.com\r\n").as_bytes(),
        )
        .await;
        assert!(rejected.starts_with(b"HTTP/1.1 429 "));
        let exchange = receive.recv().await.unwrap();
        assert!(
            exchange
                .reply
                .send(Err("secret-bearing error".into()))
                .is_ok()
        );
        let failed = String::from_utf8(first.await.unwrap()).unwrap();
        assert!(failed.starts_with("HTTP/1.1 502 "));
        assert!(failed.contains("gateway_execution_failed"));
        assert!(!failed.contains("secret-bearing"));
    }

    #[tokio::test]
    async fn no_body_statuses_and_head() {
        for (status, head) in [(200, true), (204, false), (304, false)] {
            let response = normalize_reply(
                GatewayReply {
                    status,
                    headers: vec![],
                    body: vec![1, 2, 3].into(),
                },
                head,
            );
            assert!(
                response
                    .into_body()
                    .collect()
                    .await
                    .unwrap()
                    .to_bytes()
                    .is_empty()
            );
        }
    }
}
