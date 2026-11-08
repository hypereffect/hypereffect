"""The renderer side of a session, without any drawing.

A :class:`Client` keeps the element tree and the signals, evaluates CEL
expressions with cel-python, runs handlers, and yields elements whose
attributes are resolved to plain JSON values. A UI layer reads the resolved
elements and draws them; :meth:`Client.take_changes` says which ones to
redraw.

A :class:`Loopback` connects a Client to a :class:`~hypereffect.Server` in
the same process, for testing an app without a renderer::

    l = Loopback(Counter)
    l.trigger("inc", "onPress")
    assert l.client.attr("value", "text") == "Count: 1"
    assert l.client.take_diagnostics() == []
"""

from __future__ import annotations

import copy
import json
import math
from collections.abc import Callable
from dataclasses import dataclass, field
from typing import Any, Generic, TypeVar

from celpy import celtypes as t

from . import _cel
from ._cel import CelError, Program, from_json, to_json, type_name
from .components import AttrType, Component, accepts
from .message import Action, Hello, Json, LineBuffer, Log, Metadata, ServerMessage, is_cel, parse_line
from .server import App, Server
from .signals import get_path, has_path, merge_patch_tracked, parse_path, path_patch, strip_local

ROOT = ""
_STRUCTURAL = {"id", "type", "parent", "before"}


@dataclass
class Resolved:
    """An element with its attributes resolved: expressions evaluated, values
    checked against the attribute types, and defaults filled in. Structural
    attributes (``bind`` and handlers) are included as they were sent, or
    None."""

    id: str
    type: Component
    parent: str | None
    children: list[str]
    attrs: dict[str, Json]


@dataclass
class ResolvedTree:
    """A resolved element with its resolved descendants."""

    id: str
    type: Component
    attrs: dict[str, Json]
    children: list[ResolvedTree]


@dataclass
class Changes:
    """What changed since the last :meth:`Client.take_changes`."""

    # The tree was discarded because a new session began.
    reset: bool = False
    # The connected state changed.
    connection: bool = False
    # Elements whose widgets must be built anew: new, or with a new type.
    created: set[str] = field(default_factory=set)
    # Elements whose resolved attributes may have changed.
    updated: set[str] = field(default_factory=set)
    removed: set[str] = field(default_factory=set)
    # Elements whose list of children changed ("" for the root).
    reordered: set[str] = field(default_factory=set)
    # The application metadata changed.
    metadata: bool = False
    _signals: bool = False

    def is_empty(self) -> bool:
        return not (self.reset or self.connection or self.created or self.updated
                    or self.removed or self.reordered or self.metadata)


@dataclass
class _Node:
    type: Component
    parent: str | None
    el: dict[str, Json]
    children: list[str] = field(default_factory=list)
    progs: dict[str, Program] = field(default_factory=dict)
    # Whether any attribute reads signals.
    reads_signals: bool = False


class _CheckError(Exception):
    pass


