import json

import pytest

from hypereffect import (
    Action,
    Component,
    Handler,
    Hello,
    LineBuffer,
    Log,
    Metadata,
    ParseError,
    ParseErrorKind,
    Remove,
    accepts,
    cel,
    parse_line,
    parse_renderer_message,
    parse_server_message,
    ui,
)
from hypereffect.signals import merge_patch, parse_path, path_patch, strip_local


def test_element_round_trip() -> None:
    e = ui.text("title").parent("main").variant("h1").text("Inbox")
    line = e.to_line()
    assert line == '{"id":"title","type":"text","parent":"main","variant":"h1","text":"Inbox"}'.join(
        ["element ", "\n"]
    )
    m = parse_server_message(line.rstrip())
    assert m == e
    assert m.to_line() == line


def test_element_builders() -> None:
    e = (
        ui.button("inc")
        .parent("buttons")
        .label("+")
        .variant("primary")
        .disabled(cel("s.count >= 10"))
        .on_press(Handler(action="add", payload=1))
    )
    assert e.to_line() == (
        'element {"id":"inc","type":"button","parent":"buttons","label":"+","variant":"primary",'
        '"disabled":{"cel":"s.count >= 10"},"onPress":{"action":"add","payload":1}}\n'
    )
    assert ui.text("t").text("é <").to_line() == 'element {"id":"t","type":"text","text":"é <"}\n'


def test_handlers() -> None:
    def j(h: Handler) -> str:
        return json.dumps(h.to_json(), separators=(",", ":"))

    assert j(Handler(action="login", indicator="_busy", patch={"_open": False})) == (
        '{"patch":{"_open":false},"action":"login","indicator":"_busy"}'
    )
    assert j(Handler(open="https://example.com")) == '{"open":"https://example.com"}'
    assert j(Handler(action="visited", open=cel("'https://example.com/' + s.id"))) == (
        '{"action":"visited","open":{"cel":"\'https://example.com/\' + s.id"}}'
    )


def test_renderer_messages() -> None:
    m = parse_renderer_message('action {"name":"archive","payload":{"contact":7},"signals":{"filter":"all"}}')
    assert isinstance(m, Action)
    assert m.signal("filter") == "all"
    assert m.payload == {"contact": 7}
    assert m.to_line() == 'action {"name":"archive","payload":{"contact":7},"signals":{"filter":"all"}}\n'
    assert parse_renderer_message("hello {}") == Hello({})
    h = parse_renderer_message('hello {"params":{"room":"abc"}}')
    assert h.to_line() == 'hello {"params":{"room":"abc"}}\n'
    with pytest.raises(ParseError):
        parse_renderer_message('hello {"params":{"n":1}}')


def test_server_messages() -> None:
    assert parse_server_message('remove {"id":"x"}') == Remove("x")
    assert parse_server_message('metadata {"name":"Plants","x":1}') == Metadata(name="Plants")
    assert parse_server_message('metadata {"url":"/a","n":1}') == Metadata(extra={"url": "/a"})
    assert Metadata(name="P", extra={"url": "/a"}).to_line() == 'metadata {"name":"P","url":"/a"}\n'
    log = parse_server_message('log {"summary":"Saved","detail":"3 files","type":"success"}')
    assert log == Log("Saved", "3 files", "success")
    assert log.to_line() == 'log {"summary":"Saved","detail":"3 files","type":"success"}\n'
    assert parse_server_message('log {"summary":"Hi","type":"other"}') == Log("Hi")
    with pytest.raises(ParseError):
        parse_server_message('log {"detail":"no summary"}')
    with pytest.raises(ParseError):
        parse_server_message('element {"id":"x","type":"marquee"}')


def test_line_buffer() -> None:
    b = LineBuffer()
    assert b.push(b"hello {}\r\n\nsig") == ["hello {}"]
    assert b.push("nal {}\n") == ["signal {}"]
    assert b.push(b"\xff\na {}\n") == ["a {}"]
    with pytest.raises(ParseError):
        parse_line("Signal {}")
    with pytest.raises(ParseError) as e:
        parse_line("signal []")
    assert e.value.kind == ParseErrorKind.NOT_AN_OBJECT
    with pytest.raises(ParseError):
        parse_line('signal {"x":NaN}')


@pytest.mark.parametrize(
    "target,patch,want",
    [
        ({"a": "b"}, {"a": "c"}, {"a": "c"}),
        ({"a": "b"}, {"b": "c"}, {"a": "b", "b": "c"}),
        ({"a": "b"}, {"a": None}, {}),
        ({"a": [{"b": "c"}]}, {"a": [1]}, {"a": [1]}),
        ({"a": "foo"}, None, None),
        ({"e": None}, {"a": 1}, {"e": None, "a": 1}),
        ({}, {"a": {"bb": {"ccc": None}}}, {"a": {"bb": {}}}),
        ({"a": 5}, {"a": {"b": 1}}, {"a": {"b": 1}}),
    ],
)
def test_rfc7386_examples(target: object, patch: object, want: object) -> None:
    assert merge_patch(target, patch) == want  # type: ignore[arg-type]


def test_local_signals_and_paths() -> None:
    v = {"a": 1, "_b": 2, "c": {"_d": 3, "e": [{"_f": 1, "g": 2}]}}
    assert strip_local(v) == {"a": 1, "c": {"e": [{"g": 2}]}}
    assert parse_path("form.email") == ["form", "email"]
    assert parse_path("_busy") == ["_busy"]
    for bad in ["a..b", "1a", ""]:
        assert parse_path(bad) is None
    assert path_patch(["a", "b"], 1) == {"a": {"b": 1}}


def test_components() -> None:
    assert len(Component) == 18
    assert accepts(None, Component.DIALOG)
    assert not accepts(None, Component.TAB)
    assert accepts(Component.TABS, Component.TAB)
    assert not accepts(Component.TABS, Component.TEXT)
    assert not accepts(Component.CARD, Component.DIALOG)
    assert not accepts(Component.TEXT, Component.TEXT)
    label = Component.TAB.attr("label")
    assert label is not None and label.default_value("general") == "general"
