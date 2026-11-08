// The renderer side of a session, without any drawing.
//
// A Client keeps the element tree and the signals, evaluates CEL expressions
// with cel-js, runs handlers, and yields elements whose attributes are
// resolved to plain JSON values. A UI layer reads the resolved elements and
// draws them; takeChanges() says which ones to redraw.
//
// A Session wraps a Client with callbacks, for renderers driven by events
// such as a browser's.
//
// A Loopback connects a Client to a Server in the same process, for testing
// an app without a renderer.

import { type Compiled, compile, fromJson, toJson, typeName, Uint } from './cel.ts';
import {
  type AttrType, type Component, accepts, attrOf, attrs, defaultValue, isComponent, isStructural,
} from './components.ts';
import {
  type Json, type JsonObject, type Log, type Metadata, type ServerMessage, Action, encodeLine,
  isCel, LineBuffer, logFromObject, metadataFromObject, parseLine, rendererMessageLine,
  serverMessageLine,
} from './message.ts';
import { type App, Server } from './server.ts';
import { getPath, isObject, mergePatchTracked, overlaps, parsePath, pathPatch, stripLocal } from './signals.ts';

const ROOT = '';
const STRUCTURAL = new Set(['id', 'type', 'parent', 'before']);

// An element with its attributes resolved: expressions evaluated, values
// checked against the attribute types, and defaults filled in. Structural
// attributes (`bind` and handlers) are included as they were sent, or null.
export interface Resolved {
  id: string;
  type: Component;
  parent: string | undefined;
  children: string[];
  attrs: JsonObject;
}

// A resolved element with its resolved descendants.
export interface ResolvedTree {
  id: string;
  type: Component;
  attrs: JsonObject;
  children: ResolvedTree[];
}

// What changed since the last takeChanges().
export interface Changes {
  // The tree was discarded because a new session began.
  reset: boolean;
  // The connected state changed.
  connection: boolean;
  // Elements whose widgets must be built anew: new, or with a new type.
  created: Set<string>;
  // Elements whose resolved attributes may have changed.
  updated: Set<string>;
  removed: Set<string>;
  // Elements whose list of children changed ('' for the root).
  reordered: Set<string>;
  // The application metadata changed.
  metadata: boolean;
}

// paths are the signal paths whose values may have changed.
const noChanges = (): Changes & { paths: string[][] } => ({
  reset: false, connection: false, created: new Set(), updated: new Set(), removed: new Set(),
  reordered: new Set(), metadata: false, paths: [],
});

interface Node {
  type: Component;
  parent: string | undefined;
  children: string[];
  el: JsonObject;
  progs: Map<string, Compiled>;
  // The signal paths its expressions and bindings read; [] means anything.
  deps: string[][];
}

class CheckError extends Error {}

export class Client {
  private nodes = new Map<string, Node>();
  private root: string[] = [];
  private _signals: JsonObject = {};
  private _metadata: Metadata = {};
  private celSignals: unknown = undefined;
  private _connected = false;
  private _resetPending = false;
  private reader = new LineBuffer();
  private changes = noChanges();
  private diagnostics: string[] = [];
  private opens: string[] = [];
  private logs: Log[] = [];

  // --- transport ---------------------------------------------------------

  // Begins a session and returns the hello line to send, with params for
  // the app: strings chosen by whoever opened the session, such as the ones
  // in its address. The previous tree is kept until the first server message
  // of the new session.
  hello(params: Record<string, string> = {}): string {
    this.reader = new LineBuffer();
    this._connected = true;
    this._resetPending = true;
    const line = rendererMessageLine({ tag: 'hello', params });
    if (this.nodes.size === 0) this.reset();
    this.changes.connection = true;
    return line;
  }

  // Feeds bytes or text received from the transport.
  receive(chunk: Uint8Array | string): void {
    for (const line of this.reader.push(chunk)) this.receiveLine(line);
  }

  receiveLine(line: string): void {
    let msg;
    try {
      msg = parseLine(line);
    } catch (e) {
      this.diag(`ignored line: ${(e as Error).message}`);
      return;
    }
    this.receiveMessage(msg.tag, msg.body);
  }

