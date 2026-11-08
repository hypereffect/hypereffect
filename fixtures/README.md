# Renderer fixtures

Each `.txt` file is a script run against a renderer core (the Rust, TypeScript,
Go and Python clients all run them). Lines are:

- `tag {json}` — a server message, fed to the renderer.
- `< tag {json}` — the next line the renderer is expected to send. Every line
  the renderer sends must be matched before the next `!` command and by the
  end of the file.
- `!start` — begin a session (the renderer sends hello).
- `!write path json` — the user changes a control bound to `path`.
- `!trigger id event` — the user triggers the handler `event` of element `id`.
- `!close` — the transport closes.
- `=attrs id {json}` — the resolved attributes of `id` include these values.
- `=children id [ids]` — the children of `id` (`-` for the root).
- `=signals {json}` — the signals are exactly this.
- `=metadata {json}` — the application metadata is exactly this.
- `=logs [json]` — the log messages received since the last check are
  exactly this.
- `=absent id` — no element has this id.
- `#` starts a comment. Empty lines are ignored.

Numbers are compared by value, so `1` and `1.0` are equal.
