import assert from "node:assert/strict";
import { mkdtemp, readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, before, describe, test } from "node:test";
import { toRuntimeSpec } from "../backend/index.js";
import { pngDimensions } from "../image.js";
import { parseSelector } from "../selector.js";
import { type Fixture, startFixtures } from "./fixtures.js";
import { DriverClient } from "./rpcclient.js";

describe("dom driver", { timeout: 120_000 }, () => {
  let fixture: Fixture;
  let client: DriverClient;
  let runDir: string;

  before(async () => {
    fixture = await startFixtures();
    runDir = await mkdtemp(join(tmpdir(), "uibox-run-"));
    client = new DriverClient({ UIBOX_RUN_DIR: runDir });
  });

  after(async () => {
    await client.dispose();
    await fixture.close();
  });

  test("driver.info reports the dom driver and its surfaces", async () => {
    const info = await client.call("driver.info");
    assert.equal(info.name, "dom");
    assert.deepEqual(info.surfaces, ["web", "tauri"]);
    assert.match(info.version, /^\d+\.\d+\.\d+$/);
  });

  test("open gates on a rendered page and returns a replayable step", async () => {
    const opened = await client.call("driver.open", {
      target: fixture.url("/ok"),
      viewport: "1280x800",
    });
    assert.match(opened.sessionId, /^dom-[0-9a-f]{12}$/);
    assert.equal(opened.surface, "web");
    assert.equal(opened.ready, true);
    assert.deepEqual(opened.viewport, { width: 1280, height: 800 });
    assert.deepEqual(opened.step, { open: fixture.url("/ok") });
    await client.call("driver.close", { sessionId: opened.sessionId });
  });

  test("text snapshots render a readable accessibility tree", async () => {
    const { sessionId } = await client.call("driver.open", { target: fixture.url("/ok") });
    const snap = await client.call("driver.snap", { sessionId, mode: "text", name: "checkout" });

    assert.equal(snap.mode, "text");
    assert.equal(snap.name, "checkout");
    assert.match(snap.text, /- heading "Acme Checkout" \[level=1\]/);
    assert.match(snap.text, /- navigation "Primary":/);
    assert.match(snap.text, /- link "Home"/);
    assert.match(snap.text, /- textbox "Email"/);
    assert.match(snap.text, /- checkbox "Accept terms"/);
    assert.match(snap.text, /- button "Submit"/);
    assert.ok(!/Welcome back/.test(snap.text), "hidden nodes must stay out of the tree");
    assert.ok(!/^\{/.test(snap.text.trim()), "text mode must not be a JSON dump");

    const onDisk = await readFile(join(runDir, "snaps", "checkout.txt"), "utf8");
    assert.equal(onDisk.trim(), snap.text.trim());
    await client.call("driver.close", { sessionId });
  });

  test("acts drive the page and return normalized steps", async () => {
    const { sessionId } = await client.call("driver.open", { target: fixture.url("/ok") });

    const typed = await client.call("driver.act", {
      sessionId,
      step: { type: { selector: "css=#email", text: "a@b.c" } },
    });
    assert.equal(typed.ok, true);
    assert.deepEqual(typed.step, { type: { selector: "css=#email", text: "a@b.c" } });

    const clicked = await client.call("driver.act", {
      sessionId,
      step: { click: "role=button[name=Submit]" },
    });
    assert.equal(clicked.ok, true, JSON.stringify(clicked.error));

    const waited = await client.call("driver.act", {
      sessionId,
      step: { wait_for: "text=Welcome" },
    });
    assert.equal(waited.ok, true, JSON.stringify(waited.error));

    const asserted = await client.call("driver.act", {
      sessionId,
      step: { assert_text: "text=Welcome a@b.c" },
    });
    assert.equal(asserted.ok, true, JSON.stringify(asserted.error));

    const keyed = await client.call("driver.act", { sessionId, step: { key: "Tab" } });
    assert.equal(keyed.ok, true, JSON.stringify(keyed.error));

    const snapped = await client.call("driver.act", {
      sessionId,
      step: { snap: { name: "after-submit", mode: "both" } },
    });
    assert.equal(snapped.ok, true);
    assert.equal(snapped.snap.name, "after-submit");
    assert.match(snapped.snap.text, /Welcome a@b\.c/);
    assert.equal(snapped.snap.pngPath, join(runDir, "snaps", "after-submit.png"));

    const png = await readFile(snapped.snap.pngPath);
    assert.equal(pngDimensions(png).width, 1024);

    await client.call("driver.close", { sessionId });
  });

  test("failed assertions come back as ok:false, never as thrown RPC errors", async () => {
    const { sessionId } = await client.call("driver.open", { target: fixture.url("/ok") });
    const result = await client.call("driver.act", {
      sessionId,
      step: { assert_text: "text=Nothing here" },
    });
    assert.equal(result.ok, false);
    assert.equal(result.error.kind, "assertion");
    assert.match(result.error.message, /no visible element matched/);
    assert.match(result.error.detail, /Enter your details/);
    assert.deepEqual(result.step, { assert_text: "text=Nothing here" });
    await client.call("driver.close", { sessionId });
  });

  test("TUI-only selectors are rejected with a clear error", async () => {
    const { sessionId } = await client.call("driver.open", { target: fixture.url("/ok") });
    const result = await client.call("driver.act", { sessionId, step: { click: "re=^Submit$" } });
    assert.equal(result.ok, false);
    assert.equal(result.error.kind, "selector");
    assert.match(result.error.message, /TUI-only/);
    await client.call("driver.close", { sessionId });
  });

  test("the readiness gate refuses a blank page", async () => {
    await assert.rejects(
      () =>
        client.call("driver.open", {
          target: fixture.url("/blank"),
          options: { readinessTimeoutMs: 2000 },
        }),
      (err: any) => {
        assert.match(err.message, /did not render/);
        assert.match(err.data.readiness.reason, /zero height|no text and no visual elements/);
        assert.equal(err.data.readiness.ready, false);
        assert.equal(err.data.readiness.textLength, 0);
        return true;
      },
    );
  });

  test("the readiness gate refuses a page that threw during load", async () => {
    await assert.rejects(
      () =>
        client.call("driver.open", {
          target: fixture.url("/boom"),
          options: { readinessTimeoutMs: 2000 },
        }),
      (err: any) => {
        assert.match(err.data.readiness.reason, /exploded during load/);
        return true;
      },
    );
  });

  test("the readiness gate waits for content that arrives late", async () => {
    const opened = await client.call("driver.open", { target: fixture.url("/late") });
    assert.equal(opened.ready, true);
    const snap = await client.call("driver.snap", { sessionId: opened.sessionId });
    assert.match(snap.text, /Arrived late/);
    await client.call("driver.close", { sessionId: opened.sessionId });
  });

  test("snaps carry console errors and failed requests since the previous snap", async () => {
    const { sessionId } = await client.call("driver.open", { target: fixture.url("/noisy") });
    const first = await client.call("driver.snap", { sessionId, mode: "text" });

    assert.ok(
      first.console.some(
        (entry: any) => entry.type === "error" && /boom from console/.test(entry.text),
      ),
      `console entries: ${JSON.stringify(first.console)}`,
    );
    assert.ok(
      first.network.some((entry: any) => entry.status === 404 && /\/missing$/.test(entry.url)),
      `network entries: ${JSON.stringify(first.network)}`,
    );

    const second = await client.call("driver.snap", { sessionId, mode: "text" });
    assert.deepEqual(second.console, []);
    assert.deepEqual(second.network, []);
    await client.call("driver.close", { sessionId });
  });

  test("uncaught exceptions during a step fail that step", async () => {
    const { sessionId } = await client.call("driver.open", { target: fixture.url("/ok") });
    const result = await client.call("driver.act", {
      sessionId,
      step: { click: "css=#terms" },
      timeout_ms: 5000,
    });
    assert.equal(result.ok, true);

    await client.call("driver.eval", {
      sessionId,
      expr: "(() => { setTimeout(() => { throw new Error('late blowup'); }, 0); return 1; })()",
    });
    await new Promise((resolve) => setTimeout(resolve, 200));

    const after = await client.call("driver.act", { sessionId, step: { key: "Tab" } });
    assert.equal(after.ok, false);
    assert.equal(after.error.kind, "pageerror");
    assert.match(after.error.message, /late blowup/);
    await client.call("driver.close", { sessionId });
  });

  test("eval returns a serialized value", async () => {
    const { sessionId } = await client.call("driver.open", { target: fixture.url("/ok") });
    const result = await client.call("driver.eval", { sessionId, expr: "document.title" });
    assert.deepEqual(result, { value: "Checkout" });
    await client.call("driver.close", { sessionId });
  });

  test("the determinism preamble makes repeated opens byte-identical", async () => {
    const first = await client.call("driver.open", { target: fixture.url("/random") });
    const one = await client.call("driver.eval", {
      sessionId: first.sessionId,
      expr: "document.getElementById('v').textContent",
    });
    await client.call("driver.close", { sessionId: first.sessionId });

    const second = await client.call("driver.open", { target: fixture.url("/random") });
    const two = await client.call("driver.eval", {
      sessionId: second.sessionId,
      expr: "document.getElementById('v').textContent",
    });
    const timezone = await client.call("driver.eval", {
      sessionId: second.sessionId,
      expr: "Intl.DateTimeFormat().resolvedOptions().timeZone",
    });
    const reduced = await client.call("driver.eval", {
      sessionId: second.sessionId,
      expr: "matchMedia('(prefers-reduced-motion: reduce)').matches",
    });
    await client.call("driver.close", { sessionId: second.sessionId });

    assert.equal(one.value, two.value, "seeded Math.random and the frozen clock must repeat");
    assert.equal(timezone.value, "UTC");
    assert.equal(reduced.value, true);
  });

  test("multiple sessions coexist in one driver process", async () => {
    const a = await client.call("driver.open", { target: fixture.url("/ok") });
    const b = await client.call("driver.open", { target: fixture.url("/noisy") });
    assert.notEqual(a.sessionId, b.sessionId);

    const titleA = await client.call("driver.eval", {
      sessionId: a.sessionId,
      expr: "document.title",
    });
    const titleB = await client.call("driver.eval", {
      sessionId: b.sessionId,
      expr: "document.title",
    });
    assert.equal(titleA.value, "Checkout");
    assert.equal(titleB.value, "Noisy");

    await client.call("driver.close", { sessionId: a.sessionId });
    await client.call("driver.close", { sessionId: b.sessionId });
    await assert.rejects(
      () => client.call("driver.eval", { sessionId: a.sessionId, expr: "1" }),
      /unknown session/,
    );
  });

  test("the shared page runtime resolves css, role and text selectors", async () => {
    const { sessionId } = await client.call("driver.open", { target: fixture.url("/ok") });

    const resolve = async (raw: string): Promise<number> => {
      const spec = JSON.stringify(toRuntimeSpec(parseSelector(raw)));
      const result = await client.call("driver.eval", {
        sessionId,
        expr: `(() => { const n = window.__uibox.mark(${spec}, "probe"); window.__uibox.clearMarks("probe"); return n; })()`,
      });
      return result.value as number;
    };

    assert.equal(await resolve("css=#email"), 1);
    assert.equal(await resolve("role=button[name=Submit]"), 1);
    assert.equal(await resolve("role=heading[level=1]"), 1);
    assert.equal(await resolve("role=heading[level=3]"), 0);
    assert.equal(await resolve("role=link"), 2);
    assert.equal(await resolve("role=link[name=Docs]"), 1);
    assert.equal(await resolve("role=link[name=/^doc/i]"), 1);
    assert.equal(await resolve("role=textbox[name=Email]"), 1);
    assert.equal(await resolve("role=checkbox[checked=false]"), 1);
    assert.equal(await resolve("role=checkbox[checked=true]"), 0);
    assert.equal(await resolve("text=Enter your details"), 1);
    assert.equal(await resolve('text="Enter your details"'), 0);
    assert.equal(await resolve('text="Enter your details to continue."'), 1);
    assert.equal(await resolve("text=Welcome back"), 0);

    const spec = JSON.stringify(toRuntimeSpec(parseSelector("role=link")));
    const texts = await client.call("driver.eval", {
      sessionId,
      expr: `window.__uibox.textOf(${spec}, 10)`,
    });
    assert.deepEqual(texts.value, ["Home", "Docs"]);

    await client.call("driver.close", { sessionId });
  });

  test("marking is reversible and leaves no attributes behind", async () => {
    const { sessionId } = await client.call("driver.open", { target: fixture.url("/ok") });
    const spec = JSON.stringify(toRuntimeSpec(parseSelector("role=button[name=Submit]")));
    await client.call("driver.eval", { sessionId, expr: `window.__uibox.mark(${spec}, "tok")` });
    const during = await client.call("driver.eval", {
      sessionId,
      expr: 'document.querySelectorAll("[data-uibox-hit]").length',
    });
    assert.equal(during.value, 1);
    await client.call("driver.eval", { sessionId, expr: 'window.__uibox.clearMarks("tok")' });
    const after = await client.call("driver.eval", {
      sessionId,
      expr: 'document.querySelectorAll("[data-uibox-hit]").length',
    });
    assert.equal(after.value, 0);
    await client.call("driver.close", { sessionId });
  });

  test("png snapshots refuse to run without a configured run directory", async () => {
    const bare = new DriverClient({ UIBOX_RUN_DIR: "", UIBOX_ARTIFACTS: "" });
    try {
      const { sessionId } = await bare.call("driver.open", { target: fixture.url("/ok") });
      await assert.rejects(
        () => bare.call("driver.snap", { sessionId, mode: "png" }),
        /no run directory is configured/,
      );
      const text = await bare.call("driver.snap", { sessionId, mode: "text" });
      assert.match(text.text, /Acme Checkout/);
      assert.equal(text.pngPath, undefined);
      await bare.call("driver.close", { sessionId });
    } finally {
      await bare.dispose();
    }
  });

  test("snap names never collide on disk", async () => {
    const { sessionId } = await client.call("driver.open", { target: fixture.url("/ok") });
    const first = await client.call("driver.snap", { sessionId, name: "dup" });
    const second = await client.call("driver.snap", { sessionId, name: "dup" });
    const auto = await client.call("driver.snap", { sessionId });
    assert.equal(first.name, "dup");
    assert.equal(second.name, "dup-2");
    assert.match(auto.name, /^snap-\d{3}$/);
    await client.call("driver.close", { sessionId });
  });

  test("the viewport from the request is applied", async () => {
    const opened = await client.call("driver.open", {
      target: fixture.url("/ok"),
      viewport: { width: 900, height: 600 },
    });
    const size = await client.call("driver.eval", {
      sessionId: opened.sessionId,
      expr: "[window.innerWidth, window.innerHeight]",
    });
    assert.deepEqual(size.value, [900, 600]);
    await client.call("driver.close", { sessionId: opened.sessionId });
  });

  test("protocol errors are reported without killing the process", async () => {
    client.writeRaw("this is not json\n");
    await assert.rejects(() => client.call("driver.teleport", {}), /unknown method/);
    await assert.rejects(() => client.call("open", { target: "http://127.0.0.1:1" }), /unknown method: open/);
    const info = await client.call("driver.info");
    assert.equal(info.name, "dom");
  });
});
