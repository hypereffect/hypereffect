"""Messages, elements and handlers, and the line framing."""

from __future__ import annotations

import json
from collections.abc import Mapping
from dataclasses import dataclass, field
from enum import StrEnum
from typing import Any, Self, TypeAlias

from .components import Component
from .signals import get_path, parse_path

Json: TypeAlias = "None | bool | int | float | str | list[Json] | dict[str, Json]"


def cel(expr: str) -> dict[str, Json]:
    """An expression attribute value: ``{"cel": "..."}``."""
    return {"cel": expr}


def is_cel(v: object) -> str | None:
    """The source of ``v`` if it is an expression: an object whose only
    member is ``cel``, a string."""
    if isinstance(v, dict) and len(v) == 1 and isinstance(v.get("cel"), str):
        return v["cel"]  # type: ignore[no-any-return]
    return None


@dataclass(frozen=True)
class Handler:
    """A handler: a local merge ``patch`` (leaf values may be expressions), a
    URL to ``open`` (a string or an expression; buttons only), an
    ``action`` sent to the server with an optional ``payload``, or any
    combination. ``indicator`` is a local signal path set to true when the
    action is sent.

    ``Handler(action="add", payload=1, indicator="_busy")``
    """

    action: str | None = None
    patch: dict[str, Json] | None = None
    payload: Json = None
    indicator: str | None = None
    open: str | dict[str, Json] | None = None

    def to_json(self) -> dict[str, Json]:
        o: dict[str, Json] = {}
        for k in ("patch", "action", "payload", "indicator", "open"):
            v = getattr(self, k)
            if v is not None:
                o[k] = v
        return o


Value: TypeAlias = "Json | Handler"


class Element:
    """An element: one instance of a component. Setters return the element,
    so an interface is built as

        ui.text("title").parent("main").variant("h1").text("Inbox")

    Attribute values are literals or expressions from :func:`cel`.
    """

    def __init__(self, id: str, type: Component | str) -> None:
        self.id = id
        self.type = Component(type)
        self.parent_id: str | None = None
        self.before_id: str | None = None
        # Attributes in the order they were set.
        self.attrs: dict[str, Value] = {}

    def __repr__(self) -> str:
        return f"Element({self.to_json()!r})"

    def __eq__(self, other: object) -> bool:
        return isinstance(other, Element) and self.to_json() == other.to_json()

    def parent(self, id: str) -> Self:
        self.parent_id = id
        return self

    def before(self, id: str) -> Self:
        self.before_id = id
        return self

    def set(self, name: str, v: Value) -> Self:
        """Sets any attribute."""
        self.attrs[name] = v
        return self

    def visible(self, v: Json) -> Self: return self.set("visible", v)
    def disabled(self, v: Json) -> Self: return self.set("disabled", v)
    def gap(self, v: Json) -> Self: return self.set("gap", v)
    def align(self, v: Json) -> Self: return self.set("align", v)
    def justify(self, v: Json) -> Self: return self.set("justify", v)
    def wrap(self, v: Json) -> Self: return self.set("wrap", v)
    def title(self, v: Json) -> Self: return self.set("title", v)
    def empty(self, v: Json) -> Self: return self.set("empty", v)
    def label(self, v: Json) -> Self: return self.set("label", v)
    def open(self, v: Json) -> Self: return self.set("open", v)
    def value(self, v: Json) -> Self: return self.set("value", v)
    def variant(self, v: Json) -> Self: return self.set("variant", v)
    def src(self, v: Json) -> Self: return self.set("src", v)
    def alt(self, v: Json) -> Self: return self.set("alt", v)
    def placeholder(self, v: Json) -> Self: return self.set("placeholder", v)
    def error(self, v: Json) -> Self: return self.set("error", v)
    def options(self, v: Json) -> Self: return self.set("options", v)
    def min(self, v: Json) -> Self: return self.set("min", v)
    def max(self, v: Json) -> Self: return self.set("max", v)
    def step(self, v: Json) -> Self: return self.set("step", v)
    def text(self, v: Json) -> Self: return self.set("text", v)

    def bind(self, path: str) -> Self:
        """Sets ``bind``, the signal path a control reads and writes."""
        return self.set("bind", path)

    def on_press(self, h: Handler) -> Self: return self.set("onPress", h)
    def on_change(self, h: Handler) -> Self: return self.set("onChange", h)
    def on_submit(self, h: Handler) -> Self: return self.set("onSubmit", h)
    def on_dismiss(self, h: Handler) -> Self: return self.set("onDismiss", h)

    tag = "element"

    def to_json(self) -> dict[str, Json]:
        o: dict[str, Json] = {"id": self.id, "type": self.type.value}
        if self.parent_id is not None:
            o["parent"] = self.parent_id
        if self.before_id is not None:
            o["before"] = self.before_id
        for k, v in self.attrs.items():
            o[k] = v.to_json() if isinstance(v, Handler) else v
        return o

    def to_line(self) -> str:
        return encode_line("element", self.to_json())

    @classmethod
    def from_json(cls, body: dict[str, Json]) -> Element:
        """Reads an element from its JSON object. Raises ParseError."""
        body = dict(body)
        id, type = body.pop("id", None), body.pop("type", None)
        if not isinstance(id, str):
            raise ParseError(ParseErrorKind.INVALID, "element needs a string id")
        if type not in Component._value2member_map_:
            raise ParseError(ParseErrorKind.INVALID, f"unknown type {json.dumps(type)}")
        e = cls(id, str(type))
        for k in ("parent", "before"):
            v = body.pop(k, None)
            if v is not None and not isinstance(v, str):
                raise ParseError(ParseErrorKind.INVALID, f"{k} must be a string")
            setattr(e, f"{k}_id", v)
        e.attrs = dict(body)
        return e


