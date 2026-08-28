use std::path::{Path, PathBuf};

use serde_json::Value;

pub const CONSOLE: &str = "console.jsonl";
pub const NETWORK: &str = "network.jsonl";

pub const DEFAULT_MAX_CHARS: usize = 20_000;
pub const MAX_EVENTS: usize = 20;
pub const MAX_IMAGE_BYTES: usize = 3 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cursor {
    pub console: usize,
    pub network: usize,
}

#[derive(Debug, Default)]
pub struct Events {
    pub console: Vec<String>,
    pub network: Vec<String>,
    pub console_dropped: usize,
    pub network_dropped: usize,
    pub cursor: Cursor,
}

impl Events {
    pub fn render(&self) -> Vec<(String, String)> {
        let mut blocks = Vec::new();
        if !self.console.is_empty() {
            let mut body = self.console.join("\n");
            if self.console_dropped > 0 {
                body.push_str(&format!("\n... and {} more", self.console_dropped));
            }
            blocks.push(("console errors".to_string(), body));
        }
        if !self.network.is_empty() {
            let mut body = self.network.join("\n");
            if self.network_dropped > 0 {
                body.push_str(&format!("\n... and {} more", self.network_dropped));
            }
            blocks.push(("failed network requests".to_string(), body));
        }
        blocks
    }
}

pub async fn events_since(run_dir: &Path, from: Cursor) -> Events {
    let console_lines = read_lines(&run_dir.join(CONSOLE)).await;
    let network_lines = read_lines(&run_dir.join(NETWORK)).await;

    let cursor = Cursor {
        console: console_lines.len(),
        network: network_lines.len(),
    };

    let fresh_console = console_lines
        .get(from.console.min(console_lines.len())..)
        .unwrap_or(&[]);
    let fresh_network = network_lines
        .get(from.network.min(network_lines.len())..)
        .unwrap_or(&[]);

    let mut console: Vec<String> = fresh_console.iter().filter_map(console_error).collect();
    let mut network: Vec<String> = fresh_network.iter().filter_map(network_failure).collect();

    let console_dropped = console.len().saturating_sub(MAX_EVENTS);
    let network_dropped = network.len().saturating_sub(MAX_EVENTS);
    console.truncate(MAX_EVENTS);
    network.truncate(MAX_EVENTS);

    Events {
        console,
        network,
        console_dropped,
        network_dropped,
        cursor,
    }
}

async fn read_lines(path: &Path) -> Vec<Value> {
    let Ok(raw) = tokio::fs::read_to_string(path).await else {
        return Vec::new();
    };
    raw.lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect()
}

fn console_error(entry: &Value) -> Option<String> {
    let kind = entry.get("type").and_then(Value::as_str).unwrap_or("log");
    if kind != "error" && kind != "pageerror" {
        return None;
    }
    let text = entry.get("text").and_then(Value::as_str).unwrap_or("");
    match entry.get("location").and_then(Value::as_str) {
        Some(location) if !location.is_empty() => Some(format!("[{kind}] {text} ({location})")),
        _ => Some(format!("[{kind}] {text}")),
    }
}

fn network_failure(entry: &Value) -> Option<String> {
    let method = entry.get("method").and_then(Value::as_str).unwrap_or("?");
    let url = entry.get("url").and_then(Value::as_str).unwrap_or("?");
    if let Some(failure) = entry.get("failure").and_then(Value::as_str) {
        return Some(format!("{method} {url} -> failed: {failure}"));
    }
    let status = entry.get("status").and_then(Value::as_u64)?;
    (status >= 400).then(|| format!("{method} {url} -> {status}"))
}

pub async fn snapshot_text(path: &Path, max_chars: usize) -> Option<(String, usize)> {
    let raw = tokio::fs::read_to_string(path).await.ok()?;
    let total = raw.chars().count();
    if total <= max_chars {
        return Some((raw, 0));
    }
    let kept: String = raw.chars().take(max_chars).collect();
    Some((kept, total - max_chars))
}

pub async fn snapshot_png(path: &Path) -> Result<Vec<u8>, String> {
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(format!(
            "{} is {} bytes, over the {MAX_IMAGE_BYTES} byte inline limit; open the file directly",
            path.display(),
            bytes.len()
        ));
    }
    Ok(bytes)
}

pub fn snap_paths(snap: &Value) -> (Option<PathBuf>, Option<PathBuf>) {
    let text = snap.get("text").and_then(Value::as_str).map(PathBuf::from);
    let png = snap.get("png").and_then(Value::as_str).map(PathBuf::from);
    (text, png)
}

pub fn last_png(snaps: &Value) -> Option<PathBuf> {
    snaps
        .as_array()?
        .iter()
        .rev()
        .find_map(|snap| snap.get("png").and_then(Value::as_str).map(PathBuf::from))
}

pub fn last_text(snaps: &Value) -> Option<PathBuf> {
    snaps
        .as_array()?
        .iter()
        .rev()
        .find_map(|snap| snap.get("text").and_then(Value::as_str).map(PathBuf::from))
}
