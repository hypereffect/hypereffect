//! The renderer side of a session, without any drawing.
//!
//! A [`Client`] keeps the element tree and the signals, evaluates CEL
//! expressions with the [`cel`] crate, runs handlers, and yields elements
//! whose attributes are resolved to plain JSON values. A UI layer reads the
//! resolved elements and draws them; [`Client::take_changes`] says which ones
//! to redraw.
//!
//! ```
//! use hypereffect::client::Client;
//! use hypereffect::json;
//!
//! let mut c = Client::new();
//! let hello = c.hello();
//! assert_eq!(hello, "hello {}\n");
//! c.receive(b"signal {\"count\":2}\n");
//! c.receive(b"element {\"id\":\"v\",\"type\":\"text\",\"text\":{\"cel\":\"'Count: ' + string(s.count)\"}}\n");
//! assert_eq!(c.get("v").unwrap().attrs["text"], json!("Count: 2"));
//! ```

use crate::component::{accepts, AttrType, Component};
use crate::message::{
    encode_line, is_cel, parse_line, LineBuffer, Log, Metadata, RendererMessage, ServerMessage,
};
use crate::signals::{get_path, merge_patch_tracked, parse_path, path_patch, strip_local_map};
use cel::objects::{Key, Map as CelMap};
use cel::{Context, Env, Program};
use serde::Serialize;
use serde_json::{Map, Number, Value};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

const ROOT: &str = "";

/// An element with its attributes resolved: expressions evaluated, values
/// checked against the attribute types, and defaults filled in. Structural
/// attributes (`bind` and handlers) are included as they were sent, or null.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Resolved {
    pub id: String,
    #[serde(rename = "type")]
    pub component: Component,
    pub parent: Option<String>,
    pub children: Vec<String>,
    pub attrs: Map<String, Value>,
}

/// A resolved element with its resolved descendants.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResolvedTree {
    pub id: String,
    #[serde(rename = "type")]
    pub component: Component,
    pub attrs: Map<String, Value>,
    pub children: Vec<ResolvedTree>,
}

/// What changed since the last [`Client::take_changes`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Changes {
    /// The tree was discarded because a new session began.
    pub reset: bool,
    /// The connected state changed.
    pub connection: bool,
    /// Elements whose widgets must be built anew: new, or with a new type.
    pub created: BTreeSet<String>,
    /// Elements whose resolved attributes may have changed.
    pub updated: BTreeSet<String>,
    pub removed: BTreeSet<String>,
    /// Elements whose list of children changed (`""` for the root).
    pub reordered: BTreeSet<String>,
    /// The application metadata changed.
    pub metadata: bool,
    signals: bool,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        !self.reset
            && !self.connection
            && self.created.is_empty()
            && self.updated.is_empty()
            && self.removed.is_empty()
            && self.reordered.is_empty()
            && !self.metadata
    }
}

struct Node {
    component: Component,
    parent: Option<String>,
    children: Vec<String>,
    el: Map<String, Value>,
    progs: HashMap<&'static str, Program>,
    /// Whether any attribute reads signals.
    reads_signals: bool,
}

pub struct Client {
    env: Arc<Env>,
    nodes: HashMap<String, Node>,
    root: Vec<String>,
    signals: Value,
    metadata: Metadata,
    cel_signals: RefCell<Option<cel::Value>>,
    connected: bool,
    reset_pending: bool,
    reader: LineBuffer,
    changes: Changes,
    diagnostics: RefCell<Vec<String>>,
    opens: Vec<String>,
    logs: Vec<Log>,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    pub fn new() -> Self {
        let mut env = Env::stdlib();
        env.add_extension(cel::extensions::strings)
            .expect("strings extension");
        env.add_extension(cel::extensions::math)
            .expect("math extension");
        env.add_extension(cel::extensions::lists)
            .expect("lists extension");
        Client {
            env: Arc::new(env),
            nodes: HashMap::new(),
            root: Vec::new(),
            signals: Value::Object(Map::new()),
            metadata: Metadata::default(),
            cel_signals: RefCell::new(None),
            connected: false,
            reset_pending: false,
            reader: LineBuffer::new(),
            changes: Changes::default(),
            diagnostics: RefCell::new(Vec::new()),
            opens: Vec::new(),
            logs: Vec::new(),
        }
    }

