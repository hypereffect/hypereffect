//! Interfaces as values: build the whole tree of elements from your data with
//! a plain function, send it with [`render`], and when the data changes build
//! it again and send only the difference with [`diff`].
//!
//! ```
//! use hypereffect::vtree::{self, column, list, text, Node};
//!
//! fn view(items: &[&str]) -> Node {
//!     column("main").children([
//!         text("title").variant("h1").text(format!("{} items", items.len())),
//!         list("items").children(items.iter().map(|i| text(format!("item-{i}")).text(*i))),
//!     ])
//! }
//!
//! let before = view(&["milk", "eggs"]);
//! assert_eq!(vtree::render(&before).len(), 5);
//!
//! // The title changes and one item is added; nothing else is sent.
//! let after = view(&["milk", "bread", "eggs"]);
//! let lines: Vec<String> = vtree::diff(&before, &after).iter().map(|m| m.to_line()).collect();
//! assert_eq!(lines, [
//!     "element {\"id\":\"title\",\"type\":\"text\",\"parent\":\"main\",\"variant\":\"h1\",\"text\":\"3 items\"}\n",
//!     "element {\"id\":\"item-bread\",\"type\":\"text\",\"parent\":\"items\",\"before\":\"item-eggs\",\"text\":\"bread\"}\n",
//! ]);
//! ```
//!
//! Elements are matched by id, so ids should identify the data they show
//! (`item-42`, not `item-3` for the third row): an element whose id is kept
//! is updated or moved in place, even to another parent, and the renderer
//! keeps its widget state. Each id must appear once in a tree.
//!
//! The messages follow the rules of section 7 of the spec: parents are sent
//! before their children, an element is sent at most once and only if its
//! attributes, type, parent or position changed, and removals come last.

use crate::message::{attribute_setters, Element, ServerMessage};
use crate::Component;
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};

/// An element with its children.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    /// The element. Its `parent` and `before` are ignored: they come from
    /// the node's place in the tree.
    pub element: Element,
    pub children: Vec<Node>,
}

impl Node {
    pub fn new(id: impl Into<String>, component: Component) -> Self {
        Element::new(id, component).into()
    }

    pub fn id(&self) -> &str {
        &self.element.id
    }

    /// Sets any attribute.
    pub fn set(mut self, name: &str, v: impl Into<Value>) -> Self {
        self.element = self.element.set(name, v);
        self
    }

    attribute_setters!();

    /// Appends a child.
    pub fn child(mut self, child: impl Into<Node>) -> Self {
        self.children.push(child.into());
        self
    }

    /// Appends children.
    pub fn children<N: Into<Node>>(mut self, children: impl IntoIterator<Item = N>) -> Self {
        self.children.extend(children.into_iter().map(Into::into));
        self
    }
}

impl From<Element> for Node {
    fn from(element: Element) -> Self {
        Node {
            element,
            children: Vec::new(),
        }
    }
}

macro_rules! constructors {
    ($($fn:ident => $c:ident),* $(,)?) => {
        $(
            #[doc = concat!("A new `", stringify!($fn), "` node.")]
            pub fn $fn(id: impl Into<String>) -> Node {
                Node::new(id, Component::$c)
            }
        )*
    };
}

constructors! {
    column => Column, row => Row, card => Card, list => List, tabs => Tabs, tab => Tab,
    dialog => Dialog, text => Text, notice => Notice, badge => Badge, image => Image,
    divider => Divider, progress => Progress, button => Button, input => Input,
    checkbox => Checkbox, select => Select, slider => Slider,
}

/// The messages that create a tree at the root of an empty renderer.
pub fn render(root: &Node) -> Vec<ServerMessage> {
    render_into(None, std::slice::from_ref(root))
}

/// The messages that turn the renderer's tree from `old` (as sent with
/// [`render`] or a previous diff) into `new`.
pub fn diff(old: &Node, new: &Node) -> Vec<ServerMessage> {
    diff_into(None, std::slice::from_ref(old), std::slice::from_ref(new))
}

/// The messages that create `roots`, in order, as the last children of
/// `parent` (or of the root).
pub fn render_into(parent: Option<&str>, roots: &[Node]) -> Vec<ServerMessage> {
    diff_into(parent, &[], roots)
}

/// The messages that turn `old` into `new`, both being the children of
/// `parent` (or of the root) that this tree manages. Other children of
/// `parent` are left alone, but may end up between the managed ones.
///
/// Panics if an id appears more than once in `old` or in `new`.
pub fn diff_into(parent: Option<&str>, old: &[Node], new: &[Node]) -> Vec<ServerMessage> {
    let mut model = Model::new(old);
    let mut wanted = HashSet::new();
    index(new, &mut |n| {
        assert!(wanted.insert(n.id()), "duplicate id '{}' in tree", n.id());
    });
    let mut out = Vec::new();
    model.place(None, new, parent, &mut out);
    index(old, &mut |n| {
        if !wanted.contains(n.id()) && model.nodes.contains_key(n.id()) {
            model.remove(n.id());
            out.push(ServerMessage::remove(n.id()));
        }
    });
    out
}

