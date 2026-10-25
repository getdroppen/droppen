// The fake hub on `api.droppen.test` (and, for the compatibility alias, the
// apex's `/v1/*`): the few endpoints the page and a machine handshake need,
// with the browser CORS allowlist and a WebSocket upgrade. Every request is
// appended to a JSON-lines log the specs read — that is how "the page called
// the API at api., from my." is observed rather than assumed.
import { appendFileSync } from "node:fs";
import { resolve } from "node:path";
import { handshake, readFrames, sendText } from "./ws.mjs";

const DEVICE_TOKEN = "e2e-device-session";
const NONCE = "9f".repeat(32);

function json(res, status, value, cors) {
  res.writeHead(status, { "content-type": "application/json", ...cors });
  res.end(JSON.stringify(value));
}

/**
 * `origins`: app/site/apex/api; `mutation`: `"cors"` drops the app's origin;
 * `logFile`: where to append one line per request.
 */
export function createHub({ origins, mutation = "", logFile }) {
  const allowed = new Set([origins.app, origins.site, origins.apex]);
  if (mutation === "cors") allowed.delete(origins.app);
  const logPath = logFile ?? resolve(process.cwd(), ".e2e", "hub-log.jsonl");

  const log = (entry) => {
    try {
      appendFileSync(logPath, `${JSON.stringify({ ts: Date.now(), ...entry })}\n`);
    } catch {
      // the log is a test convenience, not a reason to fail a request
    }
  };

  const corsFor = (origin) => {
    if (!origin || !allowed.has(origin)) return {};
    return {
      "access-control-allow-origin": origin,
      "access-control-allow-headers": "authorization, content-type",
      "access-control-allow-methods": "GET, POST, PUT, PATCH, DELETE, OPTIONS",
      "access-control-max-age": "600",
      vary: "origin",
    };
  };

  function handle(req, res) {
    let url;
    try {
      url = new URL(req.url, "https://hub.test");
    } catch {
      res.writeHead(400);
      return res.end();
    }
    const origin = req.headers.origin ?? null;
    const cors = corsFor(origin);
    const path = url.pathname;
    log({ kind: "http", method: req.method, path, origin, allowed: Object.keys(cors).length > 0, host: req.headers.host ?? null });
    res.on("finish", () => log({ kind: "done", method: req.method, path, origin, status: res.statusCode }));

    if (req.method === "OPTIONS") {
      res.writeHead(204, cors);
      return res.end();
    }
    if (path === "/healthz" || path === "/readyz") {
      res.writeHead(200, { "content-type": "text/plain", ...cors });
      return res.end("ok");
    }
    if (path === "/.well-known/droppen-hub") {
      return json(res, 200, { origin: origins.hub, app: origins.app, site: origins.site }, cors);
    }
    if (path === "/v1/signup/config") {
      return json(res, 200, { signup: "open", invite_required: false, verification: "off", passkeys: null }, cors);
    }
    if (path === "/v1/session/challenge" && req.method === "POST") {
      return json(res, 200, { nonce: NONCE }, cors);
    }
    if (path === "/v1/session" && req.method === "POST") {
      let body = "";
      req.on("data", (chunk) => (body += chunk));
      req.on("end", () =>
        json(
          res,
          200,
          {
            account: "acct-e2e",
            device: "dev-e2e",
            token: DEVICE_TOKEN,
            expires_ms: Date.now() + 60 * 60 * 1000,
            nonce_ok: /"nonce"\s*:\s*"/.test(body),
          },
          cors,
        ),
      );
      return;
    }
    if (path === "/v1/me") {
      if ((req.headers.authorization ?? "") !== `Bearer ${DEVICE_TOKEN}`) {
        return json(res, 401, { error: "unauthorized", message: "no device session" }, cors);
      }
      return json(res, 200, { account: "acct-e2e", email: "e2e@example.test", device: "dev-e2e", chain_v3: true }, cors);
    }
    if (path === "/v1/agent" || path === "/v1/client") {
      // These are WebSocket-only; a plain GET is not an upgrade.
      return json(res, 426, { error: "upgrade_required" }, cors);
    }
    return json(res, 404, { error: "not_found" }, cors);
  }

  /** `upgrade` handler: the browser's `/v1/client`, and a machine's `/v1/agent`. */
  function upgrade(req, socket) {
    let url;
    try {
      url = new URL(req.url, "https://hub.test");
    } catch {
      socket.destroy();
      return;
    }
    const conn = handshake(req, socket);
    if (!conn) return;
    const path = url.pathname;
    log({ kind: "ws-open", path, origin: conn.origin, host: req.headers.host ?? null });
    if (path === "/v1/client") {
      sendText(socket, JSON.stringify({ t: "hello" }));
      readFrames(socket, (text) => log({ kind: "ws-message", path, origin: conn.origin, text: text.slice(0, 200) }));
      socket.on("close", () => log({ kind: "ws-close", path }));
      return;
    }
    if (path === "/v1/agent") {
      readFrames(socket, (text) => {
        let message = null;
        try {
          message = JSON.parse(text);
        } catch {}
        const hub = message?.hub;
        const accepted = hub === origins.hub || hub === `wss://${new URL(origins.apex).host}`;
        log({ kind: "ws-handshake", path, origin: conn.origin, hub, accepted });
        sendText(socket, JSON.stringify(accepted ? { t: "accepted", hub } : { t: "refused", error: "bad_hub" }));
      });
      socket.on("close", () => log({ kind: "ws-close", path }));
      return;
    }
    log({ kind: "ws-rejected", path, origin: conn.origin });
    socket.destroy();
  }

  return { handle, upgrade, allowedOrigins: allowed };
}