    // --- transport ---------------------------------------------------------

    /// Begins a session and returns the hello line to send. The previous
    /// tree is kept until the first server message of the new session.
    pub fn hello(&mut self) -> String {
        self.hello_with(&BTreeMap::new())
    }

    /// Like [`hello`](Self::hello), with params for the app: strings chosen
    /// by whoever opened the session, such as the ones in its address.
    pub fn hello_with(&mut self, params: &BTreeMap<String, String>) -> String {
        self.reader = LineBuffer::new();
        self.connected = true;
        self.reset_pending = true;
        let line = RendererMessage::Hello {
            params: params.clone(),
        }
        .to_line();
        if self.nodes.is_empty() {
            self.reset();
        }
        self.changes.connection = true;
        line
    }

    /// Feeds bytes received from the transport.
    pub fn receive(&mut self, bytes: &[u8]) {
        for line in self.reader.push(bytes) {
            self.receive_line(&line);
        }
    }

    pub fn receive_line(&mut self, line: &str) {
        match parse_line(line) {
            Ok((tag, body)) => self.receive_message(tag, body),
            Err(e) => self.diag(format!("ignored line: {e}")),
        }
    }

    /// Processes a message given as a tag and its JSON object.
    pub fn receive_message(&mut self, tag: &str, body: Map<String, Value>) {
        if self.reset_pending {
            self.reset();
        }
        match tag {
            "element" => self.element(body),
            "remove" => {
                if let Some(Value::String(id)) = body.get("id") {
                    self.remove(&id.clone());
                }
            }
            "signal" => self.patch(&Value::Object(body)),
            "metadata" => self.set_metadata(Metadata::from_map(&body)),
            "log" => match Log::from_map(&body) {
                Ok(log) => self.logs.push(log),
                Err(e) => self.diag(format!("ignored log: {e}")),
            },
            t => self.diag(format!("ignored message with unknown tag '{t}'")),
        }
    }

    /// Processes a message, as the in-process transport.
    pub fn apply(&mut self, m: &ServerMessage) {
        let Value::Object(body) = m.body() else {
            unreachable!()
        };
        self.receive_message(m.tag(), body);
    }

    /// Marks the transport closed. The tree is kept for display.
    pub fn close(&mut self) {
        self.connected = false;
        self.changes.connection = true;
    }

    pub fn is_connected(&self) -> bool {
        self.connected
    }

    // --- reading -----------------------------------------------------------

    /// The children of `id`, or of the root.
    pub fn children(&self, id: Option<&str>) -> &[String] {
        match id {
            None | Some(ROOT) => &self.root,
            Some(id) => self.nodes.get(id).map_or(&[], |n| &n.children),
        }
    }

    pub fn contains(&self, id: &str) -> bool {
        self.nodes.contains_key(id)
    }

    pub fn get(&self, id: &str) -> Option<Resolved> {
        let n = self.nodes.get(id)?;
        let attrs = n
            .component
            .attrs()
            .iter()
            .map(|a| {
                (
                    a.name.to_string(),
                    self.attr(id, a.name).unwrap_or(Value::Null),
                )
            })
            .collect();
        Some(Resolved {
            id: id.to_string(),
            component: n.component,
            parent: n.parent.clone(),
            children: n.children.clone(),
            attrs,
        })
    }

