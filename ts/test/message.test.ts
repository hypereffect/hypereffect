import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  accepts, cel, COMPONENTS, Handler, LineBuffer, mergePatch, parseLine, ParseError, parsePath,
  parseRendererMessage, parseServerMessage, pathPatch, rendererMessageLine, serverMessageLine,
  stripLocal, ui, type Json,
} from '../src/index.ts';

test('element round trip', () => {
  const e = ui.text('title').parent('main').variant('h1').text('Inbox');
  const line = e.toLine();
  assert.equal(line, 'element {"id":"title","type":"text","parent":"main","variant":"h1","text":"Inbox"}\n');
  assert.equal(serverMessageLine(parseServerMessage(line.trimEnd())), line);
});

test('element builders', () => {
  const e = ui.button('inc')
    .parent('buttons')
    .label('+')
    .variant('primary')
    .disabled(cel('s.count >= 10'))
    .onPress(Handler.action('add').withPayload(1));
  assert.equal(
    e.toLine(),
    'element {"id":"inc","type":"button","parent":"buttons","label":"+","variant":"primary","disabled":{"cel":"s.count >= 10"},"onPress":{"action":"add","payload":1}}\n',
  );
});

test('handlers', () => {
  const j = (h: Handler) => JSON.stringify(h);
  assert.equal(
    j(Handler.action('login').withIndicator('_busy').withPatch({ _open: false })),
    '{"patch":{"_open":false},"action":"login","indicator":"_busy"}',
  );
  assert.equal(j(Handler.open('https://example.com')), '{"open":"https://example.com"}');
  assert.equal(
    j(Handler.action('visited').withOpen(cel("'https://example.com/' + s.id"))),
    '{"action":"visited","open":{"cel":"\'https://example.com/\' + s.id"}}',
  );
});

test('renderer messages', () => {
  const m = parseRendererMessage('action {"name":"archive","payload":{"contact":7},"signals":{"filter":"all"}}');
  assert.equal(m.tag, 'action');
  if (m.tag !== 'action') return;
  assert.equal(m.action.signal('filter'), 'all');
  assert.deepEqual(m.action.payload, { contact: 7 });
  assert.equal(rendererMessageLine(m), 'action {"name":"archive","payload":{"contact":7},"signals":{"filter":"all"}}\n');
  assert.deepEqual(parseRendererMessage('hello {}'), { tag: 'hello', params: {} });
  const h = parseRendererMessage('hello {"params":{"room":"abc"}}');
  assert.equal(rendererMessageLine(h), 'hello {"params":{"room":"abc"}}\n');
  assert.throws(() => parseRendererMessage('hello {"params":{"n":1}}'), ParseError);
});

test('server messages', () => {
  assert.deepEqual(parseServerMessage('remove {"id":"x"}'), { tag: 'remove', id: 'x' });
  assert.deepEqual(parseServerMessage('metadata {"name":"Plants","x":1}'), { tag: 'metadata', metadata: { name: 'Plants' } });
  assert.deepEqual(parseServerMessage('metadata {"url":"/a","n":1}'), { tag: 'metadata', metadata: { url: '/a' } });
  const log = parseServerMessage('log {"summary":"Saved","detail":"3 files","type":"success"}');
  assert.deepEqual(log, { tag: 'log', log: { summary: 'Saved', detail: '3 files', type: 'success' } });
  assert.equal(
    serverMessageLine(log),
    'log {"summary":"Saved","detail":"3 files","type":"success"}\n',
  );
  assert.deepEqual(parseServerMessage('log {"summary":"Hi","type":"other"}'), {
    tag: 'log', log: { summary: 'Hi', type: 'info' },
  });
  assert.throws(() => parseServerMessage('log {"detail":"no summary"}'), ParseError);
  assert.throws(() => parseServerMessage('element {"id":"x","type":"marquee"}'), ParseError);
});

test('line buffer', () => {
  const b = new LineBuffer();
  assert.deepEqual(b.push(new TextEncoder().encode('hello {}\r\n\nsig')), ['hello {}']);
  assert.deepEqual(b.push('nal {}\n'), ['signal {}']);
  assert.deepEqual(b.push(new Uint8Array([0xff, 10, 0x61, 32, 123, 125, 10])), ['a {}']);
  assert.throws(() => parseLine('Signal {}'), ParseError);
  assert.throws(() => parseLine('signal []'), (e: ParseError) => e.kind === 'not-an-object');
});

test('rfc 7386 examples', () => {
  const cases: [Json, Json, Json][] = [
    [{ a: 'b' }, { a: 'c' }, { a: 'c' }],
    [{ a: 'b' }, { b: 'c' }, { a: 'b', b: 'c' }],
    [{ a: 'b' }, { a: null }, {}],
    [{ a: [{ b: 'c' }] }, { a: [1] }, { a: [1] }],
    [{ a: 'foo' }, null, null],
    [{ e: null }, { a: 1 }, { e: null, a: 1 }],
    [{}, { a: { bb: { ccc: null } } }, { a: { bb: {} } }],
    [{ a: 5 }, { a: { b: 1 } }, { a: { b: 1 } }],
  ];
  for (const [t, p, want] of cases) assert.deepEqual(mergePatch(t, p), want);
});

test('local signals and paths', () => {
  assert.deepEqual(stripLocal({ a: 1, _b: 2, c: { _d: 3, e: [{ _f: 1, g: 2 }] } }), { a: 1, c: { e: [{ g: 2 }] } });
  assert.deepEqual(parsePath('form.email'), ['form', 'email']);
  assert.deepEqual(parsePath('_busy'), ['_busy']);
  for (const bad of ['a..b', '1a', '']) assert.equal(parsePath(bad), undefined);
  assert.deepEqual(pathPatch(['a', 'b'], 1), { a: { b: 1 } });
});

test('components', () => {
  assert.equal(COMPONENTS.length, 18);
  assert.ok(accepts(null, 'dialog'));
  assert.ok(!accepts(null, 'tab'));
  assert.ok(accepts('tabs', 'tab'));
  assert.ok(!accepts('tabs', 'text'));
  assert.ok(!accepts('card', 'dialog'));
  assert.ok(!accepts('text', 'text'));
});
