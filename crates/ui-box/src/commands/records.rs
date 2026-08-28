use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::cli::{RecordArgs, RecordFormat, RunsArgs, ShowArgs, ShowWhat};
use crate::config::{Config, Surface};
use crate::flow::Flow;
use crate::note;
use crate::output::Summary;
use crate::playwright;
use crate::run::{list_runs, RunDir};

pub fn record(config: &Config, args: &RecordArgs) -> Result<Summary> {
    let run = RunDir::open(&config.artifacts, &args.id)?;
    let steps = run.read_steps()?;
    let meta = run.read_meta().ok();

    let surface = meta
        .as_ref()
        .map(|meta| meta.surface)
        .unwrap_or(Surface::Web);
    let target = args
        .target
        .clone()
        .or_else(|| meta.as_ref().and_then(|meta| meta.target.clone()))
        .context("run has no target in meta.json, pass --target")?;
    let name = args
        .flow
        .clone()
        .or_else(|| meta.as_ref().and_then(|meta| meta.flow.clone()))
        .unwrap_or_else(|| run.id.clone());
    let viewport = meta
        .as_ref()
        .and_then(|meta| meta.viewport)
        .map(|view| view.label());

    let flow = Flow {
        version: 1,
        flow: name,
        surface,
        target,
        viewport,
        steps,
    };
    let step_count = flow.steps.len();
    let (rendered, default_name, format) = match args.format {
        RecordFormat::Uibox => (flow.to_yaml()?, "flow.yaml", "uibox"),
        RecordFormat::Playwright => (playwright::emit(&flow), "flow.spec.ts", "playwright"),
    };

    let destination = args
        .out
        .clone()
        .unwrap_or_else(|| run.path.join(default_name));
    if destination == Path::new("-") {
        print!("{rendered}");
        return Ok(Summary::ok(json!({
            "run": run.id,
            "flow": flow.flow,
            "format": format,
            "steps": step_count,
            "out": "-",
        }))
        .on_stderr());
    }

    std::fs::write(&destination, &rendered)
        .with_context(|| format!("cannot write {}", destination.display()))?;
    note!("wrote {step_count} steps to {}", destination.display());
    Ok(Summary::ok(json!({
        "run": run.id,
        "flow": flow.flow,
        "format": format,
        "steps": step_count,
        "out": destination,
    })))
}

pub fn runs(config: &Config, args: &RunsArgs) -> Result<Summary> {
    let all = list_runs(&config.artifacts)?;
    let total = all.len();
    let entries: Vec<Value> = all
        .iter()
        .take(args.limit)
        .map(|run| match run.read_meta() {
            Ok(meta) => json!({
                "run": run.id,
                "started": meta.started,
                "ended": meta.ended,
                "verdict": meta.verdict,
                "surface": meta.surface.as_str(),
                "flow": meta.flow,
                "target": meta.target,
                "steps_total": meta.steps_total,
                "steps_failed": meta.steps_failed,
            }),
            Err(_) => json!({ "run": run.id, "verdict": "unknown" }),
        })
        .collect();

    Ok(Summary::ok(json!({
        "artifacts": config.artifacts,
        "total": total,
        "shown": entries.len(),
        "runs": entries,
    })))
}

pub fn show(config: &Config, args: &ShowArgs) -> Result<Summary> {
    let run = RunDir::open(&config.artifacts, &args.id)?;
    let mut body = json!({ "run": run.id, "run_dir": run.path });

    let want_meta = matches!(args.what, ShowWhat::Meta | ShowWhat::All);
    let want_steps = matches!(args.what, ShowWhat::Steps | ShowWhat::All);
    let want_report = matches!(args.what, ShowWhat::Report | ShowWhat::All);
    let want_snaps = matches!(args.what, ShowWhat::Snaps | ShowWhat::All);

    if want_meta {
        body["meta"] = serde_json::to_value(run.read_meta()?)?;
    }
    if want_steps {
        body["steps"] = serde_json::to_value(run.read_steps()?)?;
    }
    if want_report {
        let path = run.report_path();
        body["report"] = if path.is_file() {
            let raw = std::fs::read_to_string(&path)
                .with_context(|| format!("cannot read {}", path.display()))?;
            serde_json::from_str(&raw).unwrap_or(Value::Null)
        } else {
            Value::Null
        };
    }
    if want_snaps {
        let snaps: Vec<String> = run
            .snapshots()?
            .iter()
            .map(|path| path.display().to_string())
            .collect();
        body["snaps"] = serde_json::to_value(snaps)?;
    }

    Ok(Summary::ok(body))
}
