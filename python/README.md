# hypereffect (Python)

Types and helpers for writing [hypereffect](https://hypereffect.dev) servers in
Python, ported from the [Rust crate](https://github.com/hypereffect/hypereffect/tree/master/rust):
elements, handlers and messages with their line framing, signals, the
components and their nesting rules, an `App`/`Server` session driver, and a
renderer client with a loopback for testing apps without a renderer.

```
pip install hypereffect-ui
```

The distribution is `hypereffect-ui`; the package is imported as
`hypereffect`. It needs Python 3.11 or later.

## Servers

```python
from hypereffect import Action, App, Handler, Outbox, cel, ui


class Counter(App):
    def __init__(self) -> None:
        self.count = 0

    def hello(self, out: Outbox, params: dict[str, str]) -> None:
        out.signal({"count": 0})
        out.element(ui.column("main"))
        out.element(ui.text("value").parent("main").text(cel("'Count: ' + string(s.count)")))
        out.element(ui.button("inc").parent("main").label("+").on_press(Handler(action="add")))

    def action(self, out: Outbox, action: Action) -> None:
        if action.name == "add":
            self.count += 1
            out.signal({"count": self.count})
```

- `ui.text("id")` and the other constructors return an `Element` with setters
  for every attribute (`label`, `variant`, `disabled`, …, or `set` for any
  other); each takes a literal or `cel("...")`. `bind` sets a signal path, and
  `on_press`, `on_change`, `on_submit` and `on_dismiss` take a `Handler`,
  built with keywords: `Handler(action="save", payload=7, indicator="_busy")`,
  `Handler(patch={"_open": True})` or `Handler(open="https://…")`.
- `parse_server_message`, `parse_renderer_message` and each message's
  `to_line` convert to and from lines; `LineBuffer` splits a stream of bytes
  or text into lines.
- `hypereffect.signals` has merge patch, local-signal stripping and signal
  paths.
- `Server` drives an `App` from what a renderer sends, creating a new app on
  every hello. An app that overrides `tick_interval` gets `tick` calls.

The library does not open connections. Any transport that carries lines
works: pass what arrives to `server.receive`, then send
`server.output.take()`. Over stdio:

```python
import sys

server = Server(Counter)
for chunk in iter(sys.stdin.buffer.readline, b""):
    server.receive(chunk)
    sys.stdout.write(server.output.take())
    sys.stdout.flush()
```

## Testing

`hypereffect.client` is the renderer side without drawing, on top of
[cel-python](https://github.com/cloud-custodian/cel-python).
`Loopback` connects it to a `Server` in one process:

```python
from hypereffect.client import Loopback


def test_counter() -> None:
    l = Loopback(Counter)               # sends hello
    assert l.trigger("inc", "onPress")  # True if an action was sent
    assert l.client.attr("value", "text") == "Count: 1"
    assert l.client.take_diagnostics() == []  # rejected elements or failed expressions
```

`l.write(path, value)` writes a bound signal as a control does,
`l.tick(now_ms)` runs the timer, `l.client.value(path)` reads a signal,
`l.client.take_opens()` returns the URLs buttons asked to open and
`l.client.take_logs()` the log messages from the server.
`examples/todos.py` is the task list demo; `tests/test_todos.py` tests it.

cel-python lacks parts of the CEL environment the spec requires, so
`hypereffect._cel` adds them: comparison of ints and doubles by value, the
strings, math and lists extensions (following cel-go), the `sortBy` macro,
and the `math.`, `strings.` and `lists.` namespaces. It relies on
cel-python's evaluator internals, so the dependency is pinned to 0.5.x.

There is no WebAssembly export like the Rust `wasm_app!`.

## Tests

```
uv run pytest
uv run mypy
```

The tests include the shared renderer fixtures in the repository's
[`fixtures/`](https://github.com/hypereffect/hypereffect/tree/master/fixtures).
