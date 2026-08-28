use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use super::executor::{snap_json, Executor, SnapArtifacts};
use super::{driver_options, ensure_surface, probe_backend, terminate};
use crate::backend;
use crate::cli::{RunArgs, VerifyArgs};
use crate::config::{Config, Viewport};
use crate::driver::{self, Connection};
use crate::flow::{Flow, Step};
use crate::note;
use crate::output::Summary;
use crate::pipeline::{self, Placement, Request};
use crate::run::{now_iso, Meta, RunDir};
use crate::vision::Vision;

pub const ARTIFACT_TOKEN: &str = "{{artifact}}";
pub const FLOWS_DIR: &str = "flows";

struct Outcome {
    body: Value,
    passed: bool,
}

pub fn run(config: &Config, args: &RunArgs) -> Result<Summary> {
    let outcome = execute(config, args, None)?;
    Ok(finish(outcome))
}

pub fn verify(config: &Config, args: &VerifyArgs) -> Result<Summary> {
    if let Some(since) = &args.since {
        if !moved_since(since) {
            note!("nothing changed since {since}, skipping verification");
            return Ok(Summary::ok(json!({
                "skipped": true,
                "since": since,
                "reason": "the tree has not moved since this ref",
            })));
        }
    }

    let flows = discover_flows(config, args)?;
    if flows.is_empty() {
        note!("no flows to verify");
        return Ok(Summary::ok(json!({
            "skipped": true,
            "reason": "no flows found",
            "looked_in": flows_dir(config, args),
        })));
    }

    let mut results = Vec::new();
    let mut failed = 0;
    for path in &flows {
        let mut run_args = args.run.clone();
        run_args.flow = Some(path.clone());
        let outcome = execute(config, &run_args, Some(args))?;
        if !outcome.passed {
            failed += 1;
        }
        results.push(outcome.body);
    }

    let body = json!({
        "since": args.since,
        "flows": flows.len(),
        "failed": failed,
        "verdict": if failed == 0 { "pass" } else { "fail" },
        "results": results,
    });
    note!("{} of {} flows failed", failed, flows.len());
    Ok(if failed == 0 {
        Summary::ok(body)
    } else {
        Summary::failed(body)
    })
}

fn finish(outcome: Outcome) -> Summary {
    if outcome.passed {
        Summary::ok(outcome.body)
    } else {
        Summary::failed(outcome.body)
    }
}

