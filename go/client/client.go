// Package client is the renderer side of a session, without any drawing.
//
// A [Client] keeps the element tree and the signals, evaluates CEL
// expressions with cel-go, runs handlers, and yields elements whose
// attributes are resolved to plain JSON values. A UI layer reads the
// resolved elements and draws them; [Client.TakeChanges] says which ones to
// redraw.
//
// A [Loopback] connects a Client to a [hypereffect.Server] in the same
// process, for testing an app without a renderer.
package client

import (
	"encoding/json"
	"fmt"
	"math"
	"slices"
	"strings"

	"cel.dev/cel-go/cel"
	"cel.dev/cel-go/common/types"
	"cel.dev/cel-go/common/types/ref"
	"cel.dev/cel-go/common/types/traits"
	"cel.dev/cel-go/ext"

	he "github.com/hypereffect/hypereffect/go"
	"github.com/hypereffect/hypereffect/go/internal/jsonx"
	"github.com/hypereffect/hypereffect/go/signals"
)

// Resolved is an element with its attributes resolved: expressions
// evaluated, values checked against the attribute types, and defaults filled
// in. Structural attributes (`bind` and handlers) are included as they were
// sent, or nil.
type Resolved struct {
	ID       string
	Type     he.Component
	Parent   string // "" for the root
	Children []string
	Attrs    map[string]any
}

// ResolvedTree is a resolved element with its resolved descendants.
type ResolvedTree struct {
	ID       string
	Type     he.Component
	Attrs    map[string]any
	Children []ResolvedTree
}

// Changes says what changed since the last [Client.TakeChanges].
type Changes struct {
	// Reset: the tree was discarded because a new session began.
	Reset bool
	// Connection: the connected state changed.
	Connection bool
	// Created holds elements whose widgets must be built anew: new, or with
	// a new type.
	Created map[string]bool
	// Updated holds elements whose resolved attributes may have changed.
	Updated map[string]bool
	Removed map[string]bool
	// Reordered holds elements whose list of children changed ("" for the
	// root).
	Reordered map[string]bool
	// Metadata: the application metadata changed.
	Metadata bool
	signals  bool
}

func newChanges() Changes {
	return Changes{
		Created:   map[string]bool{},
		Updated:   map[string]bool{},
		Removed:   map[string]bool{},
		Reordered: map[string]bool{},
	}
}

func (c *Changes) IsEmpty() bool {
	return !c.Reset && !c.Connection && len(c.Created) == 0 && len(c.Updated) == 0 &&
		len(c.Removed) == 0 && len(c.Reordered) == 0 && !c.Metadata
}

type node struct {
	component he.Component
	parent    string
	children  []string
	el        map[string]any
	progs     map[string]cel.Program
	// readsSignals: whether any attribute reads signals.
	readsSignals bool
}

// Client is the renderer side of a session. Create one with [New].
type Client struct {
	env          *cel.Env
	nodes        map[string]*node
	root         []string
	signals      map[string]any
	metadata     he.Metadata
	celSignals   any
	connected    bool
	resetPending bool
	reader       he.LineBuffer
	changes      Changes
	diagnostics  []string
	opens        []string
	logs         []he.Log
}

// New returns a client with no session.
func New() *Client {
	env, err := cel.NewEnv(
		cel.Variable("s", cel.DynType),
		ext.Strings(), ext.Math(), ext.Lists(),
	)
	if err != nil {
		panic("hypereffect: cannot create the CEL environment: " + err.Error())
	}
	return &Client{
		env:     env,
		nodes:   map[string]*node{},
		signals: map[string]any{},
		changes: newChanges(),
	}
}

// --- transport ---------------------------------------------------------

// Hello begins a session and returns the hello line to send. The previous
// tree is kept until the first server message of the new session.
func (c *Client) Hello() string {
	return c.HelloWith(nil)
}

// HelloWith is like Hello, with params for the app: strings chosen by
// whoever opened the session, such as the ones in its address.
func (c *Client) HelloWith(params map[string]string) string {
	c.reader = he.LineBuffer{}
	c.connected = true
	c.resetPending = true
	line := he.Hello{Params: params}.Line()
	if len(c.nodes) == 0 {
		c.reset()
	}
	c.changes.Connection = true
	return line
}

// Receive feeds bytes received from the transport.
func (c *Client) Receive(data []byte) {
	for _, line := range c.reader.Push(data) {
		c.ReceiveLine(line)
	}
}