    /// One resolved attribute, or `None` if the element or attribute does
    /// not exist.
    pub fn attr(&self, id: &str, name: &str) -> Option<Value> {
        let n = self.nodes.get(id)?;
        let spec = n.component.attr(name)?;
        if spec.ty.is_structural() {
            return Some(n.el.get(name).cloned().unwrap_or(Value::Null));
        }
        let def = spec.default.value(id);
        let Some(raw) = n.el.get(name) else {
            return Some(def);
        };
        let value = if is_cel(raw).is_some() {
            let Some(prog) = n.progs.get(spec.name) else {
                return Some(def);
            };
            match self.eval(prog) {
                Ok(v) => v,
                Err(e) => {
                    self.diag(format!("{id}.{name}: {e}"));
                    return Some(def);
                }
            }
        } else if raw.is_null() {
            return Some(def);
        } else {
            from_json(raw)
        };
        Some(check(spec.ty, &value, def).unwrap_or_else(|(e, def)| {
            self.diag(format!("{id}.{name}: {e}"));
            def
        }))
    }

    /// The whole tree, resolved.
    pub fn tree(&self) -> Vec<ResolvedTree> {
        self.tree_of(&self.root)
    }

    fn tree_of(&self, ids: &[String]) -> Vec<ResolvedTree> {
        ids.iter()
            .filter_map(|id| {
                let r = self.get(id)?;
                Some(ResolvedTree {
                    id: r.id,
                    component: r.component,
                    attrs: r.attrs,
                    children: self.tree_of(&r.children),
                })
            })
            .collect()
    }

    /// The details the server sent about the application, if any.
    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    pub fn signals(&self) -> &Value {
        &self.signals
    }

    /// The value at a signal path such as `form.email`.
    pub fn value(&self, path: &str) -> Option<&Value> {
        get_path(&self.signals, &parse_path(path)?)
    }

    // --- user input --------------------------------------------------------

    /// Writes a value at a signal path, as a control with `bind` does.
    pub fn write(&mut self, path: &str, value: Value) {
        if let Some(segs) = parse_path(path) {
            let patch = path_patch(&segs, value);
            self.patch(&patch);
        }
    }

    /// Runs the handler in attribute `event` of element `id`, following
    /// section 10 of the spec. Returns the action line to send, if any. A URL
    /// to open is queued for [`Client::take_opens`].
    pub fn trigger(&mut self, id: &str, event: &str) -> Option<String> {
        let n = self.nodes.get(id)?;
        if n.component.attr(event)?.ty != AttrType::Handler {
            return None;
        }
        let h = n.el.get(event)?.as_object()?.clone();
        let mut a = Some(id.to_string());
        while let Some(cur) = a {
            if self.attr(&cur, "visible") != Some(Value::Bool(true)) {
                return None;
            }
            if self.attr(&cur, "disabled") == Some(Value::Bool(true)) {
                return None;
            }
            a = self.nodes[&cur].parent.clone();
        }
        if let Some(Value::Object(patch)) = h.get("patch") {
            let p = self.eval_patch(patch, &format!("{id}.{event}"));
            self.patch(&Value::Object(p));
        }
        if let Some(open) = h.get("open") {
            if let Some(url) = self.eval_open(open, &format!("{id}.{event}")) {
                self.opens.push(url);
            }
        }
        let action = h.get("action")?.as_str()?.to_string();
        if !self.connected {
            return None;
        }
        if let Some(segs) = h.get("indicator").and_then(|i| parse_path(i.as_str()?)) {
            let p = path_patch(&segs, Value::Bool(true));
            self.patch(&p);
        }
        let mut msg = Map::new();
        msg.insert("name".into(), action.into());
        if let Some(p) = h.get("payload") {
            msg.insert("payload".into(), p.clone());
        }
        let signals = self
            .signals
            .as_object()
            .map(strip_local_map)
            .unwrap_or_default();
        msg.insert("signals".into(), Value::Object(signals));
        Some(encode_line("action", &msg))
    }

    // --- changes -----------------------------------------------------------

