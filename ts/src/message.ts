// Messages, elements and handlers, and the line framing.

import { type Component, isComponent } from './components.ts';
import { getPath, isObject, parsePath } from './signals.ts';

export type Json = null | boolean | number | string | Json[] | JsonObject;
export interface JsonObject {
  [key: string]: Json;
}

// An expression attribute value: {"cel": "..."}.
export interface Expr {
  cel: string;
}

export function cel(expr: string): Expr {
  return { cel: expr };
}

// The source of v if it is an expression: an object whose only member is
// `cel`, a string.
export function isCel(v: unknown): string | undefined {
  if (!isObject(v)) return undefined;
  const keys = Object.keys(v);
  return keys.length === 1 && keys[0] === 'cel' && typeof v.cel === 'string' ? v.cel : undefined;
}

// An attribute value: a literal, or an expression from cel().
export type Value = Json | Expr;

// An element: one instance of a component. Setters return the element, so
// an interface is built as
//
//   ui.text('title').parent('main').variant('h1').text('Inbox')
export class Element {
  id: string;
  type: Component;
  parentId: string | undefined;
  beforeId: string | undefined;
  // Attributes in the order they were set.
  attrs: Record<string, Value | Handler> = {};

  constructor(id: string, type: Component) {
    this.id = id;
    this.type = type;
  }

  parent(id: string): this {
    this.parentId = id;
    return this;
  }

  before(id: string): this {
    this.beforeId = id;
    return this;
  }

  // Sets any attribute.
  set(name: string, v: Value | Handler): this {
    this.attrs[name] = v;
    return this;
  }

  visible(v: Value) { return this.set('visible', v); }
  disabled(v: Value) { return this.set('disabled', v); }
  gap(v: Value) { return this.set('gap', v); }
  align(v: Value) { return this.set('align', v); }
  justify(v: Value) { return this.set('justify', v); }
  wrap(v: Value) { return this.set('wrap', v); }
  title(v: Value) { return this.set('title', v); }
  empty(v: Value) { return this.set('empty', v); }
  label(v: Value) { return this.set('label', v); }
  open(v: Value) { return this.set('open', v); }
  value(v: Value) { return this.set('value', v); }
  variant(v: Value) { return this.set('variant', v); }
  src(v: Value) { return this.set('src', v); }
  alt(v: Value) { return this.set('alt', v); }
  placeholder(v: Value) { return this.set('placeholder', v); }
  error(v: Value) { return this.set('error', v); }
  options(v: Value) { return this.set('options', v); }
  min(v: Value) { return this.set('min', v); }
  max(v: Value) { return this.set('max', v); }
  step(v: Value) { return this.set('step', v); }
  text(v: Value) { return this.set('text', v); }

  // Sets `bind`, the signal path a control reads and writes.
  bind(path: string) { return this.set('bind', path); }

  onPress(h: Handler) { return this.set('onPress', h); }
  onChange(h: Handler) { return this.set('onChange', h); }
  onSubmit(h: Handler) { return this.set('onSubmit', h); }
  onDismiss(h: Handler) { return this.set('onDismiss', h); }

  toJSON(): JsonObject {
    const o: JsonObject = { id: this.id, type: this.type };
    if (this.parentId !== undefined) o.parent = this.parentId;
    if (this.beforeId !== undefined) o.before = this.beforeId;
    for (const [k, v] of Object.entries(this.attrs)) {
      o[k] = v instanceof Handler ? v.toJSON() : (v as Json);
    }
    return o;
  }

  toLine(): string {
    return encodeLine('element', this);
  }

  // Reads an element from its JSON object. Throws ParseError.
  static fromJSON(body: JsonObject): Element {
    const { id, type, parent, before, ...attrs } = body;
    if (typeof id !== 'string') throw new ParseError('invalid', 'element needs a string id');
    if (!isComponent(type)) throw new ParseError('invalid', `unknown type ${JSON.stringify(type)}`);
    for (const [k, v] of [['parent', parent], ['before', before]] as const) {
      if (v !== undefined && typeof v !== 'string') throw new ParseError('invalid', `${k} must be a string`);
    }
    const e = new Element(id, type);
    e.parentId = parent as string | undefined;
    e.beforeId = before as string | undefined;
    e.attrs = attrs;
    return e;
  }
}

const ui_ = (type: Component) => (id: string) => new Element(id, type);