fn execute(config: &Config, args: &RunArgs, verify: Option<&VerifyArgs>) -> Result<Outcome> {
    let flow_path = args
        .flow
        .clone()
        .context("no flow: pass a step-format yaml file, e.g. `ui-box run flows/checkout.yaml`")?;
    let flow = Flow::load(&flow_path)?;
    let surface = args.surface.unwrap_or(flow.surface);
    let viewport = match args.viewport.as_ref().or(flow.viewport.as_ref()) {
        Some(raw) => Viewport::from_str(raw)?,
        None => config.viewport,
    };

    let backend = backend::select(config)?;
    probe_backend(backend.as_ref())?;

    let placement = if args.no_place {
        None
    } else {
        place_artifact(config, args)?
    };
    let target = effective_target(
        args.target.clone().unwrap_or_else(|| flow.target.clone()),
        placement.as_ref(),
    );

    let spec = driver::resolve(surface, config)?;
    let run = RunDir::create(&config.artifacts)?;

    let mut meta = Meta::new(&run.id, &backend.url(), surface);
    meta.project = args.project.clone().or_else(|| config.project.clone());
    meta.lab = args.lab.clone().or_else(|| config.lab.clone());
    meta.flow = Some(flow.flow.clone());
    meta.target = Some(target.clone());
    meta.viewport = Some(viewport);
    if let Some(placed) = &placement {
        meta.git_sha = placed.git_sha.clone().or(meta.git_sha);
        meta.diff_hash = placed.diff_hash.clone();
        meta.artifact_hash = placed.artifact_hash.clone();
        meta.remote_path = Some(placed.remote_path.display().to_string());
        meta.source = placed
            .source
            .as_ref()
            .map(|tree| tree.display().to_string());
        meta.cached = Some(placed.cached);
    }
    run.write_meta(&meta)?;

    note!("run {} on {surface} at {target}", run.id);

    let mut conn = Connection::spawn(&spec, config.rpc_timeout)?;
    let pid = conn.pid().unwrap_or_default();
    let prepared = (|| -> Result<String> {
        let info = conn.info()?;
        ensure_surface(&info, surface)?;
        conn.open(&target, viewport, driver_options(config, surface, &run))
    })();
    let driver_session = match prepared {
        Ok(session) => session,
        Err(err) => {
            terminate(pid);
            meta.ended = Some(now_iso());
            meta.verdict = "error".to_string();
            run.write_meta(&meta)?;
            return Err(err);
        }
    };

    let mut executor = Executor::new(conn, run, driver_session, 0);
    let mut snaps: Vec<SnapArtifacts> = Vec::new();
    let mut halted: Option<String> = None;
    let mut failure: Option<anyhow::Error> = None;
    let mut opened_target = false;

    for step in &flow.steps {
        if let Step::Open(requested) = step {
            if !opened_target && requested == &target {
                opened_target = true;
                executor.run.append_step(step)?;
                executor.total += 1;
                continue;
            }
        }
        match executor.perform(step) {
            Ok(outcome) => {
                if let Some(artifacts) = outcome.snap {
                    snaps.push(artifacts);
                }
                if outcome.ok {
                    note!("ok   {}", step.label());
                } else {
                    note!(
                        "fail {}: {}",
                        step.label(),
                        outcome.error.as_deref().unwrap_or("no reason given")
                    );
                    if !args.keep_going {
                        halted = Some(step.label());
                        break;
                    }
                }
            }
            Err(err) => {
                halted = Some(step.label());
                failure = Some(err);
                break;
            }
        }
    }

    let goldens = match (verify, failure.is_none()) {
        (Some(args), true) => compare_goldens(config, args, &flow, &executor.run, &snaps),
        _ => Ok(None),
    };

    executor.close();
    terminate(pid);

    let goldens = match goldens {
        Ok(report) => report,
        Err(err) => {
            meta.steps_total = executor.total;
            meta.steps_failed = executor.failed;
            meta.ended = Some(now_iso());
            meta.verdict = "error".to_string();
            executor.run.write_meta(&meta)?;
            return Err(err);
        }
    };

    let golden_failures = goldens
        .as_ref()
        .map(|report| report.differing.len())
        .unwrap_or(0);
    meta.steps_total = executor.total;
    meta.steps_failed = executor.failed + golden_failures;
    meta.ended = Some(now_iso());
    meta.verdict = if failure.is_some() {
        "error".to_string()
    } else if meta.steps_failed > 0 {
        "fail".to_string()
    } else {
        "pass".to_string()
    };
    executor.run.write_meta(&meta)?;

    if verify.is_some() {
        write_report(config, &executor.run);
    }

    if let Some(err) = failure {
        return Err(err);
    }

    let passed = meta.verdict == "pass";
    let body = json!({
        "run": executor.run.id,
        "run_dir": executor.run.path,
        "flow": flow.flow,
        "flow_file": flow_path,
        "surface": surface.as_str(),
        "target": target,
        "backend": backend.url(),
        "verdict": meta.verdict,
        "steps_total": meta.steps_total,
        "steps_failed": meta.steps_failed,
        "halted_at": halted,
        "placed": placement.as_ref().map(|placed| json!({
            "remote_path": placed.remote_path,
            "cached": placed.cached,
        })),
        "snaps": snaps.iter().map(snap_json).collect::<Vec<Value>>(),
        "goldens": goldens.as_ref().map(golden_json),
    });

    note!(
        "verdict {} ({} of {} steps failed)",
        meta.verdict,
        meta.steps_failed,
        meta.steps_total
    );
    Ok(Outcome { body, passed })
}

fn place_artifact(config: &Config, args: &RunArgs) -> Result<Option<Placement>> {
    let Some(artifact) = args.artifact.clone() else {
        note!("no --artifact: replaying against the target as it stands");
        return Ok(None);
    };
    let project = args
        .project
        .clone()
        .or_else(|| config.project.clone())
        .context("placing an artifact needs --project or project in uibox.toml")?;
    let lab = args
        .lab
        .clone()
        .or_else(|| config.lab.clone())
        .context("placing an artifact needs --lab, the lab holding the checkout under test")?;
    let target = args
        .target_lab
        .clone()
        .or_else(|| config.target_lab.clone())
        .or_else(|| config.backend.host().map(str::to_string))
        .context(
            "cannot tell which lab to place into: set UIBOX_BACKEND to the target lab \
             or pass --target-lab",
        )?;

    let explicit = args
        .source
        .clone()
        .or_else(|| config.source.clone().map(PathBuf::from));
    let source = resolve_source(
        explicit.as_deref(),
        config.project_root.as_deref(),
        args.lab_checkout,
    )?;
    let build_backend = backend::for_lab(config, &lab)?;
    note!("building in {lab}, placing into {target}");
    match &source {
        Some(tree) => note!("syncing {} into {lab}", tree.display()),
        None => note!("building from {lab}'s own checkout"),
    }
    let request = Request {
        project,
        lab,
        target,
        source,
        build: args.build.clone(),
        artifact,
    };
    let placed = pipeline::place(&request, build_backend.as_ref())?;
    if placed.cached {
        note!(
            "artifact already placed at {}",
            placed.remote_path.display()
        );
    } else {
        note!("placed artifact at {}", placed.remote_path.display());
    }
    Ok(Some(placed))
}

