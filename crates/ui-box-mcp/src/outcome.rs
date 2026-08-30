use base64::Engine;
use rmcp::model::{CallToolResult, ContentBlock};
use serde_json::{Map, Value};

use crate::uibox::{Invocation, Landing};

pub const TOOLING_BANNER: &str = "\
UI-BOX COULD NOT RUN -- INFRASTRUCTURE FAILURE, NOT A UI BUG.
The UI under test was never exercised, so this result says nothing at all about whether
the UI works. Do not go looking for an application bug: repair the tooling (backend
reachability, driver, configuration) and call the tool again.";

pub const TEST_BANNER: &str = "\
UI TEST FAILED -- THE THING UNDER TEST FAILED.
ui-box itself ran correctly, so this is a real result about the UI. The tooling is fine;
the failure is in the application or in the step that was asked of it.";

pub const REQUEST_BANNER: &str = "\
INVALID TOOL ARGUMENTS -- NOTHING WAS RUN.
No ui-box command was executed, so nothing is known about the UI or the tooling.";

pub const NOTHING_RAN_BANNER: &str = "\
NOTHING WAS VERIFIED -- THIS IS NOT A PASS.
No flow was replayed, so no UI was exercised and nothing was proven about it. Here a 0
means \"no work was done\", not \"the UI is correct\". Do not report the UI as verified on
the strength of this result.";

pub const NOTHING_PROVEN_BANNER: &str = "\
NOTHING WAS VERIFIED -- THIS IS NOT A PASS.
The UI was exercised, but an assertion could not prove anything about the page it looked
at: an absence on a page that never rendered is not evidence. Here a 0 means \"nothing was
proven\", not \"the UI is correct\" -- and it is not the application failing either.";

pub const UNKNOWN_VERDICT_BANNER: &str = "\
UNRECOGNISED VERDICT -- THIS IS NOT A PASS.
ui-box reported a verdict this build does not know how to read, so nothing can be
concluded from it either way. Treat it as unverified rather than guessing which side it
falls on.";

pub const NOTHING_STATUS: &str = "nothing_verified";

const KNOWN_VERDICTS: [&str; 4] = ["pass", "fail", "error", NOTHING_STATUS];

pub const BLANK_BANNER: &str = "\
BLANK SNAPSHOT -- THE PAGE RENDERED NOTHING.
The accessibility snapshot came back empty. Treat this as a failure of the thing under
test, not as a pass: an empty tree means nothing was on screen to describe.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Domain {
    Tooling,
    UnderTest,
    Request,
}

impl Domain {
    fn banner(self) -> &'static str {
        match self {
            Domain::Tooling => TOOLING_BANNER,
            Domain::UnderTest => TEST_BANNER,
            Domain::Request => REQUEST_BANNER,
        }
    }

    fn status(self) -> &'static str {
        match self {
            Domain::Tooling => "uibox_unusable",
            Domain::UnderTest => "ui_test_failed",
            Domain::Request => "invalid_request",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Domain::Tooling => "ui_box_tooling",
            Domain::UnderTest => "ui_under_test",
            Domain::Request => "tool_arguments",
        }
    }
}

pub struct Report {
    headline: String,
    sections: Vec<String>,
    images: Vec<Vec<u8>>,
    facts: Map<String, Value>,
    failure: Option<(Domain, String)>,
    inconclusive: Option<(&'static str, String)>,
}

impl Report {
    pub fn new(headline: impl Into<String>) -> Report {
        Report {
            headline: headline.into(),
            sections: Vec::new(),
            images: Vec::new(),
            facts: Map::new(),
            failure: None,
            inconclusive: None,
        }
    }

    pub fn invalid(reason: impl Into<String>) -> CallToolResult {
        Report::new(String::new())
            .failed(Domain::Request, reason)
            .build()
    }

    pub fn headline(&mut self, text: impl Into<String>) -> &mut Report {
        self.headline = text.into();
        self
    }

    pub fn line(&mut self, text: impl Into<String>) -> &mut Report {
        self.sections.push(text.into());
        self
    }

    pub fn block(&mut self, title: &str, body: impl AsRef<str>) -> &mut Report {
        let body = body.as_ref().trim_end();
        if !body.is_empty() {
            self.sections.push(format!("{title}:\n{body}"));
        }
        self
    }

    pub fn fact(&mut self, key: &str, value: impl Into<Value>) -> &mut Report {
        self.facts.insert(key.to_string(), value.into());
        self
    }

    pub fn facts_from(&mut self, value: &Value) -> &mut Report {
        if let Value::Object(map) = value {
            for (key, entry) in map {
                if key != "ok" {
                    self.facts.insert(key.clone(), entry.clone());
                }
            }
        }
        self
    }

    pub fn image(&mut self, png: Vec<u8>) -> &mut Report {
        self.images.push(png);
        self
    }

    pub fn failed(&mut self, domain: Domain, reason: impl Into<String>) -> &mut Report {
        if self.failure.is_none() {
            self.failure = Some((domain, reason.into()));
        }
        self
    }

