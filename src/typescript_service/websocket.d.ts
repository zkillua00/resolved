/** Current WebSocket event. Automation runs once for each connection or message event. */
interface WebSocketAutomationEvent {
  readonly eventType: "open" | "message";
  /** Text message payload, or null for connection and binary events. */
  readonly data: string | null;
  /** Exact binary message bytes encoded as base64. */
  readonly binaryBase64: string | null;
}

declare const ws: {
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

/** Write messages to the WebSocket console. */
declare const console: {
  log(...values: unknown[]): void;
  info(...values: unknown[]): void;
  warn(...values: unknown[]): void;
  error(...values: unknown[]): void;
  debug(...values: unknown[]): void;
};
