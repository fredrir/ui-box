use base64::Engine;
use rmcp::model::{CallToolResult, ContentBlock};
use serde_json::{Map, Value};

use crate::uibox::{Invocation, Landing};

pub const TOOLING_BANNER: &str = "\
UI-BOX COULD NOT RUN (exit 2) -- INFRASTRUCTURE FAILURE, NOT A UI BUG.
The UI under test was never exercised, so this result says nothing at all about whether
the UI works. Do not go looking for an application bug: repair the tooling (backend
reachability, driver, configuration) and call the tool again.";

pub const TEST_BANNER: &str = "\
UI TEST FAILED (exit 1) -- THE THING UNDER TEST FAILED.
ui-box itself ran correctly, so this is a real result about the UI. The tooling is fine;
the failure is in the application or in the step that was asked of it.";

pub const REQUEST_BANNER: &str = "\
INVALID TOOL ARGUMENTS -- NOTHING WAS RUN.
No ui-box command was executed, so nothing is known about the UI or the tooling.";

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
}

impl Report {
    pub fn new(headline: impl Into<String>) -> Report {
        Report {
            headline: headline.into(),
            sections: Vec::new(),
            images: Vec::new(),
            facts: Map::new(),
            failure: None,
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
            Landing::Passed => {}
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
        if let Some((domain, reason)) = &self.failure {
            text.push_str(domain.banner());
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
        match &self.failure {
            Some((domain, reason)) => {
                structured.insert("ok".to_string(), Value::Bool(false));
                structured.insert("status".to_string(), Value::from(domain.status()));
                structured.insert("failure_domain".to_string(), Value::from(domain.label()));
                structured.insert("reason".to_string(), Value::from(reason.clone()));
            }
            None => {
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
