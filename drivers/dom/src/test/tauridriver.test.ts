import assert from "node:assert/strict";
import { mkdtemp, writeFile } from "node:fs/promises";
import { type AddressInfo, createServer } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, before, describe, test } from "node:test";
import { findExecutable, probeTauriBins, resolveTauriBins } from "../backend/webdriver.js";
import { DriverClient } from "./rpcclient.js";
import { type FakeWebDriver, startFakeWebDriver } from "./webdriverfake.js";

const MISSING_BIN = "/nonexistent/uibox-tauri-driver";

async function freePort(): Promise<number> {
  const server = createServer();
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address() as AddressInfo;
  await new Promise<void>((resolve) => server.close(() => resolve()));
  return port;
}

async function writeScript(dir: string, name: string, body: string): Promise<string> {
  const path = join(dir, name);
  await writeFile(path, body, { mode: 0o755 });
  return path;
}

function withEnv(vars: Record<string, string | undefined>, run: () => void): void {
  const saved = new Map<string, string | undefined>();
  for (const [name, value] of Object.entries(vars)) {
    saved.set(name, process.env[name]);
    if (value === undefined) delete process.env[name];
    else process.env[name] = value;
  }
  try {
    run();
  } finally {
    for (const [name, value] of saved) {
      if (value === undefined) delete process.env[name];
      else process.env[name] = value;
    }
  }
}

describe("tauri driver binary resolution", () => {
  test("precedence is option, then environment, then default", () => {
    withEnv({ UIBOX_TAURI_DRIVER: undefined, UIBOX_NATIVE_DRIVER: undefined }, () => {
      assert.deepEqual(resolveTauriBins({}), {
        tauriDriver: "tauri-driver",
        nativeDriver: null,
        source: { tauriDriver: "default", nativeDriver: "unset" },
      });
    });

    withEnv(
      { UIBOX_TAURI_DRIVER: "/env/tauri-driver", UIBOX_NATIVE_DRIVER: "/env/WebKitWebDriver" },
      () => {
        assert.deepEqual(resolveTauriBins({}), {
          tauriDriver: "/env/tauri-driver",
          nativeDriver: "/env/WebKitWebDriver",
          source: { tauriDriver: "env", nativeDriver: "env" },
        });
        assert.deepEqual(
          resolveTauriBins({ tauriDriverBin: "/opt/td", nativeDriverBin: "/opt/nd" }),
          {
            tauriDriver: "/opt/td",
            nativeDriver: "/opt/nd",
            source: { tauriDriver: "option", nativeDriver: "option" },
          },
        );
      },
    );
  });

  test("a bare name resolves against PATH and an absolute name against the filesystem", async () => {
    const dir = await mkdtemp(join(tmpdir(), "uibox-path-"));
    const script = await writeScript(dir, "uibox-fake-driver", "#!/bin/sh\nexit 0\n");
    withEnv({ PATH: `${dir}:${process.env.PATH ?? ""}` }, () => {
      assert.equal(findExecutable("uibox-fake-driver"), script);
    });
    assert.equal(findExecutable("uibox-fake-driver-not-installed"), null);
    assert.equal(findExecutable(script), script);
    assert.equal(findExecutable(dir), null);
    assert.equal(findExecutable(MISSING_BIN), null);
  });

  test("an unresolvable binary yields a reason naming it and the driver host", () => {
    const probe = probeTauriBins(resolveTauriBins({ tauriDriverBin: MISSING_BIN }));
    assert.match(probe.reason ?? "", new RegExp(MISSING_BIN));
    assert.match(probe.reason ?? "", /from options\.tauriDriverBin/);
    assert.match(probe.reason ?? "", /where the display is, not where ui-box was invoked/);
  });

  test("a resolvable tauri-driver with an unresolvable native driver still faults", async () => {
    const dir = await mkdtemp(join(tmpdir(), "uibox-native-"));
    const script = await writeScript(dir, "tauri-driver", "#!/bin/sh\nexit 0\n");
    const probe = probeTauriBins(
      resolveTauriBins({ tauriDriverBin: script, nativeDriverBin: "/nonexistent/WebKitWebDriver" }),
    );
    assert.match(probe.reason ?? "", /native webdriver/);
    assert.match(probe.reason ?? "", /WebKitWebDriver/);
  });
});