  // Processes a message given as a tag and its JSON object.
  receiveMessage(tag: string, body: JsonObject): void {
    if (this._resetPending) this.reset();
    switch (tag) {
      case 'element': this.element(body); break;
      case 'remove': if (typeof body.id === 'string') this.remove(body.id); break;
      case 'signal': this.patch(body); break;
      case 'metadata': this.setMetadata(metadataFromObject(body)); break;
      case 'log':
        try {
          this.logs.push(logFromObject(body));
        } catch (e) {
          this.diag(`ignored log: ${(e as Error).message}`);
        }
        break;
      default: this.diag(`ignored message with unknown tag '${tag}'`);
    }
  }

  // Processes a message, as the in-process transport.
  apply(m: ServerMessage): void {
    this.receiveLine(serverMessageLine(m).slice(0, -1));
  }

  // Marks the transport closed. The tree is kept for display.
  close(): void {
    this._connected = false;
    this.changes.connection = true;
  }

  get connected(): boolean {
    return this._connected;
  }

  // Whether a new session began and the tree is still the previous one,
  // kept for display until the first server message discards it.
  get resetPending(): boolean {
    return this._resetPending;
  }

  // --- reading -----------------------------------------------------------

  // The children of id, or of the root.
  children(id?: string | null): readonly string[] {
    if (id == null || id === ROOT) return this.root;
    return this.nodes.get(id)?.children ?? [];
  }

  contains(id: string): boolean {
    return this.nodes.has(id);
  }

  get(id: string): Resolved | undefined {
    const n = this.nodes.get(id);
    if (!n) return undefined;
    const out: JsonObject = {};
    for (const a of attrs(n.type)) out[a.name] = this.attr(id, a.name) ?? null;
    return { id, type: n.type, parent: n.parent, children: [...n.children], attrs: out };
  }

  // One resolved attribute, or undefined if the element or attribute does
  // not exist.
  attr(id: string, name: string): Json | undefined {
    const n = this.nodes.get(id);
    if (!n) return undefined;
    const spec = attrOf(n.type, name);
    if (!spec) return undefined;
    if (isStructural(spec.type)) return n.el[name] ?? null;
    const def = defaultValue(spec, id) as Json;
    const raw = n.el[name];
    if (raw === undefined || raw === null) return def;
    let value: unknown;
    if (isCel(raw) !== undefined) {
      const prog = n.progs.get(name);
      if (!prog) return def;
      try {
        value = prog.eval(this.cel());
      } catch (e) {
        this.diag(`${id}.${name}: ${(e as Error).message}`);
        return def;
      }
    } else {
      value = fromJson(raw);
    }
    try {
      return check(spec.type, value, def);
    } catch (e) {
      this.diag(`${id}.${name}: ${(e as Error).message}`);
      return def;
    }
  }

  // The whole tree, resolved.
  tree(): ResolvedTree[] {
    return this.treeOf(this.root);
  }

  private treeOf(ids: readonly string[]): ResolvedTree[] {
    return ids.flatMap((id) => {
      const r = this.get(id);
      return r ? [{ id, type: r.type, attrs: r.attrs, children: this.treeOf(r.children) }] : [];
    });
  }

  // The details the server sent about the application, if any.
  get metadata(): Metadata {
    return this._metadata;
  }

  get signals(): JsonObject {
    return this._signals;
  }

  // The value at a signal path such as `form.email`.
  value(path: string): Json | undefined {
    const segs = parsePath(path);
    return segs && getPath(this._signals, segs);
  }

  // --- user input --------------------------------------------------------

  // Writes a value at a signal path, as a control with `bind` does.
  write(path: string, value: Json): void {
    const segs = parsePath(path);
    if (segs) this.patch(pathPatch(segs, structuredClone(value)));
  }

