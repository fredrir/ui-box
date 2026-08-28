use std::str::FromStr;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use super::executor::{snap_json, Executor};
use super::{driver_options, ensure_surface, probe_backend, terminate};
use crate::backend;
use crate::cli::{ActArgs, CloseArgs, EvalArgs, OpenArgs, SnapArgs, WakeArgs};
use crate::config::{Config, Viewport};
use crate::driver::{self, Connection};
use crate::flow::{self, SnapStep, Step};
use crate::note;
use crate::output::Summary;
use crate::run::{now_iso, now_unix, Meta, RunDir};
use crate::session::{SessionRecord, SessionStore};

pub fn open(config: &Config, args: &OpenArgs) -> Result<Summary> {
    let surface = config.surface_or(args.surface);
    let target = args
        .target
        .clone()
        .or_else(|| config.target.clone())
        .context("no target: pass one, or set UIBOX_TARGET / target in uibox.toml")?;
    let viewport = match &args.viewport {
        Some(raw) => Viewport::from_str(raw)?,
        None => config.viewport,
    };

    let backend = backend::select(config)?;
    probe_backend(backend.as_ref())?;

    let spec = driver::resolve(surface, config)?;
    let run = RunDir::create(&config.artifacts)?;
    let store = SessionStore::new(config);
    let session_dir = store.create_dir(&run.id)?;

    let mut conn = Connection::spawn_detached(&spec, &session_dir, config.rpc_timeout)?;
    let pid = conn.pid().unwrap_or_default();

    let opened = (|| -> Result<(driver::DriverInfo, String)> {
        let info = conn.info()?;
        ensure_surface(&info, surface)?;
        let session = conn.open(&target, viewport, driver_options(config, surface, &run))?;
        Ok((info, session))
    })();

    let (info, driver_session) = match opened {
        Ok(value) => value,
        Err(err) => {
            terminate(pid);
            let _ = store.remove(&run.id);
            return Err(err);
        }
    };

    let now = now_unix();
    let record = SessionRecord {
        id: run.id.clone(),
        driver_session: driver_session.clone(),
        driver_name: info.name.clone(),
        driver_argv: spec.argv.clone(),
        pid,
        surface,
        target: target.clone(),
        viewport,
        backend: backend.url(),
        run_dir: run.path.clone(),
        session_dir,
        created_unix: now,
        last_used_unix: now,
        ttl_secs: config.session_ttl.as_secs(),
        step_count: 0,
    };
    store.save(&record)?;

    let mut meta = Meta::new(&run.id, &backend.url(), surface);
    meta.project = config.project.clone();
    meta.lab = config.lab.clone();
    meta.flow = args.flow.clone();
    meta.target = Some(target.clone());
    meta.viewport = Some(viewport);
    run.write_meta(&meta)?;

    note!("session {} open on {surface} at {target}", run.id);
    note!("run directory {}", run.path.display());
    note!("record it with `ui-box record {}`", run.id);

    Ok(Summary::ok(json!({
        "session": run.id,
        "run": run.id,
        "run_dir": run.path,
        "surface": surface.as_str(),
        "target": target,
        "viewport": viewport.label(),
        "backend": backend.url(),
        "driver": { "name": info.name, "version": info.version },
        "expires_in": record.expires_in(),
    })))
}

pub fn act(config: &Config, args: &ActArgs) -> Result<Summary> {
    let store = SessionStore::new(config);
    let mut record = store.load(&args.session)?;
    record.ensure_usable()?;
    let run = RunDir::open(&config.artifacts, &record.id)?;
    let conn = record.connect(config.rpc_timeout)?;

    let step = build_step(args)?;
    let mut executor = Executor::new(conn, run, record.driver_session.clone(), record.step_count);
    let outcome = executor.perform(&step);

    record.step_count = executor.total;
    record.touch();
    store.save(&record)?;

    let mut meta = executor.run.read_meta()?;
    meta.steps_total = executor.total;
    meta.steps_failed += executor.failed;
    executor.run.write_meta(&meta)?;

    let outcome = outcome?;
    let body = json!({
        "session": record.id,
        "run": record.id,
        "verb": step.verb(),
        "step": step.label(),
        "step_ok": outcome.ok,
        "error": outcome.error,
        "steps_total": executor.total,
        "expires_in": record.expires_in(),
    });
    if outcome.ok {
        note!("{} ok", step.label());
        Ok(Summary::ok(body))
    } else {
        note!(
            "{} failed: {}",
            step.label(),
            outcome.error.as_deref().unwrap_or("no reason given")
        );
        Ok(Summary::failed(body))
    }
}

