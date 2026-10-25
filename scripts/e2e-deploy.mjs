#!/usr/bin/env node
/**
 * Read-only black-box checks for a deployed Droppen environment.
 *
 * This is the deployment gate for the `api.droppen.app` move (the rollout
 * runbook's "E2E gates"): DNS and TLS, health, CORS, the WebSocket upgrade,
 * the static hosts' marker and policies, and `/install.sh`. It never signs up,
 * never writes and never sends a request that changes anything. Node 22
 * built-ins only; no dependencies.
 *
 *   node scripts/e2e-deploy.mjs
 *   node scripts/e2e-deploy.mjs --app https://my.droppen.app \
 *     --site https://www.droppen.app --apex https://droppen.app \
 *     --api https://api.droppen.app [--token-file FILE] [--json]
 *   node scripts/e2e-deploy.mjs --self-test
 *
 * Exit status: 0 every check passed, 1 a check failed, 2 the checker itself
 * could not run (bad arguments, or the self-test failed).
 *
 * During step 0 of the rollout `api.` is still proxied by Cloudflare, so the
 * DNS and certificate checks are expected to fail until the record is made
 * DNS-only. Authenticated checks (`/v1/me` with a device session) run only
 * when `--token-file` is given; a device session is never minted here (that
 * would sign, and sign-in is a mutation).
 */
import { createHash, randomBytes } from "node:crypto";
import { readFileSync } from "node:fs";
import { createServer } from "node:http";
import { connect as netConnect } from "node:net";
import { connect as tlsConnect } from "node:tls";
import { resolve4, resolve6 } from "node:dns/promises";
import { pathToFileURL } from "node:url";

const DEFAULTS = {
  app: "https://my.droppen.app",
  site: "https://www.droppen.app",
  apex: "https://droppen.app",
  api: "https://api.droppen.app",
};

/** Cloudflare's published address ranges (cloudflare.com/ips-v4, ips-v6). */
export const CLOUDFLARE_V4 = [
  "173.245.48.0/20", "103.21.244.0/22", "103.22.200.0/22", "103.31.4.0/22",
  "141.101.64.0/18", "108.162.192.0/18", "190.93.240.0/20", "188.114.96.0/20",
  "197.234.240.0/22", "198.41.128.0/17", "162.158.0.0/15", "104.16.0.0/13",
  "104.24.0.0/14", "172.64.0.0/13", "131.0.72.0/22",
];
export const CLOUDFLARE_V6 = [
  "2400:cb00::/32", "2606:4700::/32", "2803:f800::/32", "2405:b500::/32",
  "2405:8100::/32", "2a06:98c0::/29", "2c0f:f248::/32",
];

/** `ip` (v4 or v6) inside any of `cidrs`; a plain string compare when it cannot parse. */
export function inCidrs(ip, cidrs) {
  const v6 = ip.includes(":");
  if (v6 !== cidrs[0].includes(":")) return false;
  const bytesOf = (addr) => {
    if (v6) {
      const halves = addr.split("::");
      const head = halves[0] ? halves[0].split(":") : [];
      const tail = halves.length > 1 && halves[1] ? halves[1].split(":") : [];
      const fill = halves.length > 1 ? 8 - head.length - tail.length : 0;
      const groups = [...head, ...Array(Math.max(0, fill)).fill("0"), ...tail];
      const out = [];
      for (const g of groups) for (const b of [parseInt(g.slice(0, 2) || "0", 16), parseInt(g.slice(2) || "0", 16)]) out.push(b);
      return out.length === 16 ? out : null;
    }
    const parts = addr.split(".").map((n) => Number(n));
    return parts.length === 4 && parts.every((n) => Number.isInteger(n) && n >= 0 && n <= 255) ? parts : null;
  };
  const addr = bytesOf(ip);
  if (!addr) return false;
  for (const cidr of cidrs) {
    const [net, bitsText] = cidr.split("/");
    const bits = Number(bitsText);
    const target = bytesOf(net);
    if (!target) continue;
    let left = bits;
    let same = true;
    for (let i = 0; i < target.length && same; i++) {
      const take = Math.min(8, left);
      if (take > 0) {
        const mask = take === 8 ? 0xff : (0xff << (8 - take)) & 0xff;
        if ((addr[i] & mask) !== (target[i] & mask)) same = false;
      }
      left -= take;
    }
    if (same) return true;
  }
  return false;
}