class Client:
    def __init__(self) -> None:
        self._nodes: dict[str, _Node] = {}
        self._root: list[str] = []
        self._signals: dict[str, Json] = {}
        self._metadata = Metadata()
        self._cel_signals: Any = None
        self._connected = False
        self._reset_pending = False
        self._reader = LineBuffer()
        self._changes = Changes()
        self._diagnostics: list[str] = []
        self._opens: list[str] = []
        self._logs: list[Log] = []

    # --- transport ---------------------------------------------------------

    def hello(self, params: dict[str, str] | None = None) -> str:
        """Begins a session and returns the hello line to send, with params
        for the app: strings chosen by whoever opened the session, such as
        the ones in its address. The previous tree is kept until the first
        server message of the new session."""
        self._reader = LineBuffer()
        self._connected = True
        self._reset_pending = True
        line = Hello(dict(params or {})).to_line()
        if not self._nodes:
            self._reset()
        self._changes.connection = True
        return line

    def receive(self, data: bytes | str) -> None:
        """Feeds bytes or text received from the transport."""
        for line in self._reader.push(data):
            self.receive_line(line)

    def receive_line(self, line: str) -> None:
        try:
            tag, body = parse_line(line)
        except ValueError as e:
            self._diag(f"ignored line: {e}")
            return
        self.receive_message(tag, body)

    def receive_message(self, tag: str, body: dict[str, Json]) -> None:
        """Processes a message given as a tag and its JSON object."""
        if self._reset_pending:
            self._reset()
        if tag == "element":
            self._element(body)
        elif tag == "remove":
            if isinstance(body.get("id"), str):
                self._remove(body["id"])  # type: ignore[arg-type]
        elif tag == "signal":
            self._patch(body)
        elif tag == "metadata":
            self._set_metadata(Metadata.from_json(body))
        elif tag == "log":
            try:
                self._logs.append(Log.from_json(body))
            except ValueError as e:
                self._diag(f"ignored log: {e}")
        else:
            self._diag(f"ignored message with unknown tag '{tag}'")

    def apply(self, m: ServerMessage) -> None:
        """Processes a message, as the in-process transport."""
        self.receive_line(m.to_line().removesuffix("\n"))

    def close(self) -> None:
        """Marks the transport closed. The tree is kept for display."""
        self._connected = False
        self._changes.connection = True

    @property
    def connected(self) -> bool:
        return self._connected

    # --- reading -----------------------------------------------------------

    def children(self, id: str | None = None) -> list[str]:
        """The children of ``id``, or of the root."""
        if id is None or id == ROOT:
            return list(self._root)
        n = self._nodes.get(id)
        return list(n.children) if n else []

    def __contains__(self, id: str) -> bool:
        return id in self._nodes

    def contains(self, id: str) -> bool:
        return id in self._nodes

    def get(self, id: str) -> Resolved | None:
        n = self._nodes.get(id)
        if n is None:
            return None
        attrs = {a.name: self.attr(id, a.name) for a in n.type.attrs}
        return Resolved(id, n.type, n.parent, list(n.children), attrs)

    def attr(self, id: str, name: str, default: Any = None) -> Json:
        """One resolved attribute, or ``default`` if the element or attribute
        does not exist."""
        n = self._nodes.get(id)
        spec = n.type.attr(name) if n else None
        if n is None or spec is None:
            return default  # type: ignore[no-any-return]
        if spec.type.is_structural:
            return n.el.get(name)
        dflt = spec.default_value(id)
        raw = n.el.get(name)
        if raw is None:
            return dflt  # type: ignore[no-any-return]
        if is_cel(raw) is not None:
            prog = n.progs.get(name)
            if prog is None:
                return dflt  # type: ignore[no-any-return]
            try:
                value = prog.eval(self._cel())
            except CelError as e:
                self._diag(f"{id}.{name}: {e}")
                return dflt  # type: ignore[no-any-return]
        else:
            value = from_json(raw)
        try:
            return _check(spec.type, value, dflt)
        except _CheckError as e:
            self._diag(f"{id}.{name}: {e}")
            return dflt  # type: ignore[no-any-return]

    def tree(self) -> list[ResolvedTree]:
        """The whole tree, resolved."""
        return self._tree_of(self._root)

    def _tree_of(self, ids: list[str]) -> list[ResolvedTree]:
        out = []
        for id in ids:
            r = self.get(id)
            if r is not None:
                out.append(ResolvedTree(id, r.type, r.attrs, self._tree_of(r.children)))
        return out

    @property
    def metadata(self) -> Metadata:
        """The details the server sent about the application, if any."""
        return self._metadata

    @property
    def signals(self) -> dict[str, Json]:
        return self._signals

    def value(self, path: str, default: Json = None) -> Json:
        """The value at a signal path such as ``form.email``."""
        segs = parse_path(path)
        return default if segs is None else get_path(self._signals, segs, default)

    def has_value(self, path: str) -> bool:
        segs = parse_path(path)
        return segs is not None and has_path(self._signals, segs)

    # --- user input --------------------------------------------------------

    def write(self, path: str, value: Json) -> None:
        """Writes a value at a signal path, as a control with ``bind`` does."""
        segs = parse_path(path)
        if segs is not None:
            self._patch(path_patch(segs, copy.deepcopy(value)))

    def trigger(self, id: str, event: str) -> str | None:
        """Runs the handler in attribute ``event`` of element ``id``,
        following section 10 of the spec. Returns the action line to send,
        if any. A URL to open is queued for :meth:`take_opens`."""
        n = self._nodes.get(id)
        spec = n.type.attr(event) if n else None
        if n is None or spec is None or spec.type.kind != "handler":
            return None
        h = n.el.get(event)
        if not isinstance(h, dict):
            return None
        cur: str | None = id
        while cur is not None:
            if self.attr(cur, "visible") is not True:
                return None
            if self.attr(cur, "disabled") is True:
                return None
            cur = self._nodes[cur].parent
        if isinstance(h.get("patch"), dict):
            self._patch(self._eval_patch(h["patch"], f"{id}.{event}"))  # type: ignore[arg-type]
        if "open" in h:
            url = self._eval_open(h["open"], f"{id}.{event}")
            if url is not None:
                self._opens.append(url)
        action = h.get("action")
        if not isinstance(action, str) or not self._connected:
            return None
        ind = h.get("indicator")
        segs = parse_path(ind) if isinstance(ind, str) else None
        if segs is not None:
            self._patch(path_patch(segs, True))
        signals = strip_local(self._signals)
        return Action(action, h.get("payload"), signals).to_line()  # type: ignore[arg-type]

    # --- changes -----------------------------------------------------------

    def take_changes(self) -> Changes:
        """Takes what changed since the last call."""
        c, self._changes = self._changes, Changes()
        if c._signals:
            c.updated.update(id for id, n in self._nodes.items() if n.reads_signals)
        # Tabs and lists present facts about their children.
        list_like = (Component.TABS, Component.LIST)
        for id in [*c.updated, *c.reordered]:
            n = self._nodes.get(id)
            if n is None:
                continue
            if n.type in list_like:
                c.updated.add(id)
            if n.parent is not None and self._nodes[n.parent].type in list_like:
                c.updated.add(n.parent)
        c.updated -= c.removed
        c.created -= c.removed
        c._signals = False
        return c

    def take_diagnostics(self) -> list[str]:
        """Takes the diagnostics reported since the last call."""
        d, self._diagnostics = self._diagnostics, []
        return d

    def take_opens(self) -> list[str]:
        """Returns and clears the URLs that handlers asked to open (a
        button's ``open``), oldest first. The host decides whether and how
        to open them."""
        o, self._opens = self._opens, []
        return o

    def take_logs(self) -> list[Log]:
        """Returns and clears the log messages received since the last call,
        oldest first. The host decides whether and how to show them."""
        logs, self._logs = self._logs, []
        return logs

    # --- internals ---------------------------------------------------------

    def _diag(self, msg: str) -> None:
        self._diagnostics.append(msg)

    def _set_metadata(self, m: Metadata) -> None:
        if m != self._metadata:
            self._metadata = m
            self._changes.metadata = True

    def _reset(self) -> None:
        self._reset_pending = False
        self._set_metadata(Metadata())
        self._changes.removed.update(self._nodes)
        self._nodes.clear()
        self._root = []
        self._signals = {}
        self._cel_signals = None
        self._changes.reset = True
        self._changes._signals = True
        self._changes.reordered.add(ROOT)

    def _cel(self) -> Any:
        if self._cel_signals is None:
            self._cel_signals = from_json(self._signals)
        return self._cel_signals

    def _patch(self, patch: Json) -> None:
        self._signals = merge_patch_tracked(self._signals, patch, [], [])  # type: ignore[assignment]
        self._cel_signals = None
        self._changes._signals = True

    def _eval_patch(self, patch: dict[str, Json], at: str) -> dict[str, Json]:
        out: dict[str, Json] = {}
        for k, v in patch.items():
            src = is_cel(v)
            if src is not None:
                try:
                    out[k] = to_json(_cel.compile(src).eval(self._cel()))
                except CelError as e:
                    self._diag(f"{at}.patch.{k}: {e}")
            elif isinstance(v, dict):
                out[k] = self._eval_patch(v, f"{at}.{k}")
            else:
                out[k] = v
        return out

    def _eval_open(self, open: Json, at: str) -> str | None:
        """The URL of a handler's open, or None if its expression fails or
        does not give a string."""
        src = is_cel(open)
        if src is None:
            return open if isinstance(open, str) else None
        try:
            v = _cel.compile(src).eval(self._cel())
        except CelError as e:
            self._diag(f"{at}.open: {e}")
            return None
        if isinstance(v, t.StringType):
            return str(v)
        self._diag(f"{at}.open: expected a string, got {type_name(v)}")
        return None

    def _reject(self, el: dict[str, Json], why: str) -> None:
        self._diag(f"rejected element {json.dumps(el.get('id'))}: {why}")

    def _element(self, el: dict[str, Json]) -> None:
        id = el.get("id")
        if not isinstance(id, str) or not id:
            return self._reject(el, "id must be a non-empty string")
        raw_type = el.get("type")
        try:
            type = Component(raw_type if isinstance(raw_type, str) else "")
        except ValueError:
            return self._reject(el, f"unknown type {json.dumps(el.get('type'))}")
        for k, v in el.items():
            a = type.attr(k)
            structural = k in _STRUCTURAL or (a is not None and a.type.is_structural)
            if structural and is_cel(v) is not None:
                return self._reject(el, f"{k} cannot be an expression")
        for k in ("parent", "before"):
            if k in el and not isinstance(el[k], str):
                return self._reject(el, f"{k} must be a string")
        for a in type.attrs:
            if a.name not in el:
                continue
            v = el[a.name]
            if a.type.kind == "path" and not (isinstance(v, str) and parse_path(v)):
                return self._reject(el, f"{a.name} is not a signal path")
            if a.type.kind == "handler" and not _valid_handler(v, type == Component.BUTTON):
                return self._reject(el, f"{a.name} is not a valid handler")
        parent: str | None = el.get("parent")  # type: ignore[assignment]
        parent_type = None
        if parent is not None:
            p = self._nodes.get(parent)
            if p is None:
                return self._reject(el, f'parent "{parent}" does not exist')
            parent_type = p.type
        if not accepts(parent_type, type):
            where = parent_type.value if parent_type else "the root"
            return self._reject(el, f"{where} cannot contain {type.value}")
        if id in self._nodes:
            cur = parent
            while cur is not None:
                if cur == id:
                    return self._reject(el, "an element cannot be inside itself")
                cur = self._nodes[cur].parent

        node = _Node(type, parent, el)
        for a in type.attrs:
            if a.name not in el:
                continue
            if a.type.kind == "path":
                node.reads_signals = True
            src = is_cel(el[a.name])
            if src is None:
                continue
            node.reads_signals = True
            try:
                node.progs[a.name] = _cel.compile(src)
            except CelError as e:
                self._diag(f"{id}.{a.name}: {e}")
        before: str | None = el.get("before")  # type: ignore[assignment]

        old = self._nodes.get(id)
        if old is None:
            self._nodes[id] = node
            self._insert(id, parent, before)
            self._changes.created.add(id)
            self._changes.removed.discard(id)
        else:
            if old.type == type:
                node.children = old.children
            else:
                for ch in old.children:
                    self._remove_tree(ch)
                self._changes.created.add(id)
            self._nodes[id] = node
            if old.parent != parent:
                self._child_list(old.parent).remove(id)
                self._changes.reordered.add(old.parent or ROOT)
                self._insert(id, parent, before)
            elif before is not None and before != id:
                sib = self._child_list(parent)
                if before in sib:
                    sib.remove(id)
                    sib.insert(sib.index(before), id)
                    self._changes.reordered.add(parent or ROOT)
        self._changes.updated.add(id)

    def _child_list(self, id: str | None) -> list[str]:
        return self._root if id is None else self._nodes[id].children

    def _insert(self, id: str, parent: str | None, before: str | None) -> None:
        lst = self._child_list(parent)
        if before is not None and before in lst:
            lst.insert(lst.index(before), id)
        else:
            lst.append(id)
        self._changes.reordered.add(parent or ROOT)

    def _remove(self, id: str) -> None:
        n = self._nodes.get(id)
        if n is None:
            return
        self._child_list(n.parent).remove(id)
        self._changes.reordered.add(n.parent or ROOT)
        self._remove_tree(id)

    def _remove_tree(self, id: str) -> None:
        n = self._nodes.pop(id, None)
        if n is None:
            return
        for ch in n.children:
            self._remove_tree(ch)
        c = self._changes
        c.removed.add(id)
        c.created.discard(id)
        c.updated.discard(id)
        c.reordered.discard(id)