func (c *Client) ReceiveLine(line string) {
	tag, body, err := he.ParseLine(line)
	if err != nil {
		c.diag("ignored line: %v", err)
		return
	}
	c.ReceiveMessage(tag, body)
}

// ReceiveMessage processes a message given as a tag and its JSON object.
func (c *Client) ReceiveMessage(tag string, body map[string]any) {
	if c.resetPending {
		c.reset()
	}
	switch tag {
	case "element":
		c.element(body)
	case "remove":
		if id, ok := body["id"].(string); ok {
			c.remove(id)
		}
	case "signal":
		c.patch(body)
	case "metadata":
		c.setMetadata(he.MetadataFromMap(body))
	case "log":
		l, err := he.LogFromMap(body)
		if err != nil {
			c.diag("ignored log: %v", err)
		} else {
			c.logs = append(c.logs, l)
		}
	default:
		c.diag("ignored message with unknown tag '%s'", tag)
	}
}

// Apply processes a message, as the in-process transport.
func (c *Client) Apply(m he.ServerMessage) {
	c.ReceiveLine(strings.TrimSuffix(m.Line(), "\n"))
}

// Close marks the transport closed. The tree is kept for display.
func (c *Client) Close() {
	c.connected = false
	c.changes.Connection = true
}

func (c *Client) Connected() bool {
	return c.connected
}

// --- reading -----------------------------------------------------------

// Children returns the children of id, or of the root if id is "".
func (c *Client) Children(id string) []string {
	if id == "" {
		return c.root
	}
	if n, ok := c.nodes[id]; ok {
		return n.children
	}
	return nil
}

func (c *Client) Contains(id string) bool {
	_, ok := c.nodes[id]
	return ok
}

// Get returns an element with its attributes resolved.
func (c *Client) Get(id string) (Resolved, bool) {
	n, ok := c.nodes[id]
	if !ok {
		return Resolved{}, false
	}
	attrs := map[string]any{}
	for _, a := range n.component.Attrs() {
		attrs[a.Name], _ = c.Attr(id, a.Name)
	}
	return Resolved{
		ID:       id,
		Type:     n.component,
		Parent:   n.parent,
		Children: slices.Clone(n.children),
		Attrs:    attrs,
	}, true
}

// Attr returns one resolved attribute. It reports false if the element or
// attribute does not exist.
func (c *Client) Attr(id, name string) (any, bool) {
	n, ok := c.nodes[id]
	if !ok {
		return nil, false
	}
	spec, ok := n.component.Attr(name)
	if !ok {
		return nil, false
	}
	if spec.Type.IsStructural() {
		return n.el[name], true
	}
	def := spec.DefaultValue(id)
	raw, ok := n.el[name]
	if !ok || raw == nil {
		return def, true
	}
	var value ref.Val
	if _, isExpr := he.IsCel(raw); isExpr {
		prog, ok := n.progs[name]
		if !ok {
			return def, true
		}
		v, err := c.eval(prog)
		if err != nil {
			c.diag("%s.%s: %v", id, name, err)
			return def, true
		}
		value = v
	} else {
		value = types.DefaultTypeAdapter.NativeToValue(fromJSON(raw))
	}
	v, err := check(spec.Type, value, def)
	if err != nil {
		c.diag("%s.%s: %v", id, name, err)
	}
	return v, true
}

// Tree returns the whole tree, resolved.
func (c *Client) Tree() []ResolvedTree {
	return c.treeOf(c.root)
}

func (c *Client) treeOf(ids []string) []ResolvedTree {
	var out []ResolvedTree
	for _, id := range ids {
		r, ok := c.Get(id)
		if !ok {
			continue
		}
		out = append(out, ResolvedTree{ID: r.ID, Type: r.Type, Attrs: r.Attrs, Children: c.treeOf(r.Children)})
	}
	return out
}

// Metadata returns the details the server sent about the application, if
// any.
func (c *Client) Metadata() he.Metadata {
	return c.metadata
}

// Signals returns the signals. Do not modify them.
func (c *Client) Signals() map[string]any {
	return c.signals
}

// Value returns the value at a signal path such as `form.email`.
func (c *Client) Value(path string) (any, bool) {
	segs, ok := signals.ParsePath(path)
	if !ok {
		return nil, false
	}
	return signals.GetPath(c.signals, segs)
}

