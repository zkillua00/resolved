#![allow(clippy::result_large_err)]

//! Sandboxed pre-request and post-response JavaScript execution.
//!
//! This module deliberately exposes a small capability surface instead of a
//! browser or Node.js environment. Each invocation creates and destroys its own
//! QuickJS runtime so globals and prototype mutations cannot leak between
//! requests.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use rquickjs::{
    CatchResultExt as _, CaughtError, Context as JsContext, Exception, Runtime as JsRuntime, Value,
    context::EvalOptions,
};
use serde::{Deserialize, Serialize};

use super::{
    RequestDraft, ResponseData,
    chain::ChainRun,
    request_namespace::{RequestNamespaceCatalog, RuntimeNamespaceSpec},
    template::{REDACTED_VALUE, secret_variants},
};

/// Runs a batch of awaited chained requests' pipelines (their own pre/post
/// scripts, HTTP exchanges, and nested chains) concurrently and returns each
/// resulting run so its environment mutations can be applied live. The
/// provider owns the async execution (typically driving all of them on one
/// shared Tokio runtime so their network calls genuinely overlap) and is
/// responsible for actually performing the (non-blocking) execution.
pub trait InlineChainer: Send + Sync {
    fn run(&self, requested: &[ChainedRequest]) -> Vec<Result<ChainRun, String>>;
}

impl<F> InlineChainer for F
where
    F: Fn(&[ChainedRequest]) -> Vec<Result<ChainRun, String>> + Send + Sync,
{
    fn run(&self, requested: &[ChainedRequest]) -> Vec<Result<ChainRun, String>> {
        self(requested)
    }
}

pub const SCRIPT_MEMORY_LIMIT_BYTES: usize = 32 * 1024 * 1024;
pub const SCRIPT_STACK_LIMIT_BYTES: usize = 256 * 1024;
pub const SCRIPT_TIMEOUT: Duration = Duration::from_secs(1);
pub const MAX_SCRIPT_SOURCE_BYTES: usize = 256 * 1024;
pub const MAX_SCRIPT_BODY_BYTES: usize = 5 * 1024 * 1024;
pub const MAX_SCRIPT_LOG_ENTRIES: usize = 100;
pub const MAX_SCRIPT_LOG_BYTES: usize = 64 * 1024;
pub const MAX_SCRIPT_RESULT_BYTES: usize = 8 * 1024 * 1024;

const PRELUDE: &str = r#"
(() => {
  "use strict";

  const input = globalThis.__API_TESTER_INPUT;
  const mutableRequest = input.phase === "pre";
  const environmentValues = Object.assign(Object.create(null), input.environment);
  const collectionValues = Object.assign(Object.create(null), input.collectionVariables);
  const environmentMutations = [];
  const logs = [];
  const tests = [];
  let logCharacters = 0;

  // Pending `api.requests.execute(...)` operations. When a script *awaits* one,
  // the native engine runs the referenced saved request's full pipeline inline
  // and calls resolve(); when it is never awaited (a plain `execute()`), it is
  // carried into `scheduledRequests` at finish so the request still runs after
  // the phase. The engine drives this via the bridge below.
  const nativeOps = [];
  globalThis.__API_TESTER_NATIVE_OPS = nativeOps;
  // Applies environment mutations produced by an inline chained request into
  // the live environment *and* records them so they reach the workspace too.
  globalThis.__API_TESTER_APPLY_CHAIN = function (mutations) {
    const list = Array.isArray(mutations) ? mutations : [];
    for (const mutation of list) {
      if (mutation && mutation.op === "set") {
        environmentValues[mutation.key] = String(mutation.value);
        environmentMutations.push({ op: "set", key: mutation.key, value: String(mutation.value) });
      } else if (mutation && mutation.op === "unset") {
        delete environmentValues[mutation.key];
        environmentMutations.push({ op: "unset", key: mutation.key });
      }
    }
    return list.length;
  };


  const own = (object, key) => Object.prototype.hasOwnProperty.call(object, key);
  const normalizeName = value => String(value).trim().toLowerCase();

  function makeHeaders(source, mutable) {
    const rows = source.map(header => ({
      enabled: header.enabled !== false,
      shared: header.shared !== false,
      name: String(header.name ?? ""),
      value: String(header.value ?? ""),
    }));

    const ensureMutable = () => {
      if (!mutable) {
        throw new TypeError("headers are read-only in post-response scripts");
      }
    };

    const bag = {
      has(name) {
        const normalized = normalizeName(name);
        return rows.some(row => row.enabled && normalizeName(row.name) === normalized);
      },
      get(name) {
        const normalized = normalizeName(name);
        const row = rows.find(item => item.enabled && normalizeName(item.name) === normalized);
        return row ? row.value : undefined;
      },
      getAll(name) {
        const normalized = normalizeName(name);
        return rows
          .filter(row => row.enabled && normalizeName(row.name) === normalized)
          .map(row => row.value);
      },
      set(name, value) {
        ensureMutable();
        const headerName = String(name).trim();
        if (!headerName) throw new TypeError("header name cannot be empty");
        const normalized = normalizeName(headerName);
        let first = -1;
        for (let index = 0; index < rows.length; index += 1) {
          if (rows[index].enabled && normalizeName(rows[index].name) === normalized) {
            if (first === -1) {
              first = index;
              rows[index].name = headerName;
              rows[index].value = String(value);
            } else {
              rows.splice(index, 1);
              index -= 1;
            }
          }
        }
        if (first === -1) {
          rows.push({ enabled: true, shared: true, name: headerName, value: String(value) });
        }
      },
      append(name, value) {
        ensureMutable();
        const headerName = String(name).trim();
        if (!headerName) throw new TypeError("header name cannot be empty");
        rows.push({ enabled: true, shared: true, name: headerName, value: String(value) });
      },
      remove(name) {
        ensureMutable();
        const normalized = normalizeName(name);
        for (let index = rows.length - 1; index >= 0; index -= 1) {
          if (normalizeName(rows[index].name) === normalized) rows.splice(index, 1);
        }
      },
      toArray() {
        return rows.map(row => ({ ...row }));
      },
    };
    return Object.freeze(bag);
  }

  function makeBodyFields(source, mutable) {
    const rows = (Array.isArray(source) ? source : []).map(field => ({
      enabled: field.enabled !== false,
      name: String(field.name ?? ""),
      value: String(field.value ?? ""),
      kind: field.kind === "file" ? "file" : "text",
    }));
    if (!mutable) {
      rows.forEach(Object.freeze);
      Object.freeze(rows);
    }
    return rows;
  }

  const requestState = {
    method: String(input.request.method),
    url: String(input.request.url),
    body: String(input.request.body),
    bodyMode: String(input.request.body_mode ?? "raw"),
    rawBodyLanguage: String(input.request.raw_body_language ?? "json"),
    bodyFields: makeBodyFields(input.request.body_fields, mutableRequest),
    headers: makeHeaders(input.request.headers, mutableRequest),
  };
  const request = mutableRequest ? requestState : Object.freeze(requestState);

  const environment = Object.freeze({
    has(key) {
      return own(environmentValues, String(key));
    },
    get(key) {
      return environmentValues[String(key)];
    },
    set(key, value) {
      const name = String(key);
      if (!name) throw new TypeError("environment variable name cannot be empty");
      const text = String(value);
      environmentValues[name] = text;
      environmentMutations.push({ op: "set", key: name, value: text });
    },
    unset(key) {
      const name = String(key);
      delete environmentValues[name];
      environmentMutations.push({ op: "unset", key: name });
    },
    toObject() {
      return { ...environmentValues };
    },
  });

  const variables = Object.freeze({
    has(key) {
      const name = String(key);
      return own(environmentValues, name) || own(collectionValues, name);
    },
    get(key) {
      const name = String(key);
      if (own(environmentValues, name)) return environmentValues[name];
      return collectionValues[name];
    },
    toObject() {
      return { ...collectionValues, ...environmentValues };
    },
  });

  // Saved-request namespace (dynamic, frozen). Built from the active
  // workspace's collection tree. Request-reference leaves carry their stable
  // saved-request id in a non-enumerable marker; namespace objects carry a
  // separate marker so passing one to api.requests.execute gives a precise
  // diagnostic.
  const REQUEST_REF_MARKER = "__apiTesterRequestRef";
  const NAMESPACE_REF_MARKER = "__apiTesterNamespaceRef";
  const scheduledRequests = [];

  function buildRequestNamespace(spec) {
    if (spec && spec.kind && spec.kind.type === "request") {
      const ref = {};
      Object.defineProperty(ref, REQUEST_REF_MARKER, {
        value: {
          id: String(spec.kind.id),
          path: String(spec.kind.path),
          method: String(spec.kind.method),
        },
        enumerable: false,
      });
      return Object.freeze(ref);
    }
    const namespace = {};
    Object.defineProperty(namespace, NAMESPACE_REF_MARKER, {
      value: true,
      enumerable: false,
    });
    const children = Array.isArray(spec.children) ? spec.children : [];
    for (const child of children) {
      Object.defineProperty(namespace, String(child.name), {
        value: buildRequestNamespace(child),
        enumerable: true,
        writable: false,
        configurable: false,
      });
    }
    return Object.freeze(namespace);
  }

  const requestReferences = Array.isArray(input.requestReferences)
    ? input.requestReferences
    : [];
  for (const root of requestReferences) {
    const name = String(root.name);
    // Never overwrite a scripting or built-in global.
    if (own(globalThis, name)) continue;
    Object.defineProperty(globalThis, name, {
      value: buildRequestNamespace(root),
      enumerable: true,
      writable: false,
      configurable: false,
    });
  }

  const requestsApi = Object.freeze({
    execute(reference) {
      if (reference === null || typeof reference !== "object") {
        throw new TypeError(
          "api.requests.execute() expects a saved request reference (for example api.requests.execute(ChatAdmin.Login)); got " +
            (reference === null ? "null" : typeof reference) +
            "."
        );
      }
      if (!own(reference, REQUEST_REF_MARKER)) {
        if (own(reference, NAMESPACE_REF_MARKER)) {
          throw new TypeError(
            "api.requests.execute() received a folder/collection namespace, which is not a request. Pass a request leaf such as api.requests.execute(ChatAdmin.Login)."
          );
        }
        throw new TypeError(
          "api.requests.execute() expects a saved request reference from the active workspace, not an arbitrary object."
        );
      }
      const metadata = reference[REQUEST_REF_MARKER];
      return new Promise(function (resolve, reject) {
        nativeOps.push({
          id: String(metadata.id),
          path: String(metadata.path),
          state: "pending",
          resolve: resolve,
          reject: reject,
        });
      });
    },
  });

  function printable(value) {
    if (typeof value === "string") return value;
    if (typeof value === "undefined") return "undefined";
    if (typeof value === "bigint") return `${value}n`;
    try {
      const json = JSON.stringify(value);
      return json === undefined ? String(value) : json;
    } catch (_) {
      try {
        return String(value);
      } catch (_) {
        return "<unprintable>";
      }
    }
  }

  function writeLog(level, values) {
    if (logs.length >= input.maxLogEntries || logCharacters >= input.maxLogBytes) return;
    let message = values.map(printable).join(" ");
    const remaining = input.maxLogBytes - logCharacters;
    if (message.length > remaining) message = message.slice(0, remaining);
    logCharacters += message.length;
    logs.push({ level, message });
  }

  const scriptConsole = Object.freeze({
    log: (...values) => writeLog("log", values),
    info: (...values) => writeLog("info", values),
    warn: (...values) => writeLog("warn", values),
    error: (...values) => writeLog("error", values),
    debug: (...values) => writeLog("debug", values),
  });

  const assert = (condition, message = "assertion failed") => {
    if (!condition) throw new Error(String(message));
  };

  const test = (name, callback) => {
    const testName = String(name);
    try {
      if (typeof callback !== "function") throw new TypeError("test callback must be a function");
      const result = callback();
      if (result && typeof result.then === "function") {
        throw new TypeError("async test callbacks are not supported");
      }
      tests.push({ name: testName, passed: true, message: null });
    } catch (error) {
      tests.push({
        name: testName,
        passed: false,
        message: error && error.message ? String(error.message) : printable(error),
      });
    }
  };

  let response = null;
  if (input.response) {
    const responseState = {
      status: input.response.status,
      statusText: input.response.statusText,
      httpVersion: input.response.httpVersion,
      url: input.response.url,
      headers: makeHeaders(input.response.headers, false),
      durationMs: input.response.durationMs,
      sizeBytes: input.response.sizeBytes,
      truncated: input.response.truncated,
      bodyBase64: input.response.bodyBase64,
      text: () => input.response.bodyText,
      json: () => JSON.parse(input.response.bodyText),
    };
    response = Object.freeze(responseState);
  }

  const api = {
    request,
    response,
    environment,
    variables,
    requests: requestsApi,
    console: scriptConsole,
  };
  if (input.phase === "post") {
    api.test = test;
    api.assert = assert;
  }

  Object.defineProperty(globalThis, "api", {
    value: Object.freeze(api),
    configurable: false,
    enumerable: true,
    writable: false,
  });
  Object.defineProperty(globalThis, "console", {
    value: scriptConsole,
    configurable: false,
    enumerable: true,
    writable: false,
  });
  for (const name of ["fetch", "require", "process", "Deno", "WebSocket", "XMLHttpRequest"]) {
    Object.defineProperty(globalThis, name, {
      value: undefined,
      configurable: false,
      enumerable: false,
      writable: false,
    });
  }

  Object.defineProperty(globalThis, "__API_TESTER_FINISH", {
    value: () => {
      // Requests that were never awaited still run after the phase: carry any
      // pending (non-awaited) execute() ops into the scheduled list.
      for (const op of nativeOps) {
        if (op.state === "pending") {
          scheduledRequests.push({ id: op.id, path: op.path });
        }
      }
      return {
        request: {
        method: String(request.method),
        url: String(request.url),
        headers: request.headers.toArray(),
        body: String(request.body),
        body_mode: String(request.bodyMode),
        raw_body_language: String(request.rawBodyLanguage),
        body_fields: request.bodyFields.map(field => ({
          enabled: field.enabled !== false,
          name: String(field.name ?? ""),
          value: String(field.value ?? ""),
          kind: field.kind === "file" ? "file" : "text",
        })),
      },
      environmentMutations,
      logs,
      tests,
      chainedRequests: scheduledRequests,
    };
    },
    configurable: false,
    enumerable: false,
    writable: false,
  });
  delete globalThis.__API_TESTER_INPUT;
})();
"#;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptPhase {
    PreRequest,
    PostResponse,
}

