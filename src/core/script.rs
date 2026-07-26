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

use super::{RequestDraft, ResponseData, template::redact_secret_values};

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

  const own = (object, key) => Object.prototype.hasOwnProperty.call(object, key);
  const normalizeName = value => String(value).trim().toLowerCase();

  function makeHeaders(source, mutable) {
    const rows = source.map(header => ({
      enabled: header.enabled !== false,
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
          rows.push({ enabled: true, name: headerName, value: String(value) });
        }
      },
      append(name, value) {
        ensureMutable();
        const headerName = String(name).trim();
        if (!headerName) throw new TypeError("header name cannot be empty");
        rows.push({ enabled: true, name: headerName, value: String(value) });
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
    value: () => ({
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
    }),
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScriptScope {
    pub environment: ScriptEnvironment,
    pub collection_variables: BTreeMap<String, String>,
    pub extra_secrets: Vec<String>,
}

impl ScriptScope {
    pub fn redactor(&self) -> SecretRedactor {
        let mut redactor = SecretRedactor::default();
        for name in &self.environment.secret_names {
            if let Some(value) = self.environment.values.get(name) {
                redactor.add(value.clone());
            }
        }
        redactor.extend(self.extra_secrets.iter().cloned());
        redactor
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SecretRedactor {
    secrets: Vec<String>,
}

#[allow(dead_code)]
impl SecretRedactor {
    pub fn new(secrets: impl IntoIterator<Item = String>) -> Self {
        let mut redactor = Self::default();
        redactor.extend(secrets);
        redactor
    }

    pub fn add(&mut self, secret: impl Into<String>) {
        let secret = secret.into();
        if !secret.is_empty() && !self.secrets.contains(&secret) {
            self.secrets.push(secret);
            self.secrets
                .sort_unstable_by_key(|value| std::cmp::Reverse(value.len()));
        }
    }

    pub fn extend(&mut self, secrets: impl IntoIterator<Item = String>) {
        for secret in secrets {
            self.add(secret);
        }
    }

    pub fn scrub(&self, text: impl AsRef<str>) -> String {
        redact_secret_values(text.as_ref(), &self.secrets)
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
    pub report: ScriptReport,
}

#[derive(Clone, Debug)]
pub struct PostResponseResult {
    pub environment_mutations: Vec<EnvironmentMutation>,
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
    logs: Vec<ScriptLog>,
    tests: Vec<ScriptTestResult>,
}

struct EngineRun {
    output: EngineOutput,
    duration: Duration,
}

pub fn execute_pre_request(
    source: &str,
    request: &RequestDraft,
    scope: &ScriptScope,
    cancellation: &ScriptCancellation,
) -> Result<PreRequestResult, ScriptError> {
    let phase = ScriptPhase::PreRequest;
    if source.trim().is_empty() {
        return Ok(PreRequestResult {
            request: request.clone(),
            environment_mutations: Vec::new(),
            report: ScriptReport::empty(phase),
        });
    }

    validate_source_and_body(phase, source, request, &scope.redactor())?;
    let mut redactor = scope.redactor();
    let input = EngineInput {
        phase: phase.engine_name(),
        request,
        response: None,
        environment: scope.environment.values(),
        collection_variables: &scope.collection_variables,
        max_log_entries: MAX_SCRIPT_LOG_ENTRIES,
        max_log_bytes: MAX_SCRIPT_LOG_BYTES,
    };
    let run = run_engine(
        source,
        phase,
        input,
        cancellation,
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
        report,
    })
}

pub fn execute_post_response(
    source: &str,
    request: &RequestDraft,
    response: &ResponseData,
    scope: &ScriptScope,
    cancellation: &ScriptCancellation,
) -> Result<PostResponseResult, ScriptError> {
    let phase = ScriptPhase::PostResponse;
    if source.trim().is_empty() {
        return Ok(PostResponseResult {
            environment_mutations: Vec::new(),
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
        max_log_entries: MAX_SCRIPT_LOG_ENTRIES,
        max_log_bytes: MAX_SCRIPT_LOG_BYTES,
    };
    let run = run_engine(
        source,
        phase,
        input,
        cancellation,
        &redactor,
        &scope.environment.secret_names,
    )?;
    extend_redactor_with_secret_mutations(&mut redactor, scope, &run.output.environment_mutations);
    let report = report_from_run(phase, &run, truncated, &redactor);

    Ok(PostResponseResult {
        environment_mutations: run.output.environment_mutations,
        report,
    })
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
    let deadline = started + SCRIPT_TIMEOUT;
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

    context.with(|ctx| {
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
        options.promise = false;
        options.filename = Some(phase.filename().to_owned());
        if let Err(caught) = ctx.eval_with_options::<(), _>(source, options).catch(&ctx) {
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
                        SCRIPT_TIMEOUT.as_millis()
                    ),
                    duration,
                    redactor,
                ));
            }
            let mut caught_redactor = redactor.clone();
            if let Ok(value) = ctx.eval::<Value<'_>, _>("__API_TESTER_FINISH()")
                && let Ok(output) = rquickjs_serde::from_value_strict::<EngineOutput>(value)
            {
                extend_redactor_with_secret_names(
                    &mut caught_redactor,
                    secret_names,
                    &output.environment_mutations,
                );
            }
            return Err(caught_error(phase, caught, duration, &caught_redactor));
        }

        let value: Value<'_> = ctx.eval("__API_TESTER_FINISH()").map_err(|error| {
            engine_error(
                phase,
                ScriptErrorKind::Runtime,
                format!("could not collect script output: {error}"),
                started.elapsed(),
                redactor,
            )
        })?;
        let output: EngineOutput = rquickjs_serde::from_value_strict(value).map_err(|error| {
            engine_error(
                phase,
                ScriptErrorKind::Runtime,
                format!("script produced invalid output: {error}"),
                started.elapsed(),
                redactor,
            )
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
    })
}

fn report_from_run(
    phase: ScriptPhase,
    run: &EngineRun,
    response_body_truncated: bool,
    redactor: &SecretRedactor,
) -> ScriptReport {
    let mut used_bytes = 0;
    let logs = run
        .output
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
    let tests = run
        .output
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
        duration: run.duration,
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
            body: br#"{"token":"next-token"}"#.to_vec(),
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
            &ScriptCancellation::new(),
        )
        .expect_err("invalid JavaScript must fail");
        assert_eq!(syntax.diagnostic.kind, ScriptErrorKind::Syntax);
        assert_eq!(syntax.diagnostic.filename, "pre-request.js");

        let runtime = execute_pre_request(
            "throw new Error('broken');",
            &request(),
            &ScriptScope::default(),
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
    fn infinite_loop_times_out_even_inside_try_catch() {
        let started = Instant::now();
        let error = execute_pre_request(
            "try { while (true) {} } catch (_) {}",
            &request(),
            &ScriptScope::default(),
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
            &ScriptCancellation::new(),
        )
        .expect("second script should have a fresh runtime");
        assert_eq!(result.report.logs[0].message, "[REDACTED]");

        let error = execute_pre_request(
            r#"throw new Error(api.environment.get("token"));"#,
            &request(),
            &scope,
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
            &ScriptCancellation::new(),
        )
        .expect("encoded secret log should be scrubbed");
        assert_eq!(
            encoded.report.logs[0].message,
            "url=https://example.test/[REDACTED]"
        );
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
            body: Vec::new(),
            duration: Duration::from_millis(1),
        };
        execute_post_response(
            "",
            &oversized,
            &response,
            &ScriptScope::default(),
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
            body: br#"{"ok":true}"#.to_vec(),
            duration: Duration::from_millis(1),
        };
        let result = execute_post_response(
            r#"api.test("async", async () => { throw new Error("late failure"); });"#,
            &request(),
            &response,
            &ScriptScope::default(),
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
            body: vec![0xff; MAX_SCRIPT_BODY_BYTES + 1],
            duration: Duration::from_millis(1),
        };
        let result = execute_post_response(
            r#"api.test("binary", () => api.assert(api.response.bodyBase64.length > 0));"#,
            &request(),
            &response,
            &ScriptScope::default(),
            &ScriptCancellation::new(),
        )
        .expect("binary response should be available to script");
        assert!(result.report.response_body_truncated);
        assert!(result.report.tests[0].passed);
    }
}