// Constructors for each component: ui.text('title') is
// new Element('title', 'text').
export const ui = {
  column: ui_('column'), row: ui_('row'), card: ui_('card'), list: ui_('list'),
  tabs: ui_('tabs'), tab: ui_('tab'), dialog: ui_('dialog'), text: ui_('text'),
  notice: ui_('notice'), badge: ui_('badge'), image: ui_('image'), divider: ui_('divider'),
  progress: ui_('progress'), button: ui_('button'), input: ui_('input'),
  checkbox: ui_('checkbox'), select: ui_('select'), slider: ui_('slider'),
};

// A local merge patch whose leaf values may be expressions.
export type Patch = { [key: string]: Value | Patch };

interface HandlerFields {
  patch?: Patch;
  action?: string;
  payload?: Json;
  indicator?: string;
  open?: string | Expr;
}

// A handler: a local patch, a URL to open (buttons only), an action sent to
// the server, or any combination. Handlers are immutable; the with* methods
// return a new one.
export class Handler {
  readonly fields: Readonly<HandlerFields>;

  private constructor(fields: HandlerFields) {
    this.fields = fields;
  }

  // A handler that sends an action.
  static action(name: string): Handler {
    return new Handler({ action: name });
  }

  // A handler that applies a local merge patch.
  static patch(patch: Patch): Handler {
    return new Handler({ patch });
  }

  // A handler that asks the renderer to open a URL: a string, or an
  // expression giving one. Only valid on a button.
  static open(url: string | Expr): Handler {
    return new Handler({ open: url });
  }

  // Adds a local patch, applied before the action is sent.
  withPatch(patch: Patch): Handler {
    return new Handler({ ...this.fields, patch });
  }

  withPayload(payload: Json): Handler {
    return new Handler({ ...this.fields, payload });
  }

  // A local signal path set to true when the action is sent.
  withIndicator(path: string): Handler {
    return new Handler({ ...this.fields, indicator: path });
  }

  // Adds a URL to open, after the patch and before the action is sent.
  withOpen(url: string | Expr): Handler {
    return new Handler({ ...this.fields, open: url });
  }

  toJSON(): JsonObject {
    const o: JsonObject = {};
    for (const k of ['patch', 'action', 'payload', 'indicator', 'open'] as const) {
      const v = this.fields[k];
      if (v !== undefined) o[k] = v as Json;
    }
    return o;
  }
}

// Optional details about the application, sent by the server. A renderer is
// free to ignore all of them.
export interface Metadata {
  name?: string;
  description?: string;
  icon?: string;
  // The other string members: extensions defined by a protocol built on
  // hypereffect, which renderers that do not know them ignore.
  [member: string]: string | undefined;
}

// Reads the members that are strings and drops everything else.
export function metadataFromObject(body: JsonObject): Metadata {
  const m: Metadata = {};
  for (const [k, v] of Object.entries(body)) {
    if (typeof v === 'string' && k !== '__proto__') m[k] = v;
  }
  return m;
}

// A message from the server to the renderer.
export type ServerMessage =
  | { tag: 'element'; element: Element }
  | { tag: 'remove'; id: string }
  // A merge patch for the signals.
  | { tag: 'signal'; patch: JsonObject }
  | { tag: 'metadata'; metadata: Metadata };

// The message as a line, including the final line feed.
export function serverMessageLine(m: ServerMessage): string {
  switch (m.tag) {
    case 'element': return m.element.toLine();
    case 'remove': return encodeLine('remove', { id: m.id });
    case 'signal': return encodeLine('signal', m.patch);
    case 'metadata': return encodeLine('metadata', m.metadata);
  }
}

// Parses a line (without its line feed). Throws ParseError.
export function parseServerMessage(line: string): ServerMessage {
  const { tag, body } = parseLine(line);
  return serverMessageFromParts(tag, body);
}

export function serverMessageFromParts(tag: string, body: JsonObject): ServerMessage {
  switch (tag) {
    case 'element': return { tag, element: Element.fromJSON(body) };
    case 'remove':
      if (typeof body.id !== 'string') throw new ParseError('invalid', 'remove needs a string id');
      return { tag, id: body.id };
    case 'signal': return { tag, patch: body };
    case 'metadata': return { tag, metadata: metadataFromObject(body) };
  }
  throw new ParseError('unknown-tag', tag);
}

// An action, sent by a handler. Its contents are user input.
export class Action {
  name: string;
  payload: Json | undefined;
  signals: JsonObject;

  constructor(name: string, payload?: Json, signals: JsonObject = {}) {
    this.name = name;
    this.payload = payload;
    this.signals = signals;
  }

