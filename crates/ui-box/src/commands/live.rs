use std::str::FromStr;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use super::executor::{snap_json, Executor};
use super::flows::NOTHING_VERDICT;
use super::{driver_options, driver_run_dir, ensure_surface, probe_backend, terminate};
use crate::backend;
use crate::cli::{ActArgs, CloseArgs, EvalArgs, OpenArgs, SnapArgs, WakeArgs};
use crate::config::{Config, Forward, Viewport};
use crate::driver::{self, forward, Connection};
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

    forward::guard(config, &target, &format!("ui-box open {target}"))?;

    let backend = backend::select(config)?;
    probe_backend(backend.as_ref())?;

    let spec = driver::resolve(surface, config)?;
    let run = RunDir::create(&config.artifacts)?;
    let remote_run_dir = driver_run_dir(&spec, backend.as_ref(), &run)?;
    let driver_dir = remote_run_dir.clone().unwrap_or_else(|| run.path.clone());
    let store = SessionStore::new(config);
    let session_dir = store.create_dir(&run.id)?;

    let mut conn = Connection::spawn_detached(&spec, &session_dir, config.rpc_timeout)?;
    let pid = conn.pid().unwrap_or_default();

    let opened = (|| -> Result<(driver::DriverInfo, String)> {
        let info = conn.info()?;
        ensure_surface(&info, surface)?;
        let session = conn.open(
            &target,
            viewport,
            driver_options(config, surface, &driver_dir),
        )?;
        Ok((info, session))
    })();

    let (info, driver_session) = match opened {
        Ok(value) => value,
        Err(err) => {
            terminate(pid);
            let _ = store.remove(&run.id);
            return Err(forward::classify(err, config));
        }
    };

    let now = now_unix();
    let record = SessionRecord {
        id: run.id.clone(),
        driver_session: driver_session.clone(),
        driver_name: spec.name.clone(),
        driver_argv: spec.argv.clone(),
        pid,
        surface,
        target: target.clone(),
        viewport,
        backend: backend.url(),
        run_dir: run.path.clone(),
        remote_run_dir,
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
    meta.forward = config.forward.clone();
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
        "forward": config.forward.iter().map(Forward::label).collect::<Vec<String>>(),
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
    if record.remote_run_dir.is_some() {
        executor = executor.pulling_from(backend::select(config)?);
    }
    let outcome = executor.perform(&step);

    record.step_count = executor.total;
    record.touch();
    store.save(&record)?;

    let mut meta = executor.run.read_meta()?;
    meta.steps_total = executor.total;
    meta.steps_failed += executor.failed;
    meta.steps_nothing += executor.nothing;
    executor.run.write_meta(&meta)?;

    let outcome = outcome?;
    let status = if outcome.ok {
        "passed"
    } else if outcome.verified_nothing() {
        NOTHING_VERDICT
    } else {
        "failed"
    };
    let body = json!({
        "session": record.id,
        "run": record.id,
        "status": status,
        "verb": step.verb(),
        "step": step.label(),
        "step_ok": outcome.ok,
        "error": outcome.error,
        "error_kind": outcome.kind,
        "steps_total": executor.total,
        "expires_in": record.expires_in(),
    });
    if outcome.ok {
        note!("{} ok", step.label());
        return Ok(Summary::ok(body));
    }
    let reason = outcome.error.as_deref().unwrap_or("no reason given");
    if outcome.verified_nothing() {
        note!("{} proved nothing: {reason}", step.label());
        note!("this is not the UI failing, the page was never in a state to check");
        return Ok(Summary::nothing(body));
    }
    note!("{} failed: {reason}", step.label());
    Ok(Summary::failed(body))
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
    if record.remote_run_dir.is_some() {
        executor = executor.pulling_from(backend::select(config)?);
    }
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
    let mut snap = snap_json(&artifacts);
    if let (Some(map), Some(text)) = (snap.as_object_mut(), &artifacts.text) {
        map.insert("text_inline".to_string(), json!(text));
    }
    Ok(Summary::ok(json!({
        "session": record.id,
        "run": record.id,
        "snap": snap,
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
    } else if meta.steps_nothing > 0 {
        NOTHING_VERDICT.to_string()
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
        "status": meta.verdict,
        "verdict": meta.verdict,
        "steps_total": meta.steps_total,
        "steps_failed": meta.steps_failed,
        "steps_nothing": meta.steps_nothing,
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
    let outcome = conn.eval_result(&record.driver_session, &args.expr)?;
    record.touch();
    store.save(&record)?;

    let carried_nothing = outcome.carried_nothing();
    let body = json!({
        "session": record.id,
        "eval": args.expr,
        "status": if carried_nothing { NOTHING_VERDICT } else { "passed" },
        "value": outcome.value,
        "value_kind": outcome.kind,
        "serializable": outcome.serializable,
        "detail": outcome.detail,
        "expires_in": record.expires_in(),
    });
    if !carried_nothing {
        return Ok(Summary::ok(body));
    }
    note!(
        "{} evaluated to {}, which the driver could not carry over the wire",
        args.expr,
        outcome.kind.as_deref().unwrap_or("a value")
    );
    note!("the null below is the absence of an answer, not the answer");
    note!(
        "wrap it in something serialisable to see it, e.g. `ui-box eval {} 'JSON.stringify({})'`",
        record.id,
        args.expr
    );
    Ok(Summary::nothing(body))
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