pub fn snap(config: &Config, args: &SnapArgs) -> Result<Summary> {
    let store = SessionStore::new(config);
    let mut record = store.load(&args.session)?;
    record.ensure_usable()?;
    let run = RunDir::open(&config.artifacts, &record.id)?;
    let conn = record.connect(config.rpc_timeout)?;

    let step = Step::Snap(SnapStep::Detail {
        name: args.name.clone(),
        mode: Some(args.mode),
    });
    let mut executor = Executor::new(conn, run, record.driver_session.clone(), record.step_count);
    let outcome = executor.perform(&step);

    record.step_count = executor.total;
    record.touch();
    store.save(&record)?;

    let mut meta = executor.run.read_meta()?;
    meta.steps_total = executor.total;
    meta.steps_failed += executor.failed;
    executor.run.write_meta(&meta)?;

    let outcome = outcome?;
    let artifacts = outcome.snap.unwrap_or_default();
    note!(
        "snapshot {} in {}",
        artifacts.name,
        executor.run.snaps_dir().display()
    );
    Ok(Summary::ok(json!({
        "session": record.id,
        "run": record.id,
        "snap": snap_json(&artifacts),
        "steps_total": executor.total,
        "expires_in": record.expires_in(),
    })))
}

pub fn close(config: &Config, args: &CloseArgs) -> Result<Summary> {
    let store = SessionStore::new(config);
    let record = store.load(&args.session)?;
    let mut driver_closed = false;

    if crate::driver::client::process_alive(record.pid) {
        match record.connect(config.rpc_timeout) {
            Ok(mut conn) => match conn.close(&record.driver_session) {
                Ok(()) => driver_closed = true,
                Err(err) => note!("driver did not confirm close: {err}"),
            },
            Err(err) => note!("cannot reach the driver channel: {err}"),
        }
        terminate(record.pid);
    }

    let run = RunDir::open(&config.artifacts, &record.id)?;
    let mut meta = run.read_meta()?;
    meta.ended = Some(now_iso());
    meta.steps_total = record.step_count;
    meta.verdict = if meta.steps_failed > 0 {
        "fail".to_string()
    } else {
        "pass".to_string()
    };
    run.write_meta(&meta)?;

    if !args.keep_channel {
        store.remove(&record.id)?;
    }

    note!(
        "session {} closed, run kept in {}",
        record.id,
        run.path.display()
    );
    Ok(Summary::ok(json!({
        "session": record.id,
        "run": record.id,
        "run_dir": run.path,
        "driver_closed": driver_closed,
        "verdict": meta.verdict,
        "steps_total": meta.steps_total,
        "steps_failed": meta.steps_failed,
    })))
}

fn build_step(args: &ActArgs) -> Result<Step> {
    match (&args.raw, args.step.is_empty()) {
        (Some(raw), true) => {
            serde_yaml::from_str(raw).with_context(|| format!("cannot parse step {raw:?}"))
        }
        (Some(_), false) => {
            bail!("act takes either positional step words or --yaml, not both")
        }
        (None, _) => flow::parse_positional(&args.step),
    }
}

pub fn eval(config: &Config, args: &EvalArgs) -> Result<Summary> {
    let store = SessionStore::new(config);
    let mut record = store.load(&args.session)?;
    record.ensure_usable()?;
    let mut conn = record.connect(config.rpc_timeout)?;
    let value = conn.eval(&record.driver_session, &args.expr)?;
    record.touch();
    store.save(&record)?;
    Ok(Summary::ok(json!({
        "session": record.id,
        "eval": args.expr,
        "value": value,
        "expires_in": record.expires_in(),
    })))
}

pub fn wake(config: &Config, args: &WakeArgs) -> Result<Summary> {
    let lab = args
        .lab
        .clone()
        .or_else(|| config.backend.ssh_target())
        .or_else(|| config.lab.clone());
    let Some(lab) = lab else {
        note!("nothing to wake: the backend is local");
        return Ok(Summary::ok(json!({ "lab": Value::Null, "state": "local" })));
    };
    let (state, detail) = backend::wake(&lab, config.force, Duration::from_secs(args.wait));
    match &detail {
        Some(detail) => note!("{lab} is {state}: {detail}"),
        None => note!("{lab} is {state}"),
    }
    Ok(Summary::ok(
        json!({ "lab": lab, "state": state, "detail": detail }),
    ))
}
