// A Client wired to a transport and a UI layer through callbacks, for
// renderers that react to events (a WebSocket, a browser) rather than poll.
//
// A Session sends lines itself, and after each batch of input (everything
// received or triggered in one task) calls onUpdate once with what changed.

import { type Changes, Client, type Resolved, type ResolvedTree } from './client.ts';
import type { Json, JsonObject, Log, Metadata } from './message.ts';

export interface SessionOptions {
  // Writes a line, with its line feed, to the transport.
  send: (line: string) => void;
  // Called after each batch of changes.
  onUpdate?: (changes: Changes) => void;
  // Receives problems with messages and expressions.
  onDiagnostic?: (msg: string) => void;
  // Asked to open a URL by a handler's `open`; the host decides whether and
  // how.
  onOpen?: (url: string) => void;
  // Receives each log message from the server, oldest first. The host decides
  // whether and how to show it.
  onLog?: (log: Log) => void;
}

export class Session {
  readonly client = new Client();
  private send: (line: string) => void;
  private onUpdate: (changes: Changes) => void;
  private onDiagnostic: (msg: string) => void;
  private onOpen: (url: string) => void;
  private onLog: (log: Log) => void;
  private scheduled = false;

  constructor({
    send, onUpdate = () => {}, onDiagnostic = (m) => console.warn('hypereffect:', m), onOpen = () => {},
    onLog = () => {},
  }: SessionOptions) {
    this.send = send;
    this.onUpdate = onUpdate;
    this.onDiagnostic = onDiagnostic;
    this.onOpen = onOpen;
    this.onLog = onLog;
  }

  // --- transport ---------------------------------------------------------

  // Begins a session: sends hello with params for the app.
  start(params: Record<string, string> = {}): void {
    this.send(this.client.hello(params));
    this.after();
  }

  // Feeds bytes or text received from the transport.
  receive(chunk: Uint8Array | string): void {
    this.client.receive(chunk);
    this.after();
  }

  receiveLine(line: string): void {
    this.client.receiveLine(line);
    this.after();
  }

  // Processes a message given as a tag and its JSON object.
  receiveMessage(tag: string, body: JsonObject): void {
    this.client.receiveMessage(tag, body);
    this.after();
  }

  // Marks the transport closed. The tree is kept for display.
  close(): void {
    this.client.close();
    this.after();
  }

  get connected(): boolean {
    return this.client.connected;
  }

  get resetPending(): boolean {
    return this.client.resetPending;
  }

  // --- reading -----------------------------------------------------------

  children(id?: string | null): readonly string[] {
    return this.client.children(id);
  }

  has(id: string): boolean {
    return this.client.contains(id);
  }

  get(id: string): Resolved | undefined {
    return this.client.get(id);
  }

  attr(id: string, name: string): Json | undefined {
    return this.client.attr(id, name);
  }

  tree(): ResolvedTree[] {
    return this.client.tree();
  }

  get metadata(): Metadata {
    return this.client.metadata;
  }

  get signals(): JsonObject {
    return this.client.signals;
  }

  value(path: string): Json | undefined {
    return this.client.value(path);
  }

  // --- user input --------------------------------------------------------

  // Writes a value at a signal path, as a control with `bind` does.
  write(path: string, value: Json): void {
    this.client.write(path, value);
    this.after();
  }

  // Runs the handler in attribute event of element id and sends its action,
  // if any. Returns whether an action was sent.
  trigger(id: string, event: string): boolean {
    const line = this.client.trigger(id, event);
    for (const url of this.client.takeOpens()) this.onOpen(url);
    if (line !== undefined) this.send(line);
    this.after();
    return line !== undefined;
  }

  // --- batching ----------------------------------------------------------

  // Delivers pending changes now. Normally this happens on a microtask after
  // each batch of input.
  flush(): void {
    this.scheduled = false;
    const c = this.client.takeChanges();
    const empty = !c.reset && !c.connection && !c.metadata && !c.created.size && !c.updated.size &&
      !c.removed.size && !c.reordered.size;
    if (!empty) this.onUpdate(c);
  }

  // Reports diagnostics right away, so that they can be attributed to the
  // input that caused them, and schedules a flush of the changes.
  private after(): void {
    for (const l of this.client.takeLogs()) this.onLog(l);
    for (const d of this.client.takeDiagnostics()) this.onDiagnostic(d);
    if (this.scheduled) return;
    this.scheduled = true;
    queueMicrotask(() => {
      if (this.scheduled) this.flush();
    });
  }
}
