use anyhow::{bail, Context, Result};
use std::path::Path;

use crate::lab::{Lab, RunOutcome};
use crate::sh;

pub struct Source {
    pub git_sha: String,
    pub diff_hash: String,
}

const SOURCE_SCRIPT: &str = r#"
set -u
cd @DIR@ || exit 2
sha=$(git rev-parse HEAD 2>/dev/null) || exit 3
diff=$(
    {
        git --no-pager diff HEAD -- .
        git ls-files --others --exclude-standard -z | sort -z | xargs -0 -r sha256sum
    } | sha256sum | cut -d' ' -f1
)
printf '%s %s\n' "$sha" "$diff"
"#;

pub fn source(lab: &Lab, project_dir: &Path) -> Result<Source> {
    let script = SOURCE_SCRIPT.replace("@DIR@", &sh::quote_path(project_dir));
    let run = lab.run(&script)?;
    interpret(
        run.code,
        run.trimmed_stdout(),
        &run.combined(),
        project_dir,
        &lab.name,
    )
}

pub fn local_source(root: &Path) -> Result<Source> {
    let script = SOURCE_SCRIPT.replace("@DIR@", &sh::quote_path(root));
    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg(&script)
        .output()
        .with_context(|| format!("could not read provenance from {}", root.display()))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    interpret(
        output.status.code().unwrap_or(-1),
        stdout.trim(),
        stderr.trim(),
        root,
        "this host",
    )
}

fn interpret(code: i32, stdout: &str, detail: &str, dir: &Path, place: &str) -> Result<Source> {
    match code {
        0 => {}
        2 => bail!("{} has no checkout at {}", place, dir.display()),
        3 => bail!(
            "{} at {} has no HEAD, so the tree under test cannot be identified",
            place,
            dir.display()
        ),
        _ => bail!("could not read provenance from {}:\n{}", place, detail),
    }

    let mut fields = stdout.split_whitespace();
    let git_sha = fields.next().unwrap_or_default().to_string();
    let diff_hash = fields.next().unwrap_or_default().to_string();

    if git_sha.is_empty() || diff_hash.is_empty() {
        bail!(
            "{} returned an unreadable provenance line: {:?}",
            place,
            stdout
        );
    }

    Ok(Source { git_sha, diff_hash })
}

const ARTIFACT_HASH_SCRIPT: &str = r#"
set -u
target=@ARTIFACT@
if [ -d "$target" ]; then
    find "$target" -type f -exec sha256sum {} + | sort | sha256sum | cut -d' ' -f1
elif [ -e "$target" ]; then
    sha256sum "$target" | cut -d' ' -f1
else
    exit 2
fi
"#;

pub fn artifact_hash(lab: &Lab, artifact: &Path) -> Result<String> {
    let script = ARTIFACT_HASH_SCRIPT.replace("@ARTIFACT@", &sh::quote_path(artifact));
    let run = lab.run(&script)?;

    if !run.succeeded() {
        bail!(
            "the build reported success but {} is not readable on {}:\n{}",
            artifact.display(),
            lab.name,
            run.combined()
        );
    }

    let hash = run.line();
    if hash.is_empty() {
        bail!("could not hash {} on {}", artifact.display(), lab.name);
    }
    Ok(hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct Repo {
        root: PathBuf,
    }

    impl Repo {
        fn new() -> Repo {
            let unique = format!(
                "ui-box-prov-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::SeqCst)
            );
            let root = std::env::temp_dir().join(unique);
            std::fs::create_dir_all(&root).unwrap();

            let repo = Repo { root };
            repo.git(&["init", "-q"]);
            repo.git(&["config", "user.email", "t@t"]);
            repo.git(&["config", "user.name", "t"]);
            repo
        }

        fn git(&self, args: &[&str]) {
            let status = Command::new("git")
                .current_dir(&self.root)
                .args(args)
                .status()
                .unwrap();
            assert!(status.success(), "git {:?} failed", args);
        }

        fn write(&self, relative: &str, body: &str) {
            let path = self.root.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }

        fn commit(&self, message: &str) {
            self.git(&["add", "-A"]);
            self.git(&["commit", "-q", "-m", message]);
        }

        fn app(&self) -> PathBuf {
            self.root.join("app")
        }
    }

    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn seeded() -> Repo {
        let repo = Repo::new();
        repo.write("root.txt", "root\n");
        repo.write("app/app.txt", "app\n");
        repo.commit("init");
        repo
    }

    #[test]
    fn a_change_outside_the_synced_tree_does_not_invalidate() {
        let repo = seeded();
        let before = local_source(&repo.app()).unwrap();

        repo.write("root.txt", "root changed\n");
        let after = local_source(&repo.app()).unwrap();

        assert_eq!(before.diff_hash, after.diff_hash);
    }

    #[test]
    fn a_tracked_change_inside_the_synced_tree_invalidates() {
        let repo = seeded();
        let before = local_source(&repo.app()).unwrap();

        repo.write("app/app.txt", "app changed\n");
        let after = local_source(&repo.app()).unwrap();

        assert_ne!(before.diff_hash, after.diff_hash);
    }

    #[test]
    fn an_untracked_file_inside_the_synced_tree_invalidates() {
        let repo = seeded();
        let before = local_source(&repo.app()).unwrap();

        repo.write("app/fresh.txt", "new\n");
        let after = local_source(&repo.app()).unwrap();

        assert_ne!(before.diff_hash, after.diff_hash);
    }

    #[test]
    fn git_sha_stays_repo_wide_so_a_commit_outside_still_invalidates() {
        let repo = seeded();
        let before = local_source(&repo.app()).unwrap();

        repo.write("root.txt", "root changed\n");
        repo.commit("outside");
        let after = local_source(&repo.app()).unwrap();

        assert_ne!(before.git_sha, after.git_sha);
        assert_eq!(before.diff_hash, after.diff_hash);
    }

    #[test]
    fn a_directory_that_is_not_a_checkout_is_refused() {
        let root = std::env::temp_dir().join(format!("ui-box-bare-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let outcome = local_source(&root);
        let _ = std::fs::remove_dir_all(&root);

        assert!(outcome.is_err());
    }
}
