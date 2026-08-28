use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{json, Map, Value};

static QUIET: AtomicBool = AtomicBool::new(false);

pub fn set_quiet(quiet: bool) {
    QUIET.store(quiet, Ordering::Relaxed);
}

pub fn quiet() -> bool {
    QUIET.load(Ordering::Relaxed)
}

pub fn note(message: impl AsRef<str>) {
    if !quiet() {
        let mut stderr = std::io::stderr().lock();
        let _ = writeln!(stderr, "{}", message.as_ref());
    }
}

#[macro_export]
macro_rules! note {
    ($($arg:tt)*) => {
        $crate::output::note(format!($($arg)*))
    };
}

pub const EXIT_PASS: i32 = 0;
pub const EXIT_FAIL: i32 = 1;
pub const EXIT_TOOL: i32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone)]
pub struct Summary {
    value: Value,
    ok: bool,
    exit_code: i32,
    stream: Stream,
}

impl Summary {
    pub fn ok(value: Value) -> Self {
        Summary {
            value,
            ok: true,
            exit_code: 0,
            stream: Stream::Stdout,
        }
    }

    pub fn failed(value: Value) -> Self {
        Summary {
            value,
            ok: false,
            exit_code: 1,
            stream: Stream::Stdout,
        }
    }

    pub fn unusable(value: Value) -> Self {
        Summary {
            value,
            ok: false,
            exit_code: EXIT_TOOL,
            stream: Stream::Stdout,
        }
    }

    pub fn on_stderr(mut self) -> Self {
        self.stream = Stream::Stderr;
        self
    }

    pub fn exit_code(&self) -> i32 {
        self.exit_code
    }

    pub fn render(&self) -> String {
        let mut map = match &self.value {
            Value::Object(map) => map.clone(),
            other => {
                let mut map = Map::new();
                map.insert("result".to_string(), other.clone());
                map
            }
        };
        map.insert("ok".to_string(), Value::Bool(self.ok));
        let ordered = Value::Object(map);
        serde_json::to_string(&ordered).unwrap_or_else(|_| r#"{"ok":false}"#.to_string())
    }

    pub fn emit(&self) {
        match self.stream {
            Stream::Stdout => println!("{}", self.render()),
            Stream::Stderr => {
                let mut stderr = std::io::stderr().lock();
                let _ = writeln!(stderr, "{}", self.render());
            }
        }
    }
}

pub fn error_summary(kind: &str, message: &str, context: Option<&str>) -> String {
    let mut value = json!({ "ok": false, "error_kind": kind, "error": message });
    if let Some(context) = context {
        value["context"] = Value::String(context.to_string());
    }
    serde_json::to_string(&value).unwrap_or_else(|_| r#"{"ok":false}"#.to_string())
}