  // Runs the handler in attribute event of element id, following section 10
  // of the spec. Returns the action line to send, if any. A URL to open is
  // queued for takeOpens().
  trigger(id: string, event: string): string | undefined {
    const n = this.nodes.get(id);
    if (!n || attrOf(n.type, event)?.type.kind !== 'handler') return undefined;
    const h = n.el[event];
    if (!isObject(h)) return undefined;
    for (let cur: string | undefined = id; cur !== undefined; cur = this.nodes.get(cur)!.parent) {
      if (this.attr(cur, 'visible') !== true) return undefined;
      if (this.attr(cur, 'disabled') === true) return undefined;
    }
    if (isObject(h.patch)) this.patch(this.evalPatch(h.patch, `${id}.${event}`));
    if (h.open !== undefined) {
      const url = this.evalOpen(h.open, `${id}.${event}`);
      if (url !== undefined) this.opens.push(url);
    }
    if (typeof h.action !== 'string' || !this._connected) return undefined;
    const ind = typeof h.indicator === 'string' ? parsePath(h.indicator) : undefined;
    if (ind) this.patch(pathPatch(ind, true));
    const payload = h.payload === undefined ? undefined : h.payload;
    return encodeLine('action', new Action(h.action, payload, stripLocal(this._signals)));
  }

  // --- changes -----------------------------------------------------------

  // Takes what changed since the last call.
  takeChanges(): Changes {
    const { paths, ...c } = this.changes;
    this.changes = noChanges();
    if (paths.length) {
      for (const [id, n] of this.nodes) {
        if (n.deps.some((d) => paths.some((p) => overlaps(d, p)))) c.updated.add(id);
      }
    }
    // Tabs and lists present facts about their children.
    const listLike = (t: Component) => t === 'tabs' || t === 'list';
    for (const id of [...c.updated, ...c.reordered]) {
      const n = this.nodes.get(id);
      if (!n) continue;
      if (listLike(n.type)) c.updated.add(id);
      if (n.parent !== undefined && listLike(this.nodes.get(n.parent)!.type)) c.updated.add(n.parent);
    }
    for (const id of c.removed) {
      c.updated.delete(id);
      c.created.delete(id);
    }
    return c;
  }

  // Takes the diagnostics reported since the last call.
  takeDiagnostics(): string[] {
    return this.diagnostics.splice(0);
  }

  // Returns and clears the URLs that handlers asked to open (a button's
  // `open`), oldest first. The host decides whether and how to open them.
  takeOpens(): string[] {
    return this.opens.splice(0);
  }

  // Returns and clears the log messages received since the last call, oldest
  // first. The host decides whether and how to show them.
  takeLogs(): Log[] {
    return this.logs.splice(0);
  }

  // --- internals ---------------------------------------------------------

  private diag(msg: string): void {
    this.diagnostics.push(msg);
  }

  private setMetadata(m: Metadata): void {
    if (JSON.stringify(m) !== JSON.stringify(this._metadata)) {
      this._metadata = m;
      this.changes.metadata = true;
    }
  }

  private reset(): void {
    this._resetPending = false;
    this.setMetadata({});
    for (const id of this.nodes.keys()) this.changes.removed.add(id);
    this.nodes.clear();
    this.root = [];
    this._signals = {};
    this.celSignals = undefined;
    this.changes.reset = true;
    this.changes.paths.push([]);
    this.changes.reordered.add(ROOT);
  }

  private cel(): unknown {
    if (this.celSignals === undefined) this.celSignals = fromJson(this._signals);
    return this.celSignals;
  }

  private patch(patch: Json): void {
    this._signals = mergePatchTracked(this._signals, patch, [], this.changes.paths) as JsonObject;
    this.celSignals = undefined;
  }

  private evalPatch(patch: JsonObject, at: string): JsonObject {
    const out: JsonObject = {};
    for (const [k, v] of Object.entries(patch)) {
      const src = isCel(v);
      if (src !== undefined) {
        try {
          out[k] = toJson(compile(src).eval(this.cel())) as Json;
        } catch (e) {
          this.diag(`${at}.patch.${k}: ${(e as Error).message}`);
        }
      } else if (isObject(v)) {
        out[k] = this.evalPatch(v, `${at}.${k}`);
      } else {
        out[k] = v;
      }
    }
    return out;
  }

  // The URL of a handler's open, or undefined if its expression fails or
  // does not give a string.
  private evalOpen(open: Json, at: string): string | undefined {
    const src = isCel(open);
    if (src === undefined) return typeof open === 'string' ? open : undefined;
    try {
      const v = compile(src).eval(this.cel());
      if (typeof v === 'string') return v;
      this.diag(`${at}.open: expected a string, got ${typeName(v)}`);
    } catch (e) {
      this.diag(`${at}.open: ${(e as Error).message}`);
    }
    return undefined;
  }

