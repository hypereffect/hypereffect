//! Types and helpers for [hypereffect](https://github.com/hypereffect/hypereffect),
//! a small hypermedia protocol for server-driven UI.
//!
//! - [`message`]: elements, handlers, server and renderer messages, and the
//!   line framing. Building an interface looks like
//!   `ui::text("title").parent("main").variant("h1").text("Inbox")`.
//! - [`signals`]: merge patch, local signals and signal paths.
//! - [`component`]: the components, their attributes and nesting rules.
//! - [`server`]: an [`App`](server::App) trait for per-session logic, and
//!   [`wasm_app!`] to export one from a WebAssembly module.
//! - [`client`]: the renderer side without drawing. It
//!   keeps the element tree and signals, evaluates CEL expressions and yields
//!   elements with resolved attribute values.
//! - [`vtree`]: build the interface as a tree from your
//!   data, and send it, or only what changed since the previous tree.

pub mod component;
pub mod message;
pub mod server;
pub mod signals;
pub mod client;
pub mod vtree;

pub use component::Component;
pub use message::{
    cel, is_cel, ui, Action, Element, Handler, LineBuffer, Log, LogType, Metadata, ParseError,
    RendererMessage, ServerMessage,
};
pub use serde_json::{self, json, Map, Value};
pub use server::{App, Outbox, Server};
