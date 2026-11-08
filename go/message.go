package hypereffect

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"maps"
	"strings"
	"unicode/utf8"

	"github.com/hypereffect/hypereffect/go/internal/jsonx"
	"github.com/hypereffect/hypereffect/go/signals"
)

// Expr is an expression attribute value: {"cel": "..."}.
type Expr struct {
	Cel string `json:"cel"`
}

// Cel returns an expression attribute value.
func Cel(expr string) Expr {
	return Expr{Cel: expr}
}

// IsCel returns the source of v if it is an expression: an Expr, or an
// object whose only member is `cel`, a string.
func IsCel(v any) (string, bool) {
	switch v := v.(type) {
	case Expr:
		return v.Cel, true
	case map[string]any:
		if len(v) != 1 {
			return "", false
		}
		s, ok := v["cel"].(string)
		return s, ok
	}
	return "", false
}

// An Element is one instance of a component. The setters return the element,
// so an interface is built as
//
//	ui.Text("title").Parent("main").Variant("h1").Text("Inbox")
//
// Attribute values are literals (strings, bools, numbers, or anything that
// encodes to JSON) or expressions from [Cel].
type Element struct {
	ID       string
	Type     Component
	ParentID string // "" for the root
	BeforeID string // "" to append
	names    []string
	attrs    map[string]any
}

// NewElement returns an element with no attributes.
func NewElement(id string, c Component) *Element {
	return &Element{ID: id, Type: c}
}

func (e *Element) Parent(id string) *Element {
	e.ParentID = id
	return e
}

func (e *Element) Before(id string) *Element {
	e.BeforeID = id
	return e
}

// Set sets any attribute.
func (e *Element) Set(name string, v any) *Element {
	if e.attrs == nil {
		e.attrs = map[string]any{}
	}
	if _, ok := e.attrs[name]; !ok {
		e.names = append(e.names, name)
	}
	e.attrs[name] = v
	return e
}

// Attr returns an attribute's value as it was set or parsed.
func (e *Element) Attr(name string) (any, bool) {
	v, ok := e.attrs[name]
	return v, ok
}

// AttrNames returns the names of the attributes set, in order.
func (e *Element) AttrNames() []string {
	return append([]string(nil), e.names...)
}

func (e *Element) Visible(v any) *Element     { return e.Set("visible", v) }
func (e *Element) Disabled(v any) *Element    { return e.Set("disabled", v) }
func (e *Element) Gap(v any) *Element         { return e.Set("gap", v) }
func (e *Element) Align(v any) *Element       { return e.Set("align", v) }
func (e *Element) Justify(v any) *Element     { return e.Set("justify", v) }
func (e *Element) Wrap(v any) *Element        { return e.Set("wrap", v) }
func (e *Element) Title(v any) *Element       { return e.Set("title", v) }
func (e *Element) Empty(v any) *Element       { return e.Set("empty", v) }
func (e *Element) Label(v any) *Element       { return e.Set("label", v) }
func (e *Element) Open(v any) *Element        { return e.Set("open", v) }
func (e *Element) Value(v any) *Element       { return e.Set("value", v) }
func (e *Element) Variant(v any) *Element     { return e.Set("variant", v) }
func (e *Element) Src(v any) *Element         { return e.Set("src", v) }
func (e *Element) Alt(v any) *Element         { return e.Set("alt", v) }
func (e *Element) Placeholder(v any) *Element { return e.Set("placeholder", v) }
func (e *Element) Error(v any) *Element       { return e.Set("error", v) }
func (e *Element) Options(v any) *Element     { return e.Set("options", v) }
func (e *Element) Min(v any) *Element         { return e.Set("min", v) }
func (e *Element) Max(v any) *Element         { return e.Set("max", v) }
func (e *Element) Step(v any) *Element        { return e.Set("step", v) }
func (e *Element) Text(v any) *Element        { return e.Set("text", v) }

// Bind sets `bind`, the signal path a control reads and writes.
func (e *Element) Bind(path string) *Element { return e.Set("bind", path) }