// --- user input --------------------------------------------------------

// Write writes a value at a signal path, as a control with `bind` does. The
// value is anything that encodes to JSON.
func (c *Client) Write(path string, value any) {
	segs, ok := signals.ParsePath(path)
	if !ok {
		return
	}
	v, err := jsonx.Normalize(value)
	if err != nil {
		c.diag("write %s: %v", path, err)
		return
	}
	c.patch(signals.PathPatch(segs, v))
}

// Trigger runs the handler in attribute event of element id, following
// section 10 of the spec. It returns the action line to send, if any. A URL
// to open is queued for [Client.TakeOpens].
func (c *Client) Trigger(id, event string) (string, bool) {
	n, ok := c.nodes[id]
	if !ok {
		return "", false
	}
	if spec, ok := n.component.Attr(event); !ok || spec.Type.Kind != he.AttrHandler {
		return "", false
	}
	h, ok := n.el[event].(map[string]any)
	if !ok {
		return "", false
	}
	for cur := id; cur != ""; cur = c.nodes[cur].parent {
		if v, _ := c.Attr(cur, "visible"); v != true {
			return "", false
		}
		if v, _ := c.Attr(cur, "disabled"); v == true {
			return "", false
		}
	}
	if patch, ok := h["patch"].(map[string]any); ok {
		c.patch(c.evalPatch(patch, id+"."+event))
	}
	if open, ok := h["open"]; ok {
		if url, ok := c.evalOpen(open, id+"."+event); ok {
			c.opens = append(c.opens, url)
		}
	}
	action, ok := h["action"].(string)
	if !ok || !c.connected {
		return "", false
	}
	if ind, ok := h["indicator"].(string); ok {
		if segs, ok := signals.ParsePath(ind); ok {
			c.patch(signals.PathPatch(segs, true))
		}
	}
	msg := &he.Action{Name: action, Signals: signals.StripLocal(c.signals).(map[string]any)}
	if p, ok := h["payload"]; ok {
		msg.Payload = jsonx.MustMarshal(p)
	}
	return msg.Line(), true
}

// --- changes -----------------------------------------------------------

// TakeChanges returns what changed since the last call.
func (c *Client) TakeChanges() Changes {
	ch := c.changes
	c.changes = newChanges()
	if ch.signals {
		for id, n := range c.nodes {
			if n.readsSignals {
				ch.Updated[id] = true
			}
		}
	}
	// Tabs and lists present facts about their children.
	var touched []string
	for id := range ch.Updated {
		touched = append(touched, id)
	}
	for id := range ch.Reordered {
		touched = append(touched, id)
	}
	listLike := func(t he.Component) bool { return t == he.Tabs || t == he.List }
	for _, id := range touched {
		n, ok := c.nodes[id]
		if !ok {
			continue
		}
		if listLike(n.component) {
			ch.Updated[id] = true
		}
		if n.parent != "" && listLike(c.nodes[n.parent].component) {
			ch.Updated[n.parent] = true
		}
	}
	for id := range ch.Removed {
		delete(ch.Updated, id)
		delete(ch.Created, id)
	}
	return ch
}

// TakeDiagnostics returns the diagnostics reported since the last call.
func (c *Client) TakeDiagnostics() []string {
	d := c.diagnostics
	c.diagnostics = nil
	return d
}

// TakeOpens returns and clears the URLs that handlers asked to open (a
// button's `open`), oldest first. The host decides whether and how to open
// them.
func (c *Client) TakeOpens() []string {
	o := c.opens
	c.opens = nil
	return o
}

// TakeLogs returns and clears the log messages received since the last call,
// oldest first. The host decides whether and how to show them.
func (c *Client) TakeLogs() []he.Log {
	l := c.logs
	c.logs = nil
	return l
}

// --- internals ---------------------------------------------------------

func (c *Client) diag(format string, args ...any) {
	c.diagnostics = append(c.diagnostics, fmt.Sprintf(format, args...))
}

func (c *Client) setMetadata(m he.Metadata) {
	if !c.metadata.Equal(m) {
		c.metadata = m
		c.changes.Metadata = true
	}
}

func (c *Client) reset() {
	c.resetPending = false
	c.setMetadata(he.Metadata{})
	for id := range c.nodes {
		c.changes.Removed[id] = true
	}
	c.nodes = map[string]*node{}
	c.root = nil
	c.signals = map[string]any{}
	c.celSignals = nil
	c.changes.Reset = true
	c.changes.signals = true
	c.changes.Reordered[""] = true
}

