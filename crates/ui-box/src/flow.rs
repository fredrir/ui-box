use std::fmt;
use std::path::Path;
use std::str::FromStr;

use anyhow::{bail, Context, Result};
use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::config::Surface;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
#[clap(rename_all = "lowercase")]
pub enum SnapMode {
    #[default]
    Text,
    Png,
    Both,
}

impl SnapMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            SnapMode::Text => "text",
            SnapMode::Png => "png",
            SnapMode::Both => "both",
        }
    }

    pub fn wants_text(&self) -> bool {
        matches!(self, SnapMode::Text | SnapMode::Both)
    }

    pub fn wants_png(&self) -> bool {
        matches!(self, SnapMode::Png | SnapMode::Both)
    }
}

impl fmt::Display for SnapMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for SnapMode {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "text" => Ok(SnapMode::Text),
            "png" => Ok(SnapMode::Png),
            "both" => Ok(SnapMode::Both),
            other => bail!("unknown snap mode {other:?}, expected text, png or both"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeStep {
    pub selector: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SnapStep {
    Named(String),
    Detail {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<SnapMode>,
    },
}

impl SnapStep {
    pub fn name(&self) -> Option<&str> {
        match self {
            SnapStep::Named(name) => Some(name),
            SnapStep::Detail { name, .. } => name.as_deref(),
        }
    }

    pub fn mode(&self) -> Option<SnapMode> {
        match self {
            SnapStep::Named(_) => None,
            SnapStep::Detail { mode, .. } => *mode,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Open(String),
    Click(String),
    Type(TypeStep),
    Key(String),
    WaitFor(String),
    AssertText(String),
    AssertAbsent(String),
    Snap(SnapStep),
}

impl Step {
    pub fn verb(&self) -> &'static str {
        match self {
            Step::Open(_) => "open",
            Step::Click(_) => "click",
            Step::Type(_) => "type",
            Step::Key(_) => "key",
            Step::WaitFor(_) => "wait_for",
            Step::AssertText(_) => "assert_text",
            Step::AssertAbsent(_) => "assert_absent",
            Step::Snap(_) => "snap",
        }
    }

    pub fn is_assertion(&self) -> bool {
        matches!(self, Step::AssertText(_) | Step::AssertAbsent(_))
    }

    pub fn snap(&self) -> Option<&SnapStep> {
        match self {
            Step::Snap(snap) => Some(snap),
            _ => None,
        }
    }

    pub fn to_json(&self) -> Result<Value> {
        serde_json::to_value(self).context("cannot encode step")
    }

    pub fn to_yaml_entry(&self) -> Result<String> {
        let rendered = serde_yaml::to_string(&vec![self]).context("cannot encode step as yaml")?;
        Ok(rendered)
    }

    pub fn label(&self) -> String {
        match self {
            Step::Open(target) => format!("open {target}"),
            Step::Click(selector) => format!("click {selector}"),
            Step::Type(step) => format!("type {} into {}", step.text, step.selector),
            Step::Key(key) => format!("key {key}"),
            Step::WaitFor(selector) => format!("wait_for {selector}"),
            Step::AssertText(selector) => format!("assert_text {selector}"),
            Step::AssertAbsent(selector) => format!("assert_absent {selector}"),
            Step::Snap(snap) => match snap.name() {
                Some(name) => format!("snap {name}"),
                None => "snap".to_string(),
            },
        }
    }
}

pub const VERBS: &[&str] = &[
    "open",
    "click",
    "type",
    "key",
    "wait_for",
    "assert_text",
    "assert_absent",
    "snap",
];

impl Serialize for Step {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(1))?;
        match self {
            Step::Open(target) => map.serialize_entry("open", target)?,
            Step::Click(selector) => map.serialize_entry("click", selector)?,
            Step::Type(step) => map.serialize_entry("type", step)?,
            Step::Key(key) => map.serialize_entry("key", key)?,
            Step::WaitFor(selector) => map.serialize_entry("wait_for", selector)?,
            Step::AssertText(selector) => map.serialize_entry("assert_text", selector)?,
            Step::AssertAbsent(selector) => map.serialize_entry("assert_absent", selector)?,
            Step::Snap(snap) => map.serialize_entry("snap", snap)?,
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Step {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        deserializer.deserialize_map(StepVisitor)
    }
}

struct StepVisitor;

impl<'de> Visitor<'de> for StepVisitor {
    type Value = Step;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a step with one verb, such as `click: \"role=button[name=Submit]\"`")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<Step, A::Error> {
        let Some(verb) = map.next_key::<String>()? else {
            return Err(de::Error::custom("a step needs one verb"));
        };
        let step = match verb.as_str() {
            "open" => Step::Open(map.next_value()?),
            "click" => Step::Click(map.next_value()?),
            "type" => Step::Type(map.next_value()?),
            "key" => Step::Key(map.next_value()?),
            "wait_for" => Step::WaitFor(map.next_value()?),
            "assert_text" => Step::AssertText(map.next_value()?),
            "assert_absent" => Step::AssertAbsent(map.next_value()?),
            "snap" => Step::Snap(map.next_value()?),
            other => return Err(de::Error::unknown_variant(other, VERBS)),
        };
        if map.next_key::<String>()?.is_some() {
            return Err(de::Error::custom(format!(
                "a step carries one verb, and this one also carries something after {verb}"
            )));
        }
        Ok(step)
    }
}

pub fn parse_positional(tokens: &[String]) -> Result<Step> {
    let Some((verb, rest)) = tokens.split_first() else {
        bail!("act needs a step, e.g. `ui-box act SESSION click \"role=button[name=Submit]\"`");
    };
    let normalized = verb.trim().replace('-', "_").to_ascii_lowercase();
    let step = match normalized.as_str() {
        "open" => Step::Open(exactly_one(rest, "open TARGET")?),
        "click" => Step::Click(exactly_one(rest, "click SELECTOR")?),
        "key" => Step::Key(exactly_one(rest, "key KEY")?),
        "wait_for" => Step::WaitFor(exactly_one(rest, "wait_for SELECTOR")?),
        "assert_text" => Step::AssertText(exactly_one(rest, "assert_text SELECTOR")?),
        "assert_absent" => Step::AssertAbsent(exactly_one(rest, "assert_absent SELECTOR")?),
        "type" => {
            if rest.len() != 2 {
                bail!("type takes a selector and a text, as in `type \"css=#email\" \"a@b.c\"`");
            }
            Step::Type(TypeStep {
                selector: rest[0].clone(),
                text: rest[1].clone(),
            })
        }
        "snap" => match rest.len() {
            0 => Step::Snap(SnapStep::Detail {
                name: None,
                mode: None,
            }),
            1 => Step::Snap(SnapStep::Named(rest[0].clone())),
            _ => bail!("snap takes at most a name, as in `snap after-submit`"),
        },
        other => bail!(
            "unknown step verb {other:?}, expected one of {}",
            VERBS.join(", ")
        ),
    };
    Ok(step)
}

fn exactly_one(rest: &[String], shape: &str) -> Result<String> {
    match rest {
        [only] => Ok(only.clone()),
        _ => bail!("{shape} takes exactly one argument, got {}", rest.len()),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Flow {
    #[serde(default = "version_one")]
    pub version: u32,
    pub flow: String,
    pub surface: Surface,
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewport: Option<String>,
    #[serde(default)]
    pub steps: Vec<Step>,
}

fn version_one() -> u32 {
    1
}

impl Flow {
    pub fn load(path: &Path) -> Result<Flow> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read flow {}", path.display()))?;
        let flow: Flow = serde_yaml::from_str(&raw)
            .with_context(|| format!("cannot parse flow {}", path.display()))?;
        if flow.version != 1 {
            bail!(
                "flow {} declares version {}, this build speaks version 1",
                path.display(),
                flow.version
            );
        }
        flow.validate()
            .with_context(|| format!("flow {} cannot be replayed", path.display()))?;
        Ok(flow)
    }

    pub fn validate(&self) -> Result<()> {
        let mut seen: Vec<&str> = Vec::new();
        for step in &self.steps {
            let Some(name) = step.snap().and_then(SnapStep::name) else {
                continue;
            };
            if seen.contains(&name) {
                bail!(
                    "two snaps are both named {name:?}. A replayed flow needs one artifact per \
                     golden, so rename one of them"
                );
            }
            seen.push(name);
        }
        Ok(())
    }

    pub fn assertions(&self) -> usize {
        self.steps.iter().filter(|step| step.is_assertion()).count()
    }

    pub fn image_snaps(&self) -> usize {
        self.steps
            .iter()
            .filter_map(Step::snap)
            .filter(|snap| matches!(snap.mode(), Some(SnapMode::Png) | Some(SnapMode::Both)))
            .count()
    }

    pub fn to_yaml(&self) -> Result<String> {
        serde_yaml::to_string(self).context("cannot encode flow as yaml")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
version: 1
flow: checkout
surface: web
target: http://host:3000
viewport: 1280x800
steps:
  - open: http://host:3000
  - click: "role=button[name=Submit]"
  - type: { selector: "css=#email", text: "a@b.c" }
  - key: Enter
  - wait_for: "text=Welcome"
  - assert_text: "text=Welcome"
  - snap: { name: after-submit, mode: text }
"#;

    #[test]
    fn parses_the_contract_example() {
        let flow: Flow = serde_yaml::from_str(SAMPLE).unwrap();
        assert_eq!(flow.flow, "checkout");
        assert_eq!(flow.surface, Surface::Web);
        assert_eq!(flow.steps.len(), 7);
        assert_eq!(
            flow.steps[1],
            Step::Click("role=button[name=Submit]".into())
        );
        assert_eq!(
            flow.steps[2],
            Step::Type(TypeStep {
                selector: "css=#email".into(),
                text: "a@b.c".into()
            })
        );
        let snap = flow.steps[6].snap().unwrap();
        assert_eq!(snap.name(), Some("after-submit"));
        assert_eq!(snap.mode(), Some(SnapMode::Text));
    }

    #[test]
    fn round_trips_steps_through_yaml() {
        let flow: Flow = serde_yaml::from_str(SAMPLE).unwrap();
        let rendered = flow.to_yaml().unwrap();
        let again: Flow = serde_yaml::from_str(&rendered).unwrap();
        assert_eq!(again.steps, flow.steps);
    }

    #[test]
    fn appends_one_yaml_entry_per_step() {
        let entry = Step::Key("Enter".into()).to_yaml_entry().unwrap();
        assert_eq!(entry, "- key: Enter\n");
        let parsed: Vec<Step> = serde_yaml::from_str(&entry).unwrap();
        assert_eq!(parsed, vec![Step::Key("Enter".into())]);
    }

    #[test]
    fn rejects_a_flow_with_two_snaps_of_one_name() {
        let mut flow: Flow = serde_yaml::from_str(SAMPLE).unwrap();
        flow.steps
            .push(Step::Snap(SnapStep::Named("after-submit".into())));
        let err = flow.validate().unwrap_err();
        assert!(err.to_string().contains("after-submit"), "{err}");
    }

    #[test]
    fn accepts_repeated_unnamed_snaps() {
        let flow = Flow {
            version: 1,
            flow: "f".into(),
            surface: Surface::Web,
            target: "http://x".into(),
            viewport: None,
            steps: vec![
                Step::Snap(SnapStep::Detail {
                    name: None,
                    mode: None,
                }),
                Step::Snap(SnapStep::Detail {
                    name: None,
                    mode: None,
                }),
            ],
        };
        assert!(flow.validate().is_ok());
    }

    #[test]
    fn parses_positional_steps() {
        let parse = |tokens: &[&str]| {
            parse_positional(&tokens.iter().map(|t| t.to_string()).collect::<Vec<_>>())
        };
        assert_eq!(
            parse(&["click", "css=#go"]).unwrap(),
            Step::Click("css=#go".into())
        );
        assert_eq!(
            parse(&["type", "css=#email", "a@b.c"]).unwrap(),
            Step::Type(TypeStep {
                selector: "css=#email".into(),
                text: "a@b.c".into()
            })
        );
        assert_eq!(parse(&["key", "Enter"]).unwrap(), Step::Key("Enter".into()));
        assert_eq!(
            parse(&["wait-for", "text=Hi"]).unwrap(),
            Step::WaitFor("text=Hi".into())
        );
        assert_eq!(
            parse(&["assert_text", "text=Hi"]).unwrap(),
            Step::AssertText("text=Hi".into())
        );
        assert_eq!(
            parse(&["snap", "after"]).unwrap().snap().unwrap().name(),
            Some("after")
        );
        assert!(parse(&["click"]).is_err());
        assert!(parse(&["swipe", "left"]).is_err());
        assert!(parse(&[]).is_err());
    }

    #[test]
    fn rejects_a_step_with_two_verbs() {
        let err = serde_yaml::from_str::<Step>("click: css=#go\nkey: Enter").unwrap_err();
        assert!(err.to_string().contains("one verb"), "{err}");
    }

    #[test]
    fn rejects_an_unknown_verb() {
        let err = serde_yaml::from_str::<Step>("swipe: left").unwrap_err();
        assert!(err.to_string().contains("swipe"), "{err}");
    }

    #[test]
    fn a_flow_of_clicks_and_snaps_asserts_nothing() {
        let flow: Flow = serde_yaml::from_str(
            "version: 1\nflow: f\nsurface: web\ntarget: http://x\nsteps:\n  \
             - click: \"css=#go\"\n  - snap: { name: after }\n",
        )
        .unwrap();
        assert_eq!(flow.assertions(), 0);
        assert_eq!(flow.image_snaps(), 0);

        let guarded: Flow = serde_yaml::from_str(SAMPLE).unwrap();
        assert_eq!(guarded.assertions(), 1);
    }

    #[test]
    fn a_negative_assertion_is_part_of_the_vocabulary() {
        let step: Step =
            serde_yaml::from_str("assert_absent: \"role=button[name=Clear]\"").unwrap();
        assert_eq!(step, Step::AssertAbsent("role=button[name=Clear]".into()));
        assert!(step.is_assertion());
        assert_eq!(
            step.to_yaml_entry().unwrap(),
            "- assert_absent: role=button[name=Clear]\n"
        );
        assert_eq!(
            parse_positional(&["assert_absent".to_string(), "css=#gone".to_string()]).unwrap(),
            Step::AssertAbsent("css=#gone".into())
        );
    }

    #[test]
    fn a_png_snap_is_what_a_golden_can_pin() {
        let flow: Flow = serde_yaml::from_str(
            "version: 1\nflow: f\nsurface: web\ntarget: http://x\nsteps:\n  \
             - snap: { name: a, mode: png }\n  - snap: { name: b }\n",
        )
        .unwrap();
        assert_eq!(flow.image_snaps(), 1);
    }

    #[test]
    fn encodes_steps_for_the_driver() {
        let json = Step::Click("css=#go".into()).to_json().unwrap();
        assert_eq!(json, serde_json::json!({ "click": "css=#go" }));
    }
}
