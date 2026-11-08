# hypereffect (Go)

Types and helpers for writing [hypereffect](https://hypereffect.dev) servers in
Go, ported from the [Rust crate](https://github.com/hypereffect/hypereffect/tree/master/rust):
elements, handlers and messages with their line framing, signals, the
components and their nesting rules, an `App`/`Server` session driver, and a
renderer client with a loopback for testing apps without a renderer.

```
go get github.com/hypereffect/hypereffect/go
```

## Servers

```go
import (
	he "github.com/hypereffect/hypereffect/go"
	"github.com/hypereffect/hypereffect/go/ui"
)

type Counter struct{ count int }

func (c *Counter) Hello(out *he.Outbox, params map[string]string) {
	out.Signal(map[string]any{"count": 0})
	out.Element(ui.Column("main"))
	out.Element(ui.Text("value").Parent("main").Text(he.Cel("'Count: ' + string(s.count)")))
	out.Element(ui.Button("inc").Parent("main").Label("+").OnPress(he.Do("add")))
}

func (c *Counter) Action(out *he.Outbox, a *he.Action) {
	if a.Name == "add" {
		c.count++
		out.Signal(map[string]any{"count": c.count})
	}
}
```

- `ui.Text("id")` and the other constructors return an `*Element` with setters
  for every attribute (`Label`, `Variant`, `Disabled`, …, or `Set` for any
  other); each takes a literal or `he.Cel("...")`. `Bind` sets a signal path,
  and `OnPress`, `OnChange`, `OnSubmit` and `OnDismiss` take a `Handler`:
  `he.Do("save")`, `he.Patch(...)` or `he.Open(url)`, refined with
  `WithPayload`, `WithIndicator`, `WithPatch` and `WithOpen`.
- `ParseServerMessage`, `ParseRendererMessage` and each message's `Line`
  convert to and from lines; `LineBuffer` splits a byte stream into lines.
- Package `signals` has merge patch, local-signal stripping and signal paths.
- `Server` drives an `App` from the bytes a renderer sends, creating a new app
  on every hello. An app that also implements `Ticker` gets `Tick` calls at
  its `TickInterval`.

The library does not open connections. Any transport that carries lines
works: write what arrives to `Server.Receive`, then send and clear
`Server.Output()`. Over stdio:

```go
s := he.NewServer(func() *Counter { return &Counter{} })
buf := make([]byte, 4096)
for {
	n, err := os.Stdin.Read(buf)
	if err != nil {
		return
	}
	s.Receive(buf[:n])
	os.Stdout.WriteString(s.Output().Take())
}
```

## Testing

Package `client` is the renderer side without drawing, on top of
[cel-go](https://github.com/google/cel-go) with its strings, math and lists
extensions. `client.Loopback` connects it to a `Server` in one process:

```go
func TestCounter(t *testing.T) {
	l := client.NewLoopback(func() *Counter { return &Counter{} }) // sends hello
	l.Trigger("inc", "onPress")                                     // true if an action was sent
	if v, _ := l.Client.Attr("value", "text"); v != "Count: 1" {
		t.Fatal(v)
	}
	if d := l.Client.TakeDiagnostics(); len(d) > 0 {
		t.Fatal(d) // rejected elements or failed expressions
	}
}
```

`l.Write(path, value)` writes a bound signal as a control does, `l.Tick(now)`
runs the timer, `l.Client.Value(path)` reads a signal,
`l.Client.TakeOpens()` returns the URLs buttons asked to open and
`l.Client.TakeLogs()` the log messages from the server.
`examples/todos` is the task list demo with its test.

Differences from the Rust crate:

- Numbers in resolved attributes are `float64`. Signals and payloads keep
  `json.Number`, so ints and doubles stay apart.
- cel-go type-checks expressions, so a literal type error such as `1 == 1.0`
  fails when the element arrives rather than when the expression is
  evaluated. Values read from signals compare across numeric types by value.
- There is no WebAssembly export like the Rust `wasm_app!`.

## Tests

```
go test ./...
```

This runs unit tests, the todos example, and the shared renderer fixtures in
the repository's [`fixtures/`](https://github.com/hypereffect/hypereffect/tree/master/fixtures).
