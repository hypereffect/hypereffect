"""Types and helpers for `hypereffect <https://hypereffect.dev>`_, a small
hypermedia protocol for server-driven UI.

- Elements, handlers, server and renderer messages, and the line framing.
  Building an interface looks like
  ``ui.text("title").parent("main").variant("h1").text("Inbox")``.
- :mod:`hypereffect.signals`: merge patch, local signals and signal paths.
- The components, their attributes and nesting rules.
- An :class:`App` base class for per-session logic, and a :class:`Server`
  that drives one from any transport.
- :mod:`hypereffect.client`: the renderer side without drawing, and a
  ``Loopback`` that connects it to a Server for tests.
"""

from . import signals, ui
from .components import Attr, AttrType, Component, accepts
from .message import (
    Action,
    Element,
    Handler,
    Hello,
    Json,
    LineBuffer,
    Log,
    LogType,
    Metadata,
    ParseError,
    ParseErrorKind,
    RendererMessage,
    Remove,
    ServerMessage,
    Signal,
    cel,
    encode_line,
    is_cel,
    parse_line,
    parse_renderer_message,
    parse_server_message,
)
from .server import App, Outbox, Server

__all__ = [
    "Action", "App", "Attr", "AttrType", "Component", "Element", "Handler", "Hello", "Json",
    "LineBuffer", "Log", "LogType", "Metadata", "Outbox", "ParseError", "ParseErrorKind",
    "RendererMessage",
    "Remove", "Server", "ServerMessage", "Signal", "accepts", "cel", "encode_line", "is_cel",
    "parse_line", "parse_renderer_message", "parse_server_message", "signals", "ui",
]