func (e *Element) OnPress(h Handler) *Element   { return e.Set("onPress", h) }
func (e *Element) OnChange(h Handler) *Element  { return e.Set("onChange", h) }
func (e *Element) OnSubmit(h Handler) *Element  { return e.Set("onSubmit", h) }
func (e *Element) OnDismiss(h Handler) *Element { return e.Set("onDismiss", h) }

func (e *Element) Tag() string { return "element" }

// Line returns the message as a line, including the final line feed.
func (e *Element) Line() string { return encodeLine("element", e) }

func (*Element) serverMessage() {}

func (e *Element) MarshalJSON() ([]byte, error) {
	var b bytes.Buffer
	member := func(k string, v any) error {
		if b.Len() == 0 {
			b.WriteByte('{')
		} else {
			b.WriteByte(',')
		}
		b.Write(jsonx.MustMarshal(k))
		b.WriteByte(':')
		j, err := jsonx.Marshal(v)
		b.Write(j)
		return err
	}
	member("id", e.ID)
	member("type", e.Type)
	if e.ParentID != "" {
		member("parent", e.ParentID)
	}
	if e.BeforeID != "" {
		member("before", e.BeforeID)
	}
	for _, k := range e.names {
		if err := member(k, e.attrs[k]); err != nil {
			return nil, fmt.Errorf("attribute %s: %w", k, err)
		}
	}
	b.WriteByte('}')
	return b.Bytes(), nil
}

func (e *Element) UnmarshalJSON(data []byte) error {
	dec := json.NewDecoder(bytes.NewReader(data))
	dec.UseNumber()
	if t, err := dec.Token(); err != nil || t != json.Delim('{') {
		return fmt.Errorf("an element must be an object")
	}
	*e = Element{}
	var hasID, hasType bool
	for dec.More() {
		t, err := dec.Token()
		if err != nil {
			return err
		}
		k := t.(string)
		var v any
		if err := dec.Decode(&v); err != nil {
			return err
		}
		str := func(dst *string) error {
			s, ok := v.(string)
			if !ok {
				return fmt.Errorf("%s must be a string", k)
			}
			*dst = s
			return nil
		}
		switch k {
		case "id":
			hasID = true
			err = str(&e.ID)
		case "type":
			hasType = true
			var c string
			if err = str(&c); err == nil && !Component(c).Valid() {
				err = fmt.Errorf("unknown type %q", c)
			}
			e.Type = Component(c)
		case "parent":
			err = str(&e.ParentID)
		case "before":
			err = str(&e.BeforeID)
		default:
			e.Set(k, v)
		}
		if err != nil {
			return err
		}
	}
	if _, err := dec.Token(); err != nil {
		return err
	}
	if _, err := dec.Token(); err != io.EOF {
		return fmt.Errorf("invalid character after top-level value")
	}
	if !hasID || !hasType {
		return fmt.Errorf("an element needs an id and a type")
	}
	return nil
}

// A Handler is a local patch, a URL to open (buttons only), an action sent
// to the server, or any combination.
type Handler struct {
	Patch     map[string]any `json:"patch,omitempty"`
	Action    string         `json:"action,omitempty"`
	Payload   any            `json:"payload,omitempty"`
	Indicator string         `json:"indicator,omitempty"`
	// Open is a URL: a string, or an expression from [Cel] giving one.
	Open any `json:"open,omitempty"`
}

// Do returns a handler that sends an action.
func Do(action string) Handler {
	return Handler{Action: action}
}

// Patch returns a handler that applies a local merge patch. Leaf values may
// be expressions.
func Patch(patch map[string]any) Handler {
	return Handler{Patch: patch}
}

// Open returns a handler that asks the renderer to open a URL: a string, or
// an expression from [Cel] giving one. Only valid on a button.
func Open(url any) Handler {
	return Handler{Open: url}
}

// WithOpen adds a URL to open, after the patch and before the action is
// sent.
func (h Handler) WithOpen(url any) Handler {
	h.Open = url
	return h
}

// WithPatch adds a local patch, applied before the action is sent.
func (h Handler) WithPatch(patch map[string]any) Handler {
	h.Patch = patch
	return h
}

func (h Handler) WithPayload(payload any) Handler {
	h.Payload = payload
	return h
}