func (c *Client) eval(prog cel.Program) (ref.Val, error) {
	if c.celSignals == nil {
		c.celSignals = fromJSON(c.signals)
	}
	v, _, err := prog.Eval(map[string]any{"s": c.celSignals})
	return v, err
}

func (c *Client) patch(patch any) {
	var changes [][]string
	c.signals = signals.MergePatchTracked(c.signals, patch, nil, &changes).(map[string]any)
	c.celSignals = nil
	c.changes.signals = true
}

func (c *Client) evalPatch(patch map[string]any, at string) map[string]any {
	out := map[string]any{}
	for k, v := range patch {
		if src, ok := he.IsCel(v); ok {
			r, err := c.evalSource(src)
			if err != nil {
				c.diag("%s.patch.%s: %v", at, k, err)
				continue
			}
			out[k] = r
		} else if m, ok := v.(map[string]any); ok {
			out[k] = c.evalPatch(m, at+"."+k)
		} else {
			out[k] = v
		}
	}
	return out
}

func (c *Client) evalSource(src string) (any, error) {
	ast, iss := c.env.Compile(src)
	if iss.Err() != nil {
		return nil, iss.Err()
	}
	prog, err := c.env.Program(ast)
	if err != nil {
		return nil, err
	}
	v, err := c.eval(prog)
	if err != nil {
		return nil, err
	}
	return ToJSON(v)
}

// evalOpen returns the URL of a handler's open, or false if its expression
// fails or does not give a string.
func (c *Client) evalOpen(open any, at string) (string, bool) {
	src, ok := he.IsCel(open)
	if !ok {
		s, ok := open.(string)
		return s, ok
	}
	ast, iss := c.env.Compile(src)
	if iss.Err() != nil {
		c.diag("%s.open: %v", at, iss.Err())
		return "", false
	}
	prog, err := c.env.Program(ast)
	if err != nil {
		c.diag("%s.open: %v", at, err)
		return "", false
	}
	v, err := c.eval(prog)
	if err != nil {
		c.diag("%s.open: %v", at, err)
		return "", false
	}
	s, ok := v.(types.String)
	if !ok {
		c.diag("%s.open: expected a string, got %s", at, typeName(v))
		return "", false
	}
	return string(s), true
}

func (c *Client) reject(el map[string]any, format string, args ...any) {
	id := string(jsonx.MustMarshal(el["id"]))
	c.diag("rejected element %s: %s", id, fmt.Sprintf(format, args...))
}

