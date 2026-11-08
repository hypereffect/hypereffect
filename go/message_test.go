package hypereffect_test

import (
	"encoding/json"
	"errors"
	"reflect"
	"testing"

	he "github.com/hypereffect/hypereffect/go"
	"github.com/hypereffect/hypereffect/go/ui"
)

func TestElementRoundTrip(t *testing.T) {
	e := ui.Text("title").Parent("main").Variant("h1").Text("Inbox")
	line := e.Line()
	want := `element {"id":"title","type":"text","parent":"main","variant":"h1","text":"Inbox"}` + "\n"
	if line != want {
		t.Fatalf("got %q", line)
	}
	m, err := he.ParseServerMessage(line[:len(line)-1])
	if err != nil {
		t.Fatal(err)
	}
	if got := m.Line(); got != line {
		t.Fatalf("round trip: %q", got)
	}
}

func TestElementBuilders(t *testing.T) {
	e := ui.Button("inc").
		Parent("buttons").
		Label("+").
		Variant("primary").
		Disabled(he.Cel("s.count >= 10")).
		OnPress(he.Do("add").WithPayload(1))
	want := `element {"id":"inc","type":"button","parent":"buttons","label":"+","variant":"primary","disabled":{"cel":"s.count >= 10"},"onPress":{"action":"add","payload":1}}` + "\n"
	if got := e.Line(); got != want {
		t.Fatalf("got %q", got)
	}
	// No HTML escaping.
	if got := ui.Text("t").Text("a < b & c").Line(); got != `element {"id":"t","type":"text","text":"a < b & c"}`+"\n" {
		t.Fatalf("got %q", got)
	}
}

func TestHandlers(t *testing.T) {
	cases := []struct {
		h    he.Handler
		want string
	}{
		{he.Do("login").WithIndicator("_busy").WithPatch(map[string]any{"_open": false}),
			`{"patch":{"_open":false},"action":"login","indicator":"_busy"}`},
		{he.Open("https://example.com"), `{"open":"https://example.com"}`},
		{he.Do("visited").WithOpen(he.Cel("'https://example.com/' + s.id")),
			`{"action":"visited","open":{"cel":"'https://example.com/' + s.id"}}`},
	}
	for _, c := range cases {
		b, _ := json.Marshal(c.h)
		if string(b) != c.want {
			t.Errorf("got %s, want %s", b, c.want)
		}
	}
}

func TestRendererMessages(t *testing.T) {
	m, err := he.ParseRendererMessage(`action {"name":"archive","payload":{"contact":7},"signals":{"filter":"all"}}`)
	if err != nil {
		t.Fatal(err)
	}
	a := m.(*he.Action)
	if v, _ := a.Signal("filter"); v != "all" {
		t.Errorf("filter is %v", v)
	}
	var p struct{ Contact int }
	if err := a.DecodePayload(&p); err != nil || p.Contact != 7 {
		t.Errorf("payload %v %v", p, err)
	}
	if got := m.Line(); got != `action {"name":"archive","payload":{"contact":7},"signals":{"filter":"all"}}`+"\n" {
		t.Errorf("got %q", got)
	}
	m, err = he.ParseRendererMessage("hello {}")
	if err != nil || !reflect.DeepEqual(m, he.Hello{Params: map[string]string{}}) {
		t.Errorf("hello: %#v %v", m, err)
	}
	m, _ = he.ParseRendererMessage(`hello {"params":{"room":"abc"}}`)
	if got := m.Line(); got != `hello {"params":{"room":"abc"}}`+"\n" {
		t.Errorf("got %q", got)
	}
	if _, err := he.ParseRendererMessage(`hello {"params":{"n":1}}`); err == nil {
		t.Error("non-string params parsed")
	}
}

func TestServerMessages(t *testing.T) {
	for line, want := range map[string]he.ServerMessage{
		`remove {"id":"x"}`:                he.Remove{ID: "x"},
		`metadata {"name":"Plants","x":1}`: he.Metadata{Name: "Plants"},
		`metadata {"url":"/a","x":1}`:      he.Metadata{Extra: map[string]string{"url": "/a"}},
		`signal {"a":{"b":true}}`:          he.Signal{"a": map[string]any{"b": true}},
		`log {"summary":"Saved","detail":"3 files","type":"success"}`: he.Log{
			Summary: "Saved", Detail: "3 files", Type: he.LogSuccess,
		},
		`log {"summary":"Hi","type":"other"}`: he.Log{Summary: "Hi", Type: he.LogInfo},
	} {
		got, err := he.ParseServerMessage(line)
		if err != nil || !reflect.DeepEqual(got, want) {
			t.Errorf("%s: %#v %v", line, got, err)
		}
	}
	m := he.Metadata{Name: "P", Extra: map[string]string{"url": "/a", "name": "Q"}}
	if got := m.Line(); got != `metadata {"name":"P","url":"/a"}`+"\n" {
		t.Errorf("got %q", got)
	}
	log := he.NewLog("Saved").WithDetail("3 files").WithType(he.LogSuccess)
	if got := log.Line(); got != `log {"summary":"Saved","detail":"3 files","type":"success"}`+"\n" {
		t.Errorf("got %q", got)
	}
	if got := (he.Log{Summary: "Hi"}).Line(); got != `log {"summary":"Hi","type":"info"}`+"\n" {
		t.Errorf("got %q", got)
	}
	if _, err := he.ParseServerMessage(`log {"detail":"no summary"}`); err == nil {
		t.Error("log without summary parsed")
	}
	if _, err := he.ParseServerMessage(`element {"id":"x","type":"text"} x`); err == nil {
		t.Error("trailing data parsed")
	}
	if _, err := he.ParseServerMessage(`element {"id":"x","type":"marquee"}`); err == nil {
		t.Error("unknown type parsed")
	}
}

func TestLineBuffer(t *testing.T) {
	var b he.LineBuffer
	if got := b.Push([]byte("hello {}\r\n\nsig")); !reflect.DeepEqual(got, []string{"hello {}"}) {
		t.Errorf("got %q", got)
	}
	if got := b.Push([]byte("nal {}\n")); !reflect.DeepEqual(got, []string{"signal {}"}) {
		t.Errorf("got %q", got)
	}
	if _, _, err := he.ParseLine("Signal {}"); err == nil {
		t.Error("uppercase tag parsed")
	}
	var pe *he.ParseError
	if _, _, err := he.ParseLine("signal []"); !errors.As(err, &pe) || pe.Kind != he.ErrNotAnObject {
		t.Errorf("got %v", err)
	}
	if _, _, err := he.ParseLine("signal {} x"); err == nil {
		t.Error("trailing data parsed")
	}
}

func TestComponents(t *testing.T) {
	if len(he.Components) != 18 {
		t.Fatal(len(he.Components))
	}
	for _, c := range []struct {
		parent, child he.Component
		ok            bool
	}{
		{"", he.Dialog, true}, {"", he.Tab, false}, {he.Tabs, he.Tab, true},
		{he.Tabs, he.Text, false}, {he.Card, he.Dialog, false}, {he.Text, he.Text, false},
	} {
		if he.Accepts(c.parent, c.child) != c.ok {
			t.Errorf("Accepts(%q, %q)", c.parent, c.child)
		}
	}
	a, _ := he.Tab.Attr("label")
	if a.DefaultValue("general") != "general" {
		t.Error("tab label default")
	}
}