// WithIndicator sets a local signal path set to true when the action is
// sent.
func (h Handler) WithIndicator(path string) Handler {
	h.Indicator = path
	return h
}

// A ServerMessage is a message from the server to the renderer: an
// *Element, a Remove, a Signal, a Metadata or a Log.
type ServerMessage interface {
	Tag() string
	// Line returns the message as a line, including the final line feed.
	Line() string
	serverMessage()
}

// Remove removes an element and its descendants.
type Remove struct {
	ID string `json:"id"`
}

func (Remove) Tag() string      { return "remove" }
func (m Remove) Line() string   { return encodeLine("remove", m) }
func (Remove) serverMessage()   {}
func (m Signal) Line() string   { return encodeLine("signal", map[string]any(m)) }
func (Signal) Tag() string      { return "signal" }
func (Signal) serverMessage()   {}
func (m Metadata) Line() string { return encodeLine("metadata", m) }
func (Metadata) Tag() string    { return "metadata" }
func (Metadata) serverMessage() {}
func (m Log) Line() string      { return encodeLine("log", m) }
func (Log) Tag() string         { return "log" }
func (Log) serverMessage()      {}

// Signal is a merge patch for the signals.
type Signal map[string]any

// Metadata holds optional details about the application, sent by the
// server. A renderer is free to ignore all of them.
type Metadata struct {
	// The name of the application.
	Name string `json:"name,omitempty"`
	// A short description of the application.
	Description string `json:"description,omitempty"`
	// The URL of an icon for the application.
	Icon string `json:"icon,omitempty"`
	// The other string members: extensions defined by a protocol built on
	// hypereffect, which renderers that do not know them ignore. Sent as
	// members of the metadata object, next to the ones above; keys that are
	// one of those are dropped.
	Extra map[string]string `json:"-"`
}

var knownMetadata = map[string]bool{"name": true, "description": true, "icon": true}

// MarshalJSON writes the extension members next to the known ones.
func (m Metadata) MarshalJSON() ([]byte, error) {
	out := map[string]string{}
	for k, v := range m.Extra {
		if !knownMetadata[k] {
			out[k] = v
		}
	}
	for k, v := range map[string]string{"name": m.Name, "description": m.Description, "icon": m.Icon} {
		if v != "" {
			out[k] = v
		}
	}
	return jsonx.Marshal(out)
}

// Equal reports whether m and o hold the same members.
func (m Metadata) Equal(o Metadata) bool {
	return m.Name == o.Name && m.Description == o.Description && m.Icon == o.Icon &&
		maps.Equal(m.Extra, o.Extra)
}

// MetadataFromMap reads the members that are strings and drops everything
// else.
func MetadataFromMap(body map[string]any) Metadata {
	get := func(k string) string {
		s, _ := body[k].(string)
		return s
	}
	m := Metadata{Name: get("name"), Description: get("description"), Icon: get("icon")}
	for k, v := range body {
		if s, ok := v.(string); ok && !knownMetadata[k] {
			if m.Extra == nil {
				m.Extra = map[string]string{}
			}
			m.Extra[k] = s
		}
	}
	return m
}

// LogType is the severity of a log message.
type LogType string

const (
	LogWarning LogType = "warning"
	LogError   LogType = "error"
	LogSuccess LogType = "success"
	LogInfo    LogType = "info"
	LogDebug   LogType = "debug"
)

// Log is an event for the user or the host's developer log. It is not part of
// the element tree or the signals.
type Log struct {
	// Summary is a short summary of the event.
	Summary string `json:"summary"`
	// Detail holds details of the event.
	Detail string `json:"detail,omitempty"`
	// Type is the severity of the event. The zero value means LogInfo.
	Type LogType `json:"type"`
}

// NewLog returns an info log with this summary.
func NewLog(summary string) Log {
	return Log{Summary: summary, Type: LogInfo}
}

// WithDetail adds details to the event.
func (l Log) WithDetail(detail string) Log {
	l.Detail = detail
	return l
}

// WithType sets the severity of the event.
func (l Log) WithType(t LogType) Log {
	l.Type = t
	return l
}

