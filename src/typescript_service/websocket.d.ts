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

/** Shared scripting API. Request edits affect only this event's snapshot. */
declare const api: Resolved.PreRequestApi & Pick<Resolved.PostResponseApi, "test" | "assert">;
declare const console: Resolved.ScriptConsole;
