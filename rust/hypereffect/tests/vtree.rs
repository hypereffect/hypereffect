//! Diffs applied to a real renderer core give the same tree as rendering the
//! new tree from scratch.
use hypereffect::client::Client;
use hypereffect::vtree::{self, card, column, row, text, Node};
use hypereffect::ServerMessage;

/// A small deterministic generator, so failures can be replayed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// A random tree drawing ids from a small pool, so that consecutive trees
/// share ids at different depths, parents and positions.
fn tree(rng: &mut Rng) -> Node {
    let mut pool: Vec<usize> = (0..24).collect();
    for i in (1..pool.len()).rev() {
        pool.swap(i, rng.below(i + 1));
    }
    let take = 4 + rng.below(18);
    pool.truncate(take);
    let mut root = column("root");
    let mut budget = pool.into_iter();
    fill(rng, &mut root, &mut budget, 0);
    root
}

fn fill(rng: &mut Rng, parent: &mut Node, ids: &mut impl Iterator<Item = usize>, depth: usize) {
    let n = rng.below(5);
    for _ in 0..n {
        let Some(id) = ids.next() else { return };
        let id = format!("n{id}");
        let mut child = match (depth < 3, rng.below(4)) {
            (true, 0) => column(id),
            (true, 1) => row(id),
            (true, 2) => card(id).title(format!("t{}", rng.below(3))),
            _ => text(id).text(format!("v{}", rng.below(3))),
        };
        if child.element.component != hypereffect::Component::Text {
            fill(rng, &mut child, ids, depth + 1);
        }
        parent.children.push(child);
    }
}

fn apply(c: &mut Client, ms: &[ServerMessage]) {
    for m in ms {
        c.apply(m);
    }
    assert_eq!(c.take_diagnostics(), Vec::<String>::new());
}

fn fresh(t: &Node) -> Client {
    let mut c = Client::new();
    c.hello();
    apply(&mut c, &vtree::render(t));
    c
}

#[test]
fn diffs_reach_the_same_tree_as_a_fresh_render() {
    for seed in 1..400u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let mut c = Client::new();
        c.hello();
        let mut old = tree(&mut rng);
        apply(&mut c, &vtree::render(&old));
        for step in 0..8 {
            let new = tree(&mut rng);
            let ms = vtree::diff(&old, &new);
            apply(&mut c, &ms);
            assert_eq!(c.tree(), fresh(&new).tree(), "seed {seed}, step {step}");
            assert!(vtree::diff(&new, &new).is_empty());
            old = new;
        }
    }
}

#[test]
fn kept_elements_are_never_recreated() {
    // An element that keeps its id and type is updated or moved, never
    // removed and created again, so the renderer keeps its widget state.
    let old = column("root")
        .child(card("a").child(text("x").text("1")))
        .child(card("b"));
    let new = column("root")
        .child(card("b").child(text("x").text("2")))
        .child(card("a"));
    let mut c = fresh(&old);
    c.take_changes();
    apply(&mut c, &vtree::diff(&old, &new));
    let changes = c.take_changes();
    assert!(changes.created.is_empty(), "{changes:?}");
    assert!(changes.removed.is_empty(), "{changes:?}");
    assert_eq!(c.tree(), fresh(&new).tree());
}