impl ScriptPhase {
    pub const fn filename(self) -> &'static str {
        match self {
            Self::PreRequest => "pre-request.js",
            Self::PostResponse => "post-response.js",
        }
    }

    const fn engine_name(self) -> &'static str {
        match self {
            Self::PreRequest => "pre",
            Self::PostResponse => "post",
        }
    }
}

impl fmt::Display for ScriptPhase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::PreRequest => "pre-request",
            Self::PostResponse => "post-response",
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScriptEnvironment {
    values: BTreeMap<String, String>,
    secret_names: BTreeSet<String>,
}

#[allow(dead_code)]
impl ScriptEnvironment {
    pub fn new(values: BTreeMap<String, String>) -> Self {
        Self {
            values,
            secret_names: BTreeSet::new(),
        }
    }

    pub fn values(&self) -> &BTreeMap<String, String> {
        &self.values
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    pub fn insert(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.values.insert(name.into(), value.into());
    }

    pub fn insert_secret(&mut self, name: impl Into<String>, value: impl Into<String>) {
        let name = name.into();
        self.secret_names.insert(name.clone());
        self.values.insert(name, value.into());
    }

    pub fn mark_secret(&mut self, name: impl Into<String>) {
        self.secret_names.insert(name.into());
    }

    pub fn is_secret(&self, name: &str) -> bool {
        self.secret_names.contains(name)
    }

    pub fn apply_mutations(&mut self, mutations: &[EnvironmentMutation]) {
        for mutation in mutations {
            match mutation {
                EnvironmentMutation::Set { key, value } => {
                    self.values.insert(key.clone(), value.clone());
                }
                EnvironmentMutation::Unset { key } => {
                    self.values.remove(key);
                    self.secret_names.remove(key);
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptScope {
    pub environment: ScriptEnvironment,
    pub collection_variables: BTreeMap<String, String>,
    pub extra_secrets: Vec<String>,
    /// Per-phase execution budget, including anything the phase `await`s (the
    /// full awaited request pipeline and its own pre/post scripts). Defaults to
    /// [`SCRIPT_TIMEOUT`]; the app overrides it from persisted settings.
    pub script_timeout: std::time::Duration,
}

impl Default for ScriptScope {
    fn default() -> Self {
        Self {
            environment: ScriptEnvironment::default(),
            collection_variables: BTreeMap::new(),
            extra_secrets: Vec::new(),
            script_timeout: SCRIPT_TIMEOUT,
        }
    }
}

impl ScriptScope {
    pub fn redactor(&self) -> SecretRedactor {
        let mut redactor = SecretRedactor::default();
        redactor.extend(
            self.environment
                .secret_names
                .iter()
                .filter_map(|name| self.environment.values.get(name).cloned()),
        );
        redactor.extend(self.extra_secrets.iter().cloned());
        redactor
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SecretRedactor {
    /// Canonical secret values, kept sorted longest-first (ties broken by
    /// value) and deduplicated, so variant derivation is deterministic.
    secrets: Vec<String>,
    /// Cached spelling variants of `secrets` (see `template::secret_variants`),
    /// re-derived only when `secrets` changes; never recomputed per scrub.
    variants: Vec<String>,
}

#[allow(dead_code)]
impl SecretRedactor {
    pub fn new(secrets: impl IntoIterator<Item = String>) -> Self {
        let mut redactor = Self::default();
        redactor.extend(secrets);
        redactor
    }

    pub fn add(&mut self, secret: impl Into<String>) {
        if self.insert_secret(secret.into()) {
            self.rebuild_variants();
        }
    }

    pub fn extend(&mut self, secrets: impl IntoIterator<Item = String>) {
        let mut changed = false;
        for secret in secrets {
            changed |= self.insert_secret(secret);
        }
        if changed {
            self.rebuild_variants();
        }
    }

    /// Inserts `secret`, keeping `secrets` sorted longest-first and
    /// deduplicated via a binary-search probe. Returns true when the set
    /// actually changed (so callers can skip the variant rebuild).
    fn insert_secret(&mut self, secret: String) -> bool {
        if secret.is_empty() {
            return false;
        }
        let position = self
            .secrets
            .binary_search_by(|existing| {
                existing
                    .len()
                    .cmp(&secret.len())
                    .reverse()
                    .then_with(|| existing.cmp(&secret))
            })
            .unwrap_or_else(|position| position);
        if self.secrets.get(position) == Some(&secret) {
            return false;
        }
        self.secrets.insert(position, secret);
        true
    }

    fn rebuild_variants(&mut self) {
        self.variants = secret_variants(&self.secrets);
    }

    pub fn scrub(&self, text: impl AsRef<str>) -> String {
        let text = text.as_ref();
        if self.variants.is_empty() {
            return text.to_owned();
        }
        self.variants.iter().fold(text.to_owned(), |text, value| {
            text.replace(value, REDACTED_VALUE)
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct ScriptCancellation {
    cancelled: Arc<AtomicBool>,
}

impl ScriptCancellation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum EnvironmentMutation {
    Set { key: String, value: String },
    Unset { key: String },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ScriptLogLevel {
    Log,
    Info,
    Warn,
    Error,
    Debug,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ScriptLog {
    pub level: ScriptLogLevel,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ScriptTestResult {
    pub name: String,
    pub passed: bool,
    pub message: Option<String>,
}

/// A saved-request reference scheduled by `api.requests.execute(...)`.
///
/// Carries the stable saved-request id (never resolved by name after the
/// reference was created) plus the dotted access path for diagnostics.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct ChainedRequest {
    pub id: String,
    pub path: String,
}

#[derive(Clone, Debug)]
pub struct ScriptReport {
    pub phase: ScriptPhase,
    pub duration: Duration,
    pub logs: Vec<ScriptLog>,
    pub tests: Vec<ScriptTestResult>,
    pub response_body_truncated: bool,
}

impl ScriptReport {
    fn empty(phase: ScriptPhase) -> Self {
        Self {
            phase,
            duration: Duration::ZERO,
            logs: Vec::new(),
            tests: Vec::new(),
            response_body_truncated: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PreRequestResult {
    pub request: RequestDraft,
    pub environment_mutations: Vec<EnvironmentMutation>,
    pub chained_requests: Vec<ChainedRequest>,
    pub report: ScriptReport,
}

#[derive(Clone, Debug)]
pub struct PostResponseResult {
    pub environment_mutations: Vec<EnvironmentMutation>,
    pub chained_requests: Vec<ChainedRequest>,
    pub report: ScriptReport,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptErrorKind {
    SourceLimit,
    BodyLimit,
    Cancelled,
    TimedOut,
    MemoryLimit,
    Syntax,
    Runtime,
    OutputLimit,
    Engine,
}

#[derive(Clone, Debug)]
pub struct ScriptDiagnostic {
    pub phase: ScriptPhase,
    pub kind: ScriptErrorKind,
    pub filename: &'static str,
    pub message: String,
    pub stack: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ScriptError {
    pub diagnostic: ScriptDiagnostic,
    pub report: ScriptReport,
}

impl fmt::Display for ScriptError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} script failed: {}",
            self.diagnostic.phase, self.diagnostic.message
        )
    }
}

impl std::error::Error for ScriptError {}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EngineInput<'a> {
    phase: &'static str,
    request: &'a RequestDraft,
    response: Option<EngineResponse>,
    environment: &'a BTreeMap<String, String>,
    collection_variables: &'a BTreeMap<String, String>,
    request_references: &'a [RuntimeNamespaceSpec],
    max_log_entries: usize,
    max_log_bytes: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EngineResponse {
    status: u16,
    status_text: String,
    http_version: String,
    url: String,
    headers: Vec<EngineHeader>,
    duration_ms: u64,
    size_bytes: usize,
    body_text: String,
    body_base64: Option<String>,
    truncated: bool,
}

#[derive(Serialize)]
struct EngineHeader {
    enabled: bool,
    name: String,
    value: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct EngineOutput {
    request: RequestDraft,
    environment_mutations: Vec<EnvironmentMutation>,
    #[serde(default)]
    chained_requests: Vec<ChainedRequest>,
    logs: Vec<ScriptLog>,
    tests: Vec<ScriptTestResult>,
}

struct EngineRun {
    output: EngineOutput,
    duration: Duration,
}

fn execute_pre_request_inner(
    source: &str,
    request: &RequestDraft,
    scope: &ScriptScope,
    request_namespace: &RequestNamespaceCatalog,
    cancellation: &ScriptCancellation,
    chain_inline: Option<&dyn InlineChainer>,
) -> Result<PreRequestResult, ScriptError> {
    let phase = ScriptPhase::PreRequest;
    if source.trim().is_empty() {
        return Ok(PreRequestResult {
            request: request.clone(),
            environment_mutations: Vec::new(),
            chained_requests: Vec::new(),
            report: ScriptReport::empty(phase),
        });
    }

    let mut redactor = scope.redactor();
    validate_source_and_body(phase, source, request, &redactor)?;
    let input = EngineInput {
        phase: phase.engine_name(),
        request,
        response: None,
        environment: scope.environment.values(),
        collection_variables: &scope.collection_variables,
        request_references: &request_namespace.runtime_specs(),
        max_log_entries: MAX_SCRIPT_LOG_ENTRIES,
        max_log_bytes: MAX_SCRIPT_LOG_BYTES,
    };
    let run = run_engine(
        source,
        phase,
        input,
        cancellation,
        chain_inline,
        scope.script_timeout,
        &redactor,
        &scope.environment.secret_names,
    )?;
    extend_redactor_with_secret_mutations(&mut redactor, scope, &run.output.environment_mutations);
    let report = report_from_run(phase, &run, false, &redactor);
    if request_body_size(&run.output.request) > MAX_SCRIPT_BODY_BYTES {
        return Err(ScriptError {
            diagnostic: ScriptDiagnostic {
                phase,
                kind: ScriptErrorKind::BodyLimit,
                filename: phase.filename(),
                message: format!(
                    "pre-request script produced a {} byte body; the limit is {MAX_SCRIPT_BODY_BYTES} bytes",
                    request_body_size(&run.output.request)
                ),
                stack: None,
            },
            report,
        });
    }

    Ok(PreRequestResult {
        request: run.output.request,
        environment_mutations: run.output.environment_mutations,
        chained_requests: run.output.chained_requests,
        report,
    })
}

/// Runs a pre-request script without inline (awaited) chaining: `execute()`
/// only schedules requests to run after the phase.
// Used only by tests and as the schedule-only API (runtime/chain use the
// `_with_chain` variants).
#[allow(dead_code)]
pub fn execute_pre_request(
    source: &str,
    request: &RequestDraft,
    scope: &ScriptScope,
    request_namespace: &RequestNamespaceCatalog,
    cancellation: &ScriptCancellation,
) -> Result<PreRequestResult, ScriptError> {
    execute_pre_request_inner(
        source,
        request,
        scope,
        request_namespace,
        cancellation,
        None,
    )
}

/// Like [`execute_pre_request`], but supports `await api.requests.execute(...)`:
/// the provided chainer runs the referenced saved request's full pipeline
/// inline so the script only continues after it completes.
pub fn execute_pre_request_with_chain(
    source: &str,
    request: &RequestDraft,
    scope: &ScriptScope,
    request_namespace: &RequestNamespaceCatalog,
    cancellation: &ScriptCancellation,
    chain_inline: Option<&dyn InlineChainer>,
) -> Result<PreRequestResult, ScriptError> {
    execute_pre_request_inner(
        source,
        request,
        scope,
        request_namespace,
        cancellation,
        chain_inline,
    )
}

#[allow(clippy::too_many_arguments)]
fn execute_post_response_inner(
    source: &str,
    request: &RequestDraft,
    response: &ResponseData,
    scope: &ScriptScope,
    request_namespace: &RequestNamespaceCatalog,
    cancellation: &ScriptCancellation,
    chain_inline: Option<&dyn InlineChainer>,
) -> Result<PostResponseResult, ScriptError> {
    let phase = ScriptPhase::PostResponse;
    if source.trim().is_empty() {
        return Ok(PostResponseResult {
            environment_mutations: Vec::new(),
            chained_requests: Vec::new(),
            report: ScriptReport::empty(phase),
        });
    }

    let mut redactor = scope.redactor();
    validate_source_and_body(phase, source, request, &redactor)?;
    let (engine_response, truncated) = engine_response(response);
    let input = EngineInput {
        phase: phase.engine_name(),
        request,
        response: Some(engine_response),
        environment: scope.environment.values(),
        collection_variables: &scope.collection_variables,
        request_references: &request_namespace.runtime_specs(),
        max_log_entries: MAX_SCRIPT_LOG_ENTRIES,
        max_log_bytes: MAX_SCRIPT_LOG_BYTES,
    };
    let run = run_engine(
        source,
        phase,
        input,
        cancellation,
        chain_inline,
        scope.script_timeout,
        &redactor,
        &scope.environment.secret_names,
    )?;
    extend_redactor_with_secret_mutations(&mut redactor, scope, &run.output.environment_mutations);
    let report = report_from_run(phase, &run, truncated, &redactor);

    Ok(PostResponseResult {
        environment_mutations: run.output.environment_mutations,
        chained_requests: run.output.chained_requests,
        report,
    })
}

/// Runs a post-response script without inline (awaited) chaining.
// Used only by tests and as the schedule-only API (runtime/chain use the
// `_with_chain` variants).
#[allow(dead_code)]
pub fn execute_post_response(
    source: &str,
    request: &RequestDraft,
    response: &ResponseData,
    scope: &ScriptScope,
    request_namespace: &RequestNamespaceCatalog,
    cancellation: &ScriptCancellation,
) -> Result<PostResponseResult, ScriptError> {
    execute_post_response_inner(
        source,
        request,
        response,
        scope,
        request_namespace,
        cancellation,
        None,
    )
}

/// Like [`execute_post_response`], but supports `await api.requests.execute(...)`
/// via the provided inline chainer.
pub fn execute_post_response_with_chain(
    source: &str,
    request: &RequestDraft,
    response: &ResponseData,
    scope: &ScriptScope,
    request_namespace: &RequestNamespaceCatalog,
    cancellation: &ScriptCancellation,
    chain_inline: Option<&dyn InlineChainer>,
) -> Result<PostResponseResult, ScriptError> {
    execute_post_response_inner(
        source,
        request,
        response,
        scope,
        request_namespace,
        cancellation,
        chain_inline,
    )
}

fn extend_redactor_with_secret_mutations(
    redactor: &mut SecretRedactor,
    scope: &ScriptScope,
    mutations: &[EnvironmentMutation],
) {
    extend_redactor_with_secret_names(redactor, &scope.environment.secret_names, mutations);
}

fn extend_redactor_with_secret_names(
    redactor: &mut SecretRedactor,
    secret_names: &BTreeSet<String>,
    mutations: &[EnvironmentMutation],
) {
    for mutation in mutations {
        if let EnvironmentMutation::Set { key, value } = mutation
            && secret_names.contains(key)
        {
            redactor.add(value.clone());
        }
    }
}

fn validate_source_and_body(
    phase: ScriptPhase,
    source: &str,
    request: &RequestDraft,
    redactor: &SecretRedactor,
) -> Result<(), ScriptError> {
    if source.len() > MAX_SCRIPT_SOURCE_BYTES {
        return Err(simple_error(
            phase,
            ScriptErrorKind::SourceLimit,
            format!(
                "script source is {} bytes; the limit is {MAX_SCRIPT_SOURCE_BYTES} bytes",
                source.len()
            ),
            redactor,
        ));
    }
    if request_body_size(request) > MAX_SCRIPT_BODY_BYTES {
        return Err(simple_error(
            phase,
            ScriptErrorKind::BodyLimit,
            format!(
                "request body is {} bytes; scripts accept at most {MAX_SCRIPT_BODY_BYTES} bytes",
                request_body_size(request)
            ),
            redactor,
        ));
    }
    Ok(())
}

fn request_body_size(request: &RequestDraft) -> usize {
    request
        .body_fields
        .iter()
        .fold(request.body.len(), |size, field| {
            size.saturating_add(field.name.len())
                .saturating_add(field.value.len())
        })
}

fn run_engine(
    source: &str,
    phase: ScriptPhase,
    input: EngineInput<'_>,
    cancellation: &ScriptCancellation,
    chain_inline: Option<&dyn InlineChainer>,
    timeout: std::time::Duration,
    redactor: &SecretRedactor,
    secret_names: &BTreeSet<String>,
) -> Result<EngineRun, ScriptError> {
    if cancellation.is_cancelled() {
        return Err(simple_error(
            phase,
            ScriptErrorKind::Cancelled,
            "script cancelled",
            redactor,
        ));
    }

    let started = Instant::now();
    let deadline = started + timeout;
    let timed_out = Arc::new(AtomicBool::new(false));
    let timed_out_for_interrupt = Arc::clone(&timed_out);
    let cancelled_for_interrupt = Arc::clone(&cancellation.cancelled);

    let runtime = JsRuntime::new().map_err(|error| {
        engine_error(
            phase,
            ScriptErrorKind::Engine,
            error.to_string(),
            started.elapsed(),
            redactor,
        )
    })?;
    runtime.set_memory_limit(SCRIPT_MEMORY_LIMIT_BYTES);
    runtime.set_max_stack_size(SCRIPT_STACK_LIMIT_BYTES);
    runtime.set_interrupt_handler(Some(Box::new(move || {
        if cancelled_for_interrupt.load(Ordering::Acquire) {
            return true;
        }
        if Instant::now() >= deadline {
            timed_out_for_interrupt.store(true, Ordering::Release);
            return true;
        }
        false
    })));

    let context = JsContext::full(&runtime).map_err(|error| {
        engine_error(
            phase,
            ScriptErrorKind::Engine,
            error.to_string(),
            started.elapsed(),
            redactor,
        )
    })?;

    context.with(|ctx| -> Result<(), ScriptError> {
        let input_value = rquickjs_serde::to_value(ctx.clone(), &input).map_err(|error| {
            engine_error(
                phase,
                ScriptErrorKind::Engine,
                error.to_string(),
                started.elapsed(),
                redactor,
            )
        })?;
        ctx.globals()
            .set("__API_TESTER_INPUT", input_value)
            .map_err(|error| {
                engine_error(
                    phase,
                    ScriptErrorKind::Engine,
                    error.to_string(),
                    started.elapsed(),
                    redactor,
                )
            })?;

        ctx.eval::<(), _>(PRELUDE).map_err(|error| {
            engine_error(
                phase,
                ScriptErrorKind::Engine,
                error.to_string(),
                started.elapsed(),
                redactor,
            )
        })?;

        let mut options = EvalOptions::default();
        options.global = true;
        options.strict = true;
        options.backtrace_barrier = true;
        // Enable native top-level await (JS_EVAL_FLAG_ASYNC): the script runs
        // as global async code and evaluation returns a Promise we pump below.
        options.promise = true;
        options.filename = Some(phase.filename().to_owned());
        let main: Value<'_> =
            match ctx.eval_with_options::<Value<'_>, _>(source, options).catch(&ctx) {
                Ok(main) => main,
                Err(caught) => {
                    let duration = started.elapsed();
                    if cancellation.is_cancelled() {
                        return Err(engine_error(
                            phase,
                            ScriptErrorKind::Cancelled,
                            "script cancelled",
                            duration,
                            redactor,
                        ));
                    }
                    if timed_out.load(Ordering::Acquire) {
                        return Err(engine_error(
                            phase,
                            ScriptErrorKind::TimedOut,
                            format!(
                                "script exceeded its {} ms execution limit",
                                timeout.as_millis()
                            ),
                            duration,
                            redactor,
                        ));
                    }
                    let mut caught_redactor = redactor.clone();
                    let mut caught_output = None;
                    if let Ok(value) = ctx.eval::<Value<'_>, _>("__API_TESTER_FINISH()")
                        && let Ok(output) = rquickjs_serde::from_value_strict::<EngineOutput>(value)
                    {
                        extend_redactor_with_secret_names(
                            &mut caught_redactor,
                            secret_names,
                            &output.environment_mutations,
                        );
                        if serde_json::to_vec(&output)
                            .is_ok_and(|encoded| encoded.len() <= MAX_SCRIPT_RESULT_BYTES)
                        {
                            caught_output = Some(output);
                        }
                    }
                    let mut error = caught_error(phase, caught, duration, &caught_redactor);
                    if let Some(output) = caught_output {
                        error.report =
                            report_from_output(phase, &output, duration, false, &caught_redactor);
                    }
                    return Err(error);
                }
            };

        // Root the evaluation's promise so it survives the pump below, and
        // attach a settlement probe. The probe also *handles* the rejection so
        // QuickJS never reports it as unhandled; we re-throw the captured reason
        // after the pump when the script rejected.
        ctx.globals().set("__API_TESTER_MAIN", main).map_err(|error| {
            engine_error(
                phase,
                ScriptErrorKind::Engine,
                error.to_string(),
                started.elapsed(),
                redactor,
            )
        })?;
        ctx.eval::<(), _>(
            concat!(
                "globalThis.__API_TESTER_SETTLED = { done: false, rejected: false, reason: undefined };\n",
                "globalThis.__API_TESTER_MAIN.then(\n",
                "  function(value) { globalThis.__API_TESTER_SETTLED.done = true; },\n",
                "  function(reason) {\n",
                "    globalThis.__API_TESTER_SETTLED.done = true;\n",
                "    globalThis.__API_TESTER_SETTLED.rejected = true;\n",
                "    globalThis.__API_TESTER_SETTLED.reason = reason;\n",
                "  }\n",
                ");\n",
            ),
        )
        .map_err(|error| {
            engine_error(
                phase,
                ScriptErrorKind::Engine,
                error.to_string(),
                started.elapsed(),
                redactor,
            )
        })?;
        Ok(())
    })?;

    // Drive the evaluation's promise job queue to settlement, interleaving
    // awaited `api.requests.execute(...)` calls: after draining the available
    // microtasks, if the script is blocked on an awaited execute() op we run the
    // referenced saved request's full pipeline inline, apply its environment
    // mutations to the live environment, and resolve the promise so the script
    // continues only after the whole sub-chain has finished. The interrupt
    // handler covers CPU-bound loops; the deadline / cancellation checks cover
    // everything else, including awaited requests.
    loop {
        if cancellation.is_cancelled() {
            return Err(engine_error(
                phase,
                ScriptErrorKind::Cancelled,
                "script cancelled",
                started.elapsed(),
                redactor,
            ));
        }
        if timed_out.load(Ordering::Acquire) || Instant::now() >= deadline {
            timed_out.store(true, Ordering::Release);
            return Err(engine_error(
                phase,
                ScriptErrorKind::TimedOut,
                format!(
                    "script exceeded its {} ms execution limit",
                    timeout.as_millis()
                ),
                started.elapsed(),
                redactor,
            ));
        }
        let drained = loop {
            if cancellation.is_cancelled() || timed_out.load(Ordering::Acquire) {
                break false;
            }
            if Instant::now() >= deadline {
                timed_out.store(true, Ordering::Release);
                break false;
            }
            match runtime.execute_pending_job() {
                Ok(true) => continue,
                Ok(false) => break true,
                Err(_job) => {
                    let error = context.with(|ctx| {
                        if ctx
                            .eval::<bool, _>("globalThis.__API_TESTER_SETTLED.rejected")
                            .unwrap_or(false)
                        {
                            capture_top_level_rejection(&ctx, phase, started.elapsed(), redactor)
                        } else if ctx.has_exception() {
                            let value = ctx.catch();
                            let caught = match value
                                .as_object()
                                .and_then(|object| Exception::from_object(object.clone()))
                            {
                                Some(exception) => CaughtError::Exception(exception),
                                None => CaughtError::Value(value),
                            };
                            caught_error(phase, caught, started.elapsed(), redactor)
                        } else {
                            simple_error(
                                phase,
                                ScriptErrorKind::Runtime,
                                "script rejected while running",
                                redactor,
                            )
                        }
                    });
                    return Err(error);
                }
            }
        };
        if !drained {
            break;
        }

        // If the script is blocked awaiting `api.requests.execute(...)`, run the
        // referenced saved requests' pipelines concurrently (true Promise.all
        // semantics). Every pending awaited op is handed to the provider in one
        // batch; the provider drives all their pipelines on a single shared
        // async runtime so their network calls genuinely overlap, then returns
        // one result per op. Ops the script never awaited are carried to
        // `scheduledRequests` by __API_TESTER_FINISH so non-awaited execute()
        // still runs after the phase.
        match chain_inline {
            None => break,
            Some(chain_inline) => {
                let pending = context.with(
                    |ctx| -> Result<Option<Vec<(String, String)>>, ScriptError> {
                        if ctx
                            .eval::<bool, _>("globalThis.__API_TESTER_SETTLED.done")
                            .unwrap_or(false)
                        {
                            return Ok(None);
                        }
                        let json = ctx
                            .eval::<String, _>(
                                "(function(){ \
                                 var ops = globalThis.__API_TESTER_NATIVE_OPS || []; \
                                 var found = []; \
                                 for (var i = 0; i < ops.length; i++) { \
                                   if (ops[i].state === 'pending') { \
                                     ops[i].state = 'running'; \
                                     found.push([ops[i].id, ops[i].path]); \
                                   } \
                                 } \
                                 return JSON.stringify(found); })()",
                            )
                            .map_err(|error| {
                                engine_error(
                                    phase,
                                    ScriptErrorKind::Engine,
                                    format!("could not read pending request scripts: {error}"),
                                    started.elapsed(),
                                    redactor,
                                )
                            })?;
                        if json == "[]" {
                            return Ok(None);
                        }
                        let ops: Vec<(String, String)> =
                            serde_json::from_str(&json).map_err(|error| {
                                engine_error(
                                    phase,
                                    ScriptErrorKind::Engine,
                                    format!(
                                        "pending request scripts had an unexpected shape: {error}"
                                    ),
                                    started.elapsed(),
                                    redactor,
                                )
                            })?;
                        Ok(Some(ops))
                    },
                )?;

                let Some(pending) = pending else { break };

                let requested: Vec<ChainedRequest> = pending
                    .iter()
                    .map(|(id, path)| ChainedRequest {
                        id: id.clone(),
                        path: path.clone(),
                    })
                    .collect();
                // A single batch call: the provider runs every awaited pipeline
                // concurrently on one shared async runtime.
                let outcomes = chain_inline.run(&requested);

                for ((id, _path), outcome) in pending.iter().zip(outcomes) {
                    let id_json = serde_json::to_string(id).unwrap_or_else(|_| "\"\"".to_owned());
                    match outcome {
                        Ok(run) => {
                            let mutations_json = serde_json::to_string(&run.environment_mutations)
                                .unwrap_or_else(|_| "[]".to_owned());
                            context
                                .with(|ctx| {
                                    let js = format!(
                                        "__API_TESTER_APPLY_CHAIN({mutations_json}); \
                                         (function(){{ var ops = globalThis.__API_TESTER_NATIVE_OPS || []; \
                                           for (var i = 0; i < ops.length; i++) {{ \
                                             if (ops[i].id === {id_json} && ops[i].state === 'running') {{ \
                                               ops[i].state = 'resolved'; \
                                               ops[i].resolve(); \
                                             }} \
                                           }} }})();"
                                    );
                                    ctx.eval::<(), _>(js.as_str())
                                })
                                .map_err(|error| {
                                    engine_error(
                                        phase,
                                        ScriptErrorKind::Engine,
                                        format!("could not finalize awaited request: {error}"),
                                        started.elapsed(),
                                        redactor,
                                    )
                                })?;
                        }
                        Err(message) => {
                            let message_json = serde_json::to_string(&message)
                                .unwrap_or_else(|_| "\"awaited request failed\"".to_owned());
                            context
                                .with(|ctx| {
                                    let js = format!(
                                        "(function(){{ var ops = globalThis.__API_TESTER_NATIVE_OPS || []; \
                                           for (var i = 0; i < ops.length; i++) {{ \
                                             if (ops[i].id === {id_json} && ops[i].state === 'running') {{ \
                                               ops[i].state = 'rejected'; \
                                               ops[i].reject({message_json}); \
                                             }} \
                                           }} }})();"
                                    );
                                    ctx.eval::<(), _>(js.as_str())
                                })
                                .map_err(|error| {
                                    engine_error(
                                        phase,
                                        ScriptErrorKind::Engine,
                                        format!("could not reject awaited request: {error}"),
                                        started.elapsed(),
                                        redactor,
                                    )
                                })?;
                        }
                    }
                }
                continue;
            }
        }
    }
    if timed_out.load(Ordering::Acquire) {
        return Err(engine_error(
            phase,
            ScriptErrorKind::TimedOut,
            format!(
                "script exceeded its {} ms execution limit",
                timeout.as_millis()
            ),
            started.elapsed(),
            redactor,
        ));
    }
    if cancellation.is_cancelled() {
        return Err(engine_error(
            phase,
            ScriptErrorKind::Cancelled,
            "script cancelled",
            started.elapsed(),
            redactor,
        ));
    }

    // Distinguish "settled cleanly", "rejected", and "parked on an await that
    // will never settle".
    let settle = context.with(|ctx| {
        (
            ctx.eval::<bool, _>("globalThis.__API_TESTER_SETTLED.done")
                .unwrap_or(false),
            ctx.eval::<bool, _>("globalThis.__API_TESTER_SETTLED.rejected")
                .unwrap_or(false),
        )
    });
    if !settle.0 {
        return Err(engine_error(
            phase,
            ScriptErrorKind::TimedOut,
            "script finished without settling its top-level promise (an `await` at the top level never resolved)"
                .to_owned(),
            started.elapsed(),
            redactor,
        ));
    }
    if settle.1 {
        // Mirrors the synchronous-throw path: include anything the script
        // logged or mutated before it rejected, and scrub with both the
        // ambient secrets and any the script just set.
        let mut caught_redactor = redactor.clone();
        let mut caught_output = None;
        let collected = context.with(|ctx| -> Option<EngineOutput> {
            let value = ctx.eval::<Value<'_>, _>("__API_TESTER_FINISH()").ok()?;
            rquickjs_serde::from_value_strict::<EngineOutput>(value).ok()
        });
        if let Some(output) = collected {
            extend_redactor_with_secret_names(
                &mut caught_redactor,
                secret_names,
                &output.environment_mutations,
            );
            if serde_json::to_vec(&output)
                .is_ok_and(|encoded| encoded.len() <= MAX_SCRIPT_RESULT_BYTES)
            {
                caught_output = Some(output);
            }
        }
        let mut error = context.with(|ctx| {
            capture_top_level_rejection(&ctx, phase, started.elapsed(), &caught_redactor)
        });
        if let Some(output) = caught_output {
            error.report =
                report_from_output(phase, &output, started.elapsed(), false, &caught_redactor);
        }
        return Err(error);
    }

    let output: EngineOutput = context.with(|ctx| {
        let value: Value<'_> = ctx.eval("__API_TESTER_FINISH()").map_err(|error| {
            engine_error(
                phase,
                ScriptErrorKind::Runtime,
                format!("could not collect script output: {error}"),
                started.elapsed(),
                redactor,
            )
        })?;
        rquickjs_serde::from_value_strict(value).map_err(|error| {
            engine_error(
                phase,
                ScriptErrorKind::Runtime,
                format!("script produced invalid output: {error}"),
                started.elapsed(),
                redactor,
            )
        })
    })?;

    let output_size = serde_json::to_vec(&output).map_err(|error| {
        engine_error(
            phase,
            ScriptErrorKind::Engine,
            format!("could not measure script output: {error}"),
            started.elapsed(),
            redactor,
        )
    })?;
    if output_size.len() > MAX_SCRIPT_RESULT_BYTES {
        return Err(engine_error(
            phase,
            ScriptErrorKind::OutputLimit,
            format!(
                "script output is {} bytes; the limit is {MAX_SCRIPT_RESULT_BYTES} bytes",
                output_size.len()
            ),
            started.elapsed(),
            redactor,
        ));
    }

    Ok(EngineRun {
        output,
        duration: started.elapsed(),
    })
}

fn report_from_run(
    phase: ScriptPhase,
    run: &EngineRun,
    response_body_truncated: bool,
    redactor: &SecretRedactor,
) -> ScriptReport {
    report_from_output(
        phase,
        &run.output,
        run.duration,
        response_body_truncated,
        redactor,
    )
}

fn report_from_output(
    phase: ScriptPhase,
    output: &EngineOutput,
    duration: Duration,
    response_body_truncated: bool,
    redactor: &SecretRedactor,
) -> ScriptReport {
    let mut used_bytes = 0;
    let logs = output
        .logs
        .iter()
        .take(MAX_SCRIPT_LOG_ENTRIES)
        .filter_map(|log| {
            if used_bytes >= MAX_SCRIPT_LOG_BYTES {
                return None;
            }
            let mut message = redactor.scrub(&log.message);
            let remaining = MAX_SCRIPT_LOG_BYTES - used_bytes;
            truncate_utf8(&mut message, remaining);
            used_bytes += message.len();
            Some(ScriptLog {
                level: log.level,
                message,
            })
        })
        .collect();
    let tests = output
        .tests
        .iter()
        .map(|test| ScriptTestResult {
            name: redactor.scrub(&test.name),
            passed: test.passed,
            message: test.message.as_ref().map(|message| redactor.scrub(message)),
        })
        .collect();

    ScriptReport {
        phase,
        duration,
        logs,
        tests,
        response_body_truncated,
    }
}

fn caught_error(
    phase: ScriptPhase,
    caught: CaughtError<'_>,
    duration: Duration,
    redactor: &SecretRedactor,
) -> ScriptError {
    match caught {
        CaughtError::Error(rquickjs::Error::Allocation) => engine_error(
            phase,
            ScriptErrorKind::MemoryLimit,
            "script exceeded its memory limit",
            duration,
            redactor,
        ),
        CaughtError::Error(error) => engine_error(
            phase,
            ScriptErrorKind::Runtime,
            error.to_string(),
            duration,
            redactor,
        ),
        CaughtError::Exception(exception) => exception_error(phase, exception, duration, redactor),
        CaughtError::Value(value) => {
            let message = value
                .as_string()
                .and_then(|value| value.to_string().ok())
                .unwrap_or_else(|| format!("JavaScript threw a {}", value.type_name()));
            engine_error(phase, ScriptErrorKind::Runtime, message, duration, redactor)
        }
    }
}

fn exception_error(
    phase: ScriptPhase,
    exception: Exception<'_>,
    duration: Duration,
    redactor: &SecretRedactor,
) -> ScriptError {
    let name = exception
        .as_object()
        .get::<_, String>("name")
        .unwrap_or_else(|_| "Error".to_owned());
    let message = exception.message().unwrap_or_else(|| name.clone());
    let kind = if name == "SyntaxError" {
        ScriptErrorKind::Syntax
    } else if message.to_ascii_lowercase().contains("out of memory") {
        ScriptErrorKind::MemoryLimit
    } else {
        ScriptErrorKind::Runtime
    };
    let stack = exception.stack().map(|stack| redactor.scrub(stack));

    ScriptError {
        diagnostic: ScriptDiagnostic {
            phase,
            kind,
            filename: phase.filename(),
            message: redactor.scrub(message),
            stack,
        },
        report: ScriptReport {
            phase,
            duration,
            logs: Vec::new(),
            tests: Vec::new(),
            response_body_truncated: false,
        },
    }
}

fn simple_error(
    phase: ScriptPhase,
    kind: ScriptErrorKind,
    message: impl AsRef<str>,
    redactor: &SecretRedactor,
) -> ScriptError {
    engine_error(phase, kind, message, Duration::ZERO, redactor)
}

/// Converts a captured top-level rejection into a [`ScriptError`], routing the
/// reason through the same classification used for synchronous throws so that
/// error messages, stacks and secret redaction stay consistent.
fn capture_top_level_rejection(
    ctx: &rquickjs::Ctx<'_>,
    phase: ScriptPhase,
    duration: Duration,
    redactor: &SecretRedactor,
) -> ScriptError {
    match ctx
        .eval::<(), _>("throw globalThis.__API_TESTER_SETTLED.reason;")
        .catch(ctx)
    {
        Err(caught) => caught_error(phase, caught, duration, redactor),
        Ok(()) => simple_error(
            phase,
            ScriptErrorKind::Runtime,
            "script rejected its top-level promise",
            redactor,
        ),
    }
}

fn engine_error(
    phase: ScriptPhase,
    kind: ScriptErrorKind,
    message: impl AsRef<str>,
    duration: Duration,
    redactor: &SecretRedactor,
) -> ScriptError {
    ScriptError {
        diagnostic: ScriptDiagnostic {
            phase,
            kind,
            filename: phase.filename(),
            message: redactor.scrub(message),
            stack: None,
        },
        report: ScriptReport {
            phase,
            duration,
            logs: Vec::new(),
            tests: Vec::new(),
            response_body_truncated: false,
        },
    }
}

fn engine_response(response: &ResponseData) -> (EngineResponse, bool) {
    let visible_len = response.body.len().min(MAX_SCRIPT_BODY_BYTES);
    let visible_body = &response.body[..visible_len];
    let truncated = response.body.len() > visible_len;
    let is_utf8 = std::str::from_utf8(visible_body).is_ok();

    (
        EngineResponse {
            status: response.status,
            status_text: response.status_text.clone(),
            http_version: response.http_version.clone(),
            url: response.final_url.clone(),
            headers: response
                .headers
                .iter()
                .map(|header| EngineHeader {
                    enabled: true,
                    name: header.name.clone(),
                    value: header.value.clone(),
                })
                .collect(),
            duration_ms: response.duration.as_millis().min(u128::from(u64::MAX)) as u64,
            size_bytes: response.body.len(),
            body_text: String::from_utf8_lossy(visible_body).into_owned(),
            body_base64: (!is_utf8).then(|| encode_base64(visible_body)),
            truncated,
        },
        truncated,
    )
}

fn truncate_utf8(value: &mut String, max_bytes: usize) {
    if value.len() <= max_bytes {
        return;
    }
    let mut boundary = max_bytes;
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
}

fn encode_base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);

        output.push(TABLE[(first >> 2) as usize] as char);
        output.push(TABLE[(((first & 0b11) << 4) | (second >> 4)) as usize] as char);
        if chunk.len() > 1 {
            output.push(TABLE[(((second & 0b1111) << 2) | (third >> 6)) as usize] as char);
        } else {
            output.push('=');
        }
        if chunk.len() > 2 {
            output.push(TABLE[(third & 0b11_1111) as usize] as char);
        } else {
            output.push('=');
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use std::thread;

    use super::*;
    use crate::core::Workspace;
    use crate::core::request::ResponseHeader;

    fn request() -> RequestDraft {
        let mut request = RequestDraft::new("GET", "https://example.test/{{path}}");
        request.headers = vec![
            super::super::HeaderEntry::new("Accept", "application/json"),
            super::super::HeaderEntry::new("X-Repeat", "one"),
            super::super::HeaderEntry::new("X-Repeat", "two"),
        ];
        request
    }

    #[test]
    fn pre_request_mutations_are_returned_transactionally() {
        let mut scope = ScriptScope::default();
        scope.environment.insert("token", "environment-token");
        scope
            .collection_variables
            .insert("fallback".to_owned(), "collection-value".to_owned());
        let original = request();

        let result = execute_pre_request(
            r#"
api.request.method = "POST";
api.request.url = "https://example.test/users";
api.request.body = JSON.stringify({ fallback: api.variables.get("fallback") });
api.request.headers.set("X-Repeat", "replaced");
api.request.headers.append("Authorization", `Bearer ${api.environment.get("token")}`);
api.environment.set("created", "yes");
api.environment.unset("token");
console.log("prepared", api.request.method);
"#,
            &original,
            &scope,
            &RequestNamespaceCatalog::default(),
            &ScriptCancellation::new(),
        )
        .expect("pre-request script should succeed");

        assert_eq!(original.method, "GET");
        assert_eq!(result.request.method, "POST");
        assert_eq!(result.request.url, "https://example.test/users");
        assert_eq!(
            result
                .request
                .headers
                .iter()
                .filter(|header| header.name.eq_ignore_ascii_case("x-repeat"))
                .count(),
            1
        );
        assert_eq!(
            result.environment_mutations,
            vec![
                EnvironmentMutation::Set {
                    key: "created".to_owned(),
                    value: "yes".to_owned(),
                },
                EnvironmentMutation::Unset {
                    key: "token".to_owned(),
                },
            ]
        );
        assert_eq!(result.report.logs[0].message, "prepared POST");
    }

    #[test]
    fn pre_request_can_preserve_and_mutate_structured_body_metadata() {
        let mut original = request();
        original.body_mode = super::super::BodyMode::FormUrlEncoded;
        original.raw_body_language = super::super::RawBodyLanguage::Json;
        original.body_fields = vec![super::super::BodyField::text("name", "before")];

        let result = execute_pre_request(
            r#"
api.request.bodyMode = "multipart_form_data";
api.request.rawBodyLanguage = "yaml";
api.request.bodyFields[0].value = "after";
api.request.bodyFields.push({
  enabled: true,
  name: "attachment",
  value: "/tmp/file.txt",
  kind: "file",
});
"#,
            &original,
            &ScriptScope::default(),
            &RequestNamespaceCatalog::default(),
            &ScriptCancellation::new(),
        )
        .expect("structured body mutations should succeed");

        assert_eq!(
            result.request.body_mode,
            super::super::BodyMode::MultipartFormData
        );
        assert_eq!(
            result.request.raw_body_language,
            super::super::RawBodyLanguage::Yaml
        );
        assert_eq!(result.request.body_fields[0].value, "after");
        assert_eq!(
            result.request.body_fields[1].kind,
            super::super::BodyFieldKind::File
        );
    }

    #[test]
    fn post_response_exposes_response_tests_and_environment() {
        let response = ResponseData {
            status: 201,
            status_text: "Created".to_owned(),
            http_version: "HTTP/2".to_owned(),
            final_url: "https://example.test/users".to_owned(),
            headers: vec![ResponseHeader {
                name: "content-type".to_owned(),
                value: "application/json".to_owned(),
            }],
            content_type: Some("application/json".to_owned()),
            body: br#"{"token":"next-token"}"#.to_vec().into(),
            duration: Duration::from_millis(42),
        };

        let result = execute_post_response(
            r#"
api.test("created", () => {
  api.assert(api.response.status === 201, "wrong status");
  api.assert(api.response.headers.get("Content-Type") === "application/json");
});
api.environment.set("token", api.response.json().token);
console.info(api.response.durationMs);
"#,
            &request(),
            &response,
            &ScriptScope::default(),
            &RequestNamespaceCatalog::default(),
            &ScriptCancellation::new(),
        )
        .expect("post-response script should succeed");

        assert_eq!(result.report.tests.len(), 1);
        assert!(result.report.tests[0].passed);
        assert_eq!(result.report.logs[0].message, "42");
        assert_eq!(
            result.environment_mutations,
            vec![EnvironmentMutation::Set {
                key: "token".to_owned(),
                value: "next-token".to_owned(),
            }]
        );
    }

    #[test]
    fn syntax_and_runtime_errors_are_structured() {
        let syntax = execute_pre_request(
            "const = ;",
            &request(),
            &ScriptScope::default(),
            &RequestNamespaceCatalog::default(),
            &ScriptCancellation::new(),
        )
        .expect_err("invalid JavaScript must fail");
        assert_eq!(syntax.diagnostic.kind, ScriptErrorKind::Syntax);
        assert_eq!(syntax.diagnostic.filename, "pre-request.js");

        let runtime = execute_pre_request(
            "throw new Error('broken');",
            &request(),
            &ScriptScope::default(),
            &RequestNamespaceCatalog::default(),
            &ScriptCancellation::new(),
        )
        .expect_err("thrown error must fail");
        assert_eq!(runtime.diagnostic.kind, ScriptErrorKind::Runtime);
        assert!(runtime.diagnostic.message.contains("broken"));
        assert!(
            runtime
                .diagnostic
                .stack
                .as_deref()
                .is_some_and(|stack| stack.contains("pre-request.js"))
        );
    }

    #[test]
    fn runtime_errors_retain_console_output_recorded_before_throwing() {
        let mut scope = ScriptScope::default();
        scope.environment.insert_secret("token", "console-secret");
        let error = execute_pre_request(
            r#"
console.info("before", api.environment.get("token"));
throw new Error("broken");
"#,
            &request(),
            &scope,
            &RequestNamespaceCatalog::default(),
            &ScriptCancellation::new(),
        )
        .expect_err("thrown error must fail");

        assert_eq!(error.diagnostic.kind, ScriptErrorKind::Runtime);
        assert_eq!(error.report.phase, ScriptPhase::PreRequest);
        assert_eq!(error.report.logs.len(), 1);
        assert_eq!(error.report.logs[0].level, ScriptLogLevel::Info);
        assert_eq!(error.report.logs[0].message, "before [REDACTED]");
        assert!(error.report.tests.is_empty());
    }

    #[test]
    fn infinite_loop_times_out_even_inside_try_catch() {
        let started = Instant::now();
        let error = execute_pre_request(
            "try { while (true) {} } catch (_) {}",
            &request(),
            &ScriptScope::default(),
            &RequestNamespaceCatalog::default(),
            &ScriptCancellation::new(),
        )
        .expect_err("interrupt must be uncatchable");

        assert_eq!(error.diagnostic.kind, ScriptErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn cancellation_interrupts_running_script() {
        let cancellation = ScriptCancellation::new();
        let worker_cancellation = cancellation.clone();
        let worker = thread::spawn(move || {
            execute_pre_request(
                "while (true) {}",
                &request(),
                &ScriptScope::default(),
                &RequestNamespaceCatalog::default(),
                &worker_cancellation,
            )
        });

        thread::sleep(Duration::from_millis(20));
        cancellation.cancel();
        let error = worker
            .join()
            .expect("script worker should not panic")
            .expect_err("cancelled script should fail");
        assert_eq!(error.diagnostic.kind, ScriptErrorKind::Cancelled);
    }

    #[test]
    fn runtimes_are_isolated_and_secrets_are_scrubbed() {
        execute_pre_request(
            "Array.prototype.leaked = true;",
            &request(),
            &ScriptScope::default(),
            &RequestNamespaceCatalog::default(),
            &ScriptCancellation::new(),
        )
        .expect("first script should succeed");

        let mut scope = ScriptScope::default();
        scope
            .environment
            .insert_secret("token", "very-secret-token");
        let result = execute_pre_request(
            r#"
if (Array.prototype.leaked) throw new Error("prototype leaked");
console.log(api.environment.get("token"));
"#,
            &request(),
            &scope,
            &RequestNamespaceCatalog::default(),
            &ScriptCancellation::new(),
        )
        .expect("second script should have a fresh runtime");
        assert_eq!(result.report.logs[0].message, "[REDACTED]");

        let error = execute_pre_request(
            r#"throw new Error(api.environment.get("token"));"#,
            &request(),
            &scope,
            &RequestNamespaceCatalog::default(),
            &ScriptCancellation::new(),
        )
        .expect_err("script should throw");
        assert!(!error.diagnostic.message.contains("very-secret-token"));
        assert_eq!(error.diagnostic.message, "[REDACTED]");
    }

    #[test]
    fn rotated_and_short_secret_values_are_scrubbed_from_reports() {
        let mut scope = ScriptScope::default();
        scope.environment.insert_secret("pin", "42");
        let result = execute_pre_request(
            r#"
api.environment.set("pin", "7");
console.log("old", "42", "new", api.environment.get("pin"));
"#,
            &request(),
            &scope,
            &RequestNamespaceCatalog::default(),
            &ScriptCancellation::new(),
        )
        .expect("secret rotation should succeed");

        assert_eq!(
            result.report.logs[0].message,
            "old [REDACTED] new [REDACTED]"
        );

        let error = execute_pre_request(
            r#"
api.environment.set("pin", "a b/c");
throw new Error("rotated=a%20b/c");
"#,
            &request(),
            &scope,
            &RequestNamespaceCatalog::default(),
            &ScriptCancellation::new(),
        )
        .expect_err("rotated secret must remain scrubbed when the script throws");
        assert_eq!(error.diagnostic.message, "rotated=[REDACTED]");

        let mut encoded_scope = ScriptScope::default();
        encoded_scope
            .environment
            .insert_secret("path_token", "a b/c");
        let encoded = execute_pre_request(
            r#"console.log("url=https://example.test/a%20b/c");"#,
            &request(),
            &encoded_scope,
            &RequestNamespaceCatalog::default(),
            &ScriptCancellation::new(),
        )
        .expect("encoded secret log should be scrubbed");
        assert_eq!(
            encoded.report.logs[0].message,
            "url=https://example.test/[REDACTED]"
        );
    }

    #[test]
    fn redactor_scrub_matches_derived_variant_application() {
        // The legacy path re-derived `secret_variants` on every scrub and
        // applied them with chained replace. The cached-variant redactor must
        // produce byte-identical output.
        let secrets = [
            "a b/c".to_owned(),
            "top-secret".to_owned(),
            "42".to_owned(),
            "a b/c".to_owned(), // duplicate must be dropped
            String::new(),      // empty secrets are dropped
        ];
        let redactor = SecretRedactor::new(secrets);
        // Longest-first, ties broken by value.
        assert_eq!(redactor.secrets, vec!["top-secret", "a b/c", "42"]);
        assert_eq!(
            redactor.variants,
            super::super::template::secret_variants(&redactor.secrets)
        );

        for text in [
            "plain text",
            "raw a b/c form a+b%2Fc path a%20b/c",
            "token top-secret and pin 42",
            "a b/c then top-secret then 42",
            "",
        ] {
            let expected = super::super::template::secret_variants(&redactor.secrets)
                .iter()
                .fold(text.to_owned(), |text, value| {
                    text.replace(value, super::super::template::REDACTED_VALUE)
                });
            assert_eq!(
                redactor.scrub(text),
                expected,
                "redactor scrub diverged from variant application for {text:?}"
            );
        }
        for needle in ["a b/c", "a+b%2Fc", "a%20b/c", "top-secret", "42"] {
            assert!(
                !redactor.scrub("a b/c top-secret 42").contains(needle),
                "redactor leaked spelling {needle:?}"
            );
        }
    }

    #[test]
    fn redactor_variants_are_cached_and_invalidate_only_on_change() {
        let mut redactor = SecretRedactor::new(["first".to_owned()]);
        let cached = redactor.variants.clone();
        assert_eq!(redactor.scrub("first"), "[REDACTED]");
        assert_eq!(redactor.scrub("other"), "other");

        // Re-adding the same value and adding empty values must not rebuild.
        redactor.add("first");
        redactor.add("");
        assert_eq!(redactor.variants, cached);

        // A genuinely new secret invalidates the cache and scrubs thereafter.
        redactor.add("second");
        assert_ne!(redactor.variants, cached);
        assert_eq!(redactor.scrub("first second"), "[REDACTED] [REDACTED]");
        assert_eq!(redactor.scrub("third"), "third");

        // extend rebuilds once for the whole batch.
        let mut batch = redactor.clone();
        batch.extend(["third".to_owned(), "fourth".to_owned(), "first".to_owned()]);
        assert_eq!(batch.scrub("first fourth"), "[REDACTED] [REDACTED]");
    }

    #[test]
    fn redactor_scrubs_overlapping_secrets_longest_first() {
        // The shorter spelling must not consume the start of a longer secret.
        let mut redactor = SecretRedactor::new(["abc".to_owned(), "abcd".to_owned()]);
        assert_eq!(redactor.scrub("xabcd tail"), "x[REDACTED] tail");
        // Keeping the collection sorted longest-first is what makes the
        // chained application correct for overlaps.
        assert_eq!(redactor.secrets, vec!["abcd", "abc"]);
        assert_eq!(
            redactor.scrub("key=abcd token=abc"),
            "key=[REDACTED] token=[REDACTED]"
        );
        redactor.add("xy");
        assert_eq!(redactor.secrets, vec!["abcd", "abc", "xy"]);
        assert_eq!(redactor.scrub("xy tails"), "[REDACTED] tails");
    }

    #[test]
    fn pre_request_output_body_limit_is_enforced() {
        let source = format!(
            "api.request.body = \"x\".repeat({});",
            MAX_SCRIPT_BODY_BYTES + 1
        );
        let error = execute_pre_request(
            &source,
            &request(),
            &ScriptScope::default(),
            &RequestNamespaceCatalog::default(),
            &ScriptCancellation::new(),
        )
        .expect_err("a pre-script must not bypass the request-body limit");

        assert_eq!(error.diagnostic.kind, ScriptErrorKind::BodyLimit);
        assert!(
            error
                .diagnostic
                .message
                .contains("pre-request script produced")
        );
    }

    #[test]
    fn empty_scripts_do_not_apply_script_body_limits() {
        let mut oversized = request();
        oversized.body = "x".repeat(MAX_SCRIPT_BODY_BYTES + 1);
        let pre = execute_pre_request(
            "",
            &oversized,
            &ScriptScope::default(),
            &RequestNamespaceCatalog::default(),
            &ScriptCancellation::new(),
        )
        .expect("blank pre-script should not inspect the body");
        assert_eq!(pre.request.body.len(), MAX_SCRIPT_BODY_BYTES + 1);

        let response = ResponseData {
            status: 200,
            status_text: "OK".to_owned(),
            http_version: "HTTP/1.1".to_owned(),
            final_url: "https://example.test".to_owned(),
            headers: Vec::new(),
            content_type: None,
            body: Vec::new().into(),
            duration: Duration::from_millis(1),
        };
        execute_post_response(
            "",
            &oversized,
            &response,
            &ScriptScope::default(),
            &RequestNamespaceCatalog::default(),
            &ScriptCancellation::new(),
        )
        .expect("blank post-script should not inspect the request body");
    }

    #[test]
    fn async_test_callbacks_are_rejected_instead_of_false_passing() {
        let response = ResponseData {
            status: 200,
            status_text: "OK".to_owned(),
            http_version: "HTTP/1.1".to_owned(),
            final_url: "https://example.test".to_owned(),
            headers: Vec::new(),
            content_type: Some("application/json".to_owned()),
            body: br#"{"ok":true}"#.to_vec().into(),
            duration: Duration::from_millis(1),
        };
        let result = execute_post_response(
            r#"api.test("async", async () => { throw new Error("late failure"); });"#,
            &request(),
            &response,
            &ScriptScope::default(),
            &RequestNamespaceCatalog::default(),
            &ScriptCancellation::new(),
        )
        .expect("unsupported async test should be recorded as a failed test");

        assert!(!result.report.tests[0].passed);
        assert_eq!(
            result.report.tests[0].message.as_deref(),
            Some("async test callbacks are not supported")
        );
    }

    #[test]
    fn binary_body_is_base64_encoded_and_truncation_is_reported() {
        assert_eq!(encode_base64(&[0xff, 0x00, 0x01]), "/wAB");

        let response = ResponseData {
            status: 200,
            status_text: "OK".to_owned(),
            http_version: "HTTP/1.1".to_owned(),
            final_url: "https://example.test/binary".to_owned(),
            headers: Vec::new(),
            content_type: Some("application/octet-stream".to_owned()),
            body: vec![0xff; MAX_SCRIPT_BODY_BYTES + 1].into(),
            duration: Duration::from_millis(1),
        };
        let result = execute_post_response(
            r#"api.test("binary", () => api.assert(api.response.bodyBase64.length > 0));"#,
            &request(),
            &response,
            &ScriptScope::default(),
            &RequestNamespaceCatalog::default(),
            &ScriptCancellation::new(),
        )
        .expect("binary response should be available to script");
        assert!(result.report.response_body_truncated);
        assert!(result.report.tests[0].passed);
    }

    fn chaining_workspace() -> (Workspace, RequestNamespaceCatalog) {
        let mut workspace = Workspace::default();
        let chat = workspace.create_collection("ChatAdmin").unwrap();
        workspace
            .create_saved_request(
                &chat,
                "Login",
                super::super::template::RequestTemplate {
                    request: RequestDraft::new("POST", "https://a.test/login"),
                    scripts: Default::default(),
                    websocket: None,
                },
            )
            .unwrap();
        let payments = workspace.create_collection("Payments").unwrap();
        workspace
            .create_saved_request(
                &payments,
                "Login",
                super::super::template::RequestTemplate {
                    request: RequestDraft::new("POST", "https://p.test/login"),
                    scripts: Default::default(),
                    websocket: None,
                },
            )
            .unwrap();
        let catalog = RequestNamespaceCatalog::from_workspace(&workspace);
        (workspace, catalog)
    }

    #[test]
    fn pre_request_records_scheduled_request_references_in_order() {
        let (_workspace, catalog) = chaining_workspace();
        let result = execute_pre_request(
            r#"
api.requests.execute(ChatAdmin.Login);
api.requests.execute(Payments.Login);
"#,
            &request(),
            &ScriptScope::default(),
            &catalog,
            &ScriptCancellation::new(),
        )
        .expect("scheduling chained requests should succeed");

        assert_eq!(result.chained_requests.len(), 2);
        assert_eq!(result.chained_requests[0].path, "ChatAdmin.Login");
        assert_eq!(result.chained_requests[1].path, "Payments.Login");
        // The two refs carry distinct stable ids.
        assert_ne!(result.chained_requests[0].id, result.chained_requests[1].id);
        assert!(!result.chained_requests[0].id.is_empty());
    }

    #[test]
    fn execute_rejects_arbitrary_and_namespace_references_with_clear_diagnostics() {
        let (_workspace, catalog) = chaining_workspace();

        // A primitive string is not a request reference.
        let primitive = execute_pre_request(
            r#"api.requests.execute("ChatAdmin.Login");"#,
            &request(),
            &ScriptScope::default(),
            &catalog,
            &ScriptCancellation::new(),
        )
        .expect_err("a string must be rejected");
        assert!(
            primitive
                .diagnostic
                .message
                .contains("saved request reference")
        );

        // A namespace object (a folder/collection) is not a request leaf.
        let namespace = execute_pre_request(
            r#"api.requests.execute(ChatAdmin);"#,
            &request(),
            &ScriptScope::default(),
            &catalog,
            &ScriptCancellation::new(),
        )
        .expect_err("a namespace must be rejected");
        assert!(namespace.diagnostic.message.contains("namespace"));

        // An arbitrary object is not a request reference.
        let arbitrary = execute_pre_request(
            r#"api.requests.execute({});"#,
            &request(),
            &ScriptScope::default(),
            &catalog,
            &ScriptCancellation::new(),
        )
        .expect_err("an arbitrary object must be rejected");
        assert!(arbitrary.diagnostic.message.contains("arbitrary object"));
    }

    #[test]
    fn request_namespace_objects_are_frozen_and_leaves_neither_enum() {
        let (_workspace, catalog) = chaining_workspace();
        // Namespaces and request leaves are frozen, so assignment is a no-op in
        // strict mode (throws) rather than silently mutating shared state.
        let result = execute_pre_request(
            r#"
"use strict";
try {
  ChatAdmin.Users = {};
  ChatAdmin.Login = {};
  ChatAdmin.extra = {};
} catch (_) {}
api.requests.execute(ChatAdmin.Login);
"#,
            &request(),
            &ScriptScope::default(),
            &catalog,
            &ScriptCancellation::new(),
        )
        .expect("frozen namespace access should not throw from read");
        assert_eq!(result.chained_requests.len(), 1);
        let _ = result;
    }

    #[test]
    fn post_response_can_schedule_chained_requests() {
        let response = ResponseData {
            status: 200,
            status_text: "OK".to_owned(),
            http_version: "HTTP/1.1".to_owned(),
            final_url: "https://example.test/".to_owned(),
            headers: Vec::new(),
            content_type: None,
            body: Vec::new().into(),
            duration: Duration::from_millis(1),
        };
        let (_workspace, catalog) = chaining_workspace();
        let result = execute_post_response(
            r#"api.requests.execute(Payments.Login);"#,
            &request(),
            &response,
            &ScriptScope::default(),
            &catalog,
            &ScriptCancellation::new(),
        )
        .expect("post-response scheduling should succeed");
        assert_eq!(result.chained_requests.len(), 1);
        assert_eq!(result.chained_requests[0].path, "Payments.Login");
    }

    #[test]
    fn top_level_await_resolves_and_persists() {
        let result = execute_pre_request(
            r#"
const value = await Promise.resolve(41);
api.environment.set("sum", String(value + 1));
"#,
            &request(),
            &ScriptScope::default(),
            &RequestNamespaceCatalog::default(),
            &ScriptCancellation::new(),
        )
        .expect("top-level await should run to completion");
        assert_eq!(
            result.environment_mutations,
            vec![EnvironmentMutation::Set {
                key: "sum".to_owned(),
                value: "42".to_owned(),
            }]
        );
    }

    #[test]
    fn top_level_await_can_schedule_chained_requests() {
        let (_workspace, catalog) = chaining_workspace();
        let result = execute_pre_request(
            r#"
await Promise.resolve();
api.requests.execute(ChatAdmin.Login);
"#,
            &request(),
            &ScriptScope::default(),
            &catalog,
            &ScriptCancellation::new(),
        )
        .expect("top-level await with scheduling");
        assert_eq!(result.chained_requests.len(), 1);
        assert_eq!(result.chained_requests[0].path, "ChatAdmin.Login");
    }

    #[test]
    fn top_level_await_rejection_surfaces_error() {
        let error = execute_pre_request(
            r#"
await Promise.reject(new Error("async-broken"));
"#,
            &request(),
            &ScriptScope::default(),
            &RequestNamespaceCatalog::default(),
            &ScriptCancellation::new(),
        )
        .expect_err("a rejected top-level await must fail the script");
        assert_eq!(error.diagnostic.kind, ScriptErrorKind::Runtime);
        assert!(
            error.diagnostic.message.contains("async-broken"),
            "rejection message: {}",
            error.diagnostic.message
        );
    }

    #[test]
    fn never_settling_top_level_await_reports_timeout() {
        let error = execute_pre_request(
            r#"
await new Promise(function(){});
"#,
            &request(),
            &ScriptScope::default(),
            &RequestNamespaceCatalog::default(),
            &ScriptCancellation::new(),
        )
        .expect_err("a top-level await on a never-resolving promise must not hang");
        assert_eq!(error.diagnostic.kind, ScriptErrorKind::TimedOut);
    }

    #[test]
    fn chained_async_function_and_top_level_await_run_to_completion() {
        let result = execute_pre_request(
            r#"
async function compute() {
  return await Promise.resolve(7);
}
const value = await compute();
api.environment.set("seven", String(value));
"#,
            &request(),
            &ScriptScope::default(),
            &RequestNamespaceCatalog::default(),
            &ScriptCancellation::new(),
        )
        .expect("mixed nested async fn + top-level await should run");
        assert_eq!(
            result.environment_mutations,
            vec![EnvironmentMutation::Set {
                key: "seven".to_owned(),
                value: "7".to_owned(),
            }]
        );
    }

    #[test]
    fn awaited_execute_applies_env_mutations_before_continuing() {
        let (_workspace, catalog) = chaining_workspace();
        let chainer =
            |_scheduled: &[crate::core::ChainedRequest]| -> Vec<Result<crate::core::ChainRun, String>> {
                vec![Ok(crate::core::ChainRun {
                    environment_mutations: vec![EnvironmentMutation::Set {
                        key: "AUTH_TOKEN".to_owned(),
                        value: "tok-123".to_owned(),
                    }],
                    ..Default::default()
                })]
            };
        let result = execute_pre_request_with_chain(
            r#"
const token = api.environment.get("AUTH_TOKEN");
if (!token || token.trim() === "") {
  await api.requests.execute(ChatAdmin.Login);
  api.environment.set("saw", api.environment.get("AUTH_TOKEN"));
}
api.request.headers.set("Authorization", "Bearer " + api.environment.get("AUTH_TOKEN"));
"#,
            &request(),
            &ScriptScope::default(),
            &catalog,
            &ScriptCancellation::new(),
            Some(&chainer),
        )
        .expect("awaited execute should run inline");

        // The statement after the await must have observed the refreshed token.
        assert!(
            result
                .environment_mutations
                .contains(&EnvironmentMutation::Set {
                    key: "saw".to_owned(),
                    value: "tok-123".to_owned(),
                }),
            "mutations: {:?}",
            result.environment_mutations
        );
        assert!(
            result.request.headers.iter().any(|header| {
                header.name.eq_ignore_ascii_case("Authorization")
                    && header.value.contains("tok-123")
            }),
            "Authorization header did not carry the refreshed token: {:?}",
            result.request.headers
        );
    }

    #[test]
    fn awaited_execute_failure_stops_the_script_before_continuing() {
        let (_workspace, catalog) = chaining_workspace();
        let chainer =
            |_scheduled: &[crate::core::ChainedRequest]| -> Vec<Result<crate::core::ChainRun, String>> {
                vec![Err("chained login failed".to_owned())]
            };
        let error = execute_pre_request_with_chain(
            r#"
await api.requests.execute(ChatAdmin.Login);
api.request.headers.set("Authorization", "Bearer SHOULD-NOT-RUN");
"#,
            &request(),
            &ScriptScope::default(),
            &catalog,
            &ScriptCancellation::new(),
            Some(&chainer),
        )
        .expect_err("a failed awaited request must reject the script");
        assert_eq!(error.diagnostic.kind, ScriptErrorKind::Runtime);
        assert!(
            error.diagnostic.message.contains("chained login failed"),
            "message: {}",
            error.diagnostic.message
        );
    }

    #[test]
    fn promise_all_executes_chained_requests_concurrently() {
        let (_workspace, catalog) = chaining_workspace();
        // The provider (execution.rs) drives the whole awaited batch on ONE
        // shared async runtime via join_all, so concurrent chains' network
        // waits genuinely overlap instead of running back-to-back. This fake
        // mirrors that production pattern: two pipelines on a single
        // current-thread runtime must overlap (max in-flight 2), not serialize.
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let in_flight = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let max_in_flight = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let probe_calls = calls.clone();
        let probe_in_flight = in_flight.clone();
        let probe_max = max_in_flight.clone();
        let chainer = move |requested: &[crate::core::ChainedRequest]| {
            probe_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime");
            runtime.block_on(async {
                let futures = requested
                    .iter()
                    .map(|_| {
                        let in_flight = probe_in_flight.clone();
                        let max_in_flight = probe_max.clone();
                        async move {
                            let current =
                                in_flight.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                            max_in_flight.fetch_max(current, std::sync::atomic::Ordering::SeqCst);
                            tokio::time::sleep(std::time::Duration::from_millis(120)).await;
                            in_flight.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                            Ok::<crate::core::ChainRun, String>(crate::core::ChainRun::default())
                        }
                    })
                    .collect::<Vec<_>>();
                futures::future::join_all(futures).await
            })
        };
        let result = execute_pre_request_with_chain(
            r#"
await Promise.all([
  api.requests.execute(ChatAdmin.Login),
  api.requests.execute(Payments.Login),
]);
api.environment.set("done", "yes");
"#,
            &request(),
            &ScriptScope::default(),
            &catalog,
            &ScriptCancellation::new(),
            Some(&chainer),
        )
        .expect("Promise.all over execute() should settle");

        // The whole awaited set is handed to the provider in ONE batch call.
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        // The two pipelines overlapped their waits on the shared runtime.
        assert_eq!(
            max_in_flight.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "awaited chains must run concurrently (Promise.all semantics)"
        );
        assert!(
            result
                .environment_mutations
                .contains(&EnvironmentMutation::Set {
                    key: "done".to_owned(),
                    value: "yes".to_owned(),
                }),
            "script did not continue after Promise.all settled: {:?}",
            result.environment_mutations
        );
    }

    #[test]
    fn custom_script_timeout_is_honored() {
        let mut scope = ScriptScope::default();
        scope.script_timeout = std::time::Duration::from_millis(250);
        let started = Instant::now();
        let error = execute_pre_request(
            "while (true) {}",
            &request(),
            &scope,
            &RequestNamespaceCatalog::default(),
            &ScriptCancellation::new(),
        )
        .expect_err("a CPU-bound script must hit the configured deadline");
        assert_eq!(error.diagnostic.kind, ScriptErrorKind::TimedOut);
        assert!(
            error.diagnostic.message.contains("250 ms execution limit"),
            "timeout message should reflect the configured 250ms budget: {}",
            error.diagnostic.message
        );
        // A 250ms budget must stop the loop well before the old 1s default.
        assert!(
            started.elapsed() < Duration::from_millis(900),
            "custom timeout not honored; elapsed {:?}",
            started.elapsed()
        );
    }
}
