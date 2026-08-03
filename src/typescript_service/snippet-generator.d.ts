declare namespace ResolvedSnippet {
  type Category = "pre-request" | "post-response";
  type SelectionSource = "pre-request" | "post-response" | "request" | "response";
  type SelectionArea = "script" | "url" | "headers" | "body";

  interface Request {
    readonly method: Resolved.HttpMethod;
    readonly url: string;
    readonly body: string;
    readonly bodyMode: Resolved.BodyMode;
    readonly rawBodyLanguage: Resolved.RawBodyLanguage;
    readonly bodyFields: readonly Resolved.ReadonlyBodyField[];
    readonly headers: Resolved.ReadonlyHeaders;
    readonly bodyTruncated: boolean;
  }

  interface Response {
    readonly status: number;
    readonly statusText: string;
    readonly httpVersion: string;
    readonly url: string;
    readonly contentType: string | null;
    readonly headers: Resolved.ReadonlyHeaders;
    readonly durationMs: number;
    readonly sizeBytes: number;
    readonly bodyTruncated: boolean;
    text(): string;
    json(): any;
  }

  interface PreRequestGeneratorApi {
    readonly request: Request;
  }

  /** Generator-time response is nullable until an HTTP response exists. */
  interface PostResponseGeneratorApi extends PreRequestGeneratorApi {
    readonly response: Response | null;
  }

  interface TextRange {
    /** UTF-16 code-unit offset, matching JavaScript string indexing. */
    readonly start: number;
    /** Exclusive UTF-16 code-unit offset. */
    readonly end: number;
  }

  interface ExpressionOptions {
    /**
     * JavaScript root expression. Request body selections infer a root when
     * omitted. Response body selections do so only in post-response generators.
     */
    readonly root?: string;
  }

  interface Selection {
    readonly source: SelectionSource;
    readonly area: SelectionArea;
    readonly text: string;
    /** UTF-16 code-unit offset in the displayed source. */
    readonly start: number;
    /** Exclusive UTF-16 code-unit offset in the displayed source. */
    readonly end: number;
    readonly range: TextRange;
    readonly contentType: string | null;
    is(source: SelectionSource): boolean;
    /** Dot/bracket JSON path. Throws when no JSON node is mapped. */
    jsonPath(): string;
    /** RFC 6901 JSON Pointer. Throws when no JSON node is mapped. */
    jsonPointer(): string;
    /** JavaScript expression addressing the selected JSON node. */
    expression(options?: ExpressionOptions): string;
  }

  interface ResultOptions {
    /** UTF-16 code-unit offset for the preferred caret. */
    readonly cursor?: number | null;
  }

  /** Structural result accepted by the generator runtime. */
  interface GeneratorReturn {
    readonly text: unknown;
    readonly cursor?: number | null;
  }

  interface Result extends GeneratorReturn {
    readonly text: string;
    readonly cursor: number | null;
  }

  interface Generator {
    readonly apiVersion: number;
    readonly category: Category;
    readonly outputLanguage: string;
    readonly selection: Selection | null;
    /** Appends text to this generator's bounded output buffer. */
    write(value: unknown): void;
    /** Builds an explicit generated result. A returned string is also valid. */
    result(text: unknown, options?: ResultOptions): Result;
  }
}

declare const snippet: ResolvedSnippet.Generator;

/** @deprecated Prefer `snippet.write(value)`. */
declare function write(value: unknown): void;