fn resolve_source(
    explicit: Option<&Path>,
    project_root: Option<&Path>,
    lab_checkout: bool,
) -> Result<Option<PathBuf>> {
    if lab_checkout {
        return Ok(None);
    }
    if let Some(explicit) = explicit {
        if !explicit.is_dir() {
            anyhow::bail!("--source {} is not a directory", explicit.display());
        }
        return Ok(Some(absolute(explicit)));
    }
    if let Some(root) = project_root {
        let root = absolute(root);
        if !root.join(".git").exists() {
            match git_toplevel().filter(|top| top != &root) {
                Some(top) => note!(
                    "uibox.toml sits at {}, which is not a git root, so the tree synced to the \
                     lab carries no .git. Nothing fails here; a build in the lab that \
                     version-stamps from git fails later. Pass --source {} if yours does",
                    root.display(),
                    top.display()
                ),
                None => note!(
                    "{} is not a git checkout, so the tree synced to the lab carries no git \
                     metadata",
                    root.display()
                ),
            }
        }
        return Ok(Some(root));
    }
    if let Some(root) = git_toplevel() {
        return Ok(Some(root));
    }
    note!(
        "no uibox.toml and no git checkout above the working directory, \
         so nothing local is synced; pass --source DIR to override"
    );
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lab_checkout_syncs_nothing() {
        let root = std::env::temp_dir();
        assert_eq!(
            resolve_source(Some(&root), Some(&root), true).unwrap(),
            None
        );
    }

    #[test]
    fn an_explicit_tree_wins_over_the_project_root() {
        let explicit = std::env::temp_dir();
        let root = PathBuf::from("/");
        let resolved = resolve_source(Some(&explicit), Some(&root), false)
            .unwrap()
            .unwrap();
        assert_eq!(resolved, explicit.canonicalize().unwrap());
    }

    #[test]
    fn the_project_root_is_the_default_tree() {
        let root = std::env::temp_dir();
        let resolved = resolve_source(None, Some(&root), false).unwrap().unwrap();
        assert_eq!(resolved, root.canonicalize().unwrap());
        assert!(resolved.is_absolute());
    }

    #[test]
    fn a_tree_that_is_not_a_directory_is_rejected() {
        let missing = std::env::temp_dir().join("uibox-no-such-tree-9f3a");
        let err = resolve_source(Some(&missing), None, false).unwrap_err();
        assert!(err.to_string().contains("is not a directory"), "{err}");
    }
}

