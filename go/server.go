package hypereffect

import (
	"strings"
	"time"

	"github.com/hypereffect/hypereffect/go/internal/jsonx"
)

// Outbox collects messages to send to the renderer, as lines.
type Outbox struct {
	out strings.Builder
}

func (o *Outbox) Send(m ServerMessage) {
	o.out.WriteString(m.Line())
}

func (o *Outbox) Element(e *Element) {
	o.out.WriteString(e.Line())
}

func (o *Outbox) Remove(id string) {
	o.Send(Remove{ID: id})
}

// Signal sends a signal message. patch must encode to a JSON object: a map,
// a Signal or a struct. It panics otherwise.
func (o *Outbox) Signal(patch any) {
	b := jsonx.MustMarshal(patch)
	if len(b) == 0 || b[0] != '{' {
		panic("hypereffect: a signal message must be an object")
	}
	o.out.WriteString("signal ")
	o.out.Write(b)
	o.out.WriteByte('\n')
}

// Metadata sends optional details about the application, replacing any sent
// before.
func (o *Outbox) Metadata(m Metadata) {
	o.Send(m)
}

// Log sends an event for the user or the host's log. It never changes the
// element tree or the signals.
func (o *Outbox) Log(l Log) {
	o.Send(l)
}

func (o *Outbox) IsEmpty() bool {
	return o.out.Len() == 0
}

// String returns the lines written so far.
func (o *Outbox) String() string {
	return o.out.String()
}

// Take returns the lines written so far and empties the outbox.
func (o *Outbox) Take() string {
	s := o.out.String()
	o.out.Reset()
	return s
}

// App is the logic of one session. A new app is created for every hello,
// and dropped when the session ends.
type App interface {
	// Hello is called on hello. The app should build the initial interface.
	Hello(out *Outbox, params map[string]string)
	// Action is called for each action. The action's contents are user
	// input.
	Action(out *Outbox, a *Action)
}

// Ticker is implemented by apps that want a timer.
type Ticker interface {
	// Tick is called periodically, every TickInterval.
	Tick(out *Outbox, now time.Time)
	// TickInterval says how often Tick should be called; 0 for never.
	TickInterval() time.Duration
}

// Server drives an App from the bytes a renderer sends. Connect it to any
// transport that carries lines: write what the renderer sends to Receive,
// and send what Output holds after each call.
type Server[A App] struct {
	newApp func() A
	app    A
	active bool
	input  LineBuffer
	out    Outbox
}

// NewServer returns a server that calls newApp on every hello.
func NewServer[A App](newApp func() A) *Server[A] {
	return &Server[A]{newApp: newApp}
}

// Receive feeds bytes received from the renderer. Messages before hello,
// and lines that cannot be parsed, are ignored.
func (s *Server[A]) Receive(data []byte) {
	for _, line := range s.input.Push(data) {
		m, err := ParseRendererMessage(line)
		if err != nil {
			continue
		}
		switch m := m.(type) {
		case Hello:
			s.app = s.newApp()
			s.active = true
			s.app.Hello(&s.out, m.Params)
		case *Action:
			if s.active {
				s.app.Action(&s.out, m)
			}
		}
	}
}

// Tick runs the app's timer, if it has one.
func (s *Server[A]) Tick(now time.Time) {
	if t, ok := any(s.app).(Ticker); ok && s.active {
		t.Tick(&s.out, now)
	}
}

// TickInterval is how often to call Tick; 0 for never.
func (s *Server[A]) TickInterval() time.Duration {
	if t, ok := any(s.app).(Ticker); ok && s.active {
		return t.TickInterval()
	}
	return 0
}

// Close ends the session.
func (s *Server[A]) Close() {
	var zero A
	s.app, s.active = zero, false
	s.input = LineBuffer{}
}

// App returns the session's app, if a session is open.
func (s *Server[A]) App() (A, bool) {
	return s.app, s.active
}

// Output holds the lines waiting to be sent.
func (s *Server[A]) Output() *Outbox {
	return &s.out
}
