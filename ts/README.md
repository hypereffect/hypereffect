# hypereffect (TypeScript)

Types and helpers for [hypereffect](https://hypereffect.dev) in TypeScript,
ported from the [Rust crate](https://github.com/hypereffect/hypereffect/tree/master/rust):
elements, handlers and messages with their line framing, signals, the
components and their nesting rules, an `App`/`Server` session driver for
servers, and the renderer core for clients, with a loopback for testing apps
without a renderer.

```
npm install hypereffect
```

The package ships compiled JavaScript with type declarations, and uses nothing
specific to Node.

## Servers

```ts
import { type Action, type App, cel, Handler, type Outbox, ui } from 'hypereffect';

class Counter implements App {
  count = 0;

  hello(out: Outbox) {
    out.signal({ count: 0 });
    out.element(ui.column('main'));
    out.element(ui.text('value').parent('main').text(cel("'Count: ' + string(s.count)")));
    out.element(ui.button('inc').parent('main').label('+').onPress(Handler.action('add')));
  }

  action(out: Outbox, action: Action) {
    if (action.name === 'add') out.signal({ count: ++this.count });
  }
}
```

- `ui.text('id')` and the other constructors return an `Element` with setters
  for every attribute (`label`, `variant`, `disabled`, …, or `set` for any
  other); each takes a literal or `cel('...')`. `bind` sets a signal path, and
  `onPress`, `onChange`, `onSubmit` and `onDismiss` take a `Handler`:
  `Handler.action('save')`, `Handler.patch({...})` or `Handler.open(url)`,
  refined with `withPayload`, `withIndicator`, `withPatch` and `withOpen`.
- `parseServerMessage`, `parseRendererMessage`, `serverMessageLine` and
  `rendererMessageLine` convert to and from lines; `LineBuffer` splits a
  stream of bytes or text into lines.
- `mergePatch`, `stripLocal`, `parsePath` and friends handle signals.
- `Server` drives an `App` from what a renderer sends, creating a new app on
  every hello. An app with `tick` and `tickInterval` gets a timer.

The library does not open connections. Any transport that carries lines
works: pass what arrives to `server.receive`, then send
`server.output.take()`. With a WebSocket (here the `ws` package):

```ts
wss.on('connection', (ws) => {
  const server = new Server(() => new Counter());
  ws.on('message', (data) => {
    server.receive(data.toString() + '\n');
    const out = server.output.take();
    for (const line of out.split('\n')) if (line) ws.send(line);
  });
});
```

## Renderers

`hypereffect/client` is the renderer side of a session, without any drawing.
It parses lines, keeps the element tree and the signals, evaluates CEL
expressions, runs handlers, and tells a UI layer which elements changed. The UI
layer maps components to widgets; see
[`site/demo/renderer.js`](https://github.com/hypereffect/hypereffect/blob/master/site/demo/renderer.js)
for one that uses HTML.

`Session` sends lines and reports changes through callbacks:

```ts
import { Session } from 'hypereffect/client';

const ws = new WebSocket(url);
const session = new Session({
  send: (line) => ws.send(line),
  onUpdate: (u) => redraw(u), // {reset, created, updated, removed, reordered, connection, metadata}
  onOpen: (url) => maybeOpen(url),
  onLog: (log) => showLog(log), // {summary, detail?, type}
});
ws.onopen = () => session.start(); // sends hello
ws.onmessage = (e) => session.receive(e.data + '\n');
ws.onclose = () => session.close();

// Reading, for drawing:
session.children(null);      // ids at the root
session.get('title');        // {id, type, parent, children, attrs: {text: 'Inbox', variant: 'h1', visible: true, ...}}
session.value('form.email'); // a signal

// User input:
session.write('form.email', 'ada@example.com'); // a bound control changed
session.trigger('save', 'onPress');             // runs the handler; may send an action
```

Attributes in `get()` are resolved: expressions are evaluated, values are
checked against the attribute's type, and missing or failing values take their
defaults, so a UI layer never sees a `{"cel": …}` object.

Updates are batched: all messages received in one task are processed before
`onUpdate` is called once. Each expression is analysed for the signal paths it
reads, and a signal patch only marks the elements whose expressions or bindings
read a path the patch touched. Diagnostics (rejected elements, failing
expressions) go to `onDiagnostic` as they happen.

A button's handler can ask to open a URL. The session passes it to `onOpen`
and does nothing else: the host decides which URLs it opens, and how (see the
security considerations in the spec).

`Client` is the same core without callbacks or timers, for hosts that poll:
`hello()` and `trigger()` return the lines to send, and `takeChanges()`,
`takeDiagnostics()`, `takeOpens()` and `takeLogs()` return what happened since
the last call.

CEL runs on [cel-js](https://github.com/marcbachmann/cel-js), with what cel-js
lacks from the functions the spec requires: `charAt`, `replace`, `reverse`,
`format` and `strings.quote` from the cel-go strings extension, and the math
and lists extensions (`lists.first` and `lists.last` excepted, as in the
spec). In a browser, map the bare `@marcbachmann/cel-js` import with an import
map, as the site does:

```html
<script type="importmap">
{ "imports": { "@marcbachmann/cel-js": "https://cdn.jsdelivr.net/npm/@marcbachmann/cel-js@8.0.0/+esm" } }
</script>
```

## Testing

`Loopback` connects a `Client` to a `Server` in one process:

```ts
import { Loopback } from 'hypereffect/client';

test('counter', () => {
  const l = new Loopback(() => new Counter()); // sends hello
  l.trigger('inc', 'onPress');                 // true if an action was sent
  assert.equal(l.client.attr('value', 'text'), 'Count: 1');
  assert.deepEqual(l.client.takeDiagnostics(), []); // rejected elements or failed expressions
});
```

`l.write(path, value)` writes a bound signal as a control does,
`l.tick(nowMs)` runs the timer, `l.client.value(path)` reads a signal and
`l.client.takeOpens()` returns the URLs buttons asked to open.
`examples/todos.ts` is the task list demo; `test/todos.test.ts` tests it.

Differences from the Rust crate, all from JavaScript numbers and cel-js:

- JSON numbers come from `JSON.parse`, so `1.0` in signals is an int rather
  than a double.
- cel-js type-checks literals, so `1 == 1.0` is an error; values read from
  signals compare across numeric types by value.
- Lists cannot be indexed with a `uint`, `int()` of a huge double does not
  fail, and `matches` uses JavaScript regular expressions, which agree with
  RE2 on common syntax but not on inline flags such as `(?i)`.
- There is no WebAssembly export like the Rust `wasm_app!`.

## Tests

```
npm test        # node --test on the TypeScript sources (Node 22.18 or later)
npm run check   # tsc --noEmit
npm run build   # dist/
```

The tests include CEL unit tests and the shared renderer fixtures in the
repository's [`fixtures/`](https://github.com/hypereffect/hypereffect/tree/master/fixtures).
