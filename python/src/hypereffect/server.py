"""Writing servers: an outbox for messages, an :class:`App` base class for
per-session logic, and a :class:`Server` that drives an app over a stream."""

from __future__ import annotations

from collections.abc import Callable
from typing import Generic, TypeVar

from .message import Action, Element, Hello, Json, LineBuffer, Log, Metadata, ParseError, Remove, \
    ServerMessage, Signal, encode_line, parse_renderer_message


class Outbox:
    """Collects messages to send to the renderer, as lines."""

    def __init__(self) -> None:
        self._out: list[str] = []

    def send(self, m: ServerMessage) -> None:
        self._out.append(m.to_line())

    def element(self, e: Element) -> None:
        self._out.append(e.to_line())

    def remove(self, id: str) -> None:
        self.send(Remove(id))

    def signal(self, patch: dict[str, Json]) -> None:
        """Sends a signal message: a merge patch for the signals."""
        if not isinstance(patch, dict):
            raise TypeError("a signal message must be an object")
        self._out.append(encode_line("signal", patch))

    def metadata(self, m: Metadata) -> None:
        """Sends optional details about the application, replacing any sent
        before."""
        self.send(m)

    def log(self, log: Log) -> None:
        """Sends an event for the user or the host's log. It never changes the
        element tree or the signals."""
        self.send(log)

    def is_empty(self) -> bool:
        return not self._out

    def __str__(self) -> str:
        """The lines written so far."""
        return "".join(self._out)

    def take(self) -> str:
        """Takes the lines written so far."""
        s = str(self)
        self._out.clear()
        return s


class App:
    """The logic of one session. A new app is created for every hello, and
    dropped when the session ends."""

    def hello(self, out: Outbox, params: dict[str, str]) -> None:
        """Called on hello. The app should build the initial interface."""
        raise NotImplementedError

    def action(self, out: Outbox, action: Action) -> None:
        """Called for each action. The action's contents are user input."""
        raise NotImplementedError

    def tick(self, out: Outbox, now_ms: float) -> None:
        """Called periodically if :meth:`tick_interval` returns a value, with
        the host's clock in milliseconds."""

    def tick_interval(self) -> int | None:
        """How often the app wants :meth:`tick` called, in milliseconds."""
        return None


A = TypeVar("A", bound=App)


class Server(Generic[A]):
    """Drives an :class:`App` from what a renderer sends. Connect it to any
    transport that carries lines: pass what the renderer sends to
    :meth:`receive`, and send what :attr:`output` holds after each call.

    ``new_app`` is called on every hello; it is usually the app's class.
    """

    def __init__(self, new_app: Callable[[], A]) -> None:
        self._new_app = new_app
        self._app: A | None = None
        self._input = LineBuffer()
        self.output = Outbox()

    def receive(self, data: bytes | str) -> None:
        """Feeds bytes or text received from the renderer. Messages before
        hello, and lines that cannot be parsed, are ignored."""
        for line in self._input.push(data):
            try:
                m = parse_renderer_message(line)
            except ParseError:
                continue
            if isinstance(m, Hello):
                self._app = self._new_app()
                self._app.hello(self.output, m.params)
            elif self._app is not None:
                self._app.action(self.output, m)

    def tick(self, now_ms: float) -> None:
        if self._app is not None:
            self._app.tick(self.output, now_ms)

    def tick_interval(self) -> int | None:
        return self._app.tick_interval() if self._app is not None else None

    def close(self) -> None:
        """Ends the session."""
        self._app = None
        self._input = LineBuffer()

    @property
    def app(self) -> A | None:
        """The session's app, if a session is open."""
        return self._app


__all__ = ["Outbox", "App", "Server"]
