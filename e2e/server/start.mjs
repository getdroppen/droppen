// One HTTPS listener for every host in the harness: `Host` picks the fake hub,
// the app build, the site, the apex or the unrelated site. The certificate is
// the throwaway one from `scripts/certs.mjs`; nothing leaves this machine.
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:https";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { HOSTS } from "../scripts/certs.mjs";
import { createHub } from "./hub.mjs";
import { createStatic } from "./static.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "..");
const state = join(root, ".e2e");
const port = Number(process.env.E2E_HTTPS_PORT ?? 443);
const keyPath = join(state, "certs", "key.pem");
const certPath = join(state, "certs", "cert.pem");
for (const file of [keyPath, certPath, join(state, "dist", "app", "index.html"), join(state, "dist", "site", "index.html")]) {
  if (!existsSync(file)) {
    console.error(`e2e server: missing ${file} — run \`node scripts/prepare.mjs\` first`);
    process.exit(1);
  }
}

// A fresh log per run: the specs poll it, and old lines would answer for them.
const hubLog = process.env.E2E_HUB_LOG ?? join(state, "hub-log.jsonl");
mkdirSync(dirname(hubLog), { recursive: true });
rmSync(hubLog, { force: true });
writeFileSync(hubLog, "");

const mutation = process.env.E2E_MUTATION ?? (existsSync(join(state, "mutation.json")) ? JSON.parse(readFileSync(join(state, "mutation.json"), "utf8")).mutation ?? "" : "");
const origins = {
  app: `https://${HOSTS.app}`,
  site: `https://${HOSTS.site}`,
  apex: `https://${HOSTS.apex}`,
  api: `https://${HOSTS.api}`,
  hub: `wss://${HOSTS.api}`,
};
const hub = createHub({ origins, mutation, logFile: hubLog });

const app = createStatic(join(state, "dist", "app"));
const site = createStatic(join(state, "dist", "site"));
const apex = createStatic(join(state, "dist", "apex"));

const EVIL = `<!doctype html><html><head><meta charset="utf-8"><title>evil</title></head><body><h1>evil.test</h1><p>Not Droppen.</p></body></html>`;

const SERVER_HOSTS = new Set([HOSTS.api, HOSTS.app, HOSTS.site, HOSTS.apex, HOSTS.evil]);

function hostOf(req) {
  return String(req.headers.host ?? "").replace(/:\d+$/, "").toLowerCase();
}

const server = createServer({ key: readFileSync(keyPath), cert: readFileSync(certPath), minVersion: "TLSv1.2" }, (req, res) => {
  const host = hostOf(req);
  const path = (() => {
    try {
      return new URL(req.url, `https://${host}`).pathname;
    } catch {
      return "/";
    }
  })();
  if (path === "/__health") {
    res.writeHead(200, { "content-type": "text/plain" });
    return res.end("ok");
  }
  if (!SERVER_HOSTS.has(host)) {
    res.writeHead(421, { "content-type": "text/plain" });
    return res.end("misdirected");
  }
  if (host === HOSTS.api) return hub.handle(req, res);
  // The apex keeps the machine compatibility path (`/v1/*`) during the
  // window; its pages are the static interstitial.
  if (host === HOSTS.apex && path.startsWith("/v1/")) return hub.handle(req, res);
  if (host === HOSTS.apex) return apex(req, res, path);
  if (host === HOSTS.app) return app(req, res, path);
  if (host === HOSTS.site) return site(req, res, path);
  res.writeHead(200, { "content-type": "text/html; charset=utf-8", "cache-control": "no-store" });
  res.end(EVIL);
});

server.on("upgrade", (req, socket) => {
  const host = hostOf(req);
  let path = "/";
  try {
    path = new URL(req.url, `https://${host}`).pathname;
  } catch {}
  if (host === HOSTS.api || (host === HOSTS.apex && path.startsWith("/v1/"))) return hub.upgrade(req, socket);
  socket.end("HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\n");
});

server.listen(port, "0.0.0.0", () => {
  console.log(`e2e server on :${port} (app ${HOSTS.app}, site ${HOSTS.site}, apex ${HOSTS.apex}, api ${HOSTS.api}${mutation ? `, mutation ${mutation}` : ""})`);
});

const stop = () => server.close(() => process.exit(0));
process.on("SIGTERM", stop);
process.on("SIGINT", stop);
