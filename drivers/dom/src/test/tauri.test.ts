import assert from "node:assert/strict";
import { mkdtemp, readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, before, describe, test } from "node:test";
import { pngDimensions } from "../image.js";
import { keyActions, parseChord, toKeyValue } from "../webdriver/keys.js";
import { DriverClient } from "./rpcclient.js";
import { type FakeWebDriver, startFakeWebDriver } from "./webdriverfake.js";

describe("tauri surface over webdriver", { timeout: 60_000 }, () => {
  let fake: FakeWebDriver;
  let client: DriverClient;
  let runDir: string;

  before(async () => {
    fake = await startFakeWebDriver();
    runDir = await mkdtemp(join(tmpdir(), "uibox-tauri-"));
    client = new DriverClient({ UIBOX_RUN_DIR: runDir });
  });

  after(async () => {
    await client.dispose();
    await fake.close();
  });

  test("an exec: target attaches over webdriver and reports the tauri surface", async () => {
    const opened = await client.call("driver.open", {
      target: "exec:/nix/store/abc/bin/lab-app",
      viewport: "1280x800",
      options: { webdriverUrl: fake.url },
    });
    assert.equal(opened.surface, "tauri");
    assert.equal(opened.ready, true);
    assert.deepEqual(opened.step, { open: "exec:/nix/store/abc/bin/lab-app" });

    const session = fake.requests.find((entry) => entry.path === "/session");
    const always = session?.body.capabilities.alwaysMatch;
    assert.equal(always.browserName, "wry");
    assert.equal(always["tauri:options"].application, "/nix/store/abc/bin/lab-app");
    assert.equal(always["tauri:options"].env.TZ, "UTC");

    assert.ok(
      fake.scripts.some((script) => script.startsWith("(function uiboxRuntime")),
      "the shared runtime must be injected into the webview",
    );

    const rect = fake.requests.find((entry) => entry.path === "/session/s1/window/rect");
    assert.deepEqual(rect?.body, { width: 1280, height: 800, x: 0, y: 0 });

    await client.call("driver.close", { sessionId: opened.sessionId });
    assert.ok(
      fake.requests.some((entry) => entry.method === "DELETE" && entry.path === "/session/s1"),
    );
  });

  test("clicks resolve through mark then a real webdriver element click", async () => {
    const { sessionId } = await client.call("driver.open", {
      target: "exec:/opt/lab",
      options: { webdriverUrl: fake.url },
    });
    const before = fake.requests.length;

    const result = await client.call("driver.act", {
      sessionId,
      step: { click: "role=button[name=Submit]" },
    });
    assert.equal(result.ok, true, JSON.stringify(result.error));
    assert.deepEqual(result.step, { click: "role=button[name=Submit]" });

    const after = fake.requests.slice(before);
    assert.ok(
      after.some(
        (entry) => entry.path === "/session/s1/element" && entry.body.using === "css selector",
      ),
      "must look the marked element up over webdriver",
    );
    assert.ok(after.some((entry) => entry.path === "/session/s1/element/e1/click"));

    await client.call("driver.close", { sessionId });
  });

  test("typing clears then sends keys, and key steps use w3c actions", async () => {
    const { sessionId } = await client.call("driver.open", {
      target: "exec:/opt/lab",
      options: { webdriverUrl: fake.url },
    });
    const before = fake.requests.length;

    await client.call("driver.act", {
      sessionId,
      step: { type: { selector: "css=#email", text: "a@b.c" } },
    });
    await client.call("driver.act", { sessionId, step: { key: "Enter" } });

    const after = fake.requests.slice(before);
    assert.ok(after.some((entry) => entry.path === "/session/s1/element/e1/clear"));
    const sent = after.find((entry) => entry.path === "/session/s1/element/e1/value");
    assert.equal(sent?.body.text, "a@b.c");

    const actions = after.find((entry) => entry.path === "/session/s1/actions");
    assert.deepEqual(actions?.body.actions, [
      {
        type: "key",
        id: "uibox-keyboard",
        actions: [
          { type: "keyDown", value: "" },
          { type: "keyUp", value: "" },
        ],
      },
    ]);

    await client.call("driver.close", { sessionId });
  });

  test("tauri snapshots share the dom text format and cap the png at 1024px", async () => {
    const { sessionId } = await client.call("driver.open", {
      target: "exec:/opt/lab",
      options: { webdriverUrl: fake.url },
    });

    const snap = await client.call("driver.snap", { sessionId, mode: "both", name: "tauri-home" });
    assert.match(snap.text, /- heading "Tauri Lab" \[level=1\]/);
    assert.match(snap.text, /- button "Submit"/);
    assert.equal(snap.pngPath, join(runDir, "snaps", "tauri-home.png"));
    assert.equal(pngDimensions(await readFile(snap.pngPath)).width, 1024);

    assert.deepEqual(snap.console, [
      { ts: "2024-01-01T00:00:00.000Z", type: "error", text: "webkit console boom" },
    ]);
    assert.deepEqual(snap.network, [
      {
        ts: "2024-01-01T00:00:00.000Z",
        method: "GET",
        url: "tauri://localhost/api",
        status: 500,
      },
    ]);

    await client.call("driver.close", { sessionId });
  });

  test("attach failures name the endpoint instead of hanging", async () => {
    await assert.rejects(
      () =>
        client.call("driver.open", {
          target: "exec:/opt/lab",
          options: { webdriverUrl: "http://127.0.0.1:9", driverBootTimeoutMs: 300 },
        }),
      (err: any) => {
        assert.match(err.message, /no webdriver responded at http:\/\/127\.0\.0\.1:9/);
        return true;
      },
    );
  });

  test("webdriver key chords map to spec key values", () => {
    assert.equal(toKeyValue("Enter"), "");
    assert.equal(toKeyValue("Escape"), "");
    assert.equal(toKeyValue("a"), "a");
    assert.throws(() => toKeyValue("Nonsense"), /unsupported key/);

    assert.deepEqual(parseChord("Control+Shift+K"), { modifiers: ["Control", "Shift"], key: "K" });
    assert.deepEqual(parseChord("Ctrl+a"), { modifiers: ["Control"], key: "a" });
    assert.deepEqual(parseChord("Enter"), { modifiers: [], key: "Enter" });

    const [chord] = keyActions(parseChord("Control+a")) as any[];
    assert.deepEqual(chord.actions, [
      { type: "keyDown", value: "" },
      { type: "keyDown", value: "a" },
      { type: "keyUp", value: "a" },
      { type: "keyUp", value: "" },
    ]);
  });
});
