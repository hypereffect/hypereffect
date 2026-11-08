// Runs the shared fixtures in ../../fixtures against Client.

import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { test } from 'node:test';
import { Client } from '../src/client.ts';

const dir = path.join(import.meta.dirname, '..', '..', 'fixtures');

// Deep equality with numbers compared by value.
function same(a: unknown, b: unknown): boolean {
  if (Array.isArray(a)) return Array.isArray(b) && a.length === b.length && a.every((x, i) => same(x, b[i]));
  if (a && typeof a === 'object') {
    if (!b || typeof b !== 'object' || Array.isArray(b)) return false;
    const ka = Object.keys(a);
    return ka.length === Object.keys(b).length &&
      ka.every((k) => Object.hasOwn(b, k) && same((a as never)[k], (b as never)[k]));
  }
  return a === b;
}

const cut = (s: string): [string, string] => {
  const i = s.indexOf(' ');
  return i < 0 ? [s, ''] : [s.slice(0, i), s.slice(i + 1)];
};

function run(file: string) {
  const c = new Client();
  const sent: string[] = [];
  const failures: string[] = [];
  const lines = fs.readFileSync(path.join(dir, file), 'utf8').split('\n');
  lines.forEach((line, i) => {
    const at = `${file}:${i + 1}`;
    if (line === '' || line.startsWith('#')) return;
    if (line.startsWith('< ')) {
      const [tag, body] = cut(line.slice(2));
      const got = sent.shift();
      if (got === undefined) {
        failures.push(`${at}: expected ${line.slice(2)}, nothing was sent`);
        return;
      }
      const [gtag, gbody] = cut(got.trim());
      if (gtag !== tag || !same(JSON.parse(gbody), JSON.parse(body))) failures.push(`${at}: got ${got.trim()}`);
      return;
    }
    if (line.startsWith('!')) {
      if (sent.length) failures.push(`${at}: unexpected output ${sent.splice(0)[0]!.trim()}`);
      const [cmd, args] = cut(line.slice(1));
      switch (cmd) {
        case 'start': sent.push(c.hello()); break;
        case 'write': {
          const [p, v] = cut(args);
          c.write(p, JSON.parse(v));
          break;
        }
        case 'trigger': {
          const l = c.trigger(...cut(args));
          if (l !== undefined) sent.push(l);
          break;
        }
        case 'close': c.close(); break;
        default: throw new Error(`${at}: unknown command ${cmd}`);
      }
      return;
    }
    if (line.startsWith('=')) {
      const [cmd, args] = cut(line.slice(1));
      switch (cmd) {
        case 'attrs': {
          const [id, want] = cut(args);
          const r = c.get(id);
          if (!r) {
            failures.push(`${at}: no element ${id}`);
            break;
          }
          for (const [k, v] of Object.entries(JSON.parse(want))) {
            const got = r.attrs[k] ?? null;
            if (!same(got, v)) failures.push(`${at}: ${id}.${k} is ${JSON.stringify(got)}, want ${JSON.stringify(v)}`);
          }
          break;
        }
        case 'children': {
          const [id, want] = cut(args);
          const got = c.children(id === '-' ? null : id);
          if (!same(got, JSON.parse(want))) failures.push(`${at}: children of ${id} are ${JSON.stringify(got)}`);
          break;
        }
        case 'signals':
          if (!same(c.signals, JSON.parse(args))) failures.push(`${at}: signals are ${JSON.stringify(c.signals)}`);
          break;
        case 'metadata':
          if (!same(c.metadata, JSON.parse(args))) failures.push(`${at}: metadata is ${JSON.stringify(c.metadata)}`);
          break;
        case 'opened': {
          const got = c.takeOpens();
          if (!same(got, JSON.parse(args))) failures.push(`${at}: opened ${JSON.stringify(got)}`);
          break;
        }
        case 'logs': {
          const got = c.takeLogs();
          if (!same(got, JSON.parse(args))) failures.push(`${at}: logs are ${JSON.stringify(got)}`);
          break;
        }
        case 'absent':
          if (c.contains(args)) failures.push(`${at}: ${args} exists`);
          break;
        default: throw new Error(`${at}: unknown check ${cmd}`);
      }
      return;
    }
    c.receiveLine(line);
  });
  if (sent.length) failures.push(`${file}: unexpected output at the end: ${sent[0]!.trim()}`);
  const opens = c.takeOpens();
  if (opens.length) failures.push(`${file}: URLs opened but not checked: ${JSON.stringify(opens)}`);
  const logs = c.takeLogs();
  if (logs.length) failures.push(`${file}: logs received but not checked: ${JSON.stringify(logs)}`);
  assert.deepEqual(failures, []);
}

const files = fs.readdirSync(dir).filter((f) => f.endsWith('.txt')).sort();
assert.ok(files.length);
for (const f of files) test(f, () => run(f));