func (c *Client) element(el map[string]any) {
	id, _ := el["id"].(string)
	if id == "" {
		c.reject(el, "id must be a non-empty string")
		return
	}
	name, _ := el["type"].(string)
	component := he.Component(name)
	if !component.Valid() {
		c.reject(el, "unknown type %s", jsonx.MustMarshal(el["type"]))
		return
	}
	for k, v := range el {
		structural := k == "id" || k == "type" || k == "parent" || k == "before"
		if a, ok := component.Attr(k); ok && a.Type.IsStructural() {
			structural = true
		}
		if _, isExpr := he.IsCel(v); structural && isExpr {
			c.reject(el, "%s cannot be an expression", k)
			return
		}
	}
	for _, k := range []string{"parent", "before"} {
		if v, ok := el[k]; ok {
			if _, isStr := v.(string); !isStr {
				c.reject(el, "%s must be a string", k)
				return
			}
		}
	}
	for _, a := range component.Attrs() {
		v, ok := el[a.Name]
		if !ok {
			continue
		}
		if a.Type.Kind == he.AttrPath {
			s, _ := v.(string)
			if _, ok := signals.ParsePath(s); !ok {
				c.reject(el, "%s is not a signal path", a.Name)
				return
			}
		}
		if a.Type.Kind == he.AttrHandler && !validHandler(v, component == he.Button) {
			c.reject(el, "%s is not a valid handler", a.Name)
			return
		}
	}
	parent, hasParent := el["parent"].(string)
	var parentType he.Component
	if hasParent {
		p, ok := c.nodes[parent]
		if !ok {
			c.reject(el, "parent %q does not exist", parent)
			return
		}
		parentType = p.component
	}
	if !he.Accepts(parentType, component) {
		p := "the root"
		if parentType != "" {
			p = string(parentType)
		}
		c.reject(el, "%s cannot contain %s", p, component)
		return
	}
	if _, exists := c.nodes[id]; exists {
		for cur := parent; cur != ""; cur = c.nodes[cur].parent {
			if cur == id {
				c.reject(el, "an element cannot be inside itself")
				return
			}
		}
	}

	n := &node{component: component, parent: parent, el: el, progs: map[string]cel.Program{}}
	for _, a := range component.Attrs() {
		v, ok := el[a.Name]
		if !ok {
			continue
		}
		if a.Type.Kind == he.AttrPath {
			n.readsSignals = true
		}
		src, isExpr := he.IsCel(v)
		if !isExpr {
			continue
		}
		n.readsSignals = true
		ast, iss := c.env.Compile(src)
		if iss.Err() != nil {
			c.diag("%s.%s: %v", id, a.Name, iss.Err())
			continue
		}
		prog, err := c.env.Program(ast)
		if err != nil {
			c.diag("%s.%s: %v", id, a.Name, err)
			continue
		}
		n.progs[a.Name] = prog
	}
	before, _ := el["before"].(string)

	old, exists := c.nodes[id]
	if !exists {
		c.nodes[id] = n
		c.insert(id, parent, before)
		c.changes.Created[id] = true
		delete(c.changes.Removed, id)
	} else {
		delete(c.nodes, id)
		if old.component == component {
			n.children = old.children
		} else {
			for _, ch := range old.children {
				c.removeTree(ch)
			}
			c.changes.Created[id] = true
		}
		c.nodes[id] = n
		if old.parent != parent {
			c.setChildren(old.parent, slices.DeleteFunc(c.Children(old.parent), func(x string) bool { return x == id }))
			c.changes.Reordered[old.parent] = true
			c.insert(id, parent, before)
		} else if before != "" && before != id {
			sib := c.Children(parent)
			if slices.Contains(sib, before) {
				sib = slices.DeleteFunc(sib, func(x string) bool { return x == id })
				i := slices.Index(sib, before)
				c.setChildren(parent, slices.Insert(sib, i, id))
				c.changes.Reordered[parent] = true
			}
		}
	}
	c.changes.Updated[id] = true
}

func (c *Client) setChildren(id string, children []string) {
	if id == "" {
		c.root = children
	} else {
		c.nodes[id].children = children
	}
}

func (c *Client) insert(id, parent, before string) {
	list := c.Children(parent)
	if i := slices.Index(list, before); before != "" && i >= 0 {
		list = slices.Insert(list, i, id)
	} else {
		list = append(list, id)
	}
	c.setChildren(parent, list)
	c.changes.Reordered[parent] = true
}

func (c *Client) remove(id string) {
	n, ok := c.nodes[id]
	if !ok {
		return
	}
	c.setChildren(n.parent, slices.DeleteFunc(c.Children(n.parent), func(x string) bool { return x == id }))
	c.changes.Reordered[n.parent] = true
	c.removeTree(id)
}

func (c *Client) removeTree(id string) {
	n, ok := c.nodes[id]
	if !ok {
		return
	}
	delete(c.nodes, id)
	for _, ch := range n.children {
		c.removeTree(ch)
	}
	c.changes.Removed[id] = true
	delete(c.changes.Created, id)
	delete(c.changes.Updated, id)
	delete(c.changes.Reordered, id)
}

// validHandler: open is allowed only when canOpen (on a button).
func validHandler(v any, canOpen bool) bool {
	h, ok := v.(map[string]any)
	if !ok {
		return false
	}
	patch, hasPatch := h["patch"]
	action, hasAction := h["action"]
	open, hasOpen := h["open"]
	if !hasPatch && !hasAction && !hasOpen {
		return false
	}
	if hasOpen {
		_, isExpr := he.IsCel(open)
		s, _ := open.(string)
		if !canOpen || !isExpr && s == "" {
			return false
		}
	}
	if _, ok := patch.(map[string]any); hasPatch && !ok {
		return false
	}
	if s, ok := action.(string); hasAction && (!ok || s == "") {
		return false
	}
	if i, ok := h["indicator"]; ok {
		s, _ := i.(string)
		p, ok := signals.ParsePath(s)
		if !hasAction || !ok || !strings.HasPrefix(p[0], "_") {
			return false
		}
	}
	return true
}