  private reject(el: JsonObject, why: string): void {
    this.diag(`rejected element ${JSON.stringify(el.id ?? null)}: ${why}`);
  }

  private element(el: JsonObject): void {
    const id = el.id;
    if (typeof id !== 'string' || id === '') return this.reject(el, 'id must be a non-empty string');
    const type = el.type;
    if (!isComponent(type)) return this.reject(el, `unknown type ${JSON.stringify(type ?? null)}`);
    for (const [k, v] of Object.entries(el)) {
      const a = attrOf(type, k);
      const structural = STRUCTURAL.has(k) || (a !== undefined && isStructural(a.type));
      if (structural && isCel(v) !== undefined) return this.reject(el, `${k} cannot be an expression`);
    }
    for (const k of ['parent', 'before']) {
      if (el[k] !== undefined && typeof el[k] !== 'string') return this.reject(el, `${k} must be a string`);
    }
    for (const a of attrs(type)) {
      const v = el[a.name];
      if (v === undefined) continue;
      if (a.type.kind === 'path' && (typeof v !== 'string' || !parsePath(v))) {
        return this.reject(el, `${a.name} is not a signal path`);
      }
      if (a.type.kind === 'handler' && !validHandler(v, type === 'button')) {
        return this.reject(el, `${a.name} is not a valid handler`);
      }
    }
    const parent = el.parent as string | undefined;
    let parentType: Component | null = null;
    if (parent !== undefined) {
      const p = this.nodes.get(parent);
      if (!p) return this.reject(el, `parent "${parent}" does not exist`);
      parentType = p.type;
    }
    if (!accepts(parentType, type)) {
      return this.reject(el, `${parentType ?? 'the root'} cannot contain ${type}`);
    }
    if (this.nodes.has(id)) {
      for (let cur = parent; cur !== undefined; cur = this.nodes.get(cur)!.parent) {
        if (cur === id) return this.reject(el, 'an element cannot be inside itself');
      }
    }

    const node: Node = { type, parent, children: [], el, progs: new Map(), deps: [] };
    for (const a of attrs(type)) {
      const v = el[a.name];
      if (v === undefined) continue;
      if (a.type.kind === 'path') node.deps.push(parsePath(v as string)!);
      const src = isCel(v);
      if (src === undefined) continue;
      try {
        const prog = compile(src);
        node.progs.set(a.name, prog);
        node.deps.push(...prog.paths);
      } catch (e) {
        this.diag(`${id}.${a.name}: ${(e as Error).message}`);
      }
    }
    const before = el.before as string | undefined;

    const old = this.nodes.get(id);
    if (!old) {
      this.nodes.set(id, node);
      this.insert(id, parent, before);
      this.changes.created.add(id);
      this.changes.removed.delete(id);
    } else {
      if (old.type === type) {
        node.children = old.children;
      } else {
        for (const ch of old.children) this.removeTree(ch);
        this.changes.created.add(id);
      }
      this.nodes.set(id, node);
      if (old.parent !== parent) {
        const sib = this.childList(old.parent);
        sib.splice(sib.indexOf(id), 1);
        this.changes.reordered.add(old.parent ?? ROOT);
        this.insert(id, parent, before);
      } else if (before !== undefined && before !== id) {
        const sib = this.childList(parent);
        if (sib.includes(before)) {
          sib.splice(sib.indexOf(id), 1);
          sib.splice(sib.indexOf(before), 0, id);
          this.changes.reordered.add(parent ?? ROOT);
        }
      }
    }
    this.changes.updated.add(id);
  }

  private childList(id: string | undefined): string[] {
    return id === undefined ? this.root : this.nodes.get(id)!.children;
  }

  private insert(id: string, parent: string | undefined, before: string | undefined): void {
    const list = this.childList(parent);
    const i = before === undefined ? -1 : list.indexOf(before);
    if (i >= 0) list.splice(i, 0, id);
    else list.push(id);
    this.changes.reordered.add(parent ?? ROOT);
  }

