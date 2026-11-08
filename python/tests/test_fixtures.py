"""Runs the shared fixtures in ../../fixtures against Client."""

import json
from pathlib import Path

import pytest

from hypereffect.client import Client

FIXTURES = Path(__file__).parent.parent.parent / "fixtures"


def same(a: object, b: object) -> bool:
    """Deep equality with numbers compared by value."""
    num = (int, float)
    if isinstance(a, num) and not isinstance(a, bool):
        return isinstance(b, num) and not isinstance(b, bool) and a == b
    if isinstance(a, list):
        return isinstance(b, list) and len(a) == len(b) and all(same(x, y) for x, y in zip(a, b))
    if isinstance(a, dict):
        return isinstance(b, dict) and a.keys() == b.keys() and all(same(v, b[k]) for k, v in a.items())
    return type(a) is type(b) and a == b


def cut(s: str) -> tuple[str, str]:
    a, _, b = s.partition(" ")
    return a, b


def run(path: Path) -> None:
    name = path.name
    c = Client()
    sent: list[str] = []
    failures: list[str] = []
    for i, line in enumerate(path.read_text().split("\n")):
        at = f"{name}:{i + 1}"
        if not line or line.startswith("#"):
            continue
        if line.startswith("< "):
            tag, body = cut(line[2:])
            if not sent:
                failures.append(f"{at}: expected {line[2:]}, nothing was sent")
                continue
            got = sent.pop(0).strip()
            gtag, gbody = cut(got)
            if gtag != tag or not same(json.loads(gbody), json.loads(body)):
                failures.append(f"{at}: got {got}")
            continue
        if line.startswith("!"):
            if sent:
                failures.append(f"{at}: unexpected output {sent[0].strip()}")
                sent.clear()
            cmd, args = cut(line[1:])
            if cmd == "start":
                sent.append(c.hello())
            elif cmd == "write":
                p, v = cut(args)
                c.write(p, json.loads(v))
            elif cmd == "trigger":
                out = c.trigger(*cut(args))
                if out is not None:
                    sent.append(out)
            elif cmd == "close":
                c.close()
            else:
                raise AssertionError(f"{at}: unknown command {cmd}")
            continue
        if line.startswith("="):
            cmd, args = cut(line[1:])
            if cmd == "attrs":
                id, want = cut(args)
                r = c.get(id)
                if r is None:
                    failures.append(f"{at}: no element {id}")
                    continue
                for k, v in json.loads(want).items():
                    got = r.attrs.get(k)
                    if not same(got, v):
                        failures.append(f"{at}: {id}.{k} is {json.dumps(got)}, want {json.dumps(v)}")
            elif cmd == "children":
                id, want = cut(args)
                got = c.children(None if id == "-" else id)
                if got != json.loads(want):
                    failures.append(f"{at}: children of {id} are {got}")
            elif cmd == "signals":
                if not same(c.signals, json.loads(args)):
                    failures.append(f"{at}: signals are {json.dumps(c.signals)}")
            elif cmd == "metadata":
                if not same(c.metadata.to_json(), json.loads(args)):
                    failures.append(f"{at}: metadata is {c.metadata.to_json()}")
            elif cmd == "opened":
                got = c.take_opens()
                if got != json.loads(args):
                    failures.append(f"{at}: opened {got}")
            elif cmd == "logs":
                got = [log.to_json() for log in c.take_logs()]
                if not same(got, json.loads(args)):
                    failures.append(f"{at}: logs are {json.dumps(got)}")
            elif cmd == "absent":
                if c.contains(args):
                    failures.append(f"{at}: {args} exists")
            else:
                raise AssertionError(f"{at}: unknown check {cmd}")
            continue
        c.receive_line(line)
    if sent:
        failures.append(f"{name}: unexpected output at the end: {sent[0].strip()}")
    if opens := c.take_opens():
        failures.append(f"{name}: URLs opened but not checked: {opens}")
    if logs := c.take_logs():
        failures.append(f"{name}: logs received but not checked: {logs}")
    assert not failures, "\n".join(failures)


FILES = sorted(FIXTURES.glob("*.txt"))


def test_fixtures_exist() -> None:
    assert FILES


@pytest.mark.parametrize("path", FILES, ids=[f.name for f in FILES])
def test_fixture(path: Path) -> None:
    run(path)