def _valid_handler(h: Json, can_open: bool) -> bool:
    """``open`` is allowed only when ``can_open`` (on a button)."""
    if not isinstance(h, dict):
        return False
    if not ({"patch", "action", "open"} & h.keys()):
        return False
    if "open" in h:
        o = h["open"]
        if not can_open or not (is_cel(o) is not None or (isinstance(o, str) and o)):
            return False
    if "patch" in h and not isinstance(h["patch"], dict):
        return False
    if "action" in h and not (isinstance(h["action"], str) and h["action"]):
        return False
    if "indicator" in h:
        ind = h["indicator"]
        segs = parse_path(ind) if isinstance(ind, str) else None
        if "action" not in h or segs is None or not segs[0].startswith("_"):
            return False
    return True


def _check(ty: AttrType, v: Any, default: Json) -> Json:
    """Checks a CEL value against an attribute type (section 9.3 of the
    spec) and returns it as JSON. Raises _CheckError."""

    def fail(want: str) -> _CheckError:
        return _CheckError(f"expected {want}, got {type_name(v)}")

    if ty.kind == "enum":
        if not isinstance(v, t.StringType):
            raise fail("a string")
        return str(v) if v in ty.values else default
    if ty.kind == "string":
        if not isinstance(v, t.StringType):
            raise fail("a string")
        return str(v)
    if ty.kind == "bool":
        if not isinstance(v, t.BoolType):
            raise fail("a bool")
        return bool(v)
    if ty.kind in ("number", "number?"):
        if v is None and ty.kind == "number?":
            return None
        if isinstance(v, t.BoolType) or not isinstance(v, (t.IntType, t.UintType, t.DoubleType)):
            raise fail("a number")
        f = float(v)
        if not math.isfinite(f):
            raise fail("a finite number")
        return f
    if ty.kind == "options":
        if not isinstance(v, t.ListType):
            raise fail("a list")
        out: list[Json] = []
        for o in v:
            value, label = (o.get(t.StringType(k)) for k in ("value", "label")) \
                if isinstance(o, t.MapType) else (None, None)
            if not isinstance(value, t.StringType) or not isinstance(label, t.StringType):
                raise _CheckError("options must be objects with string value and label")
            out.append({"value": str(value), "label": str(label)})
        return out
    raise AssertionError("structural attributes are not checked")


A = TypeVar("A", bound=App)


class Loopback(Generic[A]):
    """Connects a :class:`Client` to a :class:`~hypereffect.Server` in the
    same process, for testing an app without a renderer. Starts a session
    whose hello carries ``params``."""

    def __init__(self, new_app: Callable[[], A], params: dict[str, str] | None = None) -> None:
        self.client = Client()
        self.server: Server[A] = Server(new_app)
        self.server.receive(self.client.hello(params))
        self.pump()

    def pump(self) -> None:
        """Delivers the server's pending output to the client."""
        self.client.receive(self.server.output.take())

    def trigger(self, id: str, event: str) -> bool:
        """Triggers a handler, delivers the action (if any) and the response.
        Returns whether an action was sent."""
        line = self.client.trigger(id, event)
        if line is None:
            return False
        self.server.receive(line)
        self.pump()
        return True

    def write(self, path: str, value: Json) -> None:
        """Writes to a bound signal, as a control does."""
        self.client.write(path, value)

    def tick(self, now_ms: float) -> None:
        self.server.tick(now_ms)
        self.pump()


__all__ = ["Client", "Loopback", "Changes", "Resolved", "ResolvedTree"]
