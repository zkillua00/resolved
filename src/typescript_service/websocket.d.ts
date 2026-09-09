/** Current WebSocket event. Automation runs once for each connection or message event. */
interface WebSocketAutomationEvent {
  readonly eventType: "open" | "message" | "close";
  /** Text message payload, or null for connection and binary events. */
  readonly data: string | null;
  /** Exact binary message bytes encoded as base64. */
  readonly binaryBase64: string | null;
  /** Close description supplied by the transport, if available. */
  readonly reason: string | null;
  /** Transport or connection error, otherwise null. */
  readonly error: string | null;
}

interface WebSocketReconnectOptions {
  /** Clear the console when scheduling the next connection. Default: false. */
  clearConsole?: boolean;
  /** Delay before reconnecting, in milliseconds (0–86400000). Default: 1000. */
  delayMs?: number;
  /** Absolute ws:// or wss:// URL for this session; does not edit the saved request. */
  url?: string;
}

interface WebSocketAutomationApi {
  readonly event: WebSocketAutomationEvent;
  /** Snapshot of enabled variables from the active environment. */
  readonly environment: Readonly<Record<string, string>>;
  /** Send text, or serialize a value as JSON. */
  send(value: unknown): void;
  /** Serialize a value as JSON and send it. */
  sendJson(value: unknown): void;
  /** Schedule a reconnect from a close event, after successful script execution.
   * The last call wins. Manual disconnect cancels pending reconnects. */
  reconnect(options?: WebSocketReconnectOptions): void;
  /** Write a log entry to the WebSocket console. */
  log(...values: unknown[]): void;
};

declare const ws: WebSocketAutomationApi;

/** A filter evaluated for the current event. Async predicates are awaited. */
type WebSocketEventCondition = (ws: WebSocketAutomationApi, event: WebSocketAutomationEvent) => boolean | Promise<boolean>;

/** Predicates for the supported WebSocket automation events. */
declare const eventTypes: {
  readonly open: (ws: WebSocketAutomationApi, event: WebSocketAutomationEvent) => boolean;
  /** Matches both text and binary messages. */
  readonly message: (ws: WebSocketAutomationApi, event: WebSocketAutomationEvent) => boolean;
  /** Matches remote closure and transport/connection failures. */
  readonly close: (ws: WebSocketAutomationApi, event: WebSocketAutomationEvent) => boolean;
};

/** Register for the current event. After module evaluation, matching handlers run in
 * registration order and are awaited. Registrations do not persist across events. */
declare function on(
  condition: WebSocketEventCondition,
  handler: (ws: WebSocketAutomationApi, event: WebSocketAutomationEvent) => void | Promise<void>,
): void;

/** Write messages to the WebSocket console. */
declare const console: {
  log(...values: unknown[]): void;
  info(...values: unknown[]): void;
  warn(...values: unknown[]): void;
  error(...values: unknown[]): void;
  debug(...values: unknown[]): void;
};