  private remove(id: string): void {
    const n = this.nodes.get(id);
    if (!n) return;
    const sib = this.childList(n.parent);
    sib.splice(sib.indexOf(id), 1);
    this.changes.reordered.add(n.parent ?? ROOT);
    this.removeTree(id);
  }

  private removeTree(id: string): void {
    const n = this.nodes.get(id);
    if (!n) return;
    this.nodes.delete(id);
    for (const ch of n.children) this.removeTree(ch);
    const c = this.changes;
    c.removed.add(id);
    c.created.delete(id);
    c.updated.delete(id);
    c.reordered.delete(id);
  }
}

// open is allowed only when canOpen (on a button).
function validHandler(h: Json, canOpen: boolean): boolean {
  if (!isObject(h)) return false;
  const has = (k: string) => Object.hasOwn(h, k);
  if (!has('patch') && !has('action') && !has('open')) return false;
  if (has('open') && !(canOpen && (isCel(h.open) !== undefined || (typeof h.open === 'string' && h.open !== '')))) {
    return false;
  }
  if (has('patch') && !isObject(h.patch)) return false;
  if (has('action') && (typeof h.action !== 'string' || h.action === '')) return false;
  if (has('indicator')) {
    const p = typeof h.indicator === 'string' ? parsePath(h.indicator) : undefined;
    if (!has('action') || !p || !p[0]!.startsWith('_')) return false;
  }
  return true;
}

// Checks a CEL value against an attribute type (section 9.3 of the spec) and
// returns it as JSON. Throws CheckError.
function check(type: AttrType, v: unknown, def: Json): Json {
  const fail = (want: string) => new CheckError(`expected ${want}, got ${typeName(v)}`);
  switch (type.kind) {
    case 'enum':
      if (typeof v !== 'string') throw fail('a string');
      return type.values.includes(v) ? v : def;
    case 'string':
      if (typeof v !== 'string') throw fail('a string');
      return v;
    case 'bool':
      if (typeof v !== 'boolean') throw fail('a bool');
      return v;
    case 'number?':
      if (v === null) return null;
    // fall through
    case 'number': {
      let n: number;
      if (typeof v === 'number') n = v;
      else if (typeof v === 'bigint') n = Number(v);
      else if (v instanceof Uint) n = Number((v as { value: bigint }).value);
      else throw fail('a number');
      if (!Number.isFinite(n)) throw fail('a finite number');
      return n;
    }
    case 'options': {
      if (!Array.isArray(v)) throw fail('a list');
      return v.map((o) => {
        const ok = isObject(o) && typeof o.value === 'string' && typeof o.label === 'string';
        if (!ok) throw new CheckError('options must be objects with string value and label');
        return { value: o.value, label: o.label } as JsonObject;
      });
    }
  }
  throw new Error('structural attributes are not checked');
}

// Connects a Client to a Server in the same process, for testing an app
// without a renderer.
//
//   const l = new Loopback(() => new Counter());
//   l.trigger('inc', 'onPress');
//   l.client.attr('value', 'text'); // 'Count: 1'
export class Loopback<A extends App> {
  readonly client = new Client();
  readonly server: Server<A>;

  // Starts a session whose hello carries these params.
  constructor(newApp: () => A, params: Record<string, string> = {}) {
    this.server = new Server(newApp);
    this.server.receive(this.client.hello(params));
    this.pump();
  }

  // Delivers the server's pending output to the client.
  pump(): void {
    this.client.receive(this.server.output.take());
  }

  // Triggers a handler, delivers the action (if any) and the response.
  // Returns whether an action was sent.
  trigger(id: string, event: string): boolean {
    const line = this.client.trigger(id, event);
    if (line === undefined) return false;
    this.server.receive(line);
    this.pump();
    return true;
  }

  // Writes to a bound signal, as a control does.
  write(path: string, value: Json): void {
    this.client.write(path, value);
  }

  tick(nowMs: number): void {
    this.server.tick(nowMs);
    this.pump();
  }
}

export { fromJson, toJson } from './cel.ts';
export { Session, type SessionOptions } from './session.ts';
