import assert from 'node:assert/strict';
import { test } from 'node:test';
import { type Changes, Session } from '../src/client.ts';

const tick = () => new Promise((r) => setTimeout(r, 0));

function open() {
  const sent: string[] = [];
  const updates: Changes[] = [];
  const diags: string[] = [];
  const opened: string[] = [];
  const logs: unknown[] = [];
  const s = new Session({
    send: (l) => sent.push(l),
    onUpdate: (u) => updates.push(u),
    onDiagnostic: (m) => diags.push(m),
    onOpen: (u) => opened.push(u),
    onLog: (l) => logs.push(l),
  });
  return { s, sent, updates, diags, opened, logs };
}

test('batches the changes of one task into one update', async () => {
  const { s, sent, updates } = open();
  s.start();
  s.receive('signal {"n":1,"m":1}\n');
  s.receive('element {"id":"a","type":"text","text":{"cel":"string(s.n)"}}\n');
  s.receive('element {"id":"b","type":"text","text":{"cel":"string(s.m)"}}\n');
  assert.deepEqual(sent, ['hello {}\n']);
  assert.equal(updates.length, 0);
  await tick();
  assert.equal(updates.length, 1);
  assert.ok(updates[0]!.reset && updates[0]!.connection);
  assert.deepEqual([...updates[0]!.created], ['a', 'b']);
});

test('a signal patch updates only the elements that read it', async () => {
  const { s, updates } = open();
  s.start();
  s.receive('element {"id":"a","type":"text","text":{"cel":"string(s.n)"}}\n');
  s.receive('element {"id":"b","type":"text","text":{"cel":"string(s.m)"}}\n');
  s.receive('element {"id":"c","type":"input","bind":"form.name"}\n');
  await tick();
  s.receive('signal {"n":2}\n');
  await tick();
  assert.deepEqual([...updates.at(-1)!.updated], ['a']);
  s.write('form.name', 'Ada');
  await tick();
  assert.deepEqual([...updates.at(-1)!.updated], ['c']);
  assert.equal(s.attr('a', 'text'), '2');
});

test('reports log messages as they happen', () => {
  const { s, logs } = open();
  s.start();
  s.receive('log {"summary":"Saved","type":"success"}\n');
  s.receive('log {"summary":"Careful"}\n');
  assert.deepEqual(logs, [
    { summary: 'Saved', type: 'success' },
    { summary: 'Careful', type: 'info' },
  ]);
});

test('reports diagnostics as they happen', () => {
  const { s, diags } = open();
  s.start();
  s.receiveLine('element {"id":"a","type":"nope"}');
  assert.equal(diags.length, 1);
});

test('trigger sends actions and passes URLs to onOpen', () => {
  const { s, sent, opened } = open();
  s.start();
  s.receive('element {"id":"go","type":"button","onPress":{"open":"https://hypereffect.dev","action":"went"}}\n');
  assert.equal(s.trigger('go', 'onPress'), true);
  assert.deepEqual(opened, ['https://hypereffect.dev']);
  assert.equal(sent.at(-1), 'action {"name":"went","signals":{}}\n');
});
