use std::path::Path;

use crate::flow::Step;

pub const MIN_VALUE_CHARS: usize = 2;
pub const MAX_VALUE_CHARS: usize = 80;

const HEADING: u8 = 0;
const NAVIGABLE: u8 = 1;
const NAMED: u8 = 2;
const CONTENT: u8 = 3;

pub fn text_selector(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("text=\"{escaped}\"")
}

pub fn assertion_for(snapshot: &str) -> Option<String> {
    identifying_value(snapshot).map(|value| text_selector(&value))
}

pub fn derive(steps: &[Step], snaps_dir: &Path) -> (Vec<Step>, usize) {
    let mut seen: Vec<String> = steps
        .iter()
        .filter_map(|step| match step {
            Step::AssertText(selector) => Some(selector.clone()),
            _ => None,
        })
        .collect();
    let mut out = Vec::with_capacity(steps.len());
    let mut derived = 0;

    for step in steps {
        out.push(step.clone());
        let Some(name) = step.snap().and_then(|snap| snap.name()) else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(snaps_dir.join(format!("{name}.txt"))) else {
            continue;
        };
        let Some(selector) = assertion_for(&text) else {
            continue;
        };
        if seen.contains(&selector) {
            continue;
        }
        seen.push(selector.clone());
        out.push(Step::AssertText(selector));
        derived += 1;
    }
    (out, derived)
}

fn identifying_value(snapshot: &str) -> Option<String> {
    let mut best: Option<(u8, String)> = None;
    for line in snapshot.lines() {
        let Some((rank, value)) = candidate(line) else {
            continue;
        };
        let better = match &best {
            Some((held, _)) => rank < *held,
            None => true,
        };
        if better {
            if rank == HEADING {
                return Some(value);
            }
            best = Some((rank, value));
        }
    }
    best.map(|(_, value)| value)
}

fn candidate(line: &str) -> Option<(u8, String)> {
    let body = line.trim_start().strip_prefix("- ")?;
    if let Some(text) = body.strip_prefix("text: ") {
        return usable(text).map(|value| (CONTENT, value));
    }
    let role = body.split([' ', ':']).next()?;
    let name = quoted_name(body)?;
    let rank = match role {
        "heading" => HEADING,
        "link" | "button" | "tab" => NAVIGABLE,
        _ => NAMED,
    };
    usable(&name).map(|value| (rank, value))
}

fn quoted_name(body: &str) -> Option<String> {
    let mut chars = body.split_once('"')?.1.chars();
    let mut out = String::new();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => out.push(chars.next()?),
            '"' => return Some(out),
            other => out.push(other),
        }
    }
    None
}

fn usable(value: &str) -> Option<String> {
    let value = value.trim();
    let width = value.chars().count();
    if !(MIN_VALUE_CHARS..=MAX_VALUE_CHARS).contains(&width) {
        return None;
    }
    Some(value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flow::SnapStep;

    const DASHBOARD: &str = "- banner:\n  - link \"Home\"\n- main:\n  \
         - heading \"Costs\" [level=1]:\n    - text: all · 11\n  - button \"Refresh\"\n";

    #[test]
    fn a_heading_identifies_a_page_state() {
        assert_eq!(assertion_for(DASHBOARD), Some("text=\"Costs\"".to_string()));
    }

    #[test]
    fn a_page_without_a_heading_falls_back_to_what_it_does_have() {
        assert_eq!(
            assertion_for("- banner:\n  - link \"Home\"\n  - text: hello there\n"),
            Some("text=\"Home\"".to_string())
        );
        assert_eq!(
            assertion_for("- text: all · 11\n"),
            Some("text=\"all · 11\"".to_string())
        );
    }

    #[test]
    fn a_blank_snapshot_yields_no_assertion_rather_than_a_weak_one() {
        for snapshot in ["", "\n\n", "- text: x\n", "- generic:\n"] {
            assert_eq!(assertion_for(snapshot), None, "{snapshot:?}");
        }
    }

    #[test]
    fn a_derived_selector_survives_quotes_and_slashes() {
        assert_eq!(
            assertion_for("- heading \"say \\\"hi\\\"\"\n"),
            Some("text=\"say \\\"hi\\\"\"".to_string())
        );
        assert_eq!(
            assertion_for("- text: /docs/api\n"),
            Some("text=\"/docs/api\"".to_string()),
            "an unquoted text= body starting with a slash would parse as a regex"
        );
    }

    #[test]
    fn a_value_too_long_to_be_a_stable_assertion_is_not_used() {
        let long = "x".repeat(MAX_VALUE_CHARS + 1);
        assert_eq!(assertion_for(&format!("- text: {long}\n")), None);
    }

    fn snap(name: &str) -> Step {
        Step::Snap(SnapStep::Named(name.to_string()))
    }

    #[test]
    fn every_snap_gains_an_assertion_from_what_it_captured() {
        let dir = std::env::temp_dir().join(format!("uibox-derive-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("one.txt"), DASHBOARD).expect("one");
        std::fs::write(dir.join("two.txt"), "- heading \"Detail\"\n").expect("two");

        let steps = vec![
            Step::Click("css=#go".into()),
            snap("one"),
            Step::Click("css=#next".into()),
            snap("two"),
        ];
        let (out, derived) = derive(&steps, &dir);

        assert_eq!(derived, 2);
        assert_eq!(out.len(), 6);
        assert_eq!(out[2], Step::AssertText("text=\"Costs\"".into()));
        assert_eq!(out[5], Step::AssertText("text=\"Detail\"".into()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_repeated_page_state_is_not_asserted_twice() {
        let dir = std::env::temp_dir().join(format!("uibox-derive-dup-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("a.txt"), DASHBOARD).expect("a");
        std::fs::write(dir.join("b.txt"), DASHBOARD).expect("b");

        let (out, derived) = derive(&[snap("a"), snap("b")], &dir);
        assert_eq!(derived, 1);
        assert_eq!(out.len(), 3);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_snap_with_no_artifact_derives_nothing_and_does_not_fail() {
        let dir = std::env::temp_dir().join(format!("uibox-derive-none-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let (out, derived) = derive(&[snap("missing")], &dir);
        assert_eq!(derived, 0);
        assert_eq!(out.len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_assertion_already_in_the_flow_is_not_duplicated() {
        let dir = std::env::temp_dir().join(format!("uibox-derive-held-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("a.txt"), DASHBOARD).expect("a");
        let steps = vec![Step::AssertText("text=\"Costs\"".into()), snap("a")];
        let (out, derived) = derive(&steps, &dir);
        assert_eq!(derived, 0);
        assert_eq!(out.len(), 2);
        std::fs::remove_dir_all(&dir).ok();
    }
}