// fromJSON converts a JSON value to the native values cel-go adapts, as in
// section 9.2 of the spec: integral numbers become ints.
func fromJSON(v any) any {
	switch v := v.(type) {
	case json.Number:
		if i, err := v.Int64(); err == nil {
			return i
		}
		f, _ := v.Float64()
		return f
	case int:
		return int64(v)
	case []any:
		out := make([]any, len(v))
		for i, x := range v {
			out[i] = fromJSON(x)
		}
		return out
	case map[string]any:
		out := make(map[string]any, len(v))
		for k, x := range v {
			out[k] = fromJSON(x)
		}
		return out
	}
	return v
}

// ToJSON converts a CEL value to JSON, for a handler's patch.
func ToJSON(v ref.Val) (any, error) {
	switch v := v.(type) {
	case types.Null:
		return nil, nil
	case types.Bool:
		return bool(v), nil
	case types.Int:
		return json.Number(fmt.Sprint(int64(v))), nil
	case types.Uint:
		return json.Number(fmt.Sprint(uint64(v))), nil
	case types.Double:
		f := float64(v)
		if math.IsInf(f, 0) || math.IsNaN(f) {
			return nil, fmt.Errorf("no JSON representation for a non-finite double")
		}
		return json.Number(string(jsonx.MustMarshal(f))), nil
	case types.String:
		return string(v), nil
	case traits.Mapper:
		out := map[string]any{}
		for it := v.Iterator(); it.HasNext() == types.True; {
			k := it.Next()
			ks, ok := k.(types.String)
			if !ok {
				return nil, fmt.Errorf("map key is not a string")
			}
			x, err := ToJSON(v.Get(k))
			if err != nil {
				return nil, err
			}
			out[string(ks)] = x
		}
		return out, nil
	case traits.Lister:
		var out []any
		for it := v.Iterator(); it.HasNext() == types.True; {
			x, err := ToJSON(it.Next())
			if err != nil {
				return nil, err
			}
			out = append(out, x)
		}
		if out == nil {
			out = []any{}
		}
		return out, nil
	}
	return nil, fmt.Errorf("no JSON representation for %s", v.Type().(ref.Type).TypeName())
}

func typeName(v ref.Val) string {
	return v.Type().(ref.Type).TypeName()
}

// check checks a CEL value against an attribute type (section 9.3 of the
// spec). On failure it returns the default and an error.
func check(ty he.AttrType, v ref.Val, def any) (any, error) {
	fail := func(want string) (any, error) {
		return def, fmt.Errorf("expected %s, got %s", want, typeName(v))
	}
	switch ty.Kind {
	case he.AttrEnum:
		s, ok := v.(types.String)
		if !ok {
			return fail("a string")
		}
		if slices.Contains(ty.Values, string(s)) {
			return string(s), nil
		}
		return def, nil
	case he.AttrString:
		if s, ok := v.(types.String); ok {
			return string(s), nil
		}
		return fail("a string")
	case he.AttrBool:
		if b, ok := v.(types.Bool); ok {
			return bool(b), nil
		}
		return fail("a bool")
	case he.AttrNumber, he.AttrNumberOrNull:
		var f float64
		switch n := v.(type) {
		case types.Null:
			if ty.Kind == he.AttrNumberOrNull {
				return nil, nil
			}
			return fail("a number")
		case types.Int:
			f = float64(n)
		case types.Uint:
			f = float64(n)
		case types.Double:
			f = float64(n)
		default:
			return fail("a number")
		}
		if math.IsInf(f, 0) || math.IsNaN(f) {
			return fail("a finite number")
		}
		return f, nil
	case he.AttrOptions:
		l, ok := v.(traits.Lister)
		if !ok {
			return fail("a list")
		}
		out := []any{}
		for it := l.Iterator(); it.HasNext() == types.True; {
			item := it.Next()
			m, ok := item.(traits.Mapper)
			var value, label types.String
			if ok {
				value, ok = m.Get(types.String("value")).(types.String)
			}
			if ok {
				label, ok = m.Get(types.String("label")).(types.String)
			}
			if !ok {
				return def, fmt.Errorf("options must be objects with string value and label")
			}
			out = append(out, map[string]any{"value": string(value), "label": string(label)})
		}
		return out, nil
	}
	panic("hypereffect: structural attributes are not checked")
}