export function isCloudflareAddress(ip) {
  return inCidrs(ip, ip.includes(":") ? CLOUDFLARE_V6 : CLOUDFLARE_V4);
}

/** The origin a shell's `rh-hub` marker names, or `null`. */
export function markerOrigin(html) {
  const tags = String(html).match(/<meta\b[^>]*>/gi) ?? [];
  for (const tag of tags) {
    const name = /\bname\s*=\s*["']([^"']*)["']/i.exec(tag)?.[1];
    if (name?.toLowerCase() !== "rh-hub") continue;
    const content = /\bcontent\s*=\s*["']([^"']*)["']/i.exec(tag)?.[1] ?? "";
    return /^wss?:\/\/[^/]+$/.test(content) ? content : null;
  }
  return null;
}

/** One `Permissions-Policy` value (`"()"`, `"(self)"`, …) or `null`. */
export function permissionsPolicyValue(header, feature) {
  if (!header) return null;
  for (const part of String(header).split(",")) {
    const at = part.indexOf("=");
    if (at < 0) continue;
    if (part.slice(0, at).trim().toLowerCase() !== feature.toLowerCase()) continue;
    return part.slice(at + 1).trim();
  }
  return null;
}

/** Whether a permissions value allows only this origin. */
export function allowsSelfOnly(value) {
  return value !== null && /^\(\s*self\s*\)$/i.test(value);
}

/** Whether a permissions value allows nobody. */
export function allowsNobody(value) {
  return value !== null && /^\(\s*\)$/.test(value);
}

/** The sources of a CSP directive (`[]` when the header has no such directive). */
export function cspSources(header, directive) {
  if (!header) return [];
  for (const part of String(header).split(";")) {
    const [name, ...values] = part.trim().split(/\s+/);
    if (name?.toLowerCase() === directive.toLowerCase()) return values;
  }
  return [];
}

/** `Header`, `Header: value` pairs as the object `http`/`fetch` gave us. */
export function headerValue(headers, name) {
  if (!headers) return null;
  if (typeof headers.get === "function") return headers.get(name);
  const lower = name.toLowerCase();
  for (const [k, v] of Object.entries(headers)) if (k.toLowerCase() === lower) return Array.isArray(v) ? v.join(", ") : String(v);
  return null;
}

/** A JSON object, or `null` (never throws). */
export function parseJson(text) {
  try {
    const value = JSON.parse(text);
    return value && typeof value === "object" && !Array.isArray(value) ? value : null;
  } catch {
    return null;
  }
}

/**
 * The default hub `install.sh` assigns to `HUB` (the part after `${DROPPEN_HUB:-`
 * when it has an override), or `null`. Comments are skipped: the script's help
 * text mentions `DROPPEN_HUB=wss://...` as documentation, not as the default.
 */
