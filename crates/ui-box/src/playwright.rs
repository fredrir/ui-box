use crate::flow::{Flow, Step};

pub fn emit(flow: &Flow) -> String {
    let mut out = String::from("import { expect, test } from '@playwright/test';\n\n");
    out.push_str(&format!(
        "test('{}', async ({{ page }}) => {{\n",
        escape(&flow.flow)
    ));
    if let Some(viewport) = &flow.viewport {
        if let Some((width, height)) = viewport.split_once('x') {
            out.push_str(&format!(
                "  await page.setViewportSize({{ width: {}, height: {} }});\n",
                width.trim(),
                height.trim()
            ));
        }
    }
    let navigates = matches!(flow.steps.first(), Some(Step::Open(_)));
    if !navigates && !flow.target.trim().is_empty() {
        out.push_str(&format!("  await page.goto('{}');\n", escape(&flow.target)));
    }
    for step in &flow.steps {
        out.push_str(&line(step));
    }
    out.push_str("});\n");
    out
}

fn line(step: &Step) -> String {
    match step {
        Step::Open(url) => format!("  await page.goto('{}');\n", escape(url)),
        Step::Click(selector) => format!("  await {}.click();\n", locator(selector)),
        Step::Type(step) => format!(
            "  await {}.fill('{}');\n",
            locator(&step.selector),
            escape(&step.text)
        ),
        Step::Key(key) => format!("  await page.keyboard.press('{}');\n", escape(key)),
        Step::WaitFor(selector) => format!("  await {}.waitFor();\n", locator(selector)),
        Step::AssertText(selector) => {
            format!("  await expect({}).toBeVisible();\n", locator(selector))
        }
        Step::AssertAbsent(selector) => {
            format!("  await expect({}).toHaveCount(0);\n", locator(selector))
        }
        Step::Snap(snap) => format!(
            "  await page.screenshot({{ path: '{}.png' }});\n",
            escape(snap.name().unwrap_or("snapshot"))
        ),
    }
}

fn unquote(value: &str) -> String {
    let trimmed = value.trim();
    let Some(inner) = trimmed
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    else {
        return trimmed.to_string();
    };
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => out.push(chars.next().unwrap_or('\\')),
            other => out.push(other),
        }
    }
    out
}

pub fn locator(selector: &str) -> String {
    if let Some(css) = selector.strip_prefix("css=") {
        return format!("page.locator('{}')", escape(css));
    }
    if let Some(text) = selector.strip_prefix("text=") {
        return format!("page.getByText('{}')", escape(&unquote(text)));
    }
    if let Some(role) = selector.strip_prefix("role=") {
        let (role, name) = split_role(role);
        return match name {
            Some(name) => {
                format!(
                    "page.getByRole('{}', {{ name: '{}' }})",
                    escape(role),
                    escape(&name)
                )
            }
            None => format!("page.getByRole('{}')", escape(role)),
        };
    }
    format!("page.locator('{}')", escape(selector))
}

fn split_role(raw: &str) -> (&str, Option<String>) {
    let Some(open) = raw.find('[') else {
        return (raw, None);
    };
    let role = &raw[..open];
    let inner = raw[open + 1..].trim_end_matches(']');
    let Some((key, value)) = inner.split_once('=') else {
        return (role, None);
    };
    if key.trim() != "name" {
        return (role, None);
    }
    let value = value.trim();
    let value = value
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|rest| rest.strip_suffix('\''))
        })
        .unwrap_or(value);
    (role, Some(value.to_string()))
}

fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Surface;
    use crate::flow::{SnapStep, TypeStep};

    fn sample() -> Flow {
        Flow {
            version: 1,
            flow: "checkout".to_string(),
            surface: Surface::Web,
            target: "http://host:3000".to_string(),
            viewport: Some("1280x800".to_string()),
            steps: vec![
                Step::Open("http://host:3000".to_string()),
                Step::Click("role=button[name=Submit]".to_string()),
                Step::Type(TypeStep {
                    selector: "css=#email".to_string(),
                    text: "a@b.c".to_string(),
                }),
                Step::Key("Enter".to_string()),
                Step::WaitFor("text=Welcome".to_string()),
                Step::AssertText("text=Welcome".to_string()),
                Step::Snap(SnapStep::Named("after-submit".to_string())),
            ],
        }
    }

    #[test]
    fn translates_the_selector_grammar() {
        assert_eq!(locator("css=#email"), "page.locator('#email')");
        assert_eq!(locator("text=Welcome"), "page.getByText('Welcome')");
        assert_eq!(
            locator("role=button[name=Submit]"),
            "page.getByRole('button', { name: 'Submit' })"
        );
        assert_eq!(locator("role=button"), "page.getByRole('button')");
        assert_eq!(
            locator("role=link[name=\"Log in\"]"),
            "page.getByRole('link', { name: 'Log in' })"
        );
    }

    #[test]
    fn emits_a_playwright_spec() {
        let spec = emit(&sample());
        assert!(
            spec.starts_with("import { expect, test } from '@playwright/test';"),
            "{spec}"
        );
        assert!(
            spec.contains("test('checkout', async ({ page }) => {"),
            "{spec}"
        );
        assert!(
            spec.contains("await page.setViewportSize({ width: 1280, height: 800 });"),
            "{spec}"
        );
        assert!(
            spec.contains("await page.goto('http://host:3000');"),
            "{spec}"
        );
        assert!(
            spec.contains("await page.getByRole('button', { name: 'Submit' }).click();"),
            "{spec}"
        );
        assert!(
            spec.contains("await page.locator('#email').fill('a@b.c');"),
            "{spec}"
        );
        assert!(
            spec.contains("await page.keyboard.press('Enter');"),
            "{spec}"
        );
        assert!(
            spec.contains("await expect(page.getByText('Welcome')).toBeVisible();"),
            "{spec}"
        );
        assert!(
            spec.contains("await page.screenshot({ path: 'after-submit.png' });"),
            "{spec}"
        );
        assert!(spec.trim_end().ends_with("});"), "{spec}");
    }

    #[test]
    fn navigates_even_when_no_open_step_was_recorded() {
        let mut flow = sample();
        flow.steps.remove(0);
        let spec = emit(&flow);
        assert_eq!(spec.matches("page.goto(").count(), 1, "{spec}");
        assert!(
            spec.contains("await page.goto('http://host:3000');"),
            "{spec}"
        );
    }

    #[test]
    fn does_not_navigate_twice_when_an_open_step_exists() {
        let spec = emit(&sample());
        assert_eq!(spec.matches("page.goto(").count(), 1, "{spec}");
    }

    #[test]
    fn escapes_quotes_in_values() {
        assert_eq!(locator("text=it's here"), "page.getByText('it\\'s here')");
    }
}