    pub fn inconclusive(&mut self, reason: impl Into<String>) -> &mut Report {
        self.nothing(NOTHING_RAN_BANNER, reason)
    }

    pub fn proved_nothing(&mut self, reason: impl Into<String>) -> &mut Report {
        self.nothing(NOTHING_PROVEN_BANNER, reason)
    }

    fn nothing(&mut self, banner: &'static str, reason: impl Into<String>) -> &mut Report {
        if self.inconclusive.is_none() {
            self.inconclusive = Some((banner, reason.into()));
        }
        self
    }

    pub fn is_failed(&self) -> bool {
        self.failure.is_some()
    }

    pub fn absorb(&mut self, invocation: &Invocation) -> &mut Report {
        self.fact("command", invocation.command_line());
        if let Some(code) = invocation.landing.exit_code() {
            self.fact("exit_code", code);
        }
        self.facts_from(&invocation.summary());

        match &invocation.landing {
            Landing::Passed => {
                let summary = invocation.summary();
                if let Some(reason) = proved_nothing_reason(&summary) {
                    if summary.get("skipped").and_then(Value::as_bool) == Some(true) {
                        self.inconclusive(reason);
                    } else {
                        self.proved_nothing(reason);
                    }
                } else if let Some(verdict) = unknown_verdict(&summary) {
                    self.nothing(
                        UNKNOWN_VERDICT_BANNER,
                        format!(
                            "ui-box reported verdict {verdict:?}, which this build does not \
                             recognise, so it cannot be read as a pass or as a failure"
                        ),
                    );
                }
            }
            Landing::Failed if invocation.summary().is_null() => {
                self.failed(
                    Domain::Tooling,
                    format!(
                        "`{}` exited 1 but printed no result line. Every run is supposed to \
                         emit one JSON object, so this build is not behaving to contract and \
                         its exit code cannot be read as a verdict about the UI.",
                        crate::uibox::PROGRAM
                    ),
                );
            }
            Landing::Failed => {
                let reason = invocation
                    .error_message()
                    .or_else(|| {
                        invocation
                            .text("halted_at")
                            .map(|at| format!("halted at {at}"))
                    })
                    .unwrap_or_else(|| "the thing under test did not pass".to_string());
                self.failed(Domain::UnderTest, reason);
            }
            other => {
                let reason = other
                    .tooling_reason()
                    .unwrap_or_else(|| "ui-box could not run".to_string());
                let detail = invocation.error_message().unwrap_or_default();
                let reason = if detail.is_empty() {
                    reason
                } else {
                    format!("{reason} {detail}")
                };
                let reason = match invocation.error_kind() {
                    Some(kind) if kind != "error" => {
                        self.fact("error_kind", kind.clone());
                        format!("[{kind}] {reason}")
                    }
                    _ => reason,
                };
                self.failed(Domain::Tooling, reason);
            }
        }

        if let Some(stderr) = invocation.stderr_verbatim() {
            let title = match self.failure.as_ref().map(|(domain, _)| *domain) {
                Some(Domain::Tooling) => "ui-box stderr, verbatim",
                _ => "ui-box detail",
            };
            self.block(title, stderr);
        }
        self
    }