  // A signal by path, such as `form.email`.
  signal(path: string): Json | undefined {
    const segs = parsePath(path);
    return segs && getPath(this.signals, segs);
  }

  toJSON(): JsonObject {
    const o: JsonObject = { name: this.name };
    if (this.payload !== undefined) o.payload = this.payload;
    o.signals = this.signals;
    return o;
  }
}

// A message from the renderer to the server. hello opens a session; its
// params are chosen by whoever opened it (for example, taken from the
// address) and are strings only.
export type RendererMessage =
  | { tag: 'hello'; params: Record<string, string> }
  | { tag: 'action'; action: Action };

export function rendererMessageLine(m: RendererMessage): string {
  if (m.tag === 'action') return encodeLine('action', m.action);
  return encodeLine('hello', Object.keys(m.params).length ? { params: m.params } : {});
}

// Parses a line (without its line feed). Throws ParseError.
export function parseRendererMessage(line: string): RendererMessage {
  const { tag, body } = parseLine(line);
  switch (tag) {
    case 'hello': {
      const params: Record<string, string> = {};
      if (body.params !== undefined) {
        if (!isObject(body.params)) throw new ParseError('invalid', 'params: not an object');
        for (const [k, v] of Object.entries(body.params)) {
          if (typeof v !== 'string') throw new ParseError('invalid', `params: ${k} is not a string`);
          params[k] = v;
        }
      }
      return { tag, params };
    }
    case 'action': {
      const { name, payload, signals = {} } = body;
      if (typeof name !== 'string') throw new ParseError('invalid', 'action needs a string name');
      if (!isObject(signals)) throw new ParseError('invalid', 'signals: not an object');
      return { tag, action: new Action(name, payload ?? undefined, signals) };
    }
  }
  throw new ParseError('unknown-tag', tag);
}

export type ParseErrorKind =
  // The line does not match `tag SP json`.
  | 'grammar'
  | 'json'
  | 'not-an-object'
  | 'unknown-tag'
  // The JSON does not have the members the message needs.
  | 'invalid';

export class ParseError extends Error {
  kind: ParseErrorKind;

  constructor(kind: ParseErrorKind, detail = '') {
    super({
      grammar: 'line does not match the grammar',
      json: `invalid JSON: ${detail}`,
      'not-an-object': 'message body is not a JSON object',
      'unknown-tag': `unknown tag '${detail}'`,
      invalid: `invalid message: ${detail}`,
    }[kind]);
    this.kind = kind;
  }
}

// Encodes a message as a line, including the final line feed.
export function encodeLine(tag: string, body: unknown): string {
  return `${tag} ${JSON.stringify(body)}\n`;
}

// Splits a line (without its line feed) into its tag and JSON object.
// Throws ParseError.
export function parseLine(line: string): { tag: string; body: JsonObject } {
  if (line.endsWith('\r')) line = line.slice(0, -1);
  const i = line.indexOf(' ');
  const tag = line.slice(0, i);
  if (i < 1 || !/^[a-z]+$/.test(tag)) throw new ParseError('grammar');
  let body: unknown;
  try {
    body = JSON.parse(line.slice(i + 1));
  } catch (e) {
    throw new ParseError('json', (e as Error).message);
  }
  if (!isObject(body)) throw new ParseError('not-an-object');
  return { tag, body };
}

const decoder = new TextDecoder('utf-8', { fatal: true });
const encoder = new TextEncoder();

// Splits a stream into lines. Empty lines are dropped and a carriage return
// before the line feed is ignored.
export class LineBuffer {
  private buf = new Uint8Array(0);

  // Adds bytes or text and returns the complete lines they finish. Lines
  // that are not valid UTF-8 are dropped.
  push(chunk: Uint8Array | string): string[] {
    const bytes = typeof chunk === 'string' ? encoder.encode(chunk) : chunk;
    const buf = new Uint8Array(this.buf.length + bytes.length);
    buf.set(this.buf);
    buf.set(bytes, this.buf.length);
    const lines: string[] = [];
    let start = 0;
    for (let i = buf.indexOf(10); i >= 0; i = buf.indexOf(10, start)) {
      const end = i > start && buf[i - 1] === 13 ? i - 1 : i;
      if (end > start) {
        try {
          lines.push(decoder.decode(buf.subarray(start, end)));
        } catch {
          // Not UTF-8.
        }
      }
      start = i + 1;
    }
    this.buf = buf.slice(start);
    return lines;
  }
}