    /// Takes what changed since the last call.
    pub fn take_changes(&mut self) -> Changes {
        let mut c = std::mem::take(&mut self.changes);
        if c.signals {
            for (id, n) in &self.nodes {
                if n.reads_signals {
                    c.updated.insert(id.clone());
                }
            }
        }
        // Tabs and lists present facts about their children.
        let touched: Vec<String> = c.updated.iter().chain(&c.reordered).cloned().collect();
        for id in touched {
            let Some(n) = self.nodes.get(&id) else {
                continue;
            };
            if matches!(n.component, Component::Tabs | Component::List) {
                c.updated.insert(id.clone());
            }
            if let Some(p) = &n.parent {
                if matches!(self.nodes[p].component, Component::Tabs | Component::List) {
                    c.updated.insert(p.clone());
                }
            }
        }
        for id in &c.removed {
            c.updated.remove(id);
            c.created.remove(id);
        }
        c
    }

    /// Takes the diagnostics reported since the last call.
    pub fn take_diagnostics(&mut self) -> Vec<String> {
        std::mem::take(self.diagnostics.get_mut())
    }

    /// Returns and clears the URLs that handlers asked to open (a button's
    /// `open`), oldest first. The host decides whether and how to open them.
    pub fn take_opens(&mut self) -> Vec<String> {
        std::mem::take(&mut self.opens)
    }

    /// Returns and clears the log messages received since the last call,
    /// oldest first. The host decides whether and how to show them.
    pub fn take_logs(&mut self) -> Vec<Log> {
        std::mem::take(&mut self.logs)
    }

    // --- internals ---------------------------------------------------------

    fn diag(&self, msg: String) {
        self.diagnostics.borrow_mut().push(msg);
    }

    fn set_metadata(&mut self, m: Metadata) {
        if self.metadata != m {
            self.metadata = m;
            self.changes.metadata = true;
        }
    }

    fn reset(&mut self) {
        self.reset_pending = false;
        self.set_metadata(Metadata::default());
        self.changes
            .removed
            .extend(self.nodes.drain().map(|(id, _)| id));
        self.root.clear();
        self.signals = Value::Object(Map::new());
        *self.cel_signals.get_mut() = None;
        self.changes.reset = true;
        self.changes.signals = true;
        self.changes.reordered.insert(ROOT.into());
    }

    fn cel_signals(&self) -> cel::Value {
        self.cel_signals
            .borrow_mut()
            .get_or_insert_with(|| from_json(&self.signals))
            .clone()
    }

    fn eval(&self, prog: &Program) -> Result<cel::Value, String> {
        let mut ctx = Context::with_env(self.env.clone());
        ctx.add_variable_from_value("s", self.cel_signals());
        prog.execute(&ctx).map_err(|e| e.to_string())
    }

    fn patch(&mut self, patch: &Value) {
        let mut changes = Vec::new();
        merge_patch_tracked(&mut self.signals, patch, &mut Vec::new(), &mut changes);
        *self.cel_signals.get_mut() = None;
        self.changes.signals = true;
    }

    fn eval_patch(&self, patch: &Map<String, Value>, at: &str) -> Map<String, Value> {
        let mut out = Map::new();
        for (k, v) in patch {
            if let Some(src) = is_cel(v) {
                let r = self
                    .env
                    .compile(src)
                    .map_err(|e| e.to_string())
                    .and_then(|p| self.eval(&p))
                    .and_then(|v| to_json(&v));
                match r {
                    Ok(v) => {
                        out.insert(k.clone(), v);
                    }
                    Err(e) => self.diag(format!("{at}.patch.{k}: {e}")),
                }
            } else if let Value::Object(m) = v {
                out.insert(
                    k.clone(),
                    Value::Object(self.eval_patch(m, &format!("{at}.{k}"))),
                );
            } else {
                out.insert(k.clone(), v.clone());
            }
        }
        out
    }

