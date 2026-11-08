//! Runs the shared fixtures in ../../fixtures against `Client`.
use hypereffect::client::Client;
use hypereffect::Value;
use std::collections::VecDeque;
use std::path::Path;

/// Deep equality with numbers compared by value.
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| same(a, b))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
        }
        _ => a == b,
    }
}

fn json(s: &str) -> Value {
    serde_json::from_str(s).unwrap_or_else(|e| panic!("bad JSON in fixture: {s}: {e}"))
}

fn run(path: &Path) {
    let name = path.file_name().unwrap().to_string_lossy();
    let src = std::fs::read_to_string(path).unwrap();
    let mut c = Client::new();
    let mut sent: VecDeque<String> = VecDeque::new();
    let mut failures = Vec::new();
    for (i, line) in src.lines().enumerate() {
        let at = format!("{name}:{}", i + 1);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("< ") {
            let (tag, body) = rest.split_once(' ').unwrap();
            match sent.pop_front() {
                None => failures.push(format!("{at}: expected {rest}, nothing was sent")),
                Some(got) => {
                    let (gtag, gbody) = got.trim_end().split_once(' ').unwrap();
                    if gtag != tag || !same(&json(gbody), &json(body)) {
                        failures.push(format!("{at}: got {}", got.trim_end()));
                    }
                }
            }
            continue;
        }
        if let Some(cmd) = line.strip_prefix('!') {
            if let Some(l) = sent.pop_front() {
                failures.push(format!("{at}: unexpected output {}", l.trim_end()));
                sent.clear();
            }
            let (cmd, args) = cmd.split_once(' ').unwrap_or((cmd, ""));
            match cmd {
                "start" => {
                    sent.push_back(c.hello());
                }
                "write" => {
                    let (p, v) = args.split_once(' ').unwrap();
                    c.write(p, json(v));
                }
                "trigger" => {
                    let (id, ev) = args.split_once(' ').unwrap();
                    sent.extend(c.trigger(id, ev));
                }
                "close" => c.close(),
                _ => panic!("{at}: unknown command {cmd}"),
            }
            continue;
        }
        if let Some(check) = line.strip_prefix('=') {
            let (cmd, args) = check.split_once(' ').unwrap();
            match cmd {
                "attrs" => {
                    let (id, want) = args.split_once(' ').unwrap();
                    let Some(r) = c.get(id) else {
                        failures.push(format!("{at}: no element {id}"));
                        continue;
                    };
                    let Value::Object(want) = json(want) else {
                        panic!()
                    };
                    for (k, v) in &want {
                        let got = r.attrs.get(k).unwrap_or(&Value::Null);
                        if !same(got, v) {
                            failures.push(format!("{at}: {id}.{k} is {got}, want {v}"));
                        }
                    }
                }
                "children" => {
                    let (id, want) = args.split_once(' ').unwrap();
                    let got = c.children(if id == "-" { None } else { Some(id) });
                    let want: Vec<String> = serde_json::from_value(json(want)).unwrap();
                    if got != want.as_slice() {
                        failures.push(format!("{at}: children of {id} are {got:?}"));
                    }
                }
                "signals" => {
                    if !same(c.signals(), &json(args)) {
                        failures.push(format!("{at}: signals are {}", c.signals()));
                    }
                }
                "metadata" => {
                    let got = serde_json::to_value(c.metadata()).unwrap();
                    if !same(&got, &json(args)) {
                        failures.push(format!("{at}: metadata is {got}"));
                    }
                }
                "opened" => {
                    let want: Vec<String> = serde_json::from_value(json(args)).unwrap();
                    let got = c.take_opens();
                    if got != want {
                        failures.push(format!("{at}: opened {got:?}"));
                    }
                }
                "logs" => {
                    let got = serde_json::to_value(c.take_logs()).unwrap();
                    if !same(&got, &json(args)) {
                        failures.push(format!("{at}: logs are {got}"));
                    }
                }
                "absent" => {
                    if c.contains(args) {
                        failures.push(format!("{at}: {args} exists"));
                    }
                }
                _ => panic!("{at}: unknown check {cmd}"),
            }
            continue;
        }
        c.receive_line(line);
    }
    if let Some(l) = sent.pop_front() {
        failures.push(format!(
            "{name}: unexpected output at the end: {}",
            l.trim_end()
        ));
    }
    let opens = c.take_opens();
    if !opens.is_empty() {
        failures.push(format!("{name}: URLs opened but not checked: {opens:?}"));
    }
    let logs = c.take_logs();
    if !logs.is_empty() {
        failures.push(format!("{name}: logs received but not checked: {logs:?}"));
    }
    c.take_diagnostics();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn fixtures() {
    // The shared fixtures live one level above the workspace; the published
    // crate carries them under `fixtures/` instead (see the symlink).
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = manifest.join("fixtures");
    let dir = if dir.exists() {
        dir
    } else {
        manifest.join("../../fixtures")
    };
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "txt"))
        .collect();
    files.sort();
    assert!(!files.is_empty());
    let mut failed = Vec::new();
    for f in &files {
        if let Err(e) = std::panic::catch_unwind(|| run(f)) {
            let msg = e
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default();
            failed.push(msg);
        }
    }
    assert!(failed.is_empty(), "\n{}", failed.join("\n"));
}
