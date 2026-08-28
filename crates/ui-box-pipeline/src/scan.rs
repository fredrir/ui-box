use anyhow::{bail, Result};
use std::path::Path;

use crate::lab::{Lab, RunOutcome};
use crate::sh;

const SCAN_SCRIPT: &str = r#"
set -u
target=@ARTIFACT@
[ -e "$target" ] || exit 2
{
    LC_ALL=C grep -rhaoE '/nix/store/[0-9a-z]{32}-[+._?=a-zA-Z0-9-]*' "$target" || true
    if command -v patchelf >/dev/null 2>&1; then
        patchelf --print-interpreter "$target" 2>/dev/null || true
        patchelf --print-rpath "$target" 2>/dev/null | tr ':' '\n' || true
    fi
    if command -v ldd >/dev/null 2>&1; then
        ldd "$target" 2>/dev/null | LC_ALL=C grep -aoE '/nix/store/[0-9a-z]{32}-[+._?=a-zA-Z0-9-]*' || true
    fi
} | LC_ALL=C grep -aoE '^/nix/store/[0-9a-z]{32}-[+._?=a-zA-Z0-9-]*' | sort -u | while IFS= read -r found; do
    [ -e "$found" ] && printf '%s\n' "$found"
done
"#;

pub fn store_refs(lab: &Lab, artifact: &Path) -> Result<Vec<String>> {
    let script = SCAN_SCRIPT.replace("@ARTIFACT@", &sh::quote_path(artifact));
    let run = lab.run(&script)?;

    if run.code == 2 {
        bail!(
            "{} vanished from {} before it could be scanned",
            artifact.display(),
            lab.name
        );
    }
    if !run.succeeded() {
        bail!(
            "could not scan {} for store references on {}:\n{}",
            artifact.display(),
            lab.name,
            run.combined()
        );
    }

    Ok(run
        .stdout
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("/nix/store/"))
        .map(str::to_string)
        .collect())
}
