import assert from "node:assert/strict";
import { test } from "node:test";
import { SelectorError } from "../errors.js";
import { parseSelector } from "../selector.js";
import { normalizeStep } from "../steps.js";

test("parses css selectors", () => {
  const parsed = parseSelector("css=#email");
  assert.equal(parsed.kind, "css");
  assert.equal(parsed.kind === "css" && parsed.value, "#email");
});

test("parses role with a name attribute", () => {
  const parsed = parseSelector("role=button[name=Submit]");
  assert.equal(parsed.kind, "role");
  if (parsed.kind !== "role") return;
  assert.equal(parsed.role, "button");
  assert.equal(parsed.options.name?.source, "Submit");
  assert.equal(parsed.options.name?.exact, false);
});

test("parses role with quoted name, level and boolean flags", () => {
  const parsed = parseSelector('role=heading[name="Sign in now"][level=2][exact]');
  assert.equal(parsed.kind, "role");
  if (parsed.kind !== "role") return;
  assert.equal(parsed.options.name?.source, "Sign in now");
  assert.equal(parsed.options.name?.exact, true);
  assert.equal(parsed.options.level, 2);
});

test("parses role with a regular expression name", () => {
  const parsed = parseSelector("role=link[name=/docs/i]");
  assert.equal(parsed.kind, "role");
  if (parsed.kind !== "role") return;
  assert.equal(parsed.options.name?.regex, true);
  assert.equal(parsed.options.name?.flags, "i");
});

test("parses text selectors in all three forms", () => {
  const loose = parseSelector("text=Welcome");
  assert.equal(loose.kind === "text" && loose.match.exact, false);
  const exact = parseSelector('text="Welcome"');
  assert.equal(exact.kind === "text" && exact.match.exact, true);
  const regex = parseSelector("text=/wel.ome/i");
  assert.equal(regex.kind === "text" && regex.match.regex, true);
});

test("rejects TUI-only selector engines with a clear message", () => {
  for (const raw of ["re=^Welcome", "cell=3,4"]) {
    assert.throws(
      () => parseSelector(raw),
      (err: unknown) => err instanceof SelectorError && /TUI-only/.test((err as Error).message),
      `expected ${raw} to be rejected`,
    );
  }
});

test("rejects unprefixed and unknown selectors", () => {
  assert.throws(() => parseSelector("#email"), SelectorError);
  assert.throws(() => parseSelector("xpath=//div"), SelectorError);
  assert.throws(() => parseSelector("role=button[bogus=1]"), SelectorError);
});

test("normalizes contract step shapes verbatim", () => {
  assert.deepEqual(normalizeStep({ open: "http://host:3000" }).step, { open: "http://host:3000" });
  assert.deepEqual(normalizeStep({ click: "role=button[name=Submit]" }).step, {
    click: "role=button[name=Submit]",
  });
  assert.deepEqual(normalizeStep({ type: { selector: "css=#email", text: "a@b.c" } }).step, {
    type: { selector: "css=#email", text: "a@b.c" },
  });
  assert.deepEqual(normalizeStep({ key: "Enter" }).step, { key: "Enter" });
  assert.deepEqual(normalizeStep({ wait_for: "text=Welcome" }).step, { wait_for: "text=Welcome" });
  assert.deepEqual(normalizeStep({ assert_text: "text=Welcome" }).step, {
    assert_text: "text=Welcome",
  });
  assert.deepEqual(normalizeStep({ snap: { name: "after-submit", mode: "text" } }).step, {
    snap: { name: "after-submit", mode: "text" },
  });
});

test("defaults snap mode to text and rejects unknown steps", () => {
  assert.deepEqual(normalizeStep({ snap: { name: "x" } }).step, {
    snap: { name: "x", mode: "text" },
  });
  assert.throws(() => normalizeStep({ hover: "css=#a" }), /unknown step/);
  assert.throws(() => normalizeStep({ click: "css=#a", key: "Enter" }), /exactly one action key/);
});
