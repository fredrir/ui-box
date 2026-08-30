import { type Server, createServer } from "node:http";
import type { AddressInfo } from "node:net";
import { PNG } from "pngjs";

export interface RecordedRequest {
  method: string;
  path: string;
  body: any;
}

export interface FakeWebDriver {
  url: string;
  requests: RecordedRequest[];
  scripts: string[];
  close(): Promise<void>;
}

export const PROTOCOL_ELEMENT_KEY = "element-6066-11e4-a52e-4f735466cecf";

const SNAPSHOT = ['- heading "Tauri Lab" [level=1]', '- button "Submit"'].join("\n");

function screenshotBase64(width: number, height: number): string {
  const png = new PNG({ width, height });
  for (let i = 0; i < png.data.length; i += 4) {
    png.data[i] = 40;
    png.data[i + 1] = 44;
    png.data[i + 2] = 52;
    png.data[i + 3] = 255;
  }
  return PNG.sync.write(png).toString("base64");
}

export async function startFakeWebDriver(): Promise<FakeWebDriver> {
  const requests: RecordedRequest[] = [];
  const scripts: string[] = [];
  let runtimeInstalled = false;

  const server: Server = createServer((req, res) => {
    const chunks: Buffer[] = [];
    req.on("data", (chunk: Buffer) => chunks.push(chunk));
    req.on("end", () => {
      const raw = Buffer.concat(chunks).toString("utf8");
      const body = raw.length > 0 ? JSON.parse(raw) : undefined;
      const path = req.url ?? "/";
      requests.push({ method: req.method ?? "GET", path, body });

      const reply = (value: unknown, status = 200): void => {
        res.writeHead(status, { "content-type": "application/json" });
        res.end(JSON.stringify({ value }));
      };

      if (path === "/status") return reply({ ready: true, message: "fake" });
      if (path === "/session" && req.method === "POST") {
        return reply({ sessionId: "s1", capabilities: { browserName: "wry" } });
      }
      if (!path.startsWith("/session/s1")) return reply({ error: "invalid session id" }, 404);

      const rest = path.slice("/session/s1".length);
      if (rest === "" && req.method === "DELETE") return reply(null);
      if (rest === "/timeouts" || rest === "/window/rect" || rest === "/url") return reply(null);
      if (rest === "/actions") return reply(null);
      if (rest === "/screenshot") return reply(screenshotBase64(1280, 800));
      if (rest === "/element") {
        return reply({ [PROTOCOL_ELEMENT_KEY]: "e1" });
      }
      if (/^\/element\/e1\/(click|clear|value)$/.test(rest)) return reply(null);
      if (rest === "/execute/sync") {
        const script = String(body?.script ?? "");
        scripts.push(script);
        if (script.includes("Boolean(window.__uibox)")) {
          const answer = runtimeInstalled;
          return reply(answer);
        }
        if (script.startsWith("(function uiboxRuntime")) {
          runtimeInstalled = true;
          return reply(null);
        }
        if (script.includes('"readiness"')) {
          return reply({
            ready: true,
            reason: "ok",
            url: "tauri://localhost/",
            title: "Tauri Lab",
            readyState: "complete",
            textLength: 42,
            paintedElements: 3,
            bodyHeight: 800,
            pageErrors: 0,
            lastPageError: null,
          });
        }
        if (script.includes('"snapshot"')) return reply(SNAPSHOT);
        if (script.includes('"mark"')) return reply(1);
        if (script.includes('"clearMarks"')) return reply(null);
        if (script.includes('"textOf"')) return reply(["Submit"]);
        if (script.includes('"drain"')) {
          return reply({
            console: [{ ts: Date.UTC(2024, 0, 1), type: "error", text: "webkit console boom" }],
            network: [
              {
                ts: Date.UTC(2024, 0, 1),
                method: "GET",
                url: "tauri://localhost/api",
                status: 500,
              },
            ],
          });
        }
        if (script.includes("document.title")) return reply("Tauri Lab");
        return reply(null);
      }
      return reply({ error: "unknown command", message: rest }, 404);
    });
  });

  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const port = (server.address() as AddressInfo).port;

  return {
    url: `http://127.0.0.1:${port}`,
    requests,
    scripts,
    close: () =>
      new Promise<void>((resolve) => {
        server.closeAllConnections();
        server.close(() => resolve());
      }),
  };
}
