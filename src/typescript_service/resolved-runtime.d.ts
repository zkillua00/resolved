declare namespace Resolved {
  type HttpMethod =
    | "GET"
    | "POST"
    | "PUT"
    | "PATCH"
    | "DELETE"
    | "HEAD"
    | "OPTIONS"
    | (string & {});

  type BodyMode =
    | "none"
    | "raw"
    | "form_url_encoded"
    | "multipart_form_data";

  type RawBodyLanguage =
    | "text"
    | "json"
    | "xml"
    | "html"
    | "javascript"
    | "typescript"
    | "css"
    | "markdown"
    | "graphql"
    | "yaml"
    | "toml"
    | "sql"
    | "shell"
    | "rust"
    | "python";

  type BodyFieldKind = "text" | "file";

  interface HeaderEntry {
    enabled: boolean;
    name: string;
    value: string;
  }

  interface ReadonlyHeaders {
    /** Returns whether an enabled header with this case-insensitive name exists. */
    readonly has: (name: unknown) => boolean;
    /** Returns the first enabled value for a case-insensitive header name. */
    readonly get: (name: unknown) => string | undefined;
    /** Returns every enabled value for a case-insensitive header name. */
    readonly getAll: (name: unknown) => string[];
    /** Returns a detached copy of the header rows. */
    readonly toArray: () => HeaderEntry[];
  }

  interface MutableHeaders extends ReadonlyHeaders {
    /** Replaces every enabled row with one value, or appends a new row. */
    readonly set: (name: unknown, value: unknown) => void;
    /** Appends an enabled header row. */
    readonly append: (name: unknown, value: unknown) => void;
    /** Removes every row with this case-insensitive name. */
    readonly remove: (name: unknown) => void;
  }

  interface MutableBodyField {
    enabled: boolean;
    name: string;
    value: string;
    kind: BodyFieldKind;
  }

  interface ReadonlyBodyField {
    readonly enabled: boolean;
    readonly name: string;
    readonly value: string;
    readonly kind: BodyFieldKind;
  }

  interface MutableRequest {
    method: HttpMethod;
    url: string;
    body: string;
    bodyMode: BodyMode;
    rawBodyLanguage: RawBodyLanguage;
    bodyFields: MutableBodyField[];
    headers: MutableHeaders;
  }

  interface ReadonlyRequest {
    readonly method: HttpMethod;
    readonly url: string;
    readonly body: string;
    readonly bodyMode: BodyMode;
    readonly rawBodyLanguage: RawBodyLanguage;
    readonly bodyFields: readonly ReadonlyBodyField[];
    readonly headers: ReadonlyHeaders;
  }

  interface Environment {
    readonly has: (key: unknown) => boolean;
    readonly get: (key: unknown) => string | undefined;
    readonly set: (key: unknown, value: unknown) => void;
    readonly unset: (key: unknown) => void;
    readonly toObject: () => Record<string, string>;
  }

  interface Variables {
    readonly has: (key: unknown) => boolean;
    readonly get: (key: unknown) => string | undefined;
    readonly toObject: () => Record<string, string>;
  }

  interface ScriptConsole {
    readonly log: (...values: unknown[]) => void;
    readonly info: (...values: unknown[]) => void;
    readonly warn: (...values: unknown[]) => void;
    readonly error: (...values: unknown[]) => void;
    readonly debug: (...values: unknown[]) => void;
  }

  interface Response {
    readonly status: number;
    readonly statusText: string;
    readonly httpVersion: string;
    readonly url: string;
    readonly headers: ReadonlyHeaders;
    readonly durationMs: number;
    readonly sizeBytes: number;
    readonly truncated: boolean;
    readonly bodyBase64: string | null;
    readonly text: () => string;
    readonly json: () => any;
  }

  interface BaseApi {
    readonly environment: Environment;
    readonly variables: Variables;
    readonly console: ScriptConsole;
  }

  interface PreRequestApi extends BaseApi {
    readonly request: MutableRequest;
    readonly response: null;
  }

  interface PostResponseApi extends BaseApi {
    readonly request: ReadonlyRequest;
    readonly response: Response;
    readonly test: <Result>(
      name: unknown,
      callback: () => Result,
      ...unsupportedAsync: Result extends PromiseLike<unknown>
        ? ["Async test callbacks are not supported"]
        : []
    ) => void;
    readonly assert: (
      condition: unknown,
      message?: unknown,
    ) => asserts condition;
  }
}

/** QuickJS base64 decoder. Throws `DOMException` for invalid input. */
declare function atob(data: unknown): string;

/** QuickJS base64 encoder. Throws `DOMException` for non-Latin-1 input. */
declare function btoa(data: unknown): string;

interface Performance {
  readonly timeOrigin: number;
  readonly now: () => number;
}

declare const performance: Performance;

interface DOMException extends Error {
  readonly code: number;
  readonly message: string;
  readonly name: string;
}

declare const DOMException: {
  readonly prototype: DOMException;
  new (message?: string, name?: string): DOMException;
  readonly INDEX_SIZE_ERR: 1;
  readonly DOMSTRING_SIZE_ERR: 2;
  readonly HIERARCHY_REQUEST_ERR: 3;
  readonly WRONG_DOCUMENT_ERR: 4;
  readonly INVALID_CHARACTER_ERR: 5;
  readonly NO_DATA_ALLOWED_ERR: 6;
  readonly NO_MODIFICATION_ALLOWED_ERR: 7;
  readonly NOT_FOUND_ERR: 8;
  readonly NOT_SUPPORTED_ERR: 9;
  readonly INUSE_ATTRIBUTE_ERR: 10;
  readonly INVALID_STATE_ERR: 11;
  readonly SYNTAX_ERR: 12;
  readonly INVALID_MODIFICATION_ERR: 13;
  readonly NAMESPACE_ERR: 14;
  readonly INVALID_ACCESS_ERR: 15;
  readonly VALIDATION_ERR: 16;
  readonly TYPE_MISMATCH_ERR: 17;
  readonly SECURITY_ERR: 18;
  readonly NETWORK_ERR: 19;
  readonly ABORT_ERR: 20;
  readonly URL_MISMATCH_ERR: 21;
  readonly QUOTA_EXCEEDED_ERR: 22;
  readonly TIMEOUT_ERR: 23;
  readonly INVALID_NODE_TYPE_ERR: 24;
  readonly DATA_CLONE_ERR: 25;
};
