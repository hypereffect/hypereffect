package client

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/hypereffect/hypereffect/go/internal/jsonx"
)

// The shared fixtures at the root of the repository.
const fixtures = "../../fixtures"

// same is deep equality with numbers compared by value.
func same(a, b any) bool {
	num := func(v any) (float64, bool) {
		switch v := v.(type) {
		case json.Number:
			f, err := v.Float64()
			return f, err == nil
		case float64:
			return v, true
		}
		return 0, false
	}
	if x, ok := num(a); ok {
		y, ok := num(b)
		return ok && x == y
	}
	switch a := a.(type) {
	case []any:
		b, ok := b.([]any)
		if !ok || len(a) != len(b) {
			return false
		}
		for i := range a {
			if !same(a[i], b[i]) {
				return false
			}
		}
		return true
	case map[string]any:
		b, ok := b.(map[string]any)
		if !ok || len(a) != len(b) {
			return false
		}
		for k, v := range a {
			w, ok := b[k]
			if !ok || !same(v, w) {
				return false
			}
		}
		return true
	}
	return a == b
}

func decode(t *testing.T, s string) any {
	var v any
	if err := jsonx.Decode([]byte(s), &v); err != nil {
		t.Fatalf("bad JSON in fixture: %s: %v", s, err)
	}
	return v
}

// normal converts a value to its JSON form, for comparison with same.
func normal(v any) any {
	n, err := jsonx.Normalize(v)
	if err != nil {
		panic(err)
	}
	return n
}

func run(t *testing.T, path string) {
	name := filepath.Base(path)
	src, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	c := New()
	var sent []string
	fail := func(at string, format string, args ...any) {
		t.Errorf("%s: %s", at, fmt.Sprintf(format, args...))
	}
	cut := func(s string) (string, string) {
		a, b, _ := strings.Cut(s, " ")
		return a, b
	}
	for i, line := range strings.Split(string(src), "\n") {
		at := fmt.Sprintf("%s:%d", name, i+1)
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		if rest, ok := strings.CutPrefix(line, "< "); ok {
			tag, body := cut(rest)
			if len(sent) == 0 {
				fail(at, "expected %s, nothing was sent", rest)
				continue
			}
			got := strings.TrimSpace(sent[0])
			sent = sent[1:]
			gtag, gbody := cut(got)
			if gtag != tag || !same(decode(t, gbody), decode(t, body)) {
				fail(at, "got %s", got)
			}
			continue
		}
		if cmd, ok := strings.CutPrefix(line, "!"); ok {
			if len(sent) > 0 {
				fail(at, "unexpected output %s", strings.TrimSpace(sent[0]))
				sent = nil
			}
			cmd, args := cut(cmd)
			switch cmd {
			case "start":
				sent = append(sent, c.Hello())
			case "write":
				p, v := cut(args)
				c.Write(p, decode(t, v))
			case "trigger":
				id, ev := cut(args)
				if l, ok := c.Trigger(id, ev); ok {
					sent = append(sent, l)
				}
			case "close":
				c.Close()
			default:
				t.Fatalf("%s: unknown command %s", at, cmd)
			}
			continue
		}
		if check, ok := strings.CutPrefix(line, "="); ok {
			cmd, args := cut(check)
			switch cmd {
			case "attrs":
				id, want := cut(args)
				r, ok := c.Get(id)
				if !ok {
					fail(at, "no element %s", id)
					continue
				}
				for k, v := range decode(t, want).(map[string]any) {
					got := normal(r.Attrs[k])
					if !same(got, v) {
						fail(at, "%s.%s is %s, want %s", id, k, jsonx.MustMarshal(got), jsonx.MustMarshal(v))
					}
				}
			case "children":
				id, want := cut(args)
				if id == "-" {
					id = ""
				}
				var w []string
				json.Unmarshal([]byte(want), &w)
				if got := c.Children(id); !slices.Equal(got, w) {
					fail(at, "children of %s are %q", id, got)
				}
			case "signals":
				if got := normal(c.Signals()); !same(got, decode(t, args)) {
					fail(at, "signals are %s", jsonx.MustMarshal(got))
				}
			case "metadata":
				if got := normal(c.Metadata()); !same(got, decode(t, args)) {
					fail(at, "metadata is %s", jsonx.MustMarshal(got))
				}
			case "opened":
				var want []string
				json.Unmarshal([]byte(args), &want)
				if got := c.TakeOpens(); !slices.Equal(got, want) {
					fail(at, "opened %q", got)
				}
			case "logs":
				if got := normal(c.TakeLogs()); !same(got, decode(t, args)) {
					fail(at, "logs are %s", jsonx.MustMarshal(got))
				}
			case "absent":
				if c.Contains(args) {
					fail(at, "%s exists", args)
				}
			default:
				t.Fatalf("%s: unknown check %s", at, cmd)
			}
			continue
		}
		c.ReceiveLine(line)
	}
	if len(sent) > 0 {
		t.Errorf("%s: unexpected output at the end: %s", name, strings.TrimSpace(sent[0]))
	}
	if opens := c.TakeOpens(); len(opens) > 0 {
		t.Errorf("%s: URLs opened but not checked: %q", name, opens)
	}
	if logs := c.TakeLogs(); len(logs) > 0 {
		t.Errorf("%s: logs received but not checked: %v", name, logs)
	}
}

func TestFixtures(t *testing.T) {
	files, _ := filepath.Glob(filepath.Join(fixtures, "*.txt"))
	if len(files) == 0 {
		t.Fatal("no fixtures")
	}
	for _, f := range files {
		t.Run(filepath.Base(f), func(t *testing.T) { run(t, f) })
	}
}
