# Writing a hypereffect server in Rust

> A guide for language models (and terse humans) writing server-side code for
> [hypereffect](https://hypereffect.dev), using the `hypereffect` crate
> (version 0.4). It covers the wire messages, how each one maps to Rust, the
> rules that make updates correct, and two complete, tested examples.
> Normative details live in the [spec](https://hypereffect.dev/spec.html) and
> the [component reference](https://hypereffect.dev/components.html).

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

## Messages → Rust

Imports used everywhere:

```rust
use hypereffect::ui::*; // column, row, card, list, tabs, tab, dialog, text, notice,
                        // badge, image, divider, progress, button, input, checkbox,
                        // select, slider
use hypereffect::{cel, json, Action, App, Element, Handler, Map, Metadata, Outbox, Value};
use std::collections::BTreeMap;
```

### The `App` trait

One `App` value exists per session. It is created with `Default` on every
`hello` and dropped when the session ends.

```rust
pub trait App {
    fn hello(&mut self, out: &mut Outbox, params: &BTreeMap<String, String>);
    fn action(&mut self, out: &mut Outbox, action: &Action);
    fn tick(&mut self, _out: &mut Outbox, _now_ms: f64) {}       // optional timer
    fn tick_interval(&self) -> Option<u32> { None }               // ms, or no timer
}
```

Everything you write to the `Outbox` during one call is sent as one batch,
in order. Renderers process a batch before redrawing, so a batch is
effectively atomic for the user.

`Server<A>` drives an `App` from raw bytes (`receive`, `tick`, `output()`);
`hypereffect::wasm_app!(MyApp)` exports one from a wasm module. Any
transport that carries lines works (stdio, TCP, WebSocket, SSE+POST).

### `element` → `out.element(...)`

| Wire | Rust |
| --- | --- |
| `{"id":"x","type":"button"}` | `button("x")` (or `Element::new("x", Component::Button)`) |
| `"parent":"p"` | `.parent("p")` |
| `"before":"y"` | `.before("y")` |
| `"label":"Save"` | `.label("Save")` |
| `"label":{"cel":"..."}` | `.label(cel("..."))` |
| `"bind":"form.email"` | `.bind("form.email")` |
| `"onPress":{"action":"save"}` | `.on_press(Handler::action("save"))` |
| any other attribute | `.set("name", value)` |

Attribute setters: `visible disabled gap align justify wrap title empty label
open value variant src alt placeholder error options min max step text`.
Each takes a literal (`&str`, `bool`, number, `json!(...)`) or `cel("...")`.
Handler setters: `on_press on_change on_submit on_dismiss`.

**`id`, `type`, `parent`, `before`, `bind` and handlers are structural: they
must be literals, never `cel(...)`.**

What the renderer does with an `element` message:

- **Unknown id → insert** under `parent` (or the root), before the sibling
  named by `before`, else appended. The parent must already exist: send
  parents before children.
- **Known id → replace.** *All* attributes are replaced; omitted ones reset to
  defaults. It is not a merge. If `type` is unchanged the element keeps its
  children and widget state. If `type` changed, its children are dropped.
- **Move:** re-send with a different `parent`, or with `before` naming a
  sibling. Children move along.

Because replacement is total, write **one builder function per element**
(or per element kind) and use it both to create and to re-send. Then a
re-send cannot accidentally drop an attribute.

### `remove` → `out.remove("id")`

Removes the element and its whole subtree. Removing a non-existent id is a
no-op. Ids can be reused afterwards.

### `signal` → `out.signal(json!({...}))`

A JSON Merge Patch over the signals:

- objects merge recursively: `{"user":{"name":"Ada"}}` changes only
  `user.name`;
- `null` **deletes** a key (so you cannot store `null`; use `""`, `false`
  or `0` as the empty value);
- any other value, **including arrays**, replaces the old value wholesale.

Consequence: keep per-item state in **objects keyed by item id**
(`{"done":{"t1":true,"t2":false}}`), not arrays. Then one item can be
patched (`{"done":{"t2":true}}`) or deleted (`{"done":{"t2":null}}`) alone.

The argument must be a JSON object (it panics otherwise). For dynamic keys
use `json!({ "done": { &id: true } })`, or build a `Map<String, Value>`.

Local signals: any key starting with `_`, at any depth. The renderer never
sends them back to the server; the server may still write them (for example
to reset an indicator or close a dialog).

### `metadata` → `out.metadata(Metadata::new().name("Tasks"))`

Optional: `name`, `description`, `icon` (URL). Other string members
(`.extra(key, value)`) are extensions that only renderers which define them
read. Each message replaces the previous one. Renderers may ignore it.

### `hello` → `App::hello(out, params)`

`params` are string key/values chosen by whoever opened the session (for
example from a URL). They are user input. The session starts with no
elements and signals `{}`. Build the initial interface here: **signals
first** (so expressions have something to read), then elements, parents
before children.

### `action` → `App::action(out, action)`

```rust
pub struct Action {
    pub name: String,                  // the handler's action name
    pub payload: Option<Value>,        // the handler's payload, verbatim
    pub signals: Map<String, Value>,   // all non-local signals at send time
}
action.payload_as::<String>()          // Option<T>: typed payload
action.signal("form.email")            // Option<&Value>: signal by path
```

Everything in an action is untrusted user input: validate the name, the
payload (an id in a payload does not prove the user was shown that item),
and every signal you read. Reply with signals/elements; there is no error
message type. Show errors with ordinary UI (`notice`, an input's `error`).

### Handlers

```rust
Handler::action("save")                          // {"action":"save"}
Handler::action("delete").payload("t3")          // payload: any JSON
Handler::action("login").indicator("_busy")      // sets s._busy = true when sent
Handler::patch(json!({ "_open": true }))         // local only, no round trip
Handler::patch(json!({ "_open": cel("!s._open") }))   // patch leaves may be CEL
Handler::action("clear").with_patch(json!({ "_confirm": false }))  // patch, then send
Handler::open("https://example.com/help")        // buttons only: opens a URL
Handler::open(cel("'mailto:' + s.email"))        // the URL may be CEL
```

When triggered, the renderer: stops if the element or an ancestor is
disabled or hidden; applies `patch`; opens `open` (the host may refuse); sets `indicator` to `true`; sends the
action. **The renderer never clears an indicator: the server must reset it
in every reply, including early-return and error paths**
(`out.signal(json!({ "_busy": false }))`).

### Binding

`bind: "a.b"` makes a control read and write `s.a.b`: `input` (string; number
or null for `variant: "number"`), `checkbox` (bool), `select` (option value
string), `slider` (number), `tabs` (selected tab id). User edits are applied
locally and immediately; the server sees them in the `signals` of the next
action. Add `on_change(Handler::action(...))` if the server must react to
each change. If the server writes a bound path, the control shows the new
value even while focused.

`bind` suits drafts and inputs (form fields, a search box, a setting the
server reads on the next submit) as they are. A bound control whose change
the server must confirm needs the extra care described in the next section.

## Server-confirmed state

When an action needs the server to say whether it happened (persisting a
change, switching a device, anything that can fail or be refused), the
server's reply is the only truth. The client may show that a request is in
flight, but **never present an unconfirmed value as settled**. There are two
ways to do it, depending on the control.

### Pattern A: request without touching the value

For buttons, badges and text, whose look is computed from the value, the
client never writes the value at all. Use two signals: the value
(server-owned) and a local loading flag (an indicator, so it must start with
`_`). Starting from off:

```
signals        {"value":false,"_loading":false}

user clicks →  renderer sets _loading = true (indicator), value untouched
renderer →     action {"name":"toggled","payload":true,"signals":{"value":false}}
server →       signal {"_loading":false,"value":true}
```

Until the reply the switch still reads "off", shown as busy and disabled.
If the server refuses, it replies `{"_loading":false,"value":false}` (plus an
error to show): nothing has to be rolled back, and nothing ever showed a
state that was not true.

Rules:

- No handler `patch` writes the value.
- Request with `Handler::action(...).indicator("_loading")`; disable the
  control and show progress with expressions over `s._loading`.
- Let the payload name the **wanted** state (`true`/`false`), not "flip",
  so a duplicated request is harmless. A payload is a literal, so use one
  control per possible request and pick the visible one with an expression
  over the value. (Re-sending the element with a new payload after each
  change also works.)
- Reply with the confirmed value **and** the loading reset in one patch, on
  every path, including refusals.

A complete switch:

```rust
use hypereffect::ui::*;
use hypereffect::{cel, json, Action, App, Handler, Outbox};
use std::collections::BTreeMap;

#[derive(Default)]
pub struct Lamp {
    on: bool,
}

impl App for Lamp {
    fn hello(&mut self, out: &mut Outbox, _: &BTreeMap<String, String>) {
        // `value` is the server's truth. `_loading` is local: an indicator
        // path must start with `_`, and the server has no use for it.
        out.signal(json!({ "value": self.on, "_loading": false }));
        out.element(row("lamp").justify("between"));
        out.element(text("lamp-name").parent("lamp").text("Porch light"));
        out.element(
            badge("lamp-state")
                .parent("lamp")
                .text(cel("s._loading ? '…' : s.value ? 'On' : 'Off'"))
                .variant(cel("s.value ? 'success' : 'neutral'")),
        );
        // A payload is a literal, so the two possible requests are two
        // buttons, and the value decides which one is shown.
        for (id, label, target) in [("turn-on", "Turn on", true), ("turn-off", "Turn off", false)] {
            out.element(
                button(id)
                    .parent("lamp")
                    .label(label)
                    .visible(cel(if target { "!s.value" } else { "s.value" }))
                    .disabled(cel("s._loading"))
                    .on_press(Handler::action("toggled").payload(target).indicator("_loading")),
            );
        }
    }

    fn action(&mut self, out: &mut Outbox, action: &Action) {
        if action.name != "toggled" {
            return;
        }
        // The payload names the wanted state, so a repeated request is
        // harmless. If the change fails, reply with the old value, an error
        // to show, and still the indicator reset.
        if let Some(want) = action.payload_as::<bool>() {
            self.on = want;
        }
        out.signal(json!({ "_loading": false, "value": self.on }));
    }
}
```

### Pattern B: bound controls (optimistic, pending, corrected)

`checkbox`, `select`, `slider` and `tabs` show their value only through
`bind`, and a user change writes the bound path before any handler runs. So
the click necessarily shows the requested value early. That is fine as long
as it is visibly a request: marked pending and disabled until the server
answers with the real value, which the control then shows (a control always
shows a value the server writes to its bound path, even while focused).

Keep the value and a local flag together, `{"auto":{"value":false,"_loading":false}}`.
Starting from off:

```
user clicks →  bind writes auto.value = true; onChange's patch sets auto._loading = true
renderer →     action {"name":"auto","signals":{"auto":{"value":true},"error":""}}
server →       signal {"auto":{"value":true,"_loading":false},"error":""}                                (success)
server →       signal {"auto":{"value":false,"_loading":false},"error":"Could not change auto-update."}  (failure: the box flips back)
```

```rust
#[derive(Default)]
struct Settings {
    auto: bool,
}

/// The real side effect: a database write, a device call. It may fail.
fn apply_auto_update(on: bool) -> Result<(), String> {
    let _ = on;
    Ok(())
}

impl App for Settings {
    fn hello(&mut self, out: &mut Outbox, _: &BTreeMap<String, String>) {
        out.signal(json!({ "auto": { "value": self.auto, "_loading": false }, "error": "" }));
        out.element(
            checkbox("auto")
                .label(cel("s.auto._loading ? 'Auto-update (saving…)' : 'Auto-update'"))
                .bind("auto.value")
                .disabled(cel("s.auto._loading"))
                // `auto._loading` is local (a `_` key at any depth) but cannot be
                // an indicator, whose first segment must start with `_`: set it
                // with a patch, which runs before the action is sent.
                .on_change(Handler::action("auto").with_patch(json!({ "auto": { "_loading": true } }))),
        );
        out.element(notice("error").variant("failure").text(cel("s.error")).visible(cel("s.error != ''")));
    }

    fn action(&mut self, out: &mut Outbox, action: &Action) {
        if action.name != "auto" {
            return;
        }
        // The bound value in this action is the request, not the truth.
        let err = match action.signal("auto.value").and_then(Value::as_bool) {
            Some(want) if apply_auto_update(want).is_ok() => {
                self.auto = want;
                ""
            }
            _ => "Could not change auto-update.",
        };
        // Always answer with the server's own value: this confirms or rolls back.
        out.signal(json!({ "auto": { "value": self.auto, "_loading": false }, "error": err }));
    }
}
```

Rules:

- Disable the control while pending (handlers of disabled controls do not
  run, so no second request can start) and say so in its label or nearby.
- Read the bound value in the action as the requested state; validate it.
- Reply on every path with the server's own value and the pending flag
  reset. Without that echo a refused change would stay on screen.
- Keep the server's state as the source of truth. While a request is
  pending, *other* actions also carry the unconfirmed value in their signals:
  never take it from there.
- If the connection drops before the reply, the box stays flipped and
  pending; the renderer disables everything, and the next `hello` rebuilds
  from the server's state.

Prefer pattern A when even a brief wrong value is unacceptable (a payment,
a door lock), and pattern B when the native control matters more.

**The mistake** both patterns avoid is a bound control or a patch that writes
the value with no pending state and no authoritative reply:

```rust
// ANTI-PATTERN: looks done the moment it is clicked, stays enabled, and
// nothing corrects it if the server refuses.
checkbox("lamp").bind("value").on_change(Handler::action("toggled"));
button("lamp").on_press(Handler::action("toggled").with_patch(json!({ "value": cel("!s.value") })));
```

## Components

A summary. The full reference, with every attribute's notes and examples, is
[components.md](https://hypereffect.dev/components.md).

Containers (accept children): `column`, `row`, `card`, `list`, `tab`,
`dialog`. `tabs` accepts only `tab`. `tab` goes only in `tabs`. `dialog` goes
only at the root (shown as an overlay). Everything else is a leaf.

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

Variants are semantic, never colours or sizes: the renderer chooses the look.
Unknown enum values fall back to the default.

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
- Formatting ids into expressions (`format!("s._editing == '{id}'")`) is
  fine when ids are server-generated and safe. Never interpolate
  user-supplied text into CEL; put it in a signal and read the signal.
- Lookup tables for variants: `{'ok':'success','bad':'failure'}[s.status]`.

## Ids

Ids are global across the tree, not per parent. Derive them from domain ids
and suffix children: row `t3`, its title `t3-text`, its button
`t3-del`. Stable ids are what make fine-grained updates possible.

## Example 1: a task list with fine-grained element updates

Every operation sends the minimum:

| User does | Server sends |
| --- | --- |
| adds a task | 1 `signal` (new task's state) + the new row and its 5 children, inserted with `before` at the top |
| marks a task done | the renderer only sets `_pending.<id>` (button disabled); the server flips its own state and replies with 1 `signal`: the confirmed `done.<id>` and the indicator reset |
| edits a title | the edit mode toggle is a local patch (no traffic); on submit, 1 re-sent `text` element (new literal) + 1 `signal` |
| moves a task up | 1 re-sent `row` with `before` (children move along) + 1 `signal` |
| deletes a task | 1 `remove` (takes the subtree) + 1 `signal` with `null`s |
| any of the above | the "N of M left" caption and the ↑ buttons' `disabled` update by themselves: they are expressions |

```rust
use hypereffect::ui::*;
use hypereffect::{cel, json, Action, App, Element, Handler, Outbox, Value};
use std::collections::BTreeMap;

struct Task {
    id: String,
    title: String,
    done: bool,
}

#[derive(Default)]
pub struct Todos {
    tasks: Vec<Task>, // in display order
    next: u32,
}

// One builder per element. Every re-send goes through the same builder,
// because a re-sent element replaces *all* of its attributes.
fn task_row(t: &Task) -> Element {
    row(t.id.as_str()).parent("items").justify("between")
}

fn task_text(t: &Task) -> Element {
    let id = &t.id;
    text(format!("{id}-text"))
        .parent(id.as_str())
        .text(t.title.as_str())
        .visible(cel(format!("s._editing != '{id}'")))
}

fn task_children(t: &Task) -> [Element; 6] {
    let id = &t.id;
    [
        // `done.<id>` is governed by the server: no control binds it, and the
        // button only asks. `_pending.<id>` is the request in flight.
        button(format!("{id}-done"))
            .parent(id.as_str())
            .label(cel(format!("s.done.{id} ? '✓' : '○'")))
            .variant(cel(format!("s.done.{id} ? 'success' : 'plain'")))
            .disabled(cel(format!("s._pending.{id}")))
            .on_press(Handler::action("toggle").payload(id.as_str()).indicator(format!("_pending.{id}"))),
        task_text(t),
        input(format!("{id}-title"))
            .parent(id.as_str())
            .bind(format!("draft.{id}"))
            .visible(cel(format!("s._editing == '{id}'")))
            .on_submit(
                Handler::action("rename")
                    .payload(id.as_str())
                    .with_patch(json!({ "_editing": "" })),
            ),
        button(format!("{id}-edit"))
            .parent(id.as_str())
            .label("Edit")
            .variant("plain")
            .visible(cel(format!("s._editing != '{id}'")))
            .on_press(Handler::patch(json!({ "_editing": id }))),
        button(format!("{id}-up"))
            .parent(id.as_str())
            .label("↑")
            .variant("plain")
            .disabled(cel(format!("s.first == '{id}'")))
            .on_press(Handler::action("up").payload(id.as_str())),
        button(format!("{id}-del"))
            .parent(id.as_str())
            .label("Delete")
            .variant("failure")
            .on_press(Handler::action("delete").payload(id.as_str())),
    ]
}

impl Todos {
    fn pos(&self, action: &Action) -> Option<usize> {
        let id: String = action.payload_as()?; // payloads are user input
        self.tasks.iter().position(|t| t.id == id)
    }

    fn first(&self) -> &str {
        self.tasks.first().map_or("", |t| &t.id)
    }
}

impl App for Todos {
    fn hello(&mut self, out: &mut Outbox, _: &BTreeMap<String, String>) {
        // Signals first: the expressions below read them.
        out.signal(json!({
            "new": "", "done": {}, "draft": {}, "first": "", "_pending": {},
            "_editing": "", "_adding": false,
        }));
        out.element(card("todos").title("Tasks"));
        out.element(row("add-row").parent("todos"));
        out.element(
            input("new")
                .parent("add-row")
                .placeholder("New task")
                .bind("new")
                .on_submit(Handler::action("add").indicator("_adding")),
        );
        out.element(
            button("add")
                .parent("add-row")
                .variant("primary")
                .label(cel("s._adding ? 'Adding…' : 'Add'"))
                .disabled(cel("s._adding || s.new.trim() == ''"))
                .on_press(Handler::action("add").indicator("_adding")),
        );
        out.element(list("items").parent("todos").empty("Nothing to do."));
        out.element(text("left").parent("todos").variant("caption").text(cel(
            "string(s.done.filter(k, !s.done[k]).size()) + ' of ' + string(size(s.done)) + ' left'",
        )));
    }

    fn action(&mut self, out: &mut Outbox, action: &Action) {
        match action.name.as_str() {
            "add" => {
                let title = action.signal("new").and_then(Value::as_str).unwrap_or("").trim();
                if title.is_empty() {
                    out.signal(json!({ "_adding": false })); // always reset the indicator
                    return;
                }
                self.next += 1;
                let t = Task { id: format!("t{}", self.next), title: title.to_string(), done: false };
                let id = t.id.clone();
                let before = self.tasks.first().map(|f| f.id.clone());
                // The new task's signals go out before the elements that read them.
                out.signal(json!({
                    "new": "", "_adding": false, "first": id,
                    "done": { &id: false }, "draft": { &id: t.title }, "_pending": { &id: false },
                }));
                // Insert at the top: only the new row and its children are sent.
                out.element(match before {
                    Some(b) => task_row(&t).before(b),
                    None => task_row(&t),
                });
                for e in task_children(&t) {
                    out.element(e);
                }
                self.tasks.insert(0, t);
            }
            "toggle" => {
                let Some(i) = self.pos(action) else { return };
                let t = &mut self.tasks[i];
                // The server's own state decides; the pending indicator kept
                // the button disabled, so a double click cannot flip twice.
                t.done = !t.done;
                // One patch: the confirmed value and the indicator reset.
                out.signal(json!({ "done": { &t.id: t.done }, "_pending": { &t.id: false } }));
            }
            "rename" => {
                let Some(i) = self.pos(action) else { return };
                let t = &mut self.tasks[i];
                let draft = action.signal(&format!("draft.{}", t.id)).and_then(Value::as_str);
                match draft.map(str::trim) {
                    Some(title) if !title.is_empty() => {
                        t.title = title.to_string();
                        // Re-send only the one element whose literal changed.
                        // Same id and type: the renderer keeps focus and state.
                        out.element(task_text(t));
                    }
                    _ => {}
                }
                // Normalize the draft (trimmed, or reverted if rejected).
                out.signal(json!({ "draft": { &t.id: t.title } }));
            }
            "up" => {
                let Some(i) = self.pos(action).filter(|&i| i > 0) else { return };
                self.tasks.swap(i, i - 1);
                // Moving = re-sending the row with `before`. Its children stay.
                let (moved, next) = (&self.tasks[i - 1], &self.tasks[i]);
                out.element(task_row(moved).before(next.id.as_str()));
                out.signal(json!({ "first": self.first() }));
            }
            "delete" => {
                let Some(i) = self.pos(action) else { return };
                let t = self.tasks.remove(i);
                out.remove(t.id.as_str()); // removes the children too
                // null deletes the key from the map.
                out.signal(json!({
                    "done": { &t.id: null }, "draft": { &t.id: null }, "_pending": { &t.id: null },
                    "first": self.first(),
                }));
            }
            _ => {}
        }
    }
}
```

Wire traffic for a short session (`>` is the renderer):

```
> action {"name":"add","signals":{"new":"Call the bank","done":{},"draft":{},"first":""}}
signal {"new":"","_adding":false,"first":"t1","done":{"t1":false},"draft":{"t1":"Call the bank"},"_pending":{"t1":false}}
element {"id":"t1","type":"row","parent":"items","justify":"between"}
element {"id":"t1-done","type":"button","parent":"t1","label":{"cel":"s.done.t1 ? '✓' : '○'"},"variant":{"cel":"s.done.t1 ? 'success' : 'plain'"},"disabled":{"cel":"s._pending.t1"},"onPress":{"action":"toggle","payload":"t1","indicator":"_pending.t1"}}
element {"id":"t1-text","type":"text","parent":"t1","text":"Call the bank","visible":{"cel":"s._editing != 't1'"}}
element {"id":"t1-title","type":"input","parent":"t1","bind":"draft.t1","visible":{"cel":"s._editing == 't1'"},"onSubmit":{"patch":{"_editing":""},"action":"rename","payload":"t1"}}
element {"id":"t1-edit","type":"button","parent":"t1","label":"Edit","variant":"plain","visible":{"cel":"s._editing != 't1'"},"onPress":{"patch":{"_editing":"t1"}}}
element {"id":"t1-up","type":"button","parent":"t1","label":"↑","variant":"plain","disabled":{"cel":"s.first == 't1'"},"onPress":{"action":"up","payload":"t1"}}
element {"id":"t1-del","type":"button","parent":"t1","label":"Delete","variant":"failure","onPress":{"action":"delete","payload":"t1"}}

> action {"name":"add","signals":{"new":"Buy milk","done":{"t1":false},"draft":{"t1":"Call the bank"},"first":"t1"}}
signal {"new":"","_adding":false,"first":"t2","done":{"t2":false},"draft":{"t2":"Buy milk"},"_pending":{"t2":false}}
element {"id":"t2","type":"row","parent":"items","before":"t1","justify":"between"}
element {"id":"t2-done","type":"button","parent":"t2","label":{"cel":"s.done.t2 ? '✓' : '○'"},"variant":{"cel":"s.done.t2 ? 'success' : 'plain'"},"disabled":{"cel":"s._pending.t2"},"onPress":{"action":"toggle","payload":"t2","indicator":"_pending.t2"}}
element {"id":"t2-text","type":"text","parent":"t2","text":"Buy milk","visible":{"cel":"s._editing != 't2'"}}
element {"id":"t2-title","type":"input","parent":"t2","bind":"draft.t2","visible":{"cel":"s._editing == 't2'"},"onSubmit":{"patch":{"_editing":""},"action":"rename","payload":"t2"}}
element {"id":"t2-edit","type":"button","parent":"t2","label":"Edit","variant":"plain","visible":{"cel":"s._editing != 't2'"},"onPress":{"patch":{"_editing":"t2"}}}
element {"id":"t2-up","type":"button","parent":"t2","label":"↑","variant":"plain","disabled":{"cel":"s.first == 't2'"},"onPress":{"action":"up","payload":"t2"}}
element {"id":"t2-del","type":"button","parent":"t2","label":"Delete","variant":"failure","onPress":{"action":"delete","payload":"t2"}}

> action {"name":"toggle","payload":"t1","signals":{"new":"","done":{"t1":false,"t2":false},"draft":{"t1":"Call the bank","t2":"Buy milk"},"first":"t2"}}
signal {"done":{"t1":true},"_pending":{"t1":false}}

> action {"name":"rename","payload":"t2","signals":{"new":"","done":{"t1":true,"t2":false},"draft":{"t1":"Call the bank","t2":"Buy oat milk"},"first":"t2"}}
element {"id":"t2-text","type":"text","parent":"t2","text":"Buy oat milk","visible":{"cel":"s._editing != 't2'"}}
signal {"draft":{"t2":"Buy oat milk"}}

> action {"name":"up","payload":"t1","signals":{"new":"","done":{"t1":true,"t2":false},"draft":{"t1":"Call the bank","t2":"Buy oat milk"},"first":"t2"}}
element {"id":"t1","type":"row","parent":"items","before":"t2","justify":"between"}
signal {"first":"t1"}

> action {"name":"delete","payload":"t2","signals":{"new":"","done":{"t1":true,"t2":false},"draft":{"t1":"Call the bank","t2":"Buy oat milk"},"first":"t1"}}
remove {"id":"t2"}
signal {"done":{"t2":null},"draft":{"t2":null},"_pending":{"t2":null},"first":"t1"}
```

Note that `_editing`, `_adding` and `_pending` never appear in the actions:
they are local. And `done.t1` is still `false` in the `toggle` action: the
renderer did not touch it, the server's reply set it.

## Example 2: a live pipeline driven only by signals

The whole interface is sent once in `hello`. After that the server sends
**only `signal` messages**, from actions and from a 100 ms timer. Every
attribute that changes (progress, badge text and colour, button label and
state, the result notice) is an expression over the signals.

```rust
use hypereffect::ui::*;
use hypereffect::{cel, json, Action, App, Handler, Map, Outbox, Value};
use std::collections::BTreeMap;

/// id, label, ticks to complete.
const STAGES: [(&str, &str, u32); 3] = [("build", "Build", 10), ("test", "Test", 8), ("deploy", "Deploy", 5)];

#[derive(Default)]
pub struct Pipeline {
    running: bool,
    stage: usize,
    step: u32,
    run: u32,
    fail_tests: bool,
}

/// `{"build": {"state": "pending", "pct": 0.0}, ...}`
fn reset_stages() -> Value {
    let mut m = Map::new();
    for (id, _, _) in STAGES {
        m.insert(id.into(), json!({ "state": "pending", "pct": 0.0 }));
    }
    Value::Object(m)
}

impl App for Pipeline {
    fn hello(&mut self, out: &mut Outbox, _: &BTreeMap<String, String>) {
        out.signal(json!({
            "status": "idle", "run": 0, "fail": false,
            "stages": reset_stages(), "_starting": false,
        }));

        // The whole interface is sent once. After this, only signals.
        out.element(card("pipeline").title("Deploy pipeline"));
        for (id, label, _) in STAGES {
            let st = format!("s.stages.{id}");
            out.element(row(id).parent("pipeline").justify("between"));
            out.element(text(format!("{id}-name")).parent(id).text(label));
            out.element(
                progress(format!("{id}-bar"))
                    .parent(id)
                    .value(cel(format!("{st}.pct")))
                    .visible(cel(format!("{st}.state == 'running'"))),
            );
            out.element(
                badge(format!("{id}-state"))
                    .parent(id)
                    .text(cel(format!("{st}.state")))
                    .variant(cel(format!(
                        "{{'pending':'neutral','running':'info','passed':'success',\
                         'failed':'failure','skipped':'neutral'}}[{st}.state]"
                    ))),
            );
        }
        out.element(
            checkbox("fail")
                .parent("pipeline")
                .label("Make the tests fail")
                .bind("fail")
                .disabled(cel("s.status == 'running'")),
        );
        out.element(row("controls").parent("pipeline").justify("end"));
        out.element(
            button("cancel")
                .parent("controls")
                .label("Cancel")
                .visible(cel("s.status == 'running'"))
                .on_press(Handler::action("cancel")),
        );
        out.element(
            button("start")
                .parent("controls")
                .variant("primary")
                .label(cel("s._starting ? 'Starting…' : s.run == 0 ? 'Run' : 'Run again'"))
                .disabled(cel("s._starting || s.status == 'running'"))
                .on_press(Handler::action("start").indicator("_starting")),
        );
        out.element(
            notice("result")
                .parent("pipeline")
                .visible(cel("s.status in ['passed', 'failed', 'cancelled']"))
                .variant(cel(
                    "{'passed':'success','failed':'failure','cancelled':'warning'}[s.status]",
                ))
                .text(cel("'Run #' + string(s.run) + ' ' + s.status + '.'")),
        );
    }

    fn action(&mut self, out: &mut Outbox, action: &Action) {
        match action.name.as_str() {
            "start" if !self.running => {
                self.fail_tests = action.signal("fail").and_then(Value::as_bool).unwrap_or(false);
                self.running = true;
                self.stage = 0;
                self.step = 0;
                self.run += 1;
                // One patch resets every stage and starts the first one.
                // The `stages` map is merged recursively.
                out.signal(json!({
                    "status": "running", "run": self.run, "_starting": false,
                    "stages": reset_stages(),
                }));
                out.signal(json!({ "stages": { STAGES[0].0: { "state": "running" } } }));
            }
            "start" => out.signal(json!({ "_starting": false })),
            "cancel" if self.running => {
                self.running = false;
                let (id, _, _) = STAGES[self.stage];
                out.signal(json!({ "status": "cancelled", "stages": { id: { "state": "skipped" } } }));
            }
            _ => {}
        }
    }

    fn tick_interval(&self) -> Option<u32> {
        Some(100)
    }

    fn tick(&mut self, out: &mut Outbox, _now_ms: f64) {
        if !self.running {
            return;
        }
        let (id, _, len) = STAGES[self.stage];
        self.step += 1;
        if self.step < len {
            // The common case: one number, deep inside the signals.
            let pct = self.step as f64 / len as f64;
            out.signal(json!({ "stages": { id: { "pct": pct } } }));
            return;
        }
        if id == "test" && self.fail_tests {
            self.running = false;
            let mut stages = Map::new();
            stages.insert(id.into(), json!({ "state": "failed" }));
            for (rest, _, _) in &STAGES[self.stage + 1..] {
                stages.insert(rest.to_string(), json!({ "state": "skipped" }));
            }
            out.signal(json!({ "status": "failed", "stages": stages }));
            return;
        }
        self.stage += 1;
        self.step = 0;
        match STAGES.get(self.stage) {
            Some((next, _, _)) => out.signal(json!({
                "stages": { id: { "state": "passed", "pct": 1.0 }, *next: { "state": "running" } },
            })),
            None => {
                self.running = false;
                out.signal(json!({
                    "status": "passed", "stages": { id: { "state": "passed", "pct": 1.0 } },
                }));
            }
        }
    }
}
```

Wire traffic (the user checked "Make the tests fail", then pressed Run):

```
> action {"name":"start","signals":{"status":"idle","run":0,"fail":true,"stages":{"build":{"state":"pending","pct":0.0},"test":{"state":"pending","pct":0.0},"deploy":{"state":"pending","pct":0.0}}}}
signal {"status":"running","run":1,"_starting":false,"stages":{"build":{"state":"pending","pct":0.0},"test":{"state":"pending","pct":0.0},"deploy":{"state":"pending","pct":0.0}}}
signal {"stages":{"build":{"state":"running"}}}

tick: signal {"stages":{"build":{"pct":0.1}}}
…
tick: signal {"stages":{"build":{"pct":0.9}}}
tick: signal {"stages":{"build":{"state":"passed","pct":1.0},"test":{"state":"running"}}}
tick: signal {"stages":{"test":{"pct":0.125}}}
…
tick: signal {"status":"failed","stages":{"test":{"state":"failed"},"deploy":{"state":"skipped"}}}
```

Each tick patches a single number deep inside `stages`; the renderer
re-evaluates only the expressions that read it.

When a set of *things* changes over time (rows in a log, new items), signals
alone cannot create elements: there is no templating. Add and remove those
elements individually, as in example 1, and keep their changing values in
signals keyed by id.

## Testing

With the `client` feature, `hypereffect::server::Loopback` runs a real
renderer core (element tree, signals, CEL) against your `App` in-process:

```rust
use hypereffect::{json, server::Loopback};

#[test]
fn add_and_toggle() {
    let mut l = Loopback::<Todos>::new();          // sends hello
    l.write("new", json!("Buy milk"));             // like typing into a bound input
    assert!(l.trigger("add", "onPress"));          // true if an action was sent
    assert_eq!(l.client.children(Some("items")), ["t1"]);
    assert!(l.trigger("t1-done", "onPress"));
    assert_eq!(l.client.attr("left", "text"), Some(json!("0 of 1 left")));
    assert!(l.client.take_diagnostics().is_empty()); // no rejected elements or failed CEL
}
```

`l.tick(now_ms)` runs the timer; `l.client.value(path)` reads a signal;
`l.client.attr(id, name)` reads a resolved attribute. Always assert
`take_diagnostics()` is empty: it catches wrong nesting, missing parents,
CEL type errors and failing expressions.

## Checklist

- Signals before the elements that read them; parents before children.
- Every signal an expression reads is initialised (no `null`: it deletes).
- Per-item state in id-keyed objects, never arrays.
- One builder per element; re-sends reuse it (replacement is not a merge).
- Update with `signal` when possible, re-send one element when a literal or
  position changes, `remove` only to delete. Never rebuild.
- UI-only state in `_` local signals, changed by handler patches.
- Server-confirmed state is never presented as settled before the reply:
  either the client does not write it (pattern A: action plus indicator), or
  a bound control shows it as pending and disabled until the server answers
  with the real value (pattern B). Every reply resets the pending flag and
  carries the server's value.
- Every action reply resets the indicators it may have set, on all paths.
- Validate every action name, payload and signal read.
- No user text inside CEL strings; no `cel(...)` in structural attributes.
- `dialog` only at the root, `tab` only in `tabs`.