/// Visits nodes in pre-order.
fn index<'a>(nodes: &'a [Node], f: &mut impl FnMut(&'a Node)) {
    for n in nodes {
        f(n);
        index(&n.children, f);
    }
}

/// An element as the renderer has it. A parent of `None` is the mount point.
struct Placed<'a> {
    component: Component,
    attrs: &'a Map<String, Value>,
    parent: Option<&'a str>,
    children: Vec<&'a str>,
}

/// The renderer's tree, kept up to date with the messages emitted so far, so
/// that each message is chosen against what the renderer will actually have.
struct Model<'a> {
    nodes: HashMap<&'a str, Placed<'a>>,
    top: Vec<&'a str>,
}

impl<'a> Model<'a> {
    fn new(old: &'a [Node]) -> Self {
        let mut m = Model {
            nodes: HashMap::new(),
            top: old.iter().map(Node::id).collect(),
        };
        m.add(None, old);
        m
    }

    fn add(&mut self, parent: Option<&'a str>, nodes: &'a [Node]) {
        for n in nodes {
            let placed = Placed {
                component: n.element.component,
                attrs: &n.element.attrs,
                parent,
                children: n.children.iter().map(Node::id).collect(),
            };
            let dup = self.nodes.insert(n.id(), placed).is_some();
            assert!(!dup, "duplicate id '{}' in tree", n.id());
            self.add(Some(n.id()), &n.children);
        }
    }

    fn children(&mut self, parent: Option<&str>) -> &mut Vec<&'a str> {
        match parent {
            None => &mut self.top,
            Some(p) => &mut self.nodes.get_mut(p).expect("parent is placed").children,
        }
    }

    /// Emits what places `want`, in order, as the children of `parent`, then
    /// does the same for their children.
    fn place(
        &mut self,
        parent: Option<&'a str>,
        want: &'a [Node],
        mount: Option<&str>,
        out: &mut Vec<ServerMessage>,
    ) {
        let stable = self.stable(parent, want);
        // From the last child to the first, so that each one can be put
        // before its next sibling, which is already in place.
        for (i, n) in want.iter().enumerate().rev() {
            let next = want.get(i + 1).map(Node::id);
            let before = match self.nodes.get(n.id()) {
                None => Some(next),
                Some(p) if p.parent != parent || !stable.contains(&i) => Some(next),
                Some(p) if p.component != n.element.component || p.attrs != &n.element.attrs => {
                    Some(None)
                }
                Some(_) => None,
            };
            if let Some(before) = before {
                let mut e = n.element.clone();
                e.parent = parent.or(mount).map(str::to_string);
                e.before = before.map(str::to_string);
                out.push(ServerMessage::Element(e));
                self.apply(n, parent, before);
            }
        }
        for n in want {
            self.place(Some(n.id()), &n.children, mount, out);
        }
    }

    /// The indexes in `want` of the current children of `parent` that can
    /// stay where they are: the longest run already in the wanted order. A
    /// child cannot be moved to the end (an element without `before` keeps
    /// its position), so if the last wanted child is already there it is
    /// always part of the run.
    fn stable(&mut self, parent: Option<&str>, want: &[Node]) -> HashSet<usize> {
        let pos: HashMap<&str, usize> = want.iter().enumerate().map(|(i, n)| (n.id(), i)).collect();
        let mut seq: Vec<usize> = self
            .children(parent)
            .iter()
            .filter_map(|id| pos.get(id).copied())
            .collect();
        let last = want.len().wrapping_sub(1);
        if let Some(at) = seq.iter().position(|&i| i == last) {
            seq.truncate(at);
            let mut run = increasing_run(&seq);
            run.push(last);
            return run.into_iter().collect();
        }
        increasing_run(&seq).into_iter().collect()
    }

    /// Applies an element message as section 7 of the spec does.
    fn apply(&mut self, n: &'a Node, parent: Option<&'a str>, before: Option<&'a str>) {
        let id = n.id();
        let Some(p) = self.nodes.get_mut(id) else {
            self.nodes.insert(
                id,
                Placed {
                    component: n.element.component,
                    attrs: &n.element.attrs,
                    parent,
                    children: Vec::new(),
                },
            );
            self.insert(id, parent, before);
            return;
        };
        p.attrs = &n.element.attrs;
        let old_parent = p.parent;
        let wiped = if p.component != n.element.component {
            p.component = n.element.component;
            std::mem::take(&mut p.children)
        } else {
            Vec::new()
        };
        for c in wiped {
            self.drop_subtree(c);
        }
        let moves = old_parent != parent
            || before.is_some_and(|b| b != id && self.children(parent).contains(&b));
        if moves {
            self.children(old_parent).retain(|c| *c != id);
            self.insert(id, parent, before);
            self.nodes.get_mut(id).expect("placed").parent = parent;
        }
    }

    fn insert(&mut self, id: &'a str, parent: Option<&str>, before: Option<&str>) {
        let siblings = self.children(parent);
        match before.and_then(|b| siblings.iter().position(|c| *c == b)) {
            Some(at) => siblings.insert(at, id),
            None => siblings.push(id),
        }
    }

    fn remove(&mut self, id: &str) {
        let parent = self.nodes[id].parent;
        self.children(parent).retain(|c| *c != id);
        self.drop_subtree(id);
    }

    fn drop_subtree(&mut self, id: &str) {
        if let Some(p) = self.nodes.remove(id) {
            for c in p.children {
                self.drop_subtree(c);
            }
        }
    }
}