@dataclass(frozen=True)
class Remove:
    """Removes an element and its descendants."""

    id: str
    tag = "remove"

    def to_line(self) -> str:
        return encode_line("remove", {"id": self.id})


@dataclass(frozen=True)
class Signal:
    """A merge patch for the signals."""

    patch: dict[str, Json]
    tag = "signal"

    def to_line(self) -> str:
        return encode_line("signal", self.patch)


@dataclass(frozen=True)
class Metadata:
    """Optional details about the application, sent by the server. A
    renderer is free to ignore all of them."""

    name: str | None = None
    description: str | None = None
    icon: str | None = None
    # The other string members: extensions defined by a protocol built on
    # hypereffect, which renderers that do not know them ignore. Sent as
    # members of the metadata object, next to the ones above.
    extra: Mapping[str, str] = field(default_factory=dict)
    tag = "metadata"

    _KNOWN = ("name", "description", "icon")

    def __post_init__(self) -> None:
        for k in self.extra:
            if k in self._KNOWN:
                raise ValueError(f"{k} is not an extension member")

    def to_json(self) -> dict[str, Json]:
        known = {k: v for k, v in (("name", self.name), ("description", self.description),
                                   ("icon", self.icon)) if v is not None}
        return {**known, **self.extra}

    def to_line(self) -> str:
        return encode_line("metadata", self.to_json())

    @classmethod
    def from_json(cls, body: dict[str, Json]) -> Metadata:
        """Reads the members that are strings and drops everything else."""
        def get(k: str) -> str | None:
            v = body.get(k)
            return v if isinstance(v, str) else None
        extra = {k: v for k, v in body.items() if k not in cls._KNOWN and isinstance(v, str)}
        return cls(get("name"), get("description"), get("icon"), extra)


ServerMessage: TypeAlias = "Element | Remove | Signal | Metadata"


def parse_server_message(line: str) -> ServerMessage:
    """Parses a line (without its line feed). Raises ParseError."""
    tag, body = parse_line(line)
    return server_message_from_parts(tag, body)


def server_message_from_parts(tag: str, body: dict[str, Json]) -> ServerMessage:
    if tag == "element":
        return Element.from_json(body)
    if tag == "remove":
        id = body.get("id")
        if not isinstance(id, str):
            raise ParseError(ParseErrorKind.INVALID, "remove needs a string id")
        return Remove(id)
    if tag == "signal":
        return Signal(body)
    if tag == "metadata":
        return Metadata.from_json(body)
    raise ParseError(ParseErrorKind.UNKNOWN_TAG, tag)


@dataclass(frozen=True)
class Hello:
    """Opens a session. The params are chosen by whoever opened it (for
    example, taken from the address) and are strings only."""

    params: dict[str, str] = field(default_factory=dict)

    def to_line(self) -> str:
        return encode_line("hello", {"params": self.params} if self.params else {})


