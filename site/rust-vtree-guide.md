# Complex interfaces in Rust with `vtree`

> A guide for language models (and terse humans) writing large or
> structurally dynamic [hypereffect](https://hypereffect.dev) servers in
> Rust, using the `vtree` module of the `hypereffect` crate (version 0.6).
> It assumes the protocol rules in
> [server-guide.md](https://hypereffect.dev/server-guide.md); the component
> reference is [components.md](https://hypereffect.dev/components.md).

## Why a view tree

Hand-written updates (send the tree in `hello`, then exactly the messages
each action needs) stay correct only while you can enumerate, per action,
every element that appears, disappears, moves or changes. That breaks down
when the structure depends on the data: items moving between groups,
sections shown only in some states, a list re-sorted on demand, a dialog
whose contents depend on what is selected.

`vtree` replaces that bookkeeping with one function:

1. `view(&self) -> Node` (or `Vec<Node>`) builds the **whole** interface
   from the state, as a plain value.
2. After each change, `vtree::diff(&shown, &next)` gives the fewest
   messages that turn the tree the renderer has into the new one: elements
   are matched **by id**, and only those whose type, attributes, parent or
   position changed are sent, parents first, removals last.
3. You keep `next` as the new `shown`.

Kept elements are updated or moved in place, never removed and re-created,
so focus, cursors and scroll positions survive. It is the same output a
careful hand-written server would send, without the per-action reasoning.

## The API

```rust
use hypereffect::vtree::{self, button, card, column, list, row, text, Node};
```

- `Node` is an element plus its children. The constructors (`column`,
  `row`, `card`, `list`, `tabs`, `tab`, `dialog`, `text`, `notice`, `badge`,
  `image`, `divider`, `progress`, `button`, `input`, `checkbox`, `select`,
  `slider`) take an id and return a `Node` with the same setters as
  `ui::*` elements (`label`, `variant`, `bind`, `on_press`, `set`, …).
  There is no `parent` or `before`: they come from the node's place in the
  tree.
- `.child(node)` appends one child; `.children(iter)` appends any iterator
  of nodes, including an `Option<Node>` (zero or one) and a `[Node; N]`.
  `Element` converts into `Node`.
- `vtree::render(&root)` creates a tree at the renderer's root;
  `vtree::diff(&old, &new)` updates it.
- `vtree::render_into(parent, &roots)` and `vtree::diff_into(parent, &old,
  &new)` do the same for a **list** of nodes, as children of `parent`
  (`None` for the renderer's root). Use them for several roots (a page and
  a dialog) or to manage only part of a hand-built interface.
- All four return `Vec<ServerMessage>`; `Outbox` implements `Extend`, so
  `out.extend(vtree::diff(...))`.

## The shape of an app

```rust
#[derive(Default)]
pub struct MyApp {
    // ... domain state ...
    shown: Vec<Node>, // the roots as last sent
}

impl MyApp {
    fn view(&self) -> Vec<Node> { /* pure: reads self, returns the tree */ }

    /// Sends what changed since the last call, and remembers what was sent.
    fn update(&mut self, out: &mut Outbox) {
        let next = self.view();
        out.extend(vtree::diff_into(None, &self.shown, &next));
        self.shown = next;
    }
}

impl App for MyApp {
    fn hello(&mut self, out: &mut Outbox, _: &BTreeMap<String, String>) {
        out.signal(json!({ /* every signal the view's expressions read */ }));
        self.update(out); // `shown` is empty: this sends the whole view
    }

    fn action(&mut self, out: &mut Outbox, action: &Action) {
        match action.name.as_str() {
            // validate, change the state, write signals (indicator resets) ...
            _ => return,
        }
        self.update(out);
    }
}
```

`hello` and every action go through the same `update`, so the first render
and every later change follow one code path. A new `App` is created on each
`hello`, so `shown` starts empty exactly when the renderer's tree does.

## Rules

**Ids identify data, never positions.** `issue-42`, not `row-3`. An element
whose id is kept is updated or moved in place, even to another parent; an
id derived from a position makes every insertion re-send everything after
it, and hands one item's widget state to another. Suffix children of an
item with its id (`i42-open`, `i42-who`).

**Each id appears once per tree.** `diff` panics on duplicates. Two views of
the same item (a row and a dialog about it) need different ids.

**`shown` must be exactly what the renderer has.** The diff is computed
against it, not against the renderer. Never send `element` or `remove` by
hand for ids the tree manages, and always store the tree you diffed to.
Ids outside the managed part are yours (see
[Mixing](#mixing-hand-written-parts-signals-and-vtree)).

**Signals before the update.** The diff only produces `element` and
`remove`. Write the signals the new elements read (`out.signal(...)`)
before calling `update`, in the same batch.

**Dialogs need a list of roots.** A `dialog` must be a child of the
renderer's root, so a single `render(&root)` cannot hold one. Return
`Vec<Node>` (the page, then the dialog only while it is open) and use
`diff_into(None, ...)`.

**Changing a node's type drops its children** on the renderer, so the diff
re-sends them. Keep a node's type stable; vary its attributes instead.

**Keep handler payloads to ids and wanted states.** Handlers are
attributes: a payload that includes a position or a counter changes, and
re-sends its element, whenever that does.

**Absent or invisible?** Leave a node out of the tree when the *server*
decides it exists (an "urgent" badge, the ← button of the first lane, the
open dialog): no signal and no expression needed. Use `visible` with a
local signal when the *client* toggles it instantly (an edit mode, a
collapsed section): no round trip.

**Hot values stay in signals.** `vtree` replaces element bookkeeping, not
signals. A value that changes on a timer or on every keystroke (progress,
readings, counters) belongs in a signal read by an expression; the tree
then does not change and the hot path skips `view` and `diff` entirely.

**Server-confirmed state works as before.** Patterns A and B of the server
guide are signals and handlers, which the tree carries unchanged: an
indicator on the handler, the confirmed value and the reset in one patch,
then `update` if the structure changed too.

**Cost.** `view` rebuilds the whole tree and `diff` walks it, linear in its
size, on every update. That is cheap for hundreds or a few thousand
elements. For more, split the interface into independently managed parts
with `diff_into`, each with its own `shown`, and only re-view the part an
action touched.

## Example: an issue board

Three lanes of issues. Issues are added, moved between lanes, sorted
urgent-first on demand, and edited in a dialog. Every action changes the
state and calls `update`; no action names the elements it touches.

| User does | Diff sends |
| --- | --- |
| adds an issue | the lane's card (new count in its title), the new row and its buttons |
| moves an issue right | both lanes' cards, the row with a new `parent` (its children move along), its → button (new payload) and a new ← button |
| sorts urgent-first | the rows that change position, with `before` |
| opens an issue | the dialog and its contents, after a `signal` with the drafts |
| saves | the changed row's parts, then `remove` of the dialog |

```rust
use hypereffect::vtree::{self, badge, button, card, checkbox, column, dialog, input, list, row, select, text, Node};
use hypereffect::{cel, json, Action, App, Handler, Outbox, Value};
use std::collections::BTreeMap;

/// Lanes, in order: key, label.
const LANES: [(&str, &str); 3] = [("todo", "To do"), ("doing", "In progress"), ("done", "Done")];
const PEOPLE: [&str; 3] = ["ana", "ben", "chen"];

struct Issue {
    id: u32,
    title: String,
    lane: usize,
    assignee: String, // "" or one of PEOPLE
    urgent: bool,
}

#[derive(Default)]
pub struct Board {
    issues: Vec<Issue>, // in manual order; a lane shows its own, in this order
    next: u32,
    urgent_first: bool,
    open: Option<u32>, // the issue shown in the dialog
    shown: Vec<Node>,  // the root nodes as last sent
}

fn people() -> Value {
    let mut opts = vec![json!({ "value": "", "label": "Nobody" })];
    opts.extend(PEOPLE.iter().map(|p| json!({ "value": p, "label": p })));
    Value::Array(opts)
}

impl Board {
    // ---- the view: a pure function of the state ----

    fn view(&self) -> Vec<Node> {
        let mut roots = vec![self.board()];
        // A dialog must be a child of the root, so the view is a list of
        // roots and the dialog is in it only while an issue is open.
        if let Some(i) = self.open.and_then(|id| self.issue(id)) {
            roots.push(detail(i));
        }
        roots
    }

    fn board(&self) -> Node {
        column("main").children([
            row("header").justify("between").children([
                text("title").variant("h1").text("Board"),
                select("sort")
                    .variant("segmented")
                    .bind("sort")
                    .options(json!([
                        { "value": "manual", "label": "Manual" },
                        { "value": "urgent", "label": "Urgent first" },
                    ]))
                    .on_change(Handler::action("sort")),
            ]),
            row("add-row").children([
                input("new")
                    .placeholder("New issue")
                    .bind("new")
                    .on_submit(Handler::action("add").indicator("_adding")),
                button("add")
                    .variant("primary")
                    .label("Add")
                    .disabled(cel("s._adding || s.new.trim() == ''"))
                    .on_press(Handler::action("add").indicator("_adding")),
            ]),
            row("lanes")
                .align("start")
                .children(LANES.iter().enumerate().map(|(n, &(key, label))| self.lane(n, key, label))),
        ])
    }

    fn lane(&self, n: usize, key: &str, label: &str) -> Node {
        let mut issues: Vec<&Issue> = self.issues.iter().filter(|i| i.lane == n).collect();
        if self.urgent_first {
            issues.sort_by_key(|i| !i.urgent); // stable: manual order within each group
        }
        card(format!("lane-{key}"))
            .title(format!("{label} · {}", issues.len()))
            .child(
                list(format!("lane-{key}-items"))
                    .empty("Nothing here.")
                    .children(issues.into_iter().map(issue_row)),
            )
    }

    fn issue(&self, id: u32) -> Option<&Issue> {
        self.issues.iter().find(|i| i.id == id)
    }

    fn pos(&self, id: Option<u32>) -> Option<usize> {
        self.issues.iter().position(|i| Some(i.id) == id)
    }

    /// Sends what changed since the last call, and remembers what was sent.
    fn update(&mut self, out: &mut Outbox) {
        let next = self.view();
        out.extend(vtree::diff_into(None, &self.shown, &next));
        self.shown = next;
    }
}

fn issue_row(i: &Issue) -> Node {
    // The id comes from the issue, not from its lane or position: moving it
    // to another lane moves this row, with its children, in one message.
    let id = format!("i{}", i.id);
    row(id.as_str())
        .justify("between")
        .child(
            button(format!("{id}-open"))
                .variant("plain")
                .label(i.title.as_str())
                .on_press(Handler::action("open").payload(i.id)),
        )
        // Optional parts are just absent from the tree (`Option` is an
        // iterator): no `visible` expression and no signal needed.
        .children(i.urgent.then(|| badge(format!("{id}-urgent")).variant("failure").text("urgent")))
        .children((!i.assignee.is_empty()).then(|| badge(format!("{id}-who")).text(i.assignee.as_str())))
        // Payloads name the wanted lane, so a repeated request is harmless.
        .children((i.lane > 0).then(|| {
            button(format!("{id}-left"))
                .variant("plain")
                .label("←")
                .on_press(Handler::action("move").payload(json!([i.id, LANES[i.lane - 1].0])))
        }))
        .children((i.lane + 1 < LANES.len()).then(|| {
            button(format!("{id}-right"))
                .variant("plain")
                .label("→")
                .on_press(Handler::action("move").payload(json!([i.id, LANES[i.lane + 1].0])))
        }))
}

fn detail(i: &Issue) -> Node {
    // The form edits drafts in `edit`, written by the server when the dialog
    // opens; nothing is applied until Save.
    dialog("detail")
        .title(format!("Issue #{}", i.id))
        .on_dismiss(Handler::action("close"))
        .children([
            input("detail-title")
                .label("Title")
                .bind("edit.title")
                .error(cel("s.edit.title.trim() == '' ? 'A title is required.' : ''")),
            select("detail-who").label("Assignee").bind("edit.assignee").options(people()),
            checkbox("detail-urgent").label("Urgent").bind("edit.urgent"),
        ])
        .child(
            row("detail-actions").justify("between").children([
                button("detail-delete")
                    .variant("failure")
                    .label("Delete")
                    .on_press(Handler::action("delete").payload(i.id)),
                button("detail-save")
                    .variant("primary")
                    .label(cel("s._saving ? 'Saving…' : 'Save'"))
                    .disabled(cel("s._saving || s.edit.title.trim() == ''"))
                    .on_press(Handler::action("save").payload(i.id).indicator("_saving")),
            ]),
        )
}

impl App for Board {
    fn hello(&mut self, out: &mut Outbox, _: &BTreeMap<String, String>) {
        // Signals first: the expressions in the view read them.
        out.signal(json!({
            "new": "", "sort": "manual", "_adding": false, "_saving": false,
            "edit": { "title": "", "assignee": "", "urgent": false },
        }));
        self.update(out); // `shown` is empty, so this sends the whole view
    }

    fn action(&mut self, out: &mut Outbox, action: &Action) {
        // Change the state, write the signals the new view reads, then send
        // the difference. Every arm ends in the same `update`.
        match action.name.as_str() {
            "add" => {
                let title = action.signal("new").and_then(Value::as_str).unwrap_or("").trim();
                if !title.is_empty() {
                    self.next += 1;
                    let issue = Issue {
                        id: self.next,
                        title: title.to_string(),
                        lane: 0,
                        assignee: String::new(),
                        urgent: false,
                    };
                    self.issues.push(issue);
                    out.signal(json!({ "new": "" }));
                }
                out.signal(json!({ "_adding": false })); // on every path
            }
            "open" => {
                let Some(i) = action.payload_as::<u32>().and_then(|id| self.issue(id)) else { return };
                out.signal(json!({
                    "edit": { "title": i.title, "assignee": i.assignee, "urgent": i.urgent },
                    "_saving": false,
                }));
                self.open = Some(i.id);
            }
            "close" => self.open = None,
            "save" => {
                let at = self.pos(action.payload_as());
                let title = action.signal("edit.title").and_then(Value::as_str).map(str::trim);
                let who = action.signal("edit.assignee").and_then(Value::as_str);
                let urgent = action.signal("edit.urgent").and_then(Value::as_bool);
                match (at, title, who, urgent) {
                    (Some(at), Some(title), Some(who), Some(urgent))
                        if !title.is_empty() && (who.is_empty() || PEOPLE.contains(&who)) =>
                    {
                        let i = &mut self.issues[at];
                        i.title = title.to_string();
                        i.assignee = who.to_string();
                        i.urgent = urgent;
                        self.open = None;
                    }
                    _ => {} // invalid: keep the dialog open
                }
                out.signal(json!({ "_saving": false }));
            }
            "delete" => {
                let Some(at) = self.pos(action.payload_as()) else { return };
                let gone = self.issues.remove(at);
                if self.open == Some(gone.id) {
                    self.open = None;
                }
            }
            "move" => {
                let Some((id, to)) = action.payload_as::<(u32, String)>() else { return };
                let Some(lane) = LANES.iter().position(|(key, _)| *key == to) else { return };
                let Some(at) = self.pos(Some(id)) else { return };
                // To the end of its new lane.
                let mut i = self.issues.remove(at);
                i.lane = lane;
                self.issues.push(i);
            }
            "sort" => match action.signal("sort").and_then(Value::as_str) {
                Some("manual") => self.urgent_first = false,
                Some("urgent") => self.urgent_first = true,
                _ => return,
            },
            _ => return,
        }
        self.update(out);
    }
}
```

Wire traffic after two issues were added (`>` is the renderer):

```
> action {"name":"move","payload":[1,"doing"],"signals":{"new":"","sort":"manual","edit":{"title":"","assignee":"","urgent":false}}}
element {"id":"lane-doing","type":"card","parent":"lanes","title":"In progress · 1"}
element {"id":"lane-todo","type":"card","parent":"lanes","title":"To do · 1"}
element {"id":"i1","type":"row","parent":"lane-doing-items","justify":"between"}
element {"id":"i1-right","type":"button","parent":"i1","variant":"plain","label":"→","onPress":{"action":"move","payload":[1,"done"]}}
element {"id":"i1-left","type":"button","parent":"i1","before":"i1-right","variant":"plain","label":"←","onPress":{"action":"move","payload":[1,"todo"]}}

> action {"name":"open","payload":2,"signals":{"new":"","sort":"manual","edit":{"title":"","assignee":"","urgent":false}}}
signal {"edit":{"title":"Write docs","assignee":"","urgent":false},"_saving":false}
element {"id":"detail","type":"dialog","title":"Issue #2","onDismiss":{"action":"close"}}
element {"id":"detail-actions","type":"row","parent":"detail","justify":"between"}
element {"id":"detail-urgent","type":"checkbox","parent":"detail","before":"detail-actions","label":"Urgent","bind":"edit.urgent"}
element {"id":"detail-who","type":"select","parent":"detail","before":"detail-urgent","label":"Assignee","bind":"edit.assignee","options":[...]}
element {"id":"detail-title","type":"input","parent":"detail","before":"detail-who","label":"Title","bind":"edit.title","error":{"cel":"s.edit.title.trim() == '' ? 'A title is required.' : ''"}}
element {"id":"detail-save","type":"button","parent":"detail-actions","variant":"primary","label":{"cel":"s._saving ? 'Saving…' : 'Save'"},"disabled":{"cel":"s._saving || s.edit.title.trim() == ''"},"onPress":{"action":"save","payload":2,"indicator":"_saving"}}
element {"id":"detail-delete","type":"button","parent":"detail-actions","before":"detail-save","variant":"failure","label":"Delete","onPress":{"action":"delete","payload":2}}

> action {"name":"save","payload":2,"signals":{"new":"","sort":"manual","edit":{"title":"Write docs","assignee":"","urgent":false}}}
signal {"_saving":false}
remove {"id":"detail"}
```

Notes:

- Moving `i1` is one `element` with a new `parent`: its `i1-open` button
  moved with it, untouched. The re-sent `i1-right` has a new payload; the
  ← button is new.
- Siblings are placed from last to first, each `before` the next one (an
  element sent without `before` keeps its position or is appended), which
  is why `i1-left` comes after `i1-right`.
- The re-sent cards replace their attributes but keep their children: their
  type did not change.
- Closing the dialog is one `remove`; its contents go with it.
- The bound controls (`sort`, `new`, the dialog's fields) are never re-sent
  by user edits: the user's typing changes signals, not the tree.

## Mixing hand-written parts, signals and `vtree`

`diff_into(Some(parent), ...)` manages only the children of `parent` that
it created; anything else is yours. A common shape is a fixed shell sent by
hand, a `vtree`-managed list inside it, and live values in signals:

```rust
#[derive(Default)]
struct Monitor {
    sensors: Vec<Sensor>,
    shown: Vec<Node>, // the children of "sensors" as last sent
}

impl Monitor {
    fn rows(&self) -> Vec<Node> {
        self.sensors
            .iter()
            .map(|s| {
                vtree::row(format!("s-{}", s.id)).justify("between").children([
                    vtree::text(format!("s-{}-name", s.id)).text(s.name.as_str()),
                    // The reading is a signal: ticks never touch the tree.
                    vtree::badge(format!("s-{}-temp", s.id))
                        .text(cel(format!("string(s.temp['{}']) + ' °C'", s.id))),
                ])
            })
            .collect()
    }

    fn temps(&self) -> Value {
        let m: Map<String, Value> = self.sensors.iter().map(|s| (s.id.clone(), json!(s.temp))).collect();
        json!({ "temp": m })
    }

    fn update(&mut self, out: &mut Outbox) {
        out.signal(self.temps()); // before the rows that read it
        let next = self.rows();
        out.extend(vtree::diff_into(Some("sensors"), &self.shown, &next));
        self.shown = next;
    }
}

impl App for Monitor {
    fn hello(&mut self, out: &mut Outbox, _: &BTreeMap<String, String>) {
        self.sensors = load_sensors();
        // The shell, by hand, once.
        out.element(card("main").title("Sensors"));
        out.element(list("sensors").parent("main").empty("No sensors."));
        self.update(out);
    }

    fn action(&mut self, out: &mut Outbox, action: &Action) {
        // ... sensors added or removed: the structure changed ...
        self.update(out);
    }

    fn tick_interval(&self) -> Option<u32> {
        Some(1000)
    }

    fn tick(&mut self, out: &mut Outbox, _now_ms: f64) {
        read_temperatures(&mut self.sensors);
        out.signal(self.temps()); // the hot path: no view, no diff
    }
}
```

Here `ui::card`/`ui::list` (hand-written `Element`s with `parent`) and
`vtree::row`/`vtree::text` (nodes) are used side by side; import one set
qualified to keep them apart.

## Testing

`server::Loopback` runs a real renderer core against the app. Beyond
asserting the resolved tree, `take_changes()` shows whether elements were
moved and updated rather than re-created, which is what keeps widget state:

```rust
use hypereffect::{json, server::Loopback};

fn add(l: &mut Loopback<Board>, title: &str) {
    l.write("new", json!(title));
    assert!(l.trigger("add", "onPress"));
}

#[test]
fn moving_an_issue_moves_its_row() {
    let mut l = Loopback::<Board>::new();
    add(&mut l, "Fix login");
    add(&mut l, "Write docs");
    assert_eq!(l.client.children(Some("lane-todo-items")), ["i1", "i2"]);
    l.client.take_changes();

    assert!(l.trigger("i1-right", "onPress"));
    assert_eq!(l.client.children(Some("lane-todo-items")), ["i2"]);
    assert_eq!(l.client.children(Some("lane-doing-items")), ["i1"]);
    assert_eq!(l.client.attr("lane-doing", "title"), Some(json!("In progress · 1")));
    let changes = l.client.take_changes();
    // The row was moved, not rebuilt; only its new ← button is new.
    assert_eq!(changes.created.into_iter().collect::<Vec<_>>(), ["i1-left"]);
    assert!(changes.removed.is_empty());
    assert!(l.client.take_diagnostics().is_empty());
}

#[test]
fn editing_in_the_dialog() {
    let mut l = Loopback::<Board>::new();
    add(&mut l, "Fix login");
    add(&mut l, "Write docs");
    assert!(l.trigger("i2-open", "onPress"));
    assert_eq!(l.client.children(None), ["main", "detail"]);
    assert_eq!(l.client.value("edit.title"), Some(&json!("Write docs")));

    l.write("edit.title", json!("Write the guide"));
    l.write("edit.assignee", json!("ben"));
    l.write("edit.urgent", json!(true));
    assert!(l.trigger("detail-save", "onPress"));
    assert_eq!(l.client.children(None), ["main"]);
    assert_eq!(l.client.attr("i2-open", "label"), Some(json!("Write the guide")));
    assert_eq!(l.client.children(Some("i2")), ["i2-open", "i2-urgent", "i2-who", "i2-right"]);

    // Urgent first reorders the lane.
    l.write("sort", json!("urgent"));
    assert!(l.trigger("sort", "onChange"));
    assert_eq!(l.client.children(Some("lane-todo-items")), ["i2", "i1"]);

    // An unknown assignee is refused and the dialog stays.
    assert!(l.trigger("i1-open", "onPress"));
    l.write("edit.assignee", json!("mallory"));
    assert!(l.trigger("detail-save", "onPress"));
    assert_eq!(l.client.children(None), ["main", "detail"]);
    assert_eq!(l.client.value("_saving"), Some(&json!(false)));

    assert!(l.trigger("detail-delete", "onPress"));
    assert_eq!(l.client.children(None), ["main"]);
    assert_eq!(l.client.children(Some("lane-todo-items")), ["i2"]);
    assert!(l.client.take_diagnostics().is_empty());
}
```

Always assert `take_diagnostics()` is empty: it catches wrong nesting
(a `dialog` not at the root, a `tab` outside `tabs`), CEL type errors and
expressions reading signals that were never initialised.

## Checklist

- `view` is a pure function of the state; every action ends in one
  `update` that diffs against `shown` and stores the new tree.
- Ids come from domain ids, never positions, and appear once per tree.
- Never send `element`/`remove` by hand for ids the tree manages.
- Signals the new elements read are written before `update`.
- Dialogs: return `Vec<Node>` and use `diff_into(None, ...)`.
- Node types stay stable; attributes vary.
- Payloads carry ids and wanted states, not positions.
- Server-decided presence: leave the node out. Client-toggled presence:
  `visible` over a local signal.
- Hot values in signals; the timer path writes signals only.
- Indicators reset on every path; server-confirmed values follow patterns A
  and B of [server-guide.md](https://hypereffect.dev/server-guide.md#server-confirmed-state).
- Validate every action name, payload and signal read.