describe("tauri driver spawn failures", { timeout: 60_000 }, () => {
  let client: DriverClient;
  let fake: FakeWebDriver;
  let dir: string;

  before(async () => {
    fake = await startFakeWebDriver();
    dir = await mkdtemp(join(tmpdir(), "uibox-tauridriver-"));
    client = new DriverClient({ UIBOX_TAURI_DRIVER: "", UIBOX_NATIVE_DRIVER: "" });
    await client.call("driver.info");
  });

  after(async () => {
    await client.dispose();
    await fake.close();
  });

  test("a missing tauri-driver fails fast and blames the binary, not the port", async () => {
    const started = Date.now();
    await assert.rejects(
      () =>
        client.call("driver.open", {
          target: "exec:/opt/lab",
          options: { tauriDriverBin: MISSING_BIN },
        }),
      (err: any) => {
        assert.match(err.message, new RegExp(MISSING_BIN));
        assert.match(err.message, /where the display is, not where ui-box was invoked/);
        assert.doesNotMatch(err.message, /no webdriver responded/);
        return true;
      },
    );
    const elapsed = Date.now() - started;
    assert.ok(elapsed < 2000, `expected a fast failure, took ${elapsed}ms`);
  });

  test("a driver that dies carries its exit code and its stderr into the error", async () => {
    const script = await writeScript(dir, "boom-driver", "#!/bin/sh\necho boom >&2\nexit 3\n");
    const port = await freePort();
    await assert.rejects(
      () =>
        client.call("driver.open", {
          target: "exec:/opt/lab",
          options: { tauriDriverBin: script, webdriverPort: port, driverBootTimeoutMs: 10_000 },
        }),
      (err: any) => {
        assert.match(err.message, /exited with code 3/);
        assert.match(err.message, /boom/);
        return true;
      },
    );
  });

  test("nativeDriverPort and nativeDriverBin reach the spawned process as flags", async () => {
    const script = await writeScript(
      dir,
      "argv-driver",
      '#!/bin/sh\necho "argv: $@" >&2\nexit 7\n',
    );
    const native = await writeScript(dir, "native-driver", "#!/bin/sh\nexit 0\n");
    const port = await freePort();
    await assert.rejects(
      () =>
        client.call("driver.open", {
          target: "exec:/opt/lab",
          options: {
            tauriDriverBin: script,
            nativeDriverBin: native,
            nativeDriverPort: 4501,
            webdriverPort: port,
            driverBootTimeoutMs: 10_000,
          },
        }),
      (err: any) => {
        assert.match(err.message, /exited with code 7/);
        assert.match(
          err.message,
          new RegExp(`argv: --port ${port} --native-driver ${native} --native-port 4501`),
        );
        return true;
      },
    );
  });

  test("an explicit webdriverUrl never reaches the spawn path", async () => {
    const opened = await client.call("driver.open", {
      target: "exec:/opt/lab",
      options: { webdriverUrl: fake.url, tauriDriverBin: MISSING_BIN },
    });
    assert.equal(opened.surface, "tauri");
    assert.equal(opened.ready, true);
    await client.call("driver.close", { sessionId: opened.sessionId });
  });
});

describe("driver.info tauri capability", { timeout: 60_000 }, () => {
  test("the no-override case is reported as such", async () => {
    const client = new DriverClient({ UIBOX_TAURI_DRIVER: "", UIBOX_NATIVE_DRIVER: "" });
    try {
      const info = await client.call("driver.info");
      assert.equal(typeof info.tauri.ok, "boolean");
      assert.deepEqual(info.tauri.source, { tauriDriver: "default", nativeDriver: "unset" });
      assert.equal(info.tauri.nativeDriver, null);
      assert.match(info.tauri.reason, /no per-session overrides/);
      if (!info.tauri.ok) assert.match(info.tauri.reason, /tauri-driver/);
    } finally {
      await client.dispose();
    }
  });

  test("an installed tauri-driver is reported ok with the path that resolved", async () => {
    const dir = await mkdtemp(join(tmpdir(), "uibox-info-"));
    const script = await writeScript(dir, "tauri-driver", "#!/bin/sh\nexit 0\n");
    const client = new DriverClient({ UIBOX_TAURI_DRIVER: script, UIBOX_NATIVE_DRIVER: "" });
    try {
      const info = await client.call("driver.info");
      assert.equal(info.tauri.ok, true);
      assert.equal(info.tauri.tauriDriver, script);
      assert.deepEqual(info.tauri.source, { tauriDriver: "env", nativeDriver: "unset" });
      assert.match(info.tauri.reason, /no per-session overrides/);
    } finally {
      await client.dispose();
    }
  });
});