@dataclass
class Action:
    """An action, sent by a handler. Its contents are user input."""

    name: str
    payload: Json = None
    signals: dict[str, Json] = field(default_factory=dict)

    def signal(self, path: str, default: Json = None) -> Json:
        """A signal by path, such as ``form.email``."""
        segs = parse_path(path)
        return default if segs is None else get_path(self.signals, segs, default)

    def to_json(self) -> dict[str, Json]:
        o: dict[str, Json] = {"name": self.name}
        if self.payload is not None:
            o["payload"] = self.payload
        o["signals"] = self.signals
        return o

    def to_line(self) -> str:
        return encode_line("action", self.to_json())


RendererMessage: TypeAlias = "Hello | Action"


def parse_renderer_message(line: str) -> RendererMessage:
    """Parses a line (without its line feed). Raises ParseError."""
    tag, body = parse_line(line)
    if tag == "hello":
        params = body.get("params", {})
        if not isinstance(params, dict):
            raise ParseError(ParseErrorKind.INVALID, "params: not an object")
        for k, v in params.items():
            if not isinstance(v, str):
                raise ParseError(ParseErrorKind.INVALID, f"params: {k} is not a string")
        return Hello(params)  # type: ignore[arg-type]
    if tag == "action":
        name, signals = body.get("name"), body.get("signals", {})
        if not isinstance(name, str):
            raise ParseError(ParseErrorKind.INVALID, "action needs a string name")
        if not isinstance(signals, dict):
            raise ParseError(ParseErrorKind.INVALID, "signals: not an object")
        return Action(name, body.get("payload"), signals)
    raise ParseError(ParseErrorKind.UNKNOWN_TAG, tag)


class ParseErrorKind(StrEnum):
    GRAMMAR = "grammar"  # the line does not match `tag SP json`
    JSON = "json"
    NOT_AN_OBJECT = "not-an-object"
    UNKNOWN_TAG = "unknown-tag"
    INVALID = "invalid"  # the JSON does not have the members the message needs


_MESSAGES = {
    ParseErrorKind.GRAMMAR: "line does not match the grammar",
    ParseErrorKind.JSON: "invalid JSON: {}",
    ParseErrorKind.NOT_AN_OBJECT: "message body is not a JSON object",
    ParseErrorKind.UNKNOWN_TAG: "unknown tag '{}'",
    ParseErrorKind.INVALID: "invalid message: {}",
}


class ParseError(ValueError):
    def __init__(self, kind: ParseErrorKind, detail: str = "") -> None:
        super().__init__(_MESSAGES[kind].format(detail))
        self.kind = kind


def _default(o: Any) -> Json:
    if hasattr(o, "to_json"):
        return o.to_json()  # type: ignore[no-any-return]
    raise TypeError(f"{type(o).__name__} is not JSON serializable")


def encode_line(tag: str, body: object) -> str:
    """Encodes a message as a line, including the final line feed."""
    return f"{tag} {json.dumps(body, separators=(',', ':'), ensure_ascii=False, allow_nan=False, default=_default)}\n"


def _no_constants(name: str) -> None:
    raise ValueError(f"{name} is not JSON")


def parse_line(line: str) -> tuple[str, dict[str, Json]]:
    """Splits a line (without its line feed) into its tag and JSON object.
    Raises ParseError."""
    line = line.removesuffix("\r")
    tag, sep, body = line.partition(" ")
    if not sep or not tag or not all("a" <= c <= "z" for c in tag):
        raise ParseError(ParseErrorKind.GRAMMAR)
    try:
        v = json.loads(body, parse_constant=_no_constants)
    except ValueError as e:
        raise ParseError(ParseErrorKind.JSON, str(e)) from None
    if not isinstance(v, dict):
        raise ParseError(ParseErrorKind.NOT_AN_OBJECT)
    return tag, v


class LineBuffer:
    """Splits a stream into lines. Empty lines are dropped and a carriage
    return before the line feed is ignored."""

    def __init__(self) -> None:
        self._buf = b""

    def push(self, data: bytes | str) -> list[str]:
        """Adds bytes or text and returns the complete lines they finish.
        Lines that are not valid UTF-8 are dropped."""
        if isinstance(data, str):
            data = data.encode()
        *lines, self._buf = (self._buf + data).split(b"\n")
        out = []
        for line in lines:
            line = line.removesuffix(b"\r")
            if line:
                try:
                    out.append(line.decode())
                except UnicodeDecodeError:
                    pass
        return out
