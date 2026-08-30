use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use super::executor::{snap_json, Executor, SnapArtifacts};
use super::{driver_options, driver_run_dir, ensure_surface, probe_backend, terminate};
use crate::backend;
use crate::cli::{RunArgs, VerifyArgs};
use crate::config::{Config, Forward, Viewport};
use crate::driver::{self, forward, Connection};
use crate::error::FlowError;
use crate::flow::{Flow, Step};
use crate::note;
use crate::output::Summary;
use crate::pipeline::{self, Placement, Request};
use crate::run::{now_iso, Meta, RunDir};
use crate::vision::Vision;

pub const ARTIFACT_TOKEN: &str = "{{artifact}}";
pub const FLOWS_DIR: &str = "flows";

pub const NOTHING_VERDICT: &str = "nothing_verified";

struct Outcome {
    body: Value,
    passed: bool,
    verified_nothing: bool,
}

pub fn run(config: &Config, args: &RunArgs) -> Result<Summary> {
    let outcome = execute(config, args, None)?;
    Ok(finish(outcome))
}

pub fn verify(config: &Config, args: &VerifyArgs) -> Result<Summary> {
    if let Some(since) = &args.since {
        if !moved_since(since) {
            note!("nothing verified: the tree has not moved since {since}");
            note!("exit 0 here means no work was done, not that the UI is correct");
            return Ok(Summary::nothing(json!({
                "status": "nothing_verified",
                "skipped": true,
                "flows": 0,
                "since": since,
                "reason": "the tree has not moved since this ref, so no flow was replayed \
                           and no UI was exercised",
            })));
        }
    }

    let flows = discover_flows(config, args)?;
    if flows.is_empty() {
        let looked_in = flows_dir(config, args);
        note!(
            "nothing verified: no flows found in {}",
            looked_in.display()
        );
        note!("exit 0 here means no work was done, not that the UI is correct");
        return Ok(Summary::nothing(json!({
            "status": "nothing_verified",
            "skipped": true,
            "flows": 0,
            "reason": "no flow files were found, so none was replayed and no UI was exercised",
            "looked_in": looked_in,
        })));
    }

    let mut results = Vec::new();
    let mut failed = 0;
    let mut proved_nothing = 0;
    for path in &flows {
        let mut run_args = args.run.clone();
        run_args.flow = Some(path.clone());
        let outcome = execute(config, &run_args, Some(args))?;
        if outcome.verified_nothing {
            proved_nothing += 1;
        } else if !outcome.passed {
            failed += 1;
        }
        results.push(outcome.body);
    }

    let verdict = if failed > 0 {
        "fail"
    } else if proved_nothing > 0 {
        NOTHING_VERDICT
    } else {
        "pass"
    };
    let body = json!({
        "status": verdict,
        "since": args.since,
        "flows": flows.len(),
        "failed": failed,
        "nothing_verified": proved_nothing,
        "verdict": verdict,
        "results": results,
    });
    note!("{} of {} flows failed", failed, flows.len());
    if verdict == NOTHING_VERDICT {
        note!("{proved_nothing} flow(s) proved nothing, which is not a pass");
    }
    Ok(match verdict {
        "fail" => Summary::failed(body),
        NOTHING_VERDICT => Summary::nothing(body),
        _ => Summary::ok(body),
    })
}

