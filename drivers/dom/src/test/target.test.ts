import assert from "node:assert/strict";
import { test } from "node:test";
import { DriverError } from "../errors.js";
import { navigableTarget } from "../target.js";

test("a bare host:port is a host and a port, not a scheme", () => {
  assert.equal(navigableTarget("localhost:3000"), "http://localhost:3000");
  assert.equal(navigableTarget("127.0.0.1:8080/app"), "http://127.0.0.1:8080/app");
  assert.equal(navigableTarget("[::1]:3000"), "http://[::1]:3000");
  assert.equal(navigableTarget("localhost:3000/x?a=1#b"), "http://localhost:3000/x?a=1#b");
});

test("a real scheme is left alone", () => {
  assert.equal(navigableTarget("data:text/html,<p>x"), "data:text/html,<p>x");
  assert.equal(navigableTarget("file:///tmp/x.html"), "file:///tmp/x.html");
  assert.equal(navigableTarget("http://host:3000"), "http://host:3000");
  assert.equal(navigableTarget("https://host:3000/app"), "https://host:3000/app");
  assert.equal(navigableTarget("about:blank"), "about:blank");
});

test("a schemeless host is prefixed", () => {
  assert.equal(navigableTarget("example.com/x"), "http://example.com/x");
  assert.equal(navigableTarget("example.com"), "http://example.com");
});

test("exec: and tui: targets are not navigable", () => {
  assert.throws(
    () => navigableTarget("exec:/nix/store/abc/bin/lab-app"),
    (err: unknown) => err instanceof DriverError && /launched, not navigated/.test(String(err)),
  );
  assert.throws(
    () => navigableTarget("tui:nsql"),
    (err: unknown) => err instanceof DriverError && /tui driver/.test(String(err)),
  );
});

test("tel: reads as host:port, which is accepted for a target that is not navigable anyway", () => {
  assert.equal(navigableTarget("tel:12345"), "http://tel:12345");
});
