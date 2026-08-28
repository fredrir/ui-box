import { type Server, createServer } from "node:http";
import type { AddressInfo } from "node:net";

const PAGES: Record<string, string> = {
  "/ok": `<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>Checkout</title></head>
<body>
  <header><h1>Acme Checkout</h1></header>
  <nav aria-label="Primary"><a href="/ok">Home</a><a href="/other">Docs</a></nav>
  <main>
    <p>Enter your details to continue.</p>
    <form id="form">
      <label for="email">Email</label>
      <input id="email" type="email" placeholder="you@example.com">
      <label><input id="terms" type="checkbox"> Accept terms</label>
      <button type="submit">Submit</button>
    </form>
    <div id="result" hidden>Welcome back</div>
  </main>
  <script>
    document.getElementById('form').addEventListener('submit', function (event) {
      event.preventDefault();
      var result = document.getElementById('result');
      result.hidden = false;
      result.textContent = 'Welcome ' + document.getElementById('email').value;
    });
  </script>
</body></html>`,

  "/blank": `<!doctype html><html lang="en"><head><title>Blank</title></head><body></body></html>`,

  "/noisy": `<!doctype html>
<html lang="en"><head><title>Noisy</title></head>
<body>
  <h1>Noisy page</h1>
  <script>
    console.error('boom from console');
    fetch('/missing').catch(function () {});
  </script>
</body></html>`,

  "/boom": `<!doctype html>
<html lang="en"><head><title>Boom</title></head>
<body><h1>Boom</h1><script>throw new Error('exploded during load');</script></body></html>`,

  "/late": `<!doctype html>
<html lang="en"><head><title>Late</title></head>
<body><script>
  setTimeout(function () {
    var h = document.createElement('h1');
    h.textContent = 'Arrived late';
    document.body.appendChild(h);
  }, 600);
</script></body></html>`,

  "/random": `<!doctype html>
<html lang="en"><head><title>Random</title></head>
<body><h1 id="v"></h1><script>
  document.getElementById('v').textContent = Math.random().toFixed(6) + ' @ ' + Date.now();
</script></body></html>`,
};

export interface Fixture {
  url(path: string): string;
  close(): Promise<void>;
}

export async function startFixtures(): Promise<Fixture> {
  const server: Server = createServer((req, res) => {
    const path = (req.url ?? "/").split("?")[0] ?? "/";
    const body = PAGES[path];
    if (!body) {
      res.writeHead(404, { "content-type": "text/plain" });
      res.end("not found");
      return;
    }
    res.writeHead(200, { "content-type": "text/html; charset=utf-8" });
    res.end(body);
  });

  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const port = (server.address() as AddressInfo).port;

  return {
    url: (path: string) => `http://127.0.0.1:${port}${path}`,
    close: () =>
      new Promise<void>((resolve) => {
        server.closeAllConnections();
        server.close(() => resolve());
      }),
  };
}