fn finish(outcome: Outcome) -> Summary {
    if outcome.verified_nothing {
        Summary::nothing(outcome.body)
    } else if outcome.passed {
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
    if flow.assertions() == 0 && !pinned_by_goldens(config, verify, &flow) {
        return Err(FlowError::AssertsNothing {
            path: flow_path.display().to_string(),
            steps: flow.steps.len(),
        }
        .into());
    }
    let mut skipped: Option<String> = None;
    let surface = args.surface.unwrap_or(flow.surface);
    let viewport = match args.viewport.as_ref().or(flow.viewport.as_ref()) {
        Some(raw) => Viewport::from_str(raw)?,
        None => config.viewport,
    };

    let declared_target = args.target.clone().unwrap_or_else(|| flow.target.clone());
    forward::guard(
        config,
        &declared_target,
        &format!("ui-box run {}", flow_path.display()),
    )?;

    let backend = backend::select(config)?;
    probe_backend(backend.as_ref())?;

    let placement = if args.no_place {
        Placed::Skipped("--no-place was passed".to_string())
    } else {
        place_artifact(config, args)?
    };
    let placement = match placement {
        Placed::Done(placed) => Some(placed),
        Placed::Skipped(reason) => {
            note!("not placing an artifact: {reason}");
            skipped = Some(reason);
            None
        }
    };
    let target = effective_target(declared_target, placement.as_ref());

    let spec = driver::resolve(surface, config)?;
    let run = RunDir::create(&config.artifacts)?;
    let remote_run_dir = driver_run_dir(&spec, backend.as_ref(), &run)?;
    let driver_dir = remote_run_dir.clone().unwrap_or_else(|| run.path.clone());

    let mut meta = Meta::new(&run.id, &backend.url(), surface);
    meta.project = args.project.clone().or_else(|| config.project.clone());
    meta.lab = args.lab.clone().or_else(|| config.lab.clone());
    meta.flow = Some(flow.flow.clone());
    meta.target = Some(target.clone());
    meta.viewport = Some(viewport);
    meta.forward = config.forward.clone();
    if let Some(placed) = &placement {
        apply_placement(&mut meta, placed);
    }
    run.write_meta(&meta)?;

    note!("run {} on {surface} at {target}", run.id);

    let mut conn = Connection::spawn(&spec, config.rpc_timeout)?;
    let pid = conn.pid().unwrap_or_default();
    let prepared = (|| -> Result<String> {
        let info = conn.info()?;
        ensure_surface(&info, surface)?;
        conn.open(
            &target,
            viewport,
            driver_options(config, surface, &driver_dir),
        )
    })();
    let driver_session = match prepared {
        Ok(session) => session,
        Err(err) => {
            terminate(pid);
            meta.ended = Some(now_iso());
            meta.verdict = "error".to_string();
            run.write_meta(&meta)?;
            return Err(forward::classify(err, config));
        }
    };

    let mut executor = Executor::new(conn, run, driver_session, 0);
    if remote_run_dir.is_some() {
        executor = executor.pulling_from(backend::select(config)?);
    }
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
            Ok(mut outcome) => {
                if let Some(artifacts) = outcome.snap.take() {
                    snaps.push(artifacts);
                }
                if outcome.ok {
                    note!("ok   {}", step.label());
                } else {
                    let verdict = if outcome.verified_nothing() {
                        "none"
                    } else {
                        "fail"
                    };
                    note!(
                        "{verdict} {}: {}",
                        step.label(),
                        outcome.error.as_deref().unwrap_or("no reason given")
                    );
                    if outcome.verified_nothing() {
                        note!("this step proved nothing, it is not the UI failing");
                    }
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
    meta.steps_nothing = executor.nothing;
    meta.ended = Some(now_iso());
    meta.verdict = if failure.is_some() {
        "error".to_string()
    } else if meta.steps_failed > 0 {
        "fail".to_string()
    } else if meta.steps_nothing > 0 {
        NOTHING_VERDICT.to_string()
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
    let verified_nothing = meta.verdict == NOTHING_VERDICT;
    let body = json!({
        "status": meta.verdict,
        "run": executor.run.id,
        "run_dir": executor.run.path,
        "flow": flow.flow,
        "flow_file": flow_path,
        "surface": surface.as_str(),
        "target": target,
        "forward": config.forward.iter().map(Forward::label).collect::<Vec<String>>(),
        "backend": backend.url(),
        "verdict": meta.verdict,
        "steps_total": meta.steps_total,
        "steps_failed": meta.steps_failed,
        "steps_nothing": meta.steps_nothing,
        "halted_at": halted,
        "placed": placement.as_ref().map(|placed| json!({
            "remote_path": placed.remote_path,
            "cached": placed.cached,
        })),
        "placement_skipped": skipped,
        "snaps": snaps.iter().map(snap_json).collect::<Vec<Value>>(),
        "goldens": goldens.as_ref().map(golden_json),
    });

    note!(
        "verdict {} ({} of {} steps failed)",
        meta.verdict,
        meta.steps_failed,
        meta.steps_total
    );
    if verified_nothing {
        note!("nothing was verified: an assertion could not prove anything about the page");
        note!("exit 0 here means no work was done, not that the UI is correct");
    }
    Ok(Outcome {
        body,
        passed,
        verified_nothing,
    })
}

fn pinned_by_goldens(config: &Config, verify: Option<&VerifyArgs>, flow: &Flow) -> bool {
    verify.is_some() && config.goldens.is_some() && flow.image_snaps() > 0
}

enum Placed {
    Done(Placement),
    Skipped(String),
}

fn place_artifact(config: &Config, args: &RunArgs) -> Result<Placed> {
    let artifact = args
        .artifact
        .clone()
        .or_else(|| config.artifact.clone().map(PathBuf::from));
    let Some(artifact) = artifact else {
        return Ok(Placed::Skipped(
            "no artifact: pass --artifact, set artifact in uibox.toml, or pass --no-place"
                .to_string(),
        ));
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
        build: args.build.clone().or_else(|| config.build.clone()),
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
    Ok(Placed::Done(placed))
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
    use crate::config::Surface;

    fn placed() -> Placement {
        Placement {
            remote_path: PathBuf::from("/nix/store/aaa-app/bin/app"),
            source: Some(PathBuf::from("/Users/fredrir/projects/thing")),
            git_sha: Some("c0803e6".to_string()),
            diff_hash: Some("d1ff".to_string()),
            artifact_hash: Some("a271".to_string()),
            cached: true,
        }
    }

    #[test]
    fn provenance_lands_in_the_right_meta_fields() {
        let mut meta = Meta::new(
            "20260828T000000Z-abcdef01",
            "ssh://fredrir@dlab-ui",
            Surface::Web,
        );
        apply_placement(&mut meta, &placed());
        assert_eq!(meta.git_sha.as_deref(), Some("c0803e6"));
        assert_eq!(meta.diff_hash.as_deref(), Some("d1ff"));
        assert_eq!(meta.artifact_hash.as_deref(), Some("a271"));
        assert_eq!(
            meta.remote_path.as_deref(),
            Some("/nix/store/aaa-app/bin/app")
        );
        assert_eq!(
            meta.source.as_deref(),
            Some("/Users/fredrir/projects/thing")
        );
        assert_eq!(meta.cached, Some(true));
    }

    #[test]
    fn a_pipeline_without_a_sha_leaves_the_local_one() {
        let mut meta = Meta::new("20260828T000000Z-abcdef01", "local://", Surface::Web);
        meta.git_sha = Some("local-head".to_string());
        let mut placement = placed();
        placement.git_sha = None;
        apply_placement(&mut meta, &placement);
        assert_eq!(meta.git_sha.as_deref(), Some("local-head"));
    }

    #[test]
    fn the_artifact_token_is_substituted() {
        let placement = placed();
        assert_eq!(
            effective_target("exec:{{artifact}}".to_string(), Some(&placement)),
            "exec:/nix/store/aaa-app/bin/app"
        );
        assert_eq!(
            effective_target("exec:".to_string(), Some(&placement)),
            "exec:/nix/store/aaa-app/bin/app"
        );
    }

    #[test]
    fn a_url_target_survives_placement_untouched() {
        let placement = placed();
        assert_eq!(
            effective_target("http://dlab-ui:3000".to_string(), Some(&placement)),
            "http://dlab-ui:3000"
        );
    }

    #[test]
    fn without_placement_the_target_is_verbatim() {
        assert_eq!(
            effective_target("exec:{{artifact}}".to_string(), None),
            "exec:{{artifact}}"
        );
    }

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

fn apply_placement(meta: &mut Meta, placed: &Placement) {
    meta.git_sha = placed.git_sha.clone().or_else(|| meta.git_sha.clone());
    meta.diff_hash = placed.diff_hash.clone();
    meta.artifact_hash = placed.artifact_hash.clone();
    meta.remote_path = Some(placed.remote_path.display().to_string());
    meta.source = placed
        .source
        .as_ref()
        .map(|tree| tree.display().to_string());
    meta.cached = Some(placed.cached);
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