    /// The URL of a handler's `open`, or None if its expression fails or does
    /// not give a string.
    fn eval_open(&self, open: &Value, at: &str) -> Option<String> {
        let Some(src) = is_cel(open) else {
            return open.as_str().map(str::to_string);
        };
        let r = self
            .env
            .compile(src)
            .map_err(|e| e.to_string())
            .and_then(|p| self.eval(&p));
        match r {
            Ok(cel::Value::String(s)) => Some(s.to_string()),
            Ok(v) => {
                self.diag(format!("{at}.open: expected a string, got {}", v.type_of()));
                None
            }
            Err(e) => {
                self.diag(format!("{at}.open: {e}"));
                None
            }
        }
    }

    fn reject(&self, el: &Map<String, Value>, why: &str) {
        let id = el.get("id").cloned().unwrap_or(Value::Null);
        self.diag(format!("rejected element {id}: {why}"));
    }

    fn element(&mut self, el: Map<String, Value>) {
        let id = match el.get("id") {
            Some(Value::String(s)) if !s.is_empty() => s.clone(),
            _ => return self.reject(&el, "id must be a non-empty string"),
        };
        let Some(component) = el
            .get("type")
            .and_then(Value::as_str)
            .and_then(Component::from_name)
        else {
            return self.reject(
                &el,
                &format!("unknown type {}", el.get("type").unwrap_or(&Value::Null)),
            );
        };
        for (k, v) in &el {
            let structural = matches!(k.as_str(), "id" | "type" | "parent" | "before")
                || component.attr(k).is_some_and(|a| a.ty.is_structural());
            if structural && is_cel(v).is_some() {
                return self.reject(&el, &format!("{k} cannot be an expression"));
            }
        }
        for k in ["parent", "before"] {
            if el.get(k).is_some_and(|v| !v.is_string()) {
                return self.reject(&el, &format!("{k} must be a string"));
            }
        }
        for a in component.attrs() {
            let Some(v) = el.get(a.name) else { continue };
            if a.ty == AttrType::Path && v.as_str().and_then(parse_path).is_none() {
                return self.reject(&el, &format!("{} is not a signal path", a.name));
            }
            if a.ty == AttrType::Handler && !valid_handler(v, component == Component::Button) {
                return self.reject(&el, &format!("{} is not a valid handler", a.name));
            }
        }
        let parent = el.get("parent").and_then(Value::as_str).map(str::to_string);
        let parent_type = match &parent {
            None => None,
            Some(p) => match self.nodes.get(p) {
                Some(n) => Some(n.component),
                None => return self.reject(&el, &format!("parent \"{p}\" does not exist")),
            },
        };
        if !accepts(parent_type, component) {
            let p = parent_type.map_or("the root".to_string(), |t| t.to_string());
            return self.reject(&el, &format!("{p} cannot contain {component}"));
        }
        if self.nodes.contains_key(&id) {
            let mut a = parent.clone();
            while let Some(cur) = a {
                if cur == id {
                    return self.reject(&el, "an element cannot be inside itself");
                }
                a = self.nodes[&cur].parent.clone();
            }
        }

        let mut progs = HashMap::new();
        let mut reads_signals = false;
        for a in component.attrs() {
            let Some(v) = el.get(a.name) else { continue };
            if a.ty == AttrType::Path {
                reads_signals = true;
            }
            let Some(src) = is_cel(v) else { continue };
            reads_signals = true;
            match self.env.compile(src) {
                Ok(p) => {
                    progs.insert(a.name, p);
                }
                Err(e) => self.diag(format!("{id}.{}: {e}", a.name)),
            }
        }
        let before = el.get("before").and_then(Value::as_str).map(str::to_string);
        let mut node = Node {
            component,
            parent: parent.clone(),
            children: Vec::new(),
            el,
            progs,
            reads_signals,
        };

        match self.nodes.remove(&id) {
            None => {
                self.nodes.insert(id.clone(), node);
                self.insert(&id, parent.as_deref(), before.as_deref());
                self.changes.created.insert(id.clone());
                self.changes.removed.remove(&id);
            }
            Some(old) => {
                if old.component == component {
                    node.children = old.children;
                } else {
                    for ch in &old.children {
                        self.remove_tree(ch);
                    }
                    self.changes.created.insert(id.clone());
                }
                self.nodes.insert(id.clone(), node);
                if old.parent != parent {
                    let sib = self.children_mut(old.parent.as_deref());
                    sib.retain(|x| x != &id);
                    self.changes
                        .reordered
                        .insert(old.parent.unwrap_or_default());
                    self.insert(&id, parent.as_deref(), before.as_deref());
                } else if let Some(b) = before.filter(|b| b != &id) {
                    let sib = self.children_mut(parent.as_deref());
                    if sib.contains(&b) {
                        sib.retain(|x| x != &id);
                        let i = sib.iter().position(|x| x == &b).unwrap();
                        sib.insert(i, id.clone());
                        self.changes.reordered.insert(parent.unwrap_or_default());
                    }
                }
            }
        }
        self.changes.updated.insert(id);
    }