/// A longest strictly increasing subsequence of `seq`.
fn increasing_run(seq: &[usize]) -> Vec<usize> {
    // tails[k]: index in seq of the smallest tail of a run of length k + 1.
    let mut tails: Vec<usize> = Vec::new();
    let mut prev = vec![usize::MAX; seq.len()];
    for (i, &v) in seq.iter().enumerate() {
        let k = tails.partition_point(|&t| seq[t] < v);
        if k > 0 {
            prev[i] = tails[k - 1];
        }
        if k == tails.len() {
            tails.push(i);
        } else {
            tails[k] = i;
        }
    }
    let mut run = Vec::with_capacity(tails.len());
    let mut i = tails.last().copied().unwrap_or(usize::MAX);
    while i != usize::MAX {
        run.push(seq[i]);
        i = prev[i];
    }
    run.reverse();
    run
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn lines(ms: Vec<ServerMessage>) -> Vec<String> {
        ms.iter().map(|m| m.to_line().trim_end().to_string()).collect()
    }

    fn items(ids: &[&str]) -> Node {
        column("l").children(ids.iter().map(|id| text(*id).text(*id)))
    }

    #[test]
    fn render_sends_parents_first() {
        let t = column("a").child(row("b").child(text("c").text("hi"))).child(divider("d"));
        assert_eq!(
            lines(render(&t)),
            [
                r#"element {"id":"a","type":"column"}"#,
                r#"element {"id":"d","type":"divider","parent":"a"}"#,
                r#"element {"id":"b","type":"row","parent":"a","before":"d"}"#,
                r#"element {"id":"c","type":"text","parent":"b","text":"hi"}"#,
            ]
        );
    }

    #[test]
    fn unchanged_tree_sends_nothing() {
        let t = items(&["a", "b", "c"]);
        assert!(diff(&t, &t.clone()).is_empty());
    }

    #[test]
    fn attribute_change_sends_one_element_in_place() {
        let old = items(&["a", "b"]);
        let mut new = old.clone();
        new.children[0] = text("a").text("A").variant("h2");
        assert_eq!(
            lines(diff(&old, &new)),
            [r#"element {"id":"a","type":"text","parent":"l","text":"A","variant":"h2"}"#]
        );
    }

    #[test]
    fn moves_the_fewest_children() {
        // Nothing can be moved to the end, so moving the first item there
        // moves the others before it.
        let old = items(&["a", "b", "c"]);
        let new = items(&["b", "c", "a"]);
        assert_eq!(
            lines(diff(&old, &new)),
            [
                r#"element {"id":"c","type":"text","parent":"l","before":"a","text":"c"}"#,
                r#"element {"id":"b","type":"text","parent":"l","before":"c","text":"b"}"#,
            ]
        );
        // Moving the last item to the front is one message.
        let new = items(&["c", "a", "b"]);
        assert_eq!(
            lines(diff(&old, &new)),
            [r#"element {"id":"c","type":"text","parent":"l","before":"a","text":"c"}"#]
        );
    }

    #[test]
    fn removals_come_last_and_only_at_the_top() {
        let old = column("r")
            .child(card("gone").child(text("x")).child(text("kept")))
            .child(text("y"));
        let new = column("r").child(text("kept")).child(text("y"));
        assert_eq!(
            lines(diff(&old, &new)),
            [
                r#"element {"id":"kept","type":"text","parent":"r","before":"y"}"#,
                r#"remove {"id":"gone"}"#,
            ]
        );
    }

    #[test]
    fn type_change_recreates_descendants() {
        let old = column("r").child(card("c").child(text("t")));
        let new = column("r").child(column("c").child(text("t")));
        assert_eq!(
            lines(diff(&old, &new)),
            [
                r#"element {"id":"c","type":"column","parent":"r"}"#,
                r#"element {"id":"t","type":"text","parent":"c"}"#,
            ]
        );
    }

    #[test]
    fn mounted_under_a_parent() {
        let old = [text("a"), text("b")];
        let new = [text("b"), text("a").text(json!("A"))];
        assert_eq!(
            lines(diff_into(Some("host"), &old, &new)),
            [
                r#"element {"id":"a","type":"text","parent":"host","text":"A"}"#,
                r#"element {"id":"b","type":"text","parent":"host","before":"a"}"#,
            ]
        );
    }

    #[test]
    #[should_panic(expected = "duplicate id 'a'")]
    fn duplicate_ids_panic() {
        render(&column("r").child(text("a")).child(text("a")));
    }

    #[test]
    fn increasing_runs() {
        assert_eq!(increasing_run(&[]), Vec::<usize>::new());
        assert_eq!(increasing_run(&[3, 0, 1, 4, 2]), [0, 1, 2]);
        assert_eq!(increasing_run(&[2, 1, 0]).len(), 1);
    }
}
