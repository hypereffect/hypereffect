//! Writing servers: an outbox for messages, an [`App`] trait for per-session
//! logic, a [`Server`] that drives an app over a byte stream, and
//! [`wasm_app!`](crate::wasm_app), which exports a server from a WebAssembly
//! module.

use crate::message::{Action, Element, LineBuffer, Log, Metadata, RendererMessage, ServerMessage};
use serde_json::Value;
use std::collections::BTreeMap;

/// Collects messages to send to the renderer, as lines.
#[derive(Debug, Default)]
pub struct Outbox {
    out: String,
}

impl Outbox {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn send(&mut self, m: impl Into<ServerMessage>) {
        self.out.push_str(&m.into().to_line());
    }

    pub fn element(&mut self, e: Element) {
        self.out.push_str(&e.to_line());
    }

    pub fn remove(&mut self, id: impl Into<String>) {
        self.send(ServerMessage::remove(id));
    }

    /// Sends a signal message. `patch` must be a JSON object.
    pub fn signal(&mut self, patch: Value) {
        self.send(ServerMessage::signal(patch));
    }

    /// Sends optional details about the application, replacing any sent before.
    pub fn metadata(&mut self, m: Metadata) {
        self.send(m);
    }

    /// Sends an event for the user or the host's log. It never changes the
    /// element tree or the signals.
    pub fn log(&mut self, log: Log) {
        self.send(log);
    }

    pub fn is_empty(&self) -> bool {
        self.out.is_empty()
    }

    /// The lines written so far.
    pub fn as_str(&self) -> &str {
        &self.out
    }

    /// Takes the lines written so far.
    pub fn take(&mut self) -> String {
        std::mem::take(&mut self.out)
    }
}

impl<M: Into<ServerMessage>> Extend<M> for Outbox {
    fn extend<I: IntoIterator<Item = M>>(&mut self, iter: I) {
        for m in iter {
            self.send(m);
        }
    }
}

/// The logic of one session. A new app is created (with [`Default`]) for
/// every hello, and dropped when the session ends.
pub trait App {
    /// Called on hello. The app should build the initial interface.
    fn hello(&mut self, out: &mut Outbox, params: &BTreeMap<String, String>);

    /// Called for each action. The action's contents are user input.
    fn action(&mut self, out: &mut Outbox, action: &Action);

    /// Called periodically if [`App::tick_interval`] returns a value, with
    /// the host's clock in milliseconds.
    fn tick(&mut self, _out: &mut Outbox, _now_ms: f64) {}

    /// How often the app wants [`App::tick`] called, in milliseconds.
    fn tick_interval(&self) -> Option<u32> {
        None
    }
}

/// Drives an [`App`] from the lines a renderer sends.
pub struct Server<A> {
    app: Option<A>,
    input: LineBuffer,
    out: Outbox,
}

impl<A: App + Default> Default for Server<A> {
    fn default() -> Self {
        Server {
            app: None,
            input: LineBuffer::new(),
            out: Outbox::new(),
        }
    }
}

impl<A: App + Default> Server<A> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds bytes received from the renderer. Messages before hello, and
    /// lines that cannot be parsed, are ignored.
    pub fn receive(&mut self, bytes: &[u8]) {
        for line in self.input.push(bytes) {
            match RendererMessage::parse(&line) {
                Ok(RendererMessage::Hello { params }) => {
                    let mut app = A::default();
                    app.hello(&mut self.out, &params);
                    self.app = Some(app);
                }
                Ok(RendererMessage::Action(a)) => {
                    if let Some(app) = &mut self.app {
                        app.action(&mut self.out, &a);
                    }
                }
                Err(_) => {}
            }
        }
    }

    pub fn tick(&mut self, now_ms: f64) {
        if let Some(app) = &mut self.app {
            app.tick(&mut self.out, now_ms);
        }
    }

    pub fn tick_interval(&self) -> Option<u32> {
        self.app.as_ref()?.tick_interval()
    }

    /// Ends the session.
    pub fn close(&mut self) {
        self.app = None;
        self.input = LineBuffer::new();
    }

    pub fn app(&self) -> Option<&A> {
        self.app.as_ref()
    }

    /// The output waiting to be sent.
    pub fn output(&mut self) -> &mut Outbox {
        &mut self.out
    }
}

