(() => {
  "use strict";

  const input = globalThis.__RESOLVED_SNIPPET_INPUT;
  const logs = [];
  const writes = [];
  let logCharacters = 0;
  let outputCharacters = 0;

  const apply = Reflect.apply;
  const arrayJoin = Array.prototype.join;
  const arrayMap = Array.prototype.map;
  const freeze = Object.freeze;
  const hasOwn = Object.prototype.hasOwnProperty;
  const own = (object, key) => apply(hasOwn, object, [key]);
  const normalizeName = value => String(value).trim().toLowerCase();

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
    if (logs.length >= input.maxLogEntries || logCharacters >= input.maxLogCharacters) return;
    let message = apply(arrayJoin, apply(arrayMap, values, [printable]), [" "]);
    const remaining = input.maxLogCharacters - logCharacters;
    if (message.length > remaining) message = message.slice(0, remaining);
    logCharacters += message.length;
    logs.push({ level, message });
  }

  const snippetConsole = Object.freeze({
    log: (...values) => writeLog("log", values),
    info: (...values) => writeLog("info", values),
    warn: (...values) => writeLog("warn", values),
    error: (...values) => writeLog("error", values),
    debug: (...values) => writeLog("debug", values),
  });

  function makeHeaders(source) {
    const rows = (Array.isArray(source) ? source : []).map(header => Object.freeze({
      enabled: header.enabled !== false,
      name: String(header.name ?? ""),
      value: String(header.value ?? ""),
    }));
    Object.freeze(rows);

    return Object.freeze({
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
      toArray() {
        return rows.map(row => ({ ...row }));
      },
    });
  }

  function makeRequest(source) {
    const bodyFields = (Array.isArray(source.bodyFields) ? source.bodyFields : []).map(field =>
      Object.freeze({
        enabled: field.enabled !== false,
        name: String(field.name ?? ""),
        value: String(field.value ?? ""),
        kind: String(field.kind ?? "text"),
      }),
    );
    Object.freeze(bodyFields);
    return Object.freeze({
      method: String(source.method),
      url: String(source.url),
      headers: makeHeaders(source.headers),
      body: String(source.body),
      bodyMode: String(source.bodyMode),
      rawBodyLanguage: String(source.rawBodyLanguage),
      bodyFields,
      bodyTruncated: source.bodyTruncated === true,
    });
  }

  function makeResponse(source) {
    if (!source) return null;
    return Object.freeze({
      status: Number(source.status),
      statusText: String(source.statusText),
      httpVersion: String(source.httpVersion),
      url: String(source.url),
      contentType: source.contentType == null ? null : String(source.contentType),
      headers: makeHeaders(source.headers),
      durationMs: Number(source.durationMs),
      sizeBytes: Number(source.sizeBytes),
      bodyTruncated: source.bodyTruncated === true,
      text: () => String(source.bodyText),
      json: () => JSON.parse(String(source.bodyText)),
    });
  }

  const request = makeRequest(input.request);
  const response = input.category === "post-response" ? makeResponse(input.response) : null;
  const api = { request };
  if (input.category === "post-response") api.response = response;
  Object.freeze(api);

  function pathSegments() {
    const path = input.selection && input.selection.jsonPath;
    if (!path || !Array.isArray(path.segments)) {
      throw new TypeError("the selected block is not mapped to a JSON value");
    }
    return path.segments;
  }

  function propertySuffix(segments) {
    let output = "";
    for (const segment of segments) {
      if (segment.kind === "index") {
        output += `[${Number(segment.value)}]`;
      } else {
        const key = String(segment.value);
        output += /^[A-Za-z_$][A-Za-z0-9_$]*$/.test(key)
          ? `.${key}`
          : `[${JSON.stringify(key)}]`;
      }
    }
    return output;
  }

  function jsonPath(segments) {
    return `$${propertySuffix(segments)}`;
  }

  function jsonPointer(segments) {
    return segments
      .map(segment => {
        const value = String(segment.value);
        return `/${value.replace(/~/g, "~0").replace(/\//g, "~1")}`;
      })
      .join("");
  }

  function makeSelection(source) {
    if (!source) return null;
    const range = Object.freeze({
      start: Number(source.range.start),
      end: Number(source.range.end),
    });
    return Object.freeze({
      source: String(source.source),
      area: String(source.area),
      text: String(source.text),
      start: range.start,
      end: range.end,
      range,
      contentType: source.contentType == null ? null : String(source.contentType),
      is(value) {
        return String(value).trim().toLowerCase() === String(source.source).toLowerCase();
      },
      jsonPath() {
        return jsonPath(pathSegments());
      },
      jsonPointer() {
        return jsonPointer(pathSegments());
      },
      expression(options = {}) {
        if (options == null || typeof options !== "object") {
          throw new TypeError("snippet.selection.expression options must be an object");
        }
        let root = options.root;
        if (root == null) {
          if (
            input.category === "post-response" &&
            source.source === "response" &&
            source.area === "body"
          ) {
            root = "api.response.json()";
          } else if (source.source === "request" && source.area === "body") {
            root = "JSON.parse(api.request.body)";
          } else {
            throw new TypeError("a root expression is required for this selected block");
          }
        }
        root = String(root).trim();
        if (!root) throw new TypeError("the root expression cannot be empty");
        return `${root}${propertySuffix(pathSegments())}`;
      },
    });
  }

  const selection = makeSelection(input.selection);

  function outputLimitError() {
    const error = new RangeError(
      `generated snippet exceeds its ${input.maxOutputCharacters} character limit`,
    );
    error.name = "SnippetOutputLimitError";
    return error;
  }

  function write(value) {
    const text = String(value);
    outputCharacters += text.length;
    if (outputCharacters > input.maxOutputCharacters) throw outputLimitError();
    writes.push(text);
  }

  function result(text, options = {}) {
    if (options == null || typeof options !== "object") {
      throw new TypeError("snippet.result options must be an object");
    }
    return freeze({
      text: String(text),
      cursor: options.cursor == null ? null : options.cursor,
    });
  }

  const snippet = Object.freeze({
    apiVersion: Number(input.apiVersion),
    category: String(input.category),
    outputLanguage: String(input.outputLanguage),
    selection,
    write,
    result,
  });

  function integerOrNull(value, name) {
    if (value == null) return null;
    if (!Number.isSafeInteger(value) || value < 0) {
      throw new TypeError(`${name} must be a non-negative safe integer`);
    }
    return value;
  }

  function logSnapshot() {
    const snapshot = apply(arrayMap, logs, [row => freeze({
      level: row.level,
      message: row.message,
    })]);
    return freeze(snapshot);
  }

  function finish(returned) {
    if (returned && typeof returned.then === "function") {
      throw new TypeError("async snippet generators are not supported");
    }
    if (writes.length > 0 && returned !== undefined) {
      throw new TypeError("a snippet generator cannot both write output and return a result");
    }

    let text;
    let cursor = null;
    if (writes.length > 0) {
      text = apply(arrayJoin, writes, [""]);
    } else if (typeof returned === "string") {
      text = returned;
    } else if (returned && typeof returned === "object" && own(returned, "text")) {
      text = String(returned.text);
      cursor = integerOrNull(returned.cursor, "snippet result cursor");
    } else {
      throw new TypeError(
        "a snippet generator must return a string, return snippet.result(...), or call snippet.write(...)"
      );
    }

    if (text.length > input.maxOutputCharacters) throw outputLimitError();
    return { text, cursor, logs: logSnapshot() };
  }

  Object.defineProperty(globalThis, "api", {
    value: api,
    configurable: false,
    enumerable: true,
    writable: false,
  });
  Object.defineProperty(globalThis, "snippet", {
    value: snippet,
    configurable: false,
    enumerable: true,
    writable: false,
  });
  Object.defineProperty(globalThis, "write", {
    value: write,
    configurable: false,
    enumerable: true,
    writable: false,
  });
  Object.defineProperty(globalThis, "console", {
    value: snippetConsole,
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

  Object.defineProperty(globalThis, "__RESOLVED_SNIPPET_FINISH", {
    value: finish,
    configurable: false,
    enumerable: false,
    writable: false,
  });
  Object.defineProperty(globalThis, "__RESOLVED_SNIPPET_PARTIAL", {
    value: () => freeze({ logs: logSnapshot() }),
    configurable: false,
    enumerable: false,
    writable: false,
  });
  delete globalThis.__RESOLVED_SNIPPET_INPUT;
})();