// MarshalJSON writes an empty Type as "info".
func (l Log) MarshalJSON() ([]byte, error) {
	if l.Type == "" {
		l.Type = LogInfo
	}
	type plain Log
	return jsonx.Marshal(plain(l))
}

// LogFromMap reads a log message. Summary must be a non-empty string; a
// missing or unknown type is LogInfo, and a Detail that is not a string is
// ignored.
func LogFromMap(body map[string]any) (Log, error) {
	summary, _ := body["summary"].(string)
	if summary == "" {
		return Log{}, &ParseError{Kind: ErrInvalid, Detail: "log needs a non-empty string summary"}
	}
	l := NewLog(summary)
	if detail, ok := body["detail"].(string); ok {
		l.Detail = detail
	}
	if s, ok := body["type"].(string); ok {
		switch LogType(s) {
		case LogWarning, LogError, LogSuccess, LogInfo, LogDebug:
			l.Type = LogType(s)
		}
	}
	return l, nil
}

// ParseServerMessage parses a line (without its line feed).
func ParseServerMessage(line string) (ServerMessage, error) {
	// Fast path for elements, which also keeps the order of attributes.
	// Anything that fails goes through the general path, which reports the
	// precise error.
	if body, ok := strings.CutPrefix(strings.TrimSuffix(line, "\r"), "element "); ok {
		var e Element
		if e.UnmarshalJSON([]byte(body)) == nil {
			return &e, nil
		}
	}
	tag, body, err := ParseLine(line)
	if err != nil {
		return nil, err
	}
	return ServerMessageFromParts(tag, body)
}

// ServerMessageFromParts builds a message from its tag and JSON object.
func ServerMessageFromParts(tag string, body map[string]any) (ServerMessage, error) {
	switch tag {
	case "element":
		var e Element
		if err := e.UnmarshalJSON(jsonx.MustMarshal(body)); err != nil {
			return nil, &ParseError{Kind: ErrInvalid, Detail: err.Error()}
		}
		return &e, nil
	case "remove":
		id, ok := body["id"].(string)
		if !ok {
			return nil, &ParseError{Kind: ErrInvalid, Detail: "remove needs a string id"}
		}
		return Remove{ID: id}, nil
	case "signal":
		return Signal(body), nil
	case "metadata":
		return MetadataFromMap(body), nil
	case "log":
		return LogFromMap(body)
	}
	return nil, &ParseError{Kind: ErrUnknownTag, Detail: tag}
}

// A RendererMessage is a message from the renderer to the server: a Hello
// or an *Action.
type RendererMessage interface {
	Line() string
	rendererMessage()
}

// Hello opens a session. The params are chosen by whoever opened it (for
// example, taken from the address) and are strings only.
type Hello struct {
	Params map[string]string
}

func (h Hello) Line() string {
	if len(h.Params) == 0 {
		return encodeLine("hello", map[string]any{})
	}
	return encodeLine("hello", map[string]any{"params": h.Params})
}

func (Hello) rendererMessage() {}

// An Action is sent by a handler. Its contents are user input.
type Action struct {
	Name    string          `json:"name"`
	Payload json.RawMessage `json:"payload,omitempty"`
	Signals map[string]any  `json:"signals"`
}

func (a *Action) Line() string {
	if a.Signals == nil {
		b := *a
		b.Signals = map[string]any{}
		a = &b
	}
	return encodeLine("action", a)
}

func (*Action) rendererMessage() {}

// DecodePayload decodes the payload into v. It fails if there is no payload.
func (a *Action) DecodePayload(v any) error {
	if len(a.Payload) == 0 {
		return fmt.Errorf("the action has no payload")
	}
	return jsonx.Decode(a.Payload, v)
}

// Signal returns a signal by path, such as `form.email`.
func (a *Action) Signal(path string) (any, bool) {
	segs, ok := signals.ParsePath(path)
	if !ok {
		return nil, false
	}
	return signals.GetPath(a.Signals, segs)
}

// SignalString returns a signal that is a string, or "".
func (a *Action) SignalString(path string) string {
	v, _ := a.Signal(path)
	s, _ := v.(string)
	return s
}