    fn children_mut(&mut self, id: Option<&str>) -> &mut Vec<String> {
        match id {
            None => &mut self.root,
            Some(id) => &mut self.nodes.get_mut(id).expect("parent exists").children,
        }
    }

    fn insert(&mut self, id: &str, parent: Option<&str>, before: Option<&str>) {
        let list = self.children_mut(parent);
        match before.and_then(|b| list.iter().position(|x| x == b)) {
            Some(i) => list.insert(i, id.to_string()),
            None => list.push(id.to_string()),
        }
        self.changes
            .reordered
            .insert(parent.unwrap_or_default().to_string());
    }

    fn remove(&mut self, id: &str) {
        let Some(n) = self.nodes.get(id) else { return };
        let parent = n.parent.clone();
        self.children_mut(parent.as_deref()).retain(|x| x != id);
        self.changes.reordered.insert(parent.unwrap_or_default());
        self.remove_tree(id);
    }

    fn remove_tree(&mut self, id: &str) {
        let Some(n) = self.nodes.remove(id) else {
            return;
        };
        for ch in &n.children {
            self.remove_tree(ch);
        }
        let c = &mut self.changes;
        c.removed.insert(id.to_string());
        c.created.remove(id);
        c.updated.remove(id);
        c.reordered.remove(id);
    }
}

/// `open` is allowed only when `can_open` (on a button).
fn valid_handler(h: &Value, can_open: bool) -> bool {
    let Some(h) = h.as_object() else { return false };
    let patch = h.get("patch");
    let action = h.get("action");
    let open = h.get("open");
    if patch.is_none() && action.is_none() && open.is_none() {
        return false;
    }
    if let Some(o) = open {
        let ok = can_open && (is_cel(o).is_some() || o.as_str().is_some_and(|s| !s.is_empty()));
        if !ok {
            return false;
        }
    }
    if patch.is_some_and(|p| !p.is_object()) {
        return false;
    }
    if action.is_some_and(|a| a.as_str().is_none_or(str::is_empty)) {
        return false;
    }
    if let Some(i) = h.get("indicator") {
        let ok = action.is_some()
            && i.as_str()
                .and_then(parse_path)
                .is_some_and(|p| p[0].starts_with('_'));
        if !ok {
            return false;
        }
    }
    true
}