export function installHub(text) {
  for (const line of String(text).split("\n")) {
    if (/^\s*#/.test(line)) continue;
    const match = /^\s*(?:export\s+)?HUB=(.+?)\s*(?:#.*)?$/.exec(line);
    if (!match) continue;
    const value = match[1].trim().replace(/^["']|["']$/g, "");
    return /\$\{DROPPEN_HUB:-([^}]*)\}/.exec(value)?.[1] ?? value;
  }
  return null;
}

/** Whether a served `sw.js` is the retiring worker a non-app host serves. */
export function swIsRetiring(body) {
  return /registration\.unregister\s*\(/.test(body) || /self\.addEventListener\s*\(\s*["']install["'][\s\S]*unregister\s*\(/.test(body);
}

const stripSlash = (u) => String(u).replace(/\/+$/, "");
const wssOf = (origin) => `wss://${new URL(origin).host}`;
const originOf = (value) => {
  try {
    return new URL(value).origin;
  } catch {
    return null;
  }
};

/** A fetch that never follows a redirect and never keeps a body around. */
async function fetchText(url, init, timeoutMs) {
  const res = await fetch(url, {
    redirect: "manual",
    signal: AbortSignal.timeout(timeoutMs),
    ...init,
    headers: { "user-agent": "droppen-e2e-deploy/1", ...(init?.headers ?? {}) },
  });
  const body = await res.text().catch(() => "");
  return { status: res.status, headers: res.headers, body };
}

/** Raw `Upgrade: websocket` request; resolves with the status line's code (0 on a network error). */
export function wsUpgrade(url, { timeoutMs = 10_000, origin = null } = {}) {
  return new Promise((resolve) => {
    const u = new URL(url);
    const secure = u.protocol === "https:" || u.protocol === "wss:";
    const port = Number(u.port || (secure ? 443 : 80));
    let socket;
    let done = false;
    const finish = (code, detail) => {
      if (done) return;
      done = true;
      try {
        socket?.destroy();
      } catch {}
      resolve({ code, detail });
    };
    try {
      socket = secure
        ? tlsConnect({ host: u.hostname, port, servername: u.hostname, rejectUnauthorized: true }, onConnect)
        : netConnect({ host: u.hostname, port }, onConnect);
    } catch (error) {
      return finish(0, error.message);
    }
    socket.setTimeout(timeoutMs, () => finish(0, "timed out"));
    socket.on("error", (error) => finish(0, error.message));
    function onConnect() {
      const key = randomBytes(16).toString("base64");
      const lines = [
        `GET ${u.pathname}${u.search} HTTP/1.1`,
        `Host: ${u.host}`,
        "Upgrade: websocket",
        "Connection: Upgrade",
        `Sec-WebSocket-Key: ${key}`,
        "Sec-WebSocket-Version: 13",
        ...(origin ? [`Origin: ${origin}`] : []),
        "",
        "",
      ];
      socket.write(lines.join("\r\n"));
    }
    let text = "";
    socket.on("data", (chunk) => {
      text += chunk.toString("latin1");
      const at = text.indexOf("\r\n\r\n");
      const line = /^HTTP\/1\.1\s+(\d{3})/.exec(text);
      if (line && (at >= 0 || text.length > 64 * 1024)) finish(Number(line[1]), text.split("\r\n")[0]);
    });
  });
}

const list = (values) => values.length ? values.join(", ") : "none";

/**
 * Run every check. `env` is an object with the four origins, `token`, and a
 * `timeoutMs`; `skip` names checks to leave out (`dns`, `tls`; the self-test
 * uses plain HTTP).
 */
export async function checkDeployment(env) {
  const { app, site, apex, api, token, timeoutMs = 10_000 } = env;
  const apiHost = new URL(api).hostname;
  const expectedWss = wssOf(api);
  const apexWss = wssOf(apex);
  const results = [];
  const add = (name, ok, expected, actual, note = "") => {
    results.push({ name, ok, expected: String(expected), actual: String(actual), note });
    return ok;
  };

  // ---- API: DNS, TLS, health, discovery ------------------------------------
  const addresses = [];
  if (!env.skip?.has("dns")) {
    for (const family of [4, 6]) {
      try {
        const found = family === 4 ? await resolve4(apiHost) : await resolve6(apiHost);
        addresses.push(...found);
      } catch {
        // no record in this family
      }
    }
    const cloudflare = addresses.filter(isCloudflareAddress);
    add(
      "api.dns.not-cloudflare",
      addresses.length > 0 && cloudflare.length === 0,
      `an address not in Cloudflare's ranges (Railway during the window)`,
      addresses.length ? `${list(addresses)}${cloudflare.length ? ` (Cloudflare: ${list(cloudflare)})` : ""}` : "no A/AAAA record",
      "step 0: the Cloudflare record must be DNS-only",
    );
  }

  if (!env.skip?.has("tls")) {
    const tls = await new Promise((resolve) => {
      let done = false;
      const finish = (value) => {
        if (!done) {
          done = true;
          resolve(value);
        }
      };
      const socket = tlsConnect({ host: apiHost, port: Number(env.apiPort ?? 443), servername: apiHost, rejectUnauthorized: true }, () => {
        try {
          const cert = socket.getPeerCertificate();
          const issuer = cert && Object.values(cert.issuer ?? {}).filter(Boolean).join(", ");
          finish({ ok: true, issuer, subject: Object.values(cert?.subject ?? {}).filter(Boolean).join(", ") });
        } catch (error) {
          finish({ ok: false, error: error.message });
        } finally {
          socket.destroy();
        }
      });
      socket.setTimeout(timeoutMs, () => {
        socket.destroy();
        finish({ ok: false, error: "timed out" });
      });
      socket.on("error", (error) => finish({ ok: false, error: error.message }));
    });
    add(
      "api.tls.issuer",
      tls.ok && !/cloudflare/i.test(tls.issuer ?? ""),
      "a valid certificate not issued by Cloudflare",
      tls.ok ? `issuer: ${tls.issuer || "unknown"}` : `TLS failed: ${tls.error}`,
      "step 0: Cloudflare in front of the API also terminates TLS",
    );
  }

  for (const [name, path] of [["healthz", "/healthz"], ["readyz", "/readyz"]]) {
    try {
      const res = await fetchText(`${api}${path}`, {}, timeoutMs);
      add(`api.${name}`, res.status === 200, "200", `${res.status}${headerValue(res.headers, "server") ? ` (server: ${headerValue(res.headers, "server")})` : ""}`);
    } catch (error) {
      add(`api.${name}`, false, "200", `request failed: ${error.message}`);
    }
  }

  try {
    const res = await fetchText(`${api}/.well-known/droppen-hub`, {}, timeoutMs);
    const doc = parseJson(res.body);
    const origin = doc?.origin;
    const appExpected = originOf(app);
    const siteExpected = originOf(site);
    const ok =
      res.status === 200 &&
      doc !== null &&
      (origin === expectedWss || origin === apexWss) &&
      originOf(doc.app) === appExpected &&
      originOf(doc.site) === siteExpected;
    add(
      "api.well-known",
      ok,
      `${api}/.well-known/droppen-hub -> origin ${expectedWss} (or ${apexWss} during the window), app ${appExpected}, site ${siteExpected}`,
      res.status === 200 ? `origin ${origin ?? "missing"}, app ${doc?.app ?? "missing"}, site ${doc?.site ?? "missing"}` : `${res.status} ${res.body.slice(0, 80)}`,
    );
  } catch (error) {
    add("api.well-known", false, "200 JSON", `request failed: ${error.message}`);
  }

  // ---- CORS ----------------------------------------------------------------
  for (const [label, origin, wantAllowed] of [
    ["app", originOf(app), true],
    ["site", originOf(site), true],
    ["evil", "https://evil.example", false],
  ]) {
    try {
      const res = await fetchText(`${api}/v1/me`, {
        method: "OPTIONS",
        headers: {
          origin,
          "access-control-request-method": "GET",
          "access-control-request-headers": "authorization",
        },
      }, timeoutMs);
      const acao = headerValue(res.headers, "access-control-allow-origin");
      const acac = headerValue(res.headers, "access-control-allow-credentials");
      const allowedHeaders = (headerValue(res.headers, "access-control-allow-headers") ?? "").toLowerCase().split(",").map((s) => s.trim());
      if (wantAllowed) {
        const ok = [200, 204].includes(res.status) && acao === origin && allowedHeaders.includes("authorization") && !acac;
        add(
          `cors.${label}`,
          ok,
          `${origin} allowed, authorization allowed, no allow-credentials`,
          `${res.status}, allow-origin ${acao ?? "absent"}, allow-headers ${allowedHeaders.filter(Boolean).join(",") || "absent"}${acac ? `, allow-credentials ${acac}` : ""}`,
        );
      } else {
        const ok = acao === null || (acao !== origin && acao !== "*");
        add(`cors.${label}`, ok, "no access-control-allow-origin", `${res.status}, allow-origin ${acao ?? "absent"}${acac ? `, allow-credentials ${acac}` : ""}`);
      }
    } catch (error) {
      add(`cors.${label}`, false, wantAllowed ? "allowed" : "not allowed", `request failed: ${error.message}`);
    }
  }

  // ---- WebSocket upgrade: api., and the apex alias during the window -------
  const ws = await wsUpgrade(`${api.replace(/^http/, "ws")}/v1/agent`, { timeoutMs, origin: originOf(app) });
  add("ws.api.agent", ws.code === 101, "101 Switching Protocols", `${ws.code || "no response"}${ws.detail ? ` (${ws.detail})` : ""}`);
  const bad = await wsUpgrade(`${api.replace(/^http/, "ws")}/v1/not-a-socket`, { timeoutMs, origin: originOf(app) });
  add("ws.api.bad-path", bad.code !== 101, "not 101", bad.code ? `${bad.code}` : "no response");
  const apexWs = await wsUpgrade(`${apex.replace(/^http/, "ws")}/v1/agent`, { timeoutMs, origin: originOf(app) });
  add(
    "ws.apex.alias",
    apexWs.code === 101,
    "101 (accepted as the temporary machine alias)",
    apexWs.code ? `${apexWs.code}` : "no response",
    "step 5 removes this",
  );

  // ---- Static hosts --------------------------------------------------------
  const staticChecks = [
    {
      label: "app",
      base: app,
      markers: [expectedWss],
      webauthn: "self",
      connectHasApi: true,
      sw: "app",
    },
    {
      label: "site",
      base: site,
      // Until step 4 the hub may still serve `www` itself, with its own marker.
      markers: [null, wssOf(site)],
      webauthn: "none",
      connectHasApi: false,
      sw: "retiring",
    },
    {
      label: "apex",
      base: apex,
      // Until step 5 the apex is the hub; a static apex has no marker.
      markers: [null, apexWss],
      webauthn: "none",
      connectHasApi: false,
      sw: "retiring",
    },
  ];
  for (const host of staticChecks) {
    let res;
    try {
      res = await fetchText(`${stripSlash(host.base)}/`, {}, timeoutMs);
    } catch (error) {
      add(`static.${host.label}`, false, "200 shell", `request failed: ${error.message}`);
      continue;
    }
    const marker = markerOrigin(res.body);
    add(
      `static.${host.label}.marker`,
      host.markers.includes(marker),
      host.markers.map((m) => (m === null ? "no marker" : m)).join(" or "),
      marker === null ? "no rh-hub marker" : `marker ${marker}`,
    );

    const pp = headerValue(res.headers, "permissions-policy");
    const get = permissionsPolicyValue(pp, "publickey-credentials-get");
    const create = permissionsPolicyValue(pp, "publickey-credentials-create");
    add(
      `static.${host.label}.webauthn`,
      host.webauthn === "self" ? allowsSelfOnly(get) && allowsSelfOnly(create) : allowsNobody(get) && allowsNobody(create),
      host.webauthn === "self" ? "publickey-credentials-get=(self), publickey-credentials-create=(self)" : "publickey-credentials-get=(), publickey-credentials-create=()",
      pp === null ? "no Permissions-Policy header" : `get=${get ?? "absent"}, create=${create ?? "absent"}`,
    );

    const csp = headerValue(res.headers, "content-security-policy");
    const connect = cspSources(csp, "connect-src");
    const namesApi = connect.some((source) => source === `https://${apiHost}` || source === `wss://${apiHost}` || source.includes(apiHost));
    add(
      `static.${host.label}.csp-connect`,
      host.connectHasApi ? connect.includes(`https://${apiHost}`) && connect.includes(`wss://${apiHost}`) : !namesApi,
      host.connectHasApi ? `connect-src names https://${apiHost} and wss://${apiHost}` : "connect-src does not name the API",
      csp === null ? "no Content-Security-Policy header" : `connect-src ${list(connect)}`,
    );

    try {
      const sw = await fetchText(`${stripSlash(host.base)}/sw.js`, {}, timeoutMs);
      if (host.sw === "app") {
        add(
          `static.${host.label}.sw`,
          sw.status === 200 && /self\.addEventListener\s*\(\s*["']install["']/.test(sw.body),
          "200, a real service worker",
          sw.status === 200 ? (swIsRetiring(sw.body) ? "a retiring worker (wrong on the app)" : "service worker") : `${sw.status}`,
        );
      } else {
        const ok = sw.status === 404 || (sw.status === 200 && swIsRetiring(sw.body));
        add(
          `static.${host.label}.sw`,
          ok,
          "404, or the retiring worker",
          sw.status === 404 ? "404" : sw.status === 200 ? (swIsRetiring(sw.body) ? "retiring worker" : "an active worker") : `${sw.status}`,
        );
      }
    } catch (error) {
      add(`static.${host.label}.sw`, false, "404 or the retiring worker", `request failed: ${error.message}`);
    }

    try {
      const wk = await fetchText(`${stripSlash(host.base)}/.well-known/droppen-hub`, {}, timeoutMs);
      const doc = parseJson(wk.body);
      const ok = wk.status === 200 && doc !== null && (doc.origin === expectedWss || doc.origin === apexWss) && originOf(doc.app) === originOf(app) && originOf(doc.site) === originOf(site);
      add(
        `static.${host.label}.well-known`,
        ok,
        `origin ${expectedWss} (or ${apexWss}), app ${originOf(app)}, site ${originOf(site)}`,
        wk.status === 200 ? `origin ${doc?.origin ?? "missing"}, app ${doc?.app ?? "missing"}, site ${doc?.site ?? "missing"}` : `${wk.status}`,
      );
    } catch (error) {
      add(`static.${host.label}.well-known`, false, "200 JSON", `request failed: ${error.message}`);
    }
  }

  // ---- /install.sh ---------------------------------------------------------
  try {
    const res = await fetchText(`${stripSlash(apex)}/install.sh`, {}, timeoutMs);
    const hub = res.status === 200 ? installHub(res.body) : null;
    add(
      "apex.install.sh",
      res.status === 200 && hub === expectedWss,
      `200 and HUB defaults to ${expectedWss}`,
      res.status === 200 ? (hub === null ? "no HUB= assignment" : `HUB defaults to ${hub}`) : `${res.status}`,
    );
  } catch (error) {
    add("apex.install.sh", false, "200 and HUB=wss://api", `request failed: ${error.message}`);
  }

  // ---- Authenticated (read-only, opt-in) -----------------------------------
  if (token) {
    try {
      const res = await fetchText(`${api}/v1/me`, { headers: { authorization: `Bearer ${token}` } }, timeoutMs);
      const doc = parseJson(res.body);
      add("auth.me", res.status === 200 && doc !== null, "200 JSON", `${res.status}${doc?.account ? " (account named)" : ""}`);
    } catch (error) {
      add("auth.me", false, "200 JSON", `request failed: ${error.message}`);
    }
    results.push({
      name: "auth.device-session",
      ok: true,
      expected: "skipped",
      actual: "skipped (minting a device session would sign; this checker is read-only)",
      note: "",
      skipped: true,
    });
  } else {
    results.push({
      name: "auth",
      ok: true,
      expected: "skipped",
      actual: "skipped (no --token-file)",
      note: "authenticated checks are opt-in and read-only",
      skipped: true,
    });
  }

  return results;
}

// --------------------------------------------------------------- self-test

function selfTestFakeHub(origins) {
  const allowed = new Set();
  const server = createServer((req, res) => {
    const origin = req.headers.origin;
    const cors = origin && (allowed.size ? allowed.has(origin) : [origins.app, origins.site].includes(origin));
    if (req.method === "OPTIONS") {
      const headers = cors ? { "access-control-allow-origin": origin, "access-control-allow-headers": "authorization, content-type", "access-control-allow-methods": "GET, POST, OPTIONS" } : {};
      res.writeHead(204, headers);
      return res.end();
    }
    if (req.url === "/healthz" || req.url === "/readyz") {
      res.writeHead(200, { "content-type": "text/plain" });
      return res.end("ok");
    }
    if (req.url === "/.well-known/droppen-hub") {
      res.writeHead(200, { "content-type": "application/json" });
      return res.end(JSON.stringify({ origin: origins.hub, app: origins.app, site: origins.site }));
    }
    if (req.url === "/v1/me") {
      const ok = (req.headers.authorization ?? "") === "Bearer test-token";
      res.writeHead(ok ? 200 : 401, { "content-type": "application/json", ...(cors ? { "access-control-allow-origin": origin } : {}) });
      return res.end(JSON.stringify(ok ? { account: "test" } : { error: "unauthorized" }));
    }
    res.writeHead(404);
    res.end();
  });
  server.on("upgrade", (req, socket) => {
    if (req.url !== "/v1/agent") {
      socket.end("HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\n");
      return;
    }
    const accept = createHash("sha1").update(`${req.headers["sec-websocket-key"]}258EAFA5-E914-47DA-95CA-C5AB0DC85B11`).digest("base64");
    socket.write(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n\r\n`);
    socket.destroy();
  });
  server.allowed = allowed;
  return server;
}

function selfTestFakeStatic({ marker, policy, origins, connectApi = true, swRetiring = false }) {
  return createServer((req, res) => {
    if (req.url === "/") {
      const apiHost = new URL(origins.hub.replace(/^wss:/, "https:")).host;
      const connect = connectApi ? `'self' https://${apiHost} wss://${apiHost}` : "'self'";
      res.writeHead(200, {
        "content-type": "text/html",
        "permissions-policy": policy,
        "content-security-policy": `default-src 'self'; connect-src ${connect}`,
      });
      return res.end(`<!doctype html><html><head>${marker ? `<meta name="rh-hub" content="${marker}" />` : ""}</head><body>x</body></html>`);
    }
    if (req.url === "/sw.js") {
      res.writeHead(200, { "content-type": "text/javascript" });
      return res.end(swRetiring ? `self.addEventListener("install", () => self.registration.unregister());` : `self.addEventListener("install", () => self.skipWaiting());`);
    }
    if (req.url === "/.well-known/droppen-hub") {
      res.writeHead(200, { "content-type": "application/json" });
      return res.end(JSON.stringify({ origin: origins.hub, app: origins.app, site: origins.site }));
    }
    res.writeHead(404);
    res.end();
  });
}

async function listen(server) {
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  return `http://127.0.0.1:${server.address().port}`;
}

export async function runSelfTest() {
  const checks = [];
  const expect = (name, cond, detail = "") => checks.push({ name, ok: !!cond, detail });

  // Pure parsers.
  expect("cidr v4 cloudflare", isCloudflareAddress("104.21.46.157"));
  expect("cidr v4 railway", !isCloudflareAddress("69.46.46.10"));
  expect("cidr v6 cloudflare", isCloudflareAddress("2606:4700:3033::6815:1e9d"));
  expect("cidr v6 railway", !isCloudflareAddress("2600:1f18::1"));
  expect("marker", markerOrigin('<meta content="wss://api.droppen.test" name="rh-hub">') === "wss://api.droppen.test");
  expect("marker absent", markerOrigin("<html></html>") === null);
  expect("permissions self", allowsSelfOnly(permissionsPolicyValue("publickey-credentials-get=(self)", "publickey-credentials-get")));
  expect("permissions none", allowsNobody(permissionsPolicyValue("a=(self), publickey-credentials-get=()", "publickey-credentials-get")));
  expect("csp connect", cspSources("default-src 'self'; connect-src 'self' wss://api.droppen.test", "connect-src").includes("wss://api.droppen.test"));
  expect("retiring worker", swIsRetiring('self.addEventListener("install", () => self.registration.unregister())'));
  expect("active worker is not retiring", !swIsRetiring('self.addEventListener("install", (e) => self.skipWaiting())'));
  expect("install.sh default", installHub('# DROPPEN_HUB=wss://old.example\nHUB=${DROPPEN_HUB:-wss://api.droppen.app}') === "wss://api.droppen.app");
  expect("install.sh plain", installHub('export HUB="wss://api.droppen.app"') === "wss://api.droppen.app");

  const origins = { hub: null, app: null, site: null };
  const hub = selfTestFakeHub(origins);
  const hubUrl = await listen(hub);
  origins.hub = `wss://${new URL(hubUrl).host}`;
  const app = selfTestFakeStatic({ marker: origins.hub, policy: "publickey-credentials-get=(self), publickey-credentials-create=(self)", origins });
  const appUrl = await listen(app);
  origins.app = appUrl;
  const site = selfTestFakeStatic({ marker: null, policy: "publickey-credentials-get=(), publickey-credentials-create=()", origins, connectApi: false, swRetiring: true });
  const siteUrl = await listen(site);
  origins.site = siteUrl;
  hub.allowed.add(appUrl);
  hub.allowed.add(siteUrl);
  // These fakes are plain HTTP (the TLS and DNS checks are skipped): the
  // network checks only care about the host, and they are the ones under test.
  const env = { app: appUrl, site: siteUrl, apex: appUrl, api: hubUrl, timeoutMs: 5_000, skip: new Set(["dns", "tls"]) };
  try {
    const results = await checkDeployment(env);
    const byName = Object.fromEntries(results.map((r) => [r.name, r]));
    expect("self-test healthz", byName["api.healthz"]?.ok);
    expect("self-test readyz", byName["api.readyz"]?.ok);
    expect("self-test well-known", byName["api.well-known"]?.ok);
    expect("self-test cors app", byName["cors.app"]?.ok);
    expect("self-test cors site", byName["cors.site"]?.ok);
    expect("self-test cors evil", byName["cors.evil"]?.ok);
    expect("self-test ws", byName["ws.api.agent"]?.ok);
    expect("self-test ws bad path", byName["ws.api.bad-path"]?.ok);
    expect("self-test app marker", byName["static.app.marker"]?.ok);
    expect("self-test app webauthn", byName["static.app.webauthn"]?.ok);
    expect("self-test site webauthn", byName["static.site.webauthn"]?.ok);
    expect("self-test site csp", byName["static.site.csp-connect"]?.ok);
    expect("self-test site sw", byName["static.site.sw"]?.ok);
    // Deliberate breaks: the checker must see them.
    const broken = await checkDeployment({ ...env, app: siteUrl });
    const brokenByName = Object.fromEntries(broken.map((r) => [r.name, r]));
    expect("self-test sees a wrong marker", brokenByName["static.app.marker"]?.ok === false);
    expect("self-test sees a site policy on the app", brokenByName["static.app.webauthn"]?.ok === false);
    const evil = await fetchText(`${hubUrl}/v1/me`, { method: "OPTIONS", headers: { origin: "https://evil.test", "access-control-request-method": "GET" } }, 5_000);
    expect("self-test sees no CORS for evil", headerValue(evil.headers, "access-control-allow-origin") === null);
  } finally {
    for (const server of [hub, app, site]) server.close();
  }
  const failed = checks.filter((c) => !c.ok);
  for (const check of checks) console.log(`${check.ok ? "PASS" : "FAIL"} self-test.${check.name}${check.detail ? ` — ${check.detail}` : ""}`);
  console.log(`self-test: ${checks.length - failed.length}/${checks.length} passed`);
  return failed.length === 0;
}

// --------------------------------------------------------------------- cli

function parseArgs(argv) {
  const opts = { ...DEFAULTS, tokenFile: null, json: false, selfTest: false, help: false, timeoutMs: 10_000 };
  const valued = { "--app": "app", "--site": "site", "--apex": "apex", "--api": "api", "--token-file": "tokenFile", "--timeout": "timeoutMs" };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg in valued) {
      if (argv[i + 1] === undefined) throw new Error(`${arg} needs a value`);
      opts[valued[arg]] = argv[++i];
    } else if (arg === "--json") opts.json = true;
    else if (arg === "--self-test") opts.selfTest = true;
    else if (arg === "-h" || arg === "--help") opts.help = true;
    else throw new Error(`unknown argument ${arg} (see --help)`);
  }
  for (const name of ["app", "site", "apex", "api"]) {
    let url;
    try {
      url = new URL(opts[name]);
    } catch {
      throw new Error(`--${name} must be an origin like https://host, not ${opts[name]}`);
    }
    if (url.protocol !== "https:" && url.protocol !== "http:") throw new Error(`--${name} must be http(s), not ${opts[name]}`);
    opts[name] = stripSlash(opts[name]);
  }
  opts.timeoutMs = Number(opts.timeoutMs);
  if (!Number.isFinite(opts.timeoutMs) || opts.timeoutMs < 500) throw new Error("--timeout must be milliseconds");
  return opts;
}

const HELP = `Read-only deployment checks for Droppen.

  node scripts/e2e-deploy.mjs [options]

  --app URL           the app host (default ${DEFAULTS.app})
  --site URL          the public site (default ${DEFAULTS.site})
  --apex URL          the apex, and the temporary machine alias (default ${DEFAULTS.apex})
  --api URL           the machine + API host (default ${DEFAULTS.api})
  --token-file FILE   a device session token for read-only /v1/me (default: skipped)
  --json              one JSON object instead of lines
  --timeout MS        per request (default 10000)
  --self-test         test the checker itself against local fakes, then exit
`;

async function main() {
  let opts;
  try {
    opts = parseArgs(process.argv.slice(2));
  } catch (error) {
    console.error(`e2e-deploy: ${error.message}`);
    process.exitCode = 2;
    return;
  }
  if (opts.help) {
    console.log(HELP);
    return;
  }
  if (opts.selfTest) {
    const ok = await runSelfTest();
    process.exitCode = ok ? 0 : 2;
    return;
  }
  const token = opts.tokenFile ? readFileSync(opts.tokenFile, "utf8").trim() : null;
  if (opts.tokenFile && !token) {
    console.error(`e2e-deploy: ${opts.tokenFile} is empty`);
    process.exitCode = 2;
    return;
  }
  const results = await checkDeployment({ ...opts, token });
  if (opts.json) {
    console.log(JSON.stringify({ hosts: { app: opts.app, site: opts.site, apex: opts.apex, api: opts.api }, results }, null, 2));
  } else {
    for (const r of results) {
      const mark = r.skipped ? "SKIP" : r.ok ? "PASS" : "FAIL";
      console.log(`${mark} ${r.name} — expected ${r.expected}; got ${r.actual}${r.note ? `; ${r.note}` : ""}`);
    }
    const failed = results.filter((r) => !r.ok);
    console.log(`${results.length - results.filter((r) => r.skipped).length - failed.length} passed, ${failed.length} failed, ${results.filter((r) => r.skipped).length} skipped`);
  }
  if (results.some((r) => !r.ok)) process.exitCode = 1;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    console.error(`e2e-deploy: ${error.stack || error.message}`);
    process.exitCode = 2;
  });
}