fn absolute(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn git_toplevel() -> Option<PathBuf> {
    let root = git(&["rev-parse", "--show-toplevel"])?;
    let root = PathBuf::from(root.trim());
    root.is_dir().then_some(root)
}

fn effective_target(target: String, placement: Option<&Placement>) -> String {
    let Some(placed) = placement else {
        return target;
    };
    let remote = placed.remote_path.display().to_string();
    if target.contains(ARTIFACT_TOKEN) {
        return target.replace(ARTIFACT_TOKEN, &remote);
    }
    if target.trim() == "exec:" || target.trim().is_empty() {
        return format!("exec:{remote}");
    }
    target
}

fn flows_dir(config: &Config, args: &VerifyArgs) -> PathBuf {
    args.flows.clone().unwrap_or_else(|| {
        config
            .project_root
            .clone()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(FLOWS_DIR)
    })
}

fn discover_flows(config: &Config, args: &VerifyArgs) -> Result<Vec<PathBuf>> {
    if let Some(explicit) = &args.run.flow {
        return Ok(vec![explicit.clone()]);
    }
    let dir = flows_dir(config, args);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut flows: Vec<PathBuf> = std::fs::read_dir(&dir)
        .with_context(|| format!("cannot read {}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.is_file()
                && matches!(
                    path.extension().and_then(|ext| ext.to_str()),
                    Some("yaml") | Some("yml")
                )
        })
        .collect();
    flows.sort();
    Ok(flows)
}

fn moved_since(since: &str) -> bool {
    let range = format!("{since}...HEAD");
    let committed = git(&["diff", "--name-only", &range]);
    let dirty = git(&["status", "--porcelain"]);
    match (committed, dirty) {
        (Some(committed), Some(dirty)) => !committed.trim().is_empty() || !dirty.trim().is_empty(),
        _ => {
            note!("cannot compare against {since}, verifying anyway");
            true
        }
    }
}

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[derive(Debug, Default)]
struct GoldenReport {
    compared: usize,
    differing: Vec<String>,
    missing: Vec<String>,
    approved: Vec<String>,
    entries: Vec<Value>,
}

fn compare_goldens(
    config: &Config,
    args: &VerifyArgs,
    flow: &Flow,
    run: &RunDir,
    snaps: &[SnapArtifacts],
) -> Result<Option<GoldenReport>> {
    let candidates: Vec<&SnapArtifacts> = snaps
        .iter()
        .filter(|snap| snap.png_path.is_some())
        .collect();
    let mut report = GoldenReport::default();
    if candidates.is_empty() {
        note!("no png snapshots in this flow, nothing to compare against goldens");
        return Ok(Some(report));
    }

    let store = config
        .goldens
        .clone()
        .context("comparing goldens needs UIBOX_GOLDENS or --goldens")?;
    let vision = Vision::require(config)?;
    let prefix = match &args.golden_prefix {
        Some(prefix) => prefix.clone(),
        None => {
            let project = args
                .run
                .project
                .clone()
                .or_else(|| config.project.clone())
                .context(
                    "golden names need --project, project in uibox.toml, or --golden-prefix",
                )?;
            format!("{project}/{}", flow.flow)
        }
    };

    let staging = std::env::temp_dir().join(format!("uibox-goldens-{}", run.id));
    std::fs::create_dir_all(&staging)
        .with_context(|| format!("cannot create {}", staging.display()))?;
    let sha = run
        .read_meta()
        .ok()
        .and_then(|meta| meta.git_sha)
        .unwrap_or_default();

    for snap in candidates {
        let Some(candidate) = &snap.png_path else {
            continue;
        };
        let name = format!("{prefix}/{}", snap.name);
        let golden = staging.join(format!("{}.png", snap.name));
        let present = vision.golden_get(&store, &name, &golden)?;
        if !present {
            report.missing.push(name.clone());
            report.entries.push(json!({ "name": name, "state": "new" }));
            if args.update_goldens {
                vision.golden_approve(&store, &name, candidate, &run.id, &sha)?;
                report.approved.push(name);
            }
            continue;
        }
        let out = run.diffs_dir().join(format!("{}.png", snap.name));
        let diff = vision.diff(&golden, candidate, &out)?;
        report.compared += 1;
        if diff.differs {
            if args.update_goldens {
                vision.golden_approve(&store, &name, candidate, &run.id, &sha)?;
                report.approved.push(name.clone());
                report.entries.push(json!({
                    "name": name,
                    "state": "approved",
                    "pixels": diff.pixels,
                    "ratio": diff.ratio,
                    "diff": out,
                }));
            } else {
                report.differing.push(name.clone());
                report.entries.push(json!({
                    "name": name,
                    "state": "differs",
                    "pixels": diff.pixels,
                    "ratio": diff.ratio,
                    "size_mismatch": diff.size_mismatch,
                    "diff": out,
                }));
            }
        } else {
            report
                .entries
                .push(json!({ "name": name, "state": "match" }));
        }
    }

    std::fs::remove_dir_all(&staging).ok();
    Ok(Some(report))
}

fn golden_json(report: &GoldenReport) -> Value {
    json!({
        "compared": report.compared,
        "differing": report.differing,
        "missing": report.missing,
        "approved": report.approved,
        "entries": report.entries,
    })
}

fn write_report(config: &Config, run: &RunDir) {
    let Some(vision) = Vision::locate(config) else {
        note!("no uibox-vision on PATH, skipping report.json");
        return;
    };
    if let Err(err) = vision.report(&run.path, &run.report_path()) {
        note!("cannot write report.json: {err}");
    }
}
