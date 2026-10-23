// The two-host move run's server (`run.mjs` starts it): one HTTPS listener,
// pages picked by `Host`, standing in for the hub as it serves its hosts.
//   https://droppen.test:<port>     the legacy origin (keys there now)
//   https://my.droppen.test:<port>  the app host (keys go there)
//   https://evil.test:<port>        another site
// Every page has the hub's headers — CSP `script-src 'self'`,
// `frame-ancestors 'none'`, `connect-src 'self'`, no COOP — and the shell's
// marker naming its own host (`wss://<host>`, plan §3.3).
//
// The hub part, enough for the move: `POST /v1/session/challenge`,
// `POST /v1/session` (the device key's `HubLogin` must name `wss://` + the
// host the request came to, as the hub's per-host sign-in will), `GET /v1/me`,
// `POST /v1/logout`; and for the run: `POST /__test/register` (a device and a
// session for it), `POST /__test/hub` (`{deny}`, `{hold}`, `{release}`),
// `GET /__test/state`.
//
//   tsx e2e/move/server.ts <port> <cert.pem> <key.pem>     (from web/)
import { readFileSync } from "node:fs";
import { createServer, type IncomingMessage, type ServerResponse } from "node:https";
import { extname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { fromHex, hex } from "../../src/lib/e2e/bytes";
import { verifyHubLogin } from "../../src/lib/e2e/sig";

const [portArg, certFile, keyFile] = process.argv.slice(2);
const port = Number(portArg ?? 8443);
const dist = fileURLToPath(new URL("./dist/", import.meta.url));
const HOSTS = new Set(["droppen.test", "my.droppen.test", "evil.test"]);

const devices = new Map<string, Uint8Array>();
const tokens = new Map<string, { account: string; device: string }>();
const nonces = new Set<string>();
const log: { at: number; what: string; host: string; detail?: unknown }[] = [];
let deny = false;
let held: (() => void)[] | null = null;

const random = (n: number) => hex(crypto.getRandomValues(new Uint8Array(n)));

function send(res: ServerResponse, status: number, body: unknown, headers: Record<string, string> = {}) {
  res.writeHead(status, { "content-type": "application/json", "cache-control": "no-store", ...headers });
  res.end(body === null ? "" : JSON.stringify(body));
}

async function readBody(req: IncomingMessage): Promise<Record<string, unknown>> {
  const chunks: Buffer[] = [];
  for await (const c of req) chunks.push(c as Buffer);
  const text = Buffer.concat(chunks).toString("utf8");
  return text ? (JSON.parse(text) as Record<string, unknown>) : {};
}

const shell = (host: string) =>
  `<!doctype html><html><head><meta charset="utf-8"><meta name="rh-hub" content="wss://${host}:${port}"><title>move run</title></head>` +
  `<body><div id="root"></div><script type="module" src="/harness.js"></script></body></html>`;

const pageHeaders = {
  "content-type": "text/html; charset=utf-8",
  "cache-control": "no-store",
  "content-security-policy": "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src 'self'; img-src 'self' data:; object-src 'none'; base-uri 'self'; frame-ancestors 'none'",
  "referrer-policy": "no-referrer",
  "x-content-type-options": "nosniff",
};

async function api(req: IncomingMessage, res: ServerResponse, host: string, path: string) {
  const bearer = (req.headers.authorization ?? "").replace(/^Bearer /, "");
  const body = req.method === "POST" ? await readBody(req) : {};
  switch (path) {
    case "/__test/register": {
      const { account, device, sign_key } = body as { account: string; device: string; sign_key: string };
      devices.set(`${account}/${device}`, fromHex(sign_key));
      const token = random(16);
      tokens.set(token, { account, device });
      return send(res, 200, { token });
    }
    case "/__test/hub":
      if (typeof body.deny === "boolean") deny = body.deny;
      if (body.hold === true) held ??= [];
      if (body.release === true) {
        const waiting = held ?? [];
        held = null;
        for (const w of waiting) w();
      }
      return send(res, 200, { deny, held: held?.length ?? null });
    case "/__test/state":
      return send(res, 200, { log, held: held?.length ?? null, deny });
    case "/v1/session/challenge": {
      const nonce = random(32);
      nonces.add(nonce);
      return send(res, 200, { nonce });
    }
    case "/v1/session": {
      if (held) await new Promise<void>((resolve) => held!.push(resolve));
      const { account, device, time_ms, nonce, signature } = body as { account: string; device: string; time_ms: number; nonce: string; signature: string };
      const key = devices.get(`${account}/${device}`);
      if (deny || !key || typeof nonce !== "string" || !nonces.delete(nonce)) {
        log.push({ at: Date.now(), what: "session refused", host, detail: { account, device, deny } });
        return send(res, 401, { error: "denied", message: "denied" });
      }
      const ok = await verifyHubLogin({ role: "deviceSession", account, id: device, hub: `wss://${host}:${port}`, nonce: fromHex(nonce), timeMs: time_ms }, key, fromHex(signature));
      if (!ok) {
        log.push({ at: Date.now(), what: "session bad signature", host, detail: { account, device } });
        return send(res, 401, { error: "denied", message: "denied" });
      }
      const token = random(16);
      tokens.set(token, { account, device });
      log.push({ at: Date.now(), what: "session", host, detail: { account, device } });
      return send(res, 200, { account, device, token, expires_ms: Date.now() + 3_600_000 });
    }
    case "/v1/me": {
      const who = tokens.get(bearer);
      if (!who) return send(res, 401, { error: "unauthorized", message: "unauthorized" });
      const list = [...devices.entries()].filter(([k]) => k.startsWith(`${who.account}/`)).map(([k, signKey]) => ({ id: k.slice(who.account.length + 1), sign_key: hex(signKey) }));
      return send(res, 200, { account: who.account, email: null, chain_seq: null, devices: list });
    }
    case "/v1/logout":
      log.push({ at: Date.now(), what: "logout", host, detail: { known: tokens.delete(bearer) } });
      return send(res, 204, null);
    default:
      return send(res, 404, { error: "not_found", message: "not found" });
  }
}

const TYPES: Record<string, string> = { ".js": "text/javascript", ".css": "text/css", ".map": "application/json" };

createServer({ cert: readFileSync(certFile!), key: readFileSync(keyFile!) }, (req, res) => {
  const host = (req.headers.host ?? "").replace(/:\d+$/, "");
  if (!HOSTS.has(host)) return send(res, 421, { error: "misdirected" });
  const url = new URL(req.url ?? "/", `https://${host}`);
  if (url.pathname.startsWith("/v1/") || url.pathname.startsWith("/__test/")) {
    api(req, res, host, url.pathname).catch((error: unknown) => send(res, 500, { error: "internal", message: String(error) }));
    return;
  }
  const ext = extname(url.pathname);
  if (ext) {
    try {
      const file = readFileSync(join(dist, url.pathname.replace(/^\/+/, "").replace(/\.\.+/g, "")));
      res.writeHead(200, { "content-type": TYPES[ext] ?? "application/octet-stream", "cache-control": "no-store" });
      return res.end(file);
    } catch {
      return send(res, 404, { error: "not_found" });
    }
  }
  res.writeHead(200, pageHeaders);
  res.end(shell(host));
}).listen(port, () => console.log(`move run server on :${port}`));