    pub fn build(&self) -> CallToolResult {
        let mut text = String::new();
        let preamble = match (&self.failure, &self.inconclusive) {
            (Some((domain, reason)), _) => Some((domain.banner(), reason)),
            (None, Some((banner, reason))) => Some((*banner, reason)),
            (None, None) => None,
        };
        if let Some((banner, reason)) = preamble {
            text.push_str(banner);
            text.push_str("\n\nreason: ");
            text.push_str(reason);
            if !self.headline.is_empty() {
                text.push_str("\n\n");
            }
        }
        text.push_str(&self.headline);
        for section in self.sections.iter().filter(|section| !section.is_empty()) {
            text.push_str("\n\n");
            text.push_str(section);
        }

        let mut content = vec![ContentBlock::text(text)];
        for png in &self.images {
            content.push(ContentBlock::image(
                base64::engine::general_purpose::STANDARD.encode(png),
                "image/png",
            ));
        }

        let mut structured = self.facts.clone();
        match (&self.failure, &self.inconclusive) {
            (Some((domain, reason)), _) => {
                structured.insert("ok".to_string(), Value::Bool(false));
                structured.insert("status".to_string(), Value::from(domain.status()));
                structured.insert("failure_domain".to_string(), Value::from(domain.label()));
                structured.insert("reason".to_string(), Value::from(reason.clone()));
            }
            (None, Some((_, reason))) => {
                structured.insert("ok".to_string(), Value::Bool(false));
                structured.insert("status".to_string(), Value::from(NOTHING_STATUS));
                structured.insert("failure_domain".to_string(), Value::Null);
                structured.insert("reason".to_string(), Value::from(reason.clone()));
            }
            (None, None) => {
                structured.insert("ok".to_string(), Value::Bool(true));
                structured.insert("status".to_string(), Value::from("passed"));
                structured.insert("failure_domain".to_string(), Value::Null);
            }
        }

        let mut result = match &self.failure {
            Some(_) => CallToolResult::error(content),
            None => CallToolResult::success(content),
        };
        result.structured_content = Some(Value::Object(structured));
        result
    }
}

fn proved_nothing_reason(summary: &Value) -> Option<String> {
    if summary.get("status").and_then(Value::as_str) != Some(NOTHING_STATUS) {
        return None;
    }
    if let Some(reason) = summary.get("reason").and_then(Value::as_str) {
        return Some(reason.to_string());
    }
    if let Some(error) = summary.get("error").and_then(Value::as_str) {
        return Some(error.to_string());
    }
    let nothing = summary
        .get("steps_nothing")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let total = summary
        .get("steps_total")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    Some(format!(
        "{nothing} of {total} step(s) could not prove anything about the page"
    ))
}

fn unknown_verdict(summary: &Value) -> Option<String> {
    let verdict = summary.get("verdict").and_then(Value::as_str)?;
    if KNOWN_VERDICTS.contains(&verdict) {
        return None;
    }
    Some(verdict.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uibox::{Invocation, Landing};

    fn ran(landing: Landing, summary: &str) -> Invocation {
        Invocation {
            argv: vec!["run".to_string()],
            landing,
            stdout: format!("{summary}\n"),
            stderr: String::new(),
        }
    }

    fn absorbed(invocation: &Invocation) -> (Value, String) {
        let mut report = Report::new("headline.".to_string());
        report.absorb(invocation);
        let built = report.build();
        let structured = built.structured_content.clone().unwrap_or(Value::Null);
        let text = built
            .content
            .iter()
            .filter_map(|block| block.as_text().map(|t| t.text.clone()))
            .collect::<Vec<String>>()
            .join("\n");
        (structured, text)
    }

    #[test]
    fn a_run_that_proved_nothing_is_not_reported_as_passed() {
        let invocation = ran(
            Landing::Passed,
            r#"{"ok":false,"status":"nothing_verified","verdict":"nothing_verified","steps_failed":0,"steps_nothing":1,"steps_total":2}"#,
        );
        let (structured, text) = absorbed(&invocation);
        assert_eq!(structured["ok"], false);
        assert_eq!(structured["status"], "nothing_verified");
        assert_eq!(structured["steps_nothing"], 1);
        assert!(text.contains("THIS IS NOT A PASS"), "{text}");
        assert!(
            text.contains("The UI was exercised"),
            "a run that replayed steps must not borrow verify's no-flows wording: {text}"
        );
    }

    #[test]
    fn a_verify_that_ran_no_flow_says_so_in_its_own_words() {
        let invocation = ran(
            Landing::Passed,
            r#"{"ok":false,"status":"nothing_verified","skipped":true,"flows":0,"reason":"no flow files were found"}"#,
        );
        let (structured, text) = absorbed(&invocation);
        assert_eq!(structured["status"], "nothing_verified");
        assert!(text.contains("No flow was replayed"), "{text}");
        assert!(!text.contains("The UI was exercised"), "{text}");
    }

    #[test]
    fn a_real_failure_is_never_softened_into_nothing_verified() {
        let invocation = ran(
            Landing::Failed,
            r#"{"ok":false,"status":"nothing_verified","verdict":"fail","steps_failed":1}"#,
        );
        let (structured, _) = absorbed(&invocation);
        assert_eq!(structured["ok"], false);
        assert_eq!(
            structured["status"], "ui_test_failed",
            "an exit 1 is the UI failing, whatever the summary says about proving nothing"
        );
    }

    #[test]
    fn an_ordinary_pass_stays_a_pass() {
        let invocation = ran(
            Landing::Passed,
            r#"{"ok":true,"verdict":"pass","steps_failed":0,"steps_total":3}"#,
        );
        let (structured, text) = absorbed(&invocation);
        assert_eq!(structured["ok"], true);
        assert_eq!(structured["status"], "passed");
        assert!(!text.contains("NOT A PASS"), "{text}");
    }

    #[test]
    fn a_verdict_this_build_cannot_read_is_not_guessed_at() {
        let invocation = ran(
            Landing::Passed,
            r#"{"ok":true,"verdict":"probably_fine","steps_failed":0}"#,
        );
        let (structured, text) = absorbed(&invocation);
        assert_eq!(structured["ok"], false, "{text}");
        assert_eq!(structured["status"], "nothing_verified");
        assert!(text.contains("UNRECOGNISED VERDICT"), "{text}");
        assert!(text.contains("probably_fine"), "{text}");
    }

    #[test]
    fn every_verdict_the_cli_emits_is_one_this_build_knows() {
        for verdict in ["pass", "fail", "error", "nothing_verified"] {
            let invocation = ran(
                Landing::Passed,
                &format!(r#"{{"ok":true,"verdict":"{verdict}"}}"#),
            );
            let (structured, _) = absorbed(&invocation);
            assert_ne!(
                structured["status"], "nothing_verified",
                "{verdict} must not trip the unrecognised-verdict guard"
            );
        }
    }
}
