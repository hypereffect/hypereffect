// Writing servers: an outbox for messages, an App interface for per-session
// logic, and a Server that drives an app over a stream.

import {
  type Action, type Element, type JsonObject, type Log, type Metadata, type ServerMessage,
  encodeLine, LineBuffer, parseRendererMessage, serverMessageLine,
} from './message.ts';
import { isObject } from './signals.ts';

// Collects messages to send to the renderer, as lines.
export class Outbox {
  private out = '';

  send(m: ServerMessage): void {
    this.out += serverMessageLine(m);
  }

  element(e: Element): void {
    this.out += e.toLine();
  }

  remove(id: string): void {
    this.send({ tag: 'remove', id });
  }

  // Sends a signal message: a merge patch for the signals.
  signal(patch: JsonObject): void {
    if (!isObject(patch)) throw new TypeError('a signal message must be an object');
    this.out += encodeLine('signal', patch);
  }

  // Sends optional details about the application, replacing any sent before.
  metadata(m: Metadata): void {
    this.send({ tag: 'metadata', metadata: m });
  }

  // Sends an event for the user or the host's log. It never changes the
  // element tree or the signals.
  log(log: Log): void {
    this.send({ tag: 'log', log });
  }

  isEmpty(): boolean {
    return this.out === '';
  }

  // The lines written so far.
  toString(): string {
    return this.out;
  }

  // Takes the lines written so far.
  take(): string {
    const s = this.out;
    this.out = '';
    return s;
  }
}

// The logic of one session. A new app is created for every hello, and
// dropped when the session ends.
export interface App {
  // Called on hello. The app should build the initial interface.
  hello(out: Outbox, params: Record<string, string>): void;
  // Called for each action. The action's contents are user input.
  action(out: Outbox, action: Action): void;
  // Called periodically if tickInterval returns a number, with the host's
  // clock in milliseconds.
  tick?(out: Outbox, nowMs: number): void;
  // How often the app wants tick called, in milliseconds.
  tickInterval?(): number | undefined;
}

// Drives an App from what a renderer sends. Connect it to any transport that
// carries lines: pass what the renderer sends to receive(), and send what
// output holds after each call.
export class Server<A extends App> {
  readonly output = new Outbox();
  private current: A | undefined;
  private input = new LineBuffer();
  private readonly newApp: () => A;

  // newApp is called on every hello.
  constructor(newApp: () => A) {
    this.newApp = newApp;
  }

  // Feeds bytes or text received from the renderer. Messages before hello,
  // and lines that cannot be parsed, are ignored.
  receive(chunk: Uint8Array | string): void {
    for (const line of this.input.push(chunk)) {
      let m;
      try {
        m = parseRendererMessage(line);
      } catch {
        continue;
      }
      if (m.tag === 'hello') {
        this.current = this.newApp();
        this.current.hello(this.output, m.params);
      } else {
        this.current?.action(this.output, m.action);
      }
    }
  }

  tick(nowMs: number): void {
    this.current?.tick?.(this.output, nowMs);
  }

  tickInterval(): number | undefined {
    return this.current?.tickInterval?.();
  }

  // Ends the session.
  close(): void {
    this.current = undefined;
    this.input = new LineBuffer();
  }

  // The session's app, if a session is open.
  get app(): A | undefined {
    return this.current;
  }
}