/// Exports an [`App`] as a hypereffect server from a WebAssembly module, for
/// a host that acts as the transport. The type must implement [`App`] and
/// [`Default`].
///
/// ```ignore
/// hypereffect::wasm_app!(Counter);
/// ```
///
/// The module imports nothing and exports:
///
/// - `he_buffer(len: u32) -> *mut u8`: a buffer of `len` bytes for input.
/// - `he_receive(len: u32)`: process the first `len` bytes of the buffer, as
///   bytes from the renderer.
/// - `he_tick(now_ms: f64)`: run the app's timer.
/// - `he_tick_interval() -> i32`: the timer interval in milliseconds, or -1.
/// - `he_close()`: the transport closed.
/// - `he_output() -> *const u8`, `he_output_len() -> u32`: lines waiting to be
///   sent to the renderer, valid until the next call into the module.
/// - `he_output_clear()`: the host has read the output.
#[macro_export]
macro_rules! wasm_app {
    ($app:ty) => {
        mod __hypereffect_wasm {
            use super::*;
            use std::cell::RefCell;

            struct State {
                server: $crate::server::Server<$app>,
                input: Vec<u8>,
            }

            thread_local! {
                static STATE: RefCell<State> = RefCell::new(State {
                    server: $crate::server::Server::new(),
                    input: Vec::new(),
                });
            }

            #[no_mangle]
            pub extern "C" fn he_buffer(len: u32) -> *mut u8 {
                STATE.with_borrow_mut(|s| {
                    s.input.resize(len as usize, 0);
                    s.input.as_mut_ptr()
                })
            }

            #[no_mangle]
            pub extern "C" fn he_receive(len: u32) {
                STATE.with_borrow_mut(|s| {
                    let State { server, input } = s;
                    server.receive(&input[..(len as usize).min(input.len())]);
                })
            }

            #[no_mangle]
            pub extern "C" fn he_tick(now_ms: f64) {
                STATE.with_borrow_mut(|s| s.server.tick(now_ms))
            }

            #[no_mangle]
            pub extern "C" fn he_tick_interval() -> i32 {
                STATE.with_borrow(|s| s.server.tick_interval().map_or(-1, |i| i as i32))
            }

            #[no_mangle]
            pub extern "C" fn he_close() {
                STATE.with_borrow_mut(|s| s.server.close())
            }

            #[no_mangle]
            pub extern "C" fn he_output() -> *const u8 {
                STATE.with_borrow_mut(|s| s.server.output().as_str().as_ptr())
            }

            #[no_mangle]
            pub extern "C" fn he_output_len() -> u32 {
                STATE.with_borrow_mut(|s| s.server.output().as_str().len() as u32)
            }

            #[no_mangle]
            pub extern "C" fn he_output_clear() {
                STATE.with_borrow_mut(|s| {
                    s.server.output().take();
                })
            }
        }
    };
}

/// Connects a [`Client`](crate::client::Client) to a [`Server`] in the same
/// process, for testing an app without a renderer.
///
/// ```
/// use hypereffect::server::{App, Loopback, Outbox};
/// use hypereffect::{json, ui, Action, Handler};
/// use std::collections::BTreeMap;
///
/// #[derive(Default)]
/// struct Hi;
/// impl App for Hi {
///     fn hello(&mut self, out: &mut Outbox, _: &BTreeMap<String, String>) {
///         out.element(ui::button("b").on_press(Handler::action("hi")));
///     }
///     fn action(&mut self, out: &mut Outbox, _: &Action) {
///         out.element(ui::text("t").text("hello"));
///     }
/// }
///
/// let mut l = Loopback::<Hi>::new();
/// assert!(l.trigger("b", "onPress"));
/// assert_eq!(l.client.attr("t", "text"), Some(json!("hello")));
/// assert!(l.client.take_diagnostics().is_empty());
/// ```
pub struct Loopback<A> {
    pub client: crate::client::Client,
    pub server: Server<A>,
}

impl<A: App + Default> Default for Loopback<A> {
    fn default() -> Self {
        Self::new()
    }
}

impl<A: App + Default> Loopback<A> {
    /// Starts a session.
    pub fn new() -> Self {
        Self::with_params(&BTreeMap::new())
    }

    /// Starts a session whose hello carries these params.
    pub fn with_params(params: &BTreeMap<String, String>) -> Self {
        let mut l = Loopback {
            client: crate::client::Client::new(),
            server: Server::new(),
        };
        let hello = l.client.hello_with(params);
        l.server.receive(hello.as_bytes());
        l.pump();
        l
    }

    /// Delivers the server's pending output to the client.
    pub fn pump(&mut self) {
        let out = self.server.output().take();
        self.client.receive(out.as_bytes());
    }

    /// Triggers a handler, delivers the action (if any) and the response.
    /// Returns whether an action was sent.
    pub fn trigger(&mut self, id: &str, event: &str) -> bool {
        let Some(line) = self.client.trigger(id, event) else {
            return false;
        };
        self.server.receive(line.as_bytes());
        self.pump();
        true
    }

    /// Writes to a bound signal, as a control does.
    pub fn write(&mut self, path: &str, value: Value) {
        self.client.write(path, value);
    }

    pub fn tick(&mut self, now_ms: f64) {
        self.server.tick(now_ms);
        self.pump();
    }
}
