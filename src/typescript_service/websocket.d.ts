/** Current WebSocket event. Automation runs once for each connection or message event. */
interface WebSocketAutomationEvent {
  readonly eventType: "open" | "message";
  /** Text message payload, or null for connection and binary events. */
  readonly data: string | null;
  /** Exact binary message bytes encoded as base64. */
  readonly binaryBase64: string | null;
}

interface WebSocketAutomationApi {
  readonly event: WebSocketAutomationEvent;
  /** Snapshot of enabled variables from the active environment. */
  readonly environment: Readonly<Record<string, string>>;
  /** Send text, or serialize a value as JSON. */
  send(value: unknown): void;
  /** Serialize a value as JSON and send it. */
  sendJson(value: unknown): void;
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
