# Writing a hypereffect server

> Guidelines for language models (and terse humans) writing server-side code
> for [hypereffect](https://hypereffect.dev), in any language. It covers the
> wire messages, the rules that make updates correct, and how the libraries
> for Rust, Go, Python and TypeScript expose them. Normative details live in
> the [spec](https://hypereffect.dev/spec.html) and the
> [component reference](https://hypereffect.dev/components.md). For large
> interfaces in Rust, see
> [rust-vtree-guide.md](https://hypereffect.dev/rust-vtree-guide.md).

## Mental model

- The **server** owns all application state and decides what the interface
  is. The **renderer** knows nothing about the app: it draws elements with
  native widgets, evaluates expressions, and sends actions back.
- The interface is a **tree of elements** (instances of 18 fixed components)
  plus a **signals** object (shared JSON state). Element attributes can be
  [CEL](https://cel.dev) expressions over the signals, recomputed by the
  renderer whenever what they read changes.
- The server updates the interface by sending, at any time, any number of:
  - `element`: create, replace or move one element,
  - `remove`: delete one element and its subtree,
  - `signal`: a JSON Merge Patch (RFC 7386) on the signals,
  - `metadata`: optional app name, description and icon.
- The renderer sends `hello` once, then `action` messages when the user
  triggers a handler. Each action carries all non-local signals.

**Design rule: build the tree once, then change as little as possible.**
Prefer, in this order:

1. **No message at all.** UI-only state (filters, open dialogs, tab choice,
   "editing" mode) lives in *local signals* (`_`-prefixed), written by handler
   `patch`es and read by expressions. No round trip.
2. **A `signal` patch.** Data the server changes (counts, statuses, progress,
   flags, per-item values) goes in signals; attributes that depend on it are
   expressions. A patch is tiny and touches only what it names.
3. **Re-sending one `element`.** Only when a *literal* attribute must change
   or an element must move. Same `id` and `type` keeps widget state (focus,
   cursor, scroll).
4. **Adding / removing elements.** Only when the set of things on screen
   changes (an item is added or deleted).

Never rebuild the interface from scratch to apply a change, and never
`remove` then re-add an element to update it: that destroys focus and
widget state, and costs bandwidth.

What that looks like for a task list, where each row `t<n>` has a title
text, a done button, an edit button and a delete button, with `done.<id>`
in signals and a "N of M left" caption computed by an expression:

| User does | Server sends |
| --- | --- |
| adds a task | 1 `signal` (the new task's state), then the new row and its children, with `before` to insert at the top |
| marks a task done | 1 `signal`: `{"done":{"t3":true}}` (plus the indicator reset) |
| opens the title editor | nothing: a handler `patch` sets the local `_editing` |
| renames a task | 1 re-sent `text` element (its literal changed) |
| moves a task up | 1 re-sent row with `before` (its children move along) |
| deletes a task | 1 `remove` of the row (takes the children) + 1 `signal` with `null`s |
| any of the above | the caption updates by itself: it is an expression |

## Wire format

Newline-delimited UTF-8 lines: `tag SP json-object LF`. Example session:

```
renderer → server
hello {}

server → renderer
signal {"count":0}
element {"id":"main","type":"column"}
element {"id":"value","type":"text","parent":"main","text":{"cel":"'Count: ' + string(s.count)"}}
element {"id":"inc","type":"button","parent":"main","label":"+","onPress":{"action":"add"}}

renderer → server
action {"name":"add","signals":{"count":0}}

server → renderer
signal {"count":1}
```

Any transport that carries lines works (stdio, TCP, WebSocket, SSE+POST).
The libraries do not open connections: feed received bytes to the server
object and send what it outputs.

## The same server in each language

Each library has an `App` (one value per session, created anew on every
`hello`) with `hello` and `action` methods, an `Outbox` they write messages
to, element constructors in a `ui` module, `cel(...)` to mark an expression,
and a `Server` that drives an app from raw bytes. Everything written to the
outbox during one call is sent as one batch, in order; renderers process a
batch before redrawing.

Rust (`cargo add hypereffect`):

```rust
use hypereffect::ui::*;
use hypereffect::{cel, json, Action, App, Handler, Outbox};

#[derive(Default)]
struct Counter { count: i64 }

impl App for Counter {
    fn hello(&mut self, out: &mut Outbox, _params: &std::collections::BTreeMap<String, String>) {
        out.signal(json!({ "count": 0 }));
        out.element(column("main"));
        out.element(text("value").parent("main").text(cel("'Count: ' + string(s.count)")));
        out.element(button("inc").parent("main").label("+").on_press(Handler::action("add")));
    }
    fn action(&mut self, out: &mut Outbox, action: &Action) {
        if action.name == "add" {
            self.count += 1;
            out.signal(json!({ "count": self.count }));
        }
    }
}
```

Go (`go get github.com/hypereffect/hypereffect/go`):

```go
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

Python (`pip install hypereffect-ui`, imported as `hypereffect`):

```python
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

TypeScript (`npm install hypereffect`):

```ts
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

Equivalents for the rest of this guide:

| Concept | Rust | Go | Python | TypeScript |
| --- | --- | --- | --- | --- |
| action handler | `Handler::action("save")` | `he.Do("save")` | `Handler(action="save")` | `Handler.action('save')` |
| with payload | `.payload(v)` | `.WithPayload(v)` | `payload=v` | `.withPayload(v)` |
| with indicator | `.indicator("_busy")` | `.WithIndicator("_busy")` | `indicator="_busy"` | `.withIndicator('_busy')` |
| local patch | `Handler::patch(json!({..}))` | `he.Patch(map[string]any{..})` | `Handler(patch={..})` | `Handler.patch({..})` |
| patch, then send | `.with_patch(..)` | `.WithPatch(..)` | `Handler(action=.., patch=..)` | `.withPatch(..)` |
| read a signal | `action.signal("a.b")` | `a.Signal("a.b")` | `action.signal("a.b")` | `action.signal('a.b')` |
| read the payload | `action.payload_as::<T>()` | `a.DecodePayload(&v)` | `action.payload` | `action.payload` |
| timer | `tick` + `tick_interval` | `Ticker` interface | `tick` + `tick_interval` | `tick` + `tickInterval` |
| test harness | `server::Loopback` | `client.NewLoopback` | `client.Loopback` | `Loopback` (`hypereffect/client`) |

Each element constructor has a setter per attribute (`label`, `variant`,
`disabled`, …; `set` for anything else) taking a literal or a `cel(...)`
expression, plus `parent`, `before`, `bind` and one setter per handler.

## Messages

### `element`

- **Unknown id → insert** under `parent` (or the root), before the sibling
  named by `before`, else appended. The parent must already exist: send
  parents before children.
- **Known id → replace.** *All* attributes are replaced; omitted ones reset
  to defaults. It is not a merge. If `type` is unchanged the element keeps
  its children and widget state. If `type` changed, its children are
  dropped.
- **Move:** re-send with a different `parent`, or with `before` naming a
  sibling. Children move along.
- `id`, `type`, `parent`, `before`, `bind` and handlers are structural: they
  are always literals, never expressions.

Because replacement is total, write **one builder function per element**
(or per element kind) and use it both to create and to re-send. Then a
re-send cannot accidentally drop an attribute.

### `remove`

Removes the element and its whole subtree. Removing a non-existent id is a
no-op. Ids can be reused afterwards.

### `signal`

A JSON Merge Patch over the signals:

- objects merge recursively: `{"user":{"name":"Ada"}}` changes only
  `user.name`;
- `null` **deletes** a key (so you cannot store `null`; use `""`, `false`
  or `0` as the empty value);
- any other value, **including arrays**, replaces the old value wholesale.

Keep per-item state in **objects keyed by item id**
(`{"done":{"t1":true,"t2":false}}`), not arrays. Then one item can be
patched (`{"done":{"t2":true}}`) or deleted (`{"done":{"t2":null}}`) alone.

Local signals are keys starting with `_`, at any depth. The renderer never
sends them back to the server; the server may still write them (to reset an
indicator or close a dialog).

### `metadata`

Optional `name`, `description` and `icon` (URL). Other string members are
extensions only renderers that define them read. Each message replaces the
previous one; renderers may ignore it.

### `hello`

Carries string `params` chosen by whoever opened the session (for example
from a URL). They are user input. The session starts with no elements and
signals `{}`. Build the initial interface here: **signals first** (so
expressions have something to read), then elements, parents before
children.

### `action`

Carries the handler's action `name`, its `payload` verbatim, and all
non-local `signals` at send time. Everything in it is untrusted: validate
the name, the payload (an id in a payload does not prove the user was shown
that item) and every signal you read. There is no error message type: show
errors with ordinary UI (a `notice`, an input's `error`).

### Handlers

A handler has any of `patch` (applied locally, leaves may be CEL), `action`
(sent to the server, with an optional `payload` and `indicator`) and, on
buttons, `open` (a URL the host may open). When triggered, the renderer
stops if the element or an ancestor is disabled or hidden, applies `patch`,
opens `open`, sets `indicator` to `true`, and sends the action.

**The renderer never clears an indicator: the server must reset it in every
reply, including early-return and error paths.**

### Binding

`bind: "a.b"` makes a control read and write `s.a.b`: `input` (string;
number or null for `variant: "number"`), `checkbox` (bool), `select`
(option value string), `slider` (number), `tabs` (selected tab id). User
edits are applied locally and immediately; the server sees them in the
`signals` of the next action. Add an `onChange` action if the server must
react to each change. If the server writes a bound path, the control shows
the new value even while focused.

`bind` suits drafts and inputs as they are. A bound control whose change the
server must confirm needs the care described next.

## Server-confirmed state

When an action needs the server to say whether it happened (persisting a
change, switching a device, anything that can fail or be refused), the
server's reply is the only truth. The client may show that a request is in
flight, but **never present an unconfirmed value as settled**.

### Pattern A: request without touching the value

For buttons, badges and text, whose look is computed from the value. The
value is server-owned; a local flag (an indicator, so it starts with `_`)
marks the request:

```
signals        {"value":false,"_loading":false}
user clicks →  renderer sets _loading = true (indicator), value untouched
renderer →     action {"name":"toggled","payload":true,"signals":{"value":false}}
server →       signal {"_loading":false,"value":true}
```

- No handler `patch` writes the value.
- Disable the control and show progress with expressions over `s._loading`.
- The payload names the **wanted** state, not "flip", so a duplicated
  request is harmless. Payloads are literals: use one button per possible
  request and show the right one with a `visible` expression.
- Reply with the confirmed value **and** the loading reset in one patch, on
  every path, including refusals.

### Pattern B: bound controls (optimistic, pending, corrected)

`checkbox`, `select`, `slider` and `tabs` show their value only through
`bind`, so a click writes the requested value before any handler runs.
Make it visibly a request:

```
signals        {"auto":{"value":false,"_loading":false},"error":""}
user clicks →  bind writes auto.value = true; onChange's patch sets auto._loading = true
renderer →     action {"name":"auto","signals":{"auto":{"value":true},"error":""}}
server →       signal {"auto":{"value":true,"_loading":false},"error":""}                                (success)
server →       signal {"auto":{"value":false,"_loading":false},"error":"Could not change auto-update."}  (failure)
```

- Set the pending flag with the handler's `patch` (an indicator path must
  *start* with `_`, so a nested flag needs a patch).
- Disable the control while pending and say so in its label.
- Read the bound value in the action as the *requested* state; validate it.
- Reply on every path with the server's own value and the flag reset.
- While pending, other actions also carry the unconfirmed value: never take
  it from there.

Prefer A when even a brief wrong value is unacceptable (a payment, a door
lock), B when the native control matters more. **The mistake** both avoid is
a bound control or a patch that writes the value with no pending state and
no authoritative reply.

## Components

The full reference is [components.md](https://hypereffect.dev/components.md).

Containers (accept children): `column`, `row`, `card`, `list`, `tab`,
`dialog`. `tabs` accepts only `tab`. `tab` goes only in `tabs`. `dialog`
goes only at the root (shown as an overlay). Everything else is a leaf.

All components have `visible` (bool, default true). Controls (`button`,
`input`, `checkbox`, `select`, `slider`) also have `disabled`.

| Component | Attributes (default) |
| --- | --- |
| `column` | `gap` none/small/**medium**/large, `align` start/center/end/**stretch** |
| `row` | `gap` (**small**), `align` (**center**), `justify` **start**/center/end/between, `wrap` (false) |
| `card` | `title`, `gap` (medium) |
| `list` | `empty` (text shown when it has no visible children) |
| `tabs` | `bind` (selected tab id), `onChange` |
| `tab` | `label` (defaults to the id), `gap` |
| `dialog` | `open` (true), `title`, `onDismiss` |
| `text` | `text`, `variant` h1/h2/h3/**body**/caption/code |
| `notice` | `title`, `text`, `variant` **info**/success/warning/failure |
| `badge` | `text`, `variant` **neutral**/info/success/warning/failure |
| `image` | `src`, `alt`, `variant` **inline**/thumbnail/avatar/full |
| `divider` | — |
| `progress` | `value` 0..1 or null (indeterminate), `label`, `variant` **bar**/spinner |
| `button` | `label`, `variant` primary/**secondary**/plain/success/failure, `onPress` |
| `input` | `bind`, `label`, `placeholder`, `error` (non-empty shows it), `variant` **text**/multiline/password/number/email, `onChange`, `onSubmit` |
| `checkbox` | `bind`, `label`, `variant` **checkbox**/switch, `onChange` |
| `select` | `bind`, `label`, `options` `[{"value":..,"label":..}]`, `variant` **dropdown**/radio/segmented, `onChange` |
| `slider` | `bind`, `label`, `min` (0), `max` (100), `step` (1), `onChange` |

Variants are semantic, never colours or sizes: the renderer chooses the
look. Unknown enum values fall back to the default.

## Expressions (CEL)

- One variable, `s`, holds the signals: `s.count`, `s.user.name`,
  `s.done['t1']`, `s.done[k]`.
- Standard CEL plus the strings extension: `size`, `in`, `has()`,
  `.exists()`, `.all()`, `.filter()`, `.map()`, `string()`, `int()`,
  `double()`, `.trim()`, `.contains()`, `.format([...])`, map literals, `?:`.
- Results are type-checked: string attributes need a string (no implicit
  conversion: `'n = ' + string(s.n)`), bool attributes need a bool.
- Ints and doubles do not mix in arithmetic. Serialize numbers consistently
  (`0.0` vs `0`), or convert with `double()`.
- A failing expression (missing signal, wrong type) silently yields the
  attribute's default. Initialise every signal an expression reads before,
  or in the same batch as, the element that reads it.
- Formatting server-generated ids into expressions (`s._editing == 't3'`)
  is fine. Never interpolate user-supplied text into CEL; put it in a signal
  and read the signal.
- Lookup tables for variants: `{'ok':'success','bad':'failure'}[s.status]`.

## Ids

Ids are global across the tree, not per parent. Derive them from domain ids
and suffix children: row `t3`, its title `t3-text`, its button `t3-del`.
Never derive them from positions. Stable ids are what make fine-grained
updates possible.

## Structuring a server

Two styles work; pick by the size of the interface.

- **Hand-written updates** (any language). Send the tree in `hello`, then
  for each action send exactly the messages it needs: a signal patch, a
  re-sent element from its builder, a new element with `before`, a
  `remove`. Best for small interfaces and for anything driven mostly by
  signals (a dashboard sent once, then only patches from a timer).
- **View as a tree, diffed** (Rust's `vtree` module). Write the interface as
  a function of the state returning a tree, keep the last tree sent, and
  after each change send the diff. Best when many actions change the
  structure (items moving between groups, conditional sections, reordering).
  See [rust-vtree-guide.md](https://hypereffect.dev/rust-vtree-guide.md).

In both, values that change often (progress, live numbers, statuses) still
belong in signals read by expressions, not in literal attributes.

## Testing

Every library has a loopback that runs a real renderer core (element tree,
signals, CEL) against your app in-process. Write to bound signals as a user
would, trigger handlers by element id and event, then assert on resolved
attributes, children and signals:

```rust
let mut l = Loopback::<Counter>::new(); // sends hello
assert!(l.trigger("inc", "onPress"));   // true if an action was sent
assert_eq!(l.client.attr("value", "text"), Some(json!("Count: 1")));
assert!(l.client.take_diagnostics().is_empty());
```

Always assert that diagnostics are empty: they catch wrong nesting, missing
parents, CEL type errors and failing expressions.

## Checklist

- Signals before the elements that read them; parents before children.
- Every signal an expression reads is initialised (no `null`: it deletes).
- Per-item state in id-keyed objects, never arrays.
- One builder per element; re-sends reuse it (replacement is not a merge).
- Update with `signal` when possible, re-send one element when a literal or
  position changes, `remove` only to delete. Never rebuild.
- UI-only state in `_` local signals, changed by handler patches.
- Server-confirmed state is never presented as settled before the reply
  (pattern A or B), and every reply carries the server's value.
- Every action reply resets the indicators it may have set, on all paths.
- Validate every action name, payload and signal read.
- No user text inside CEL strings; no expressions in structural attributes.
- `dialog` only at the root, `tab` only in `tabs`.
