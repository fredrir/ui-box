use anyhow::{bail, Context, Result};
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use crate::lab::{Lab, RunOutcome};
use crate::sh;

const PREPARE_SCRIPT: &str = r#"
set -u
staging=$(realpath -m @STAGING@) || exit 8
checkout=$(realpath -m @CHECKOUT@) || exit 8
parent=${staging%/}
case "$checkout" in
    "$staging"|"$parent"/*) exit 9 ;;
esac
mkdir -p "$staging"
"#;

pub fn stage(lab: &Lab, project: &str, local_root: &Path) -> Result<PathBuf> {
    if !local_root.is_dir() {
        bail!(
            "source {} is not a directory on this host",
            local_root.display()
        );
    }

    let staging = lab.home()?.join(".uibox/src").join(sh::slug(project));
    let checkout = lab.project_dir()?;

    refuse_overlap(&staging, &checkout)?;
    prepare(lab, &staging, &checkout)?;
    push_tree(lab, local_root, &staging)?;

    Ok(staging)
}

pub fn refuse_overlap(staging: &Path, checkout: &Path) -> Result<()> {
    if normalize(checkout).starts_with(normalize(staging)) {
        bail!(
            "refusing to sync into {}: it is, or contains, the lab's own checkout at {}, which may hold work that exists nowhere else",
            staging.display(),
            checkout.display()
        );
    }
    Ok(())
}

fn normalize(path: &Path) -> PathBuf {
    path.components()
        .filter(|part| !matches!(part, Component::CurDir))
        .collect()
}

fn prepare(lab: &Lab, staging: &Path, checkout: &Path) -> Result<()> {
    let script = PREPARE_SCRIPT
        .replace("@STAGING@", &sh::quote_path(staging))
        .replace("@CHECKOUT@", &sh::quote_path(checkout));

    let run = lab.run(&script)?;
    match run.code {
        0 => Ok(()),
        9 => bail!(
            "refusing to sync into {} on {}: after resolving symlinks it is, or contains, the lab's own checkout at {}",
            staging.display(),
            lab.name,
            checkout.display()
        ),
        _ => bail!(
            "could not prepare {} on {}:\n{}",
            staging.display(),
            lab.name,
            run.combined()
        ),
    }
}

fn push_tree(lab: &Lab, local_root: &Path, staging: &Path) -> Result<()> {
    let mut origin = local_root.to_string_lossy().to_string();
    if !origin.ends_with('/') {
        origin.push('/');
    }

    let output = Command::new("rsync")
        .arg("-a")
        .arg("-z")
        .arg("--delete")
        .arg("--filter=:- .gitignore")
        .arg(&origin)
        .arg(format!(
            "{}:{}",
            lab.name,
            sh::quote(&format!("{}/", staging.to_string_lossy()))
        ))
        .output()
        .context("could not run rsync on this host")?;

    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim_end());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed(staging: &str, checkout: &str) -> bool {
        refuse_overlap(Path::new(staging), Path::new(checkout)).is_ok()
    }

    #[test]
    fn allows_the_staging_path_beside_the_checkout() {
        assert!(allowed(
            "/home/fredrir/.uibox/src/archtex",
            "/home/fredrir/ArchTeX"
        ));
    }

    #[test]
    fn refuses_writing_onto_the_checkout_itself() {
        assert!(!allowed("/home/fredrir/ArchTeX", "/home/fredrir/ArchTeX"));
    }

    #[test]
    fn refuses_a_staging_path_that_contains_the_checkout() {
        assert!(!allowed("/home/fredrir", "/home/fredrir/ArchTeX"));
        assert!(!allowed("/", "/home/fredrir/ArchTeX"));
    }

    #[test]
    fn is_not_fooled_by_a_trailing_slash_or_a_dot() {
        assert!(!allowed("/home/fredrir/ArchTeX/", "/home/fredrir/ArchTeX"));
        assert!(!allowed("/home/fredrir/./ArchTeX", "/home/fredrir/ArchTeX"));
    }

    #[test]
    fn allows_staging_below_a_home_directory_project_root() {
        assert!(allowed("/home/fredrir/.uibox/src/cuda", "/home/fredrir"));
    }

    #[test]
    fn does_not_confuse_a_shared_name_prefix() {
        assert!(allowed("/home/fredrir/Arch", "/home/fredrir/ArchTeX"));
    }
}