// ParseRendererMessage parses a line (without its line feed).
func ParseRendererMessage(line string) (RendererMessage, error) {
	tag, body, err := ParseLine(line)
	if err != nil {
		return nil, err
	}
	switch tag {
	case "hello":
		h := Hello{Params: map[string]string{}}
		if p, ok := body["params"]; ok {
			m, ok := p.(map[string]any)
			if !ok {
				return nil, &ParseError{Kind: ErrInvalid, Detail: "params: not an object"}
			}
			for k, v := range m {
				s, ok := v.(string)
				if !ok {
					return nil, &ParseError{Kind: ErrInvalid, Detail: "params: " + k + " is not a string"}
				}
				h.Params[k] = s
			}
		}
		return h, nil
	case "action":
		name, ok := body["name"].(string)
		if !ok {
			return nil, &ParseError{Kind: ErrInvalid, Detail: "action needs a string name"}
		}
		a := &Action{Name: name, Signals: map[string]any{}}
		if p, ok := body["payload"]; ok && p != nil {
			a.Payload = jsonx.MustMarshal(p)
		}
		if s, ok := body["signals"]; ok {
			m, ok := s.(map[string]any)
			if !ok {
				return nil, &ParseError{Kind: ErrInvalid, Detail: "signals: not an object"}
			}
			a.Signals = m
		}
		return a, nil
	}
	return nil, &ParseError{Kind: ErrUnknownTag, Detail: tag}
}

// ParseErrorKind says why a line could not be parsed.
type ParseErrorKind int

const (
	// ErrGrammar: the line does not match `tag SP json`.
	ErrGrammar ParseErrorKind = iota
	ErrJSON
	ErrNotAnObject
	ErrUnknownTag
	// ErrInvalid: the JSON does not have the members the message needs.
	ErrInvalid
)

type ParseError struct {
	Kind   ParseErrorKind
	Detail string
}

func (e *ParseError) Error() string {
	switch e.Kind {
	case ErrGrammar:
		return "line does not match the grammar"
	case ErrJSON:
		return "invalid JSON: " + e.Detail
	case ErrNotAnObject:
		return "message body is not a JSON object"
	case ErrUnknownTag:
		return "unknown tag '" + e.Detail + "'"
	}
	return "invalid message: " + e.Detail
}

// EncodeLine encodes a message as a line, including the final line feed.
// It panics if body cannot be encoded.
func EncodeLine(tag string, body any) string {
	return encodeLine(tag, body)
}

func encodeLine(tag string, body any) string {
	return tag + " " + string(jsonx.MustMarshal(body)) + "\n"
}

// ParseLine splits a line (without its line feed) into its tag and JSON
// object. Numbers in the object are json.Number.
func ParseLine(line string) (string, map[string]any, error) {
	line = strings.TrimSuffix(line, "\r")
	tag, body, ok := strings.Cut(line, " ")
	if !ok || tag == "" {
		return "", nil, &ParseError{Kind: ErrGrammar}
	}
	for _, c := range []byte(tag) {
		if c < 'a' || c > 'z' {
			return "", nil, &ParseError{Kind: ErrGrammar}
		}
	}
	var v any
	if err := jsonx.Decode([]byte(body), &v); err != nil {
		return "", nil, &ParseError{Kind: ErrJSON, Detail: err.Error()}
	}
	m, ok := v.(map[string]any)
	if !ok {
		return "", nil, &ParseError{Kind: ErrNotAnObject}
	}
	return tag, m, nil
}

// LineBuffer splits a byte stream into lines. Empty lines are dropped and a
// carriage return before the line feed is ignored.
type LineBuffer struct {
	buf []byte
}

// Push adds bytes and returns the complete lines they finish. Lines that
// are not valid UTF-8 are dropped.
func (b *LineBuffer) Push(data []byte) []string {
	b.buf = append(b.buf, data...)
	var lines []string
	for {
		i := bytes.IndexByte(b.buf, '\n')
		if i < 0 {
			break
		}
		line := bytes.TrimSuffix(b.buf[:i], []byte("\r"))
		if len(line) > 0 && utf8.Valid(line) {
			lines = append(lines, string(line))
		}
		b.buf = b.buf[i+1:]
	}
	if len(b.buf) == 0 {
		b.buf = nil
	}
	return lines
}