/// Converts JSON to a CEL value as in section 9.2 of the spec.
pub fn from_json(v: &Value) -> cel::Value {
    match v {
        Value::Null => cel::Value::Null,
        Value::Bool(b) => cel::Value::Bool(*b),
        Value::Number(n) => match n.as_i64() {
            Some(i) => cel::Value::Int(i),
            None => cel::Value::Float(n.as_f64().unwrap_or(f64::NAN)),
        },
        Value::String(s) => cel::Value::String(Arc::new(s.clone())),
        Value::Array(a) => cel::Value::List(Arc::new(a.iter().map(from_json).collect())),
        Value::Object(o) => cel::Value::Map(CelMap {
            map: Arc::new(
                o.iter()
                    .map(|(k, v)| (Key::String(Arc::new(k.clone())), from_json(v)))
                    .collect(),
            ),
        }),
    }
}

/// Converts a CEL value to JSON, for a handler's patch.
pub fn to_json(v: &cel::Value) -> Result<Value, String> {
    Ok(match v {
        cel::Value::Null => Value::Null,
        cel::Value::Bool(b) => Value::Bool(*b),
        cel::Value::Int(i) => (*i).into(),
        cel::Value::UInt(u) => (*u).into(),
        cel::Value::Float(f) => Value::Number(
            Number::from_f64(*f).ok_or("no JSON representation for a non-finite double")?,
        ),
        cel::Value::String(s) => Value::String(s.to_string()),
        cel::Value::List(l) => Value::Array(l.iter().map(to_json).collect::<Result<_, _>>()?),
        cel::Value::Map(m) => {
            let mut out = Map::new();
            for (k, v) in m.map.iter() {
                let Key::String(k) = k else {
                    return Err("map key is not a string".into());
                };
                out.insert(k.to_string(), to_json(v)?);
            }
            Value::Object(out)
        }
        v => return Err(format!("no JSON representation for {}", v.type_of())),
    })
}

/// Checks a CEL value against an attribute type (section 9.3 of the spec).
/// On failure, returns the message and the default.
fn check(ty: AttrType, v: &cel::Value, def: Value) -> Result<Value, (String, Value)> {
    let fail = |want: &str, def: Value| Err((format!("expected {want}, got {}", v.type_of()), def));
    match ty {
        AttrType::Enum(values) => match v {
            cel::Value::String(s) if values.contains(&s.as_str()) => {
                Ok(Value::String(s.to_string()))
            }
            cel::Value::String(_) => Ok(def),
            _ => fail("a string", def),
        },
        AttrType::String => match v {
            cel::Value::String(s) => Ok(Value::String(s.to_string())),
            _ => fail("a string", def),
        },
        AttrType::Bool => match v {
            cel::Value::Bool(b) => Ok(Value::Bool(*b)),
            _ => fail("a bool", def),
        },
        AttrType::Number | AttrType::NumberOrNull => {
            let f = match v {
                cel::Value::Null if ty == AttrType::NumberOrNull => return Ok(Value::Null),
                cel::Value::Int(i) => *i as f64,
                cel::Value::UInt(u) => *u as f64,
                cel::Value::Float(f) => *f,
                _ => return fail("a number", def),
            };
            match Number::from_f64(f) {
                Some(n) => Ok(Value::Number(n)),
                None => fail("a finite number", def),
            }
        }
        AttrType::Options => {
            let cel::Value::List(items) = v else {
                return fail("a list", def);
            };
            let mut out = Vec::with_capacity(items.len());
            for item in items.iter() {
                let get = |m: &CelMap, k: &str| match m.map.get(&Key::String(Arc::new(k.into()))) {
                    Some(cel::Value::String(s)) => Some(s.to_string()),
                    _ => None,
                };
                let opt = match item {
                    cel::Value::Map(m) => get(m, "value").zip(get(m, "label")),
                    _ => None,
                };
                let Some((value, label)) = opt else {
                    return Err((
                        "options must be objects with string value and label".into(),
                        def,
                    ));
                };
                out.push(serde_json::json!({ "value": value, "label": label }));
            }
            Ok(Value::Array(out))
        }
        AttrType::Path | AttrType::Handler => unreachable!("structural attributes are not checked"),
    }
}
