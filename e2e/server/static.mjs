// The static hosts: `dist` + the `_headers` file the build wrote, applied the
// way a static host (Cloudflare Pages) applies it. The specs assert on these
// headers, so the server must serve the file rather than a hardcoded policy.
import { existsSync, readFileSync, statSync } from "node:fs";
import { extname, join, normalize, resolve, sep } from "node:path";

const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".webmanifest": "application/manifest+json",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".jpg": "image/jpeg",
  ".jpeg": "image/jpeg",
  ".webp": "image/webp",
  ".ico": "image/x-icon",
  ".wasm": "application/wasm",
  ".woff2": "font/woff2",
  ".txt": "text/plain; charset=utf-8",
  ".map": "application/json; charset=utf-8",
};

/**
 * A Cloudflare `_headers` file: a path pattern on a line of its own, then its
 * headers indented. Blank lines end a rule. Patterns here are `/*`, an exact
 * path, or `/<dir>/*`.
 */
export function parseHeaders(text) {
  const rules = [];
  let current = null;
  for (const line of String(text).split("\n")) {
    if (!line.trim()) {
      current = null;
      continue;
    }
    if (/^\s/.test(line)) {
      const match = /^\s+([^:]+):\s*(.*)$/.exec(line);
      if (match && current) current.headers[match[1].trim()] = match[2].trim();
      continue;
    }
    current = { pattern: line.trim(), headers: {} };
    rules.push(current);
  }
  return rules;
}

export function patternMatches(pattern, pathname) {
  if (pattern === "/*") return true;
  if (pattern.endsWith("/*")) {
    const base = pattern.slice(0, -1);
    return pathname === base.slice(0, -1) || pathname.startsWith(base);
  }
  return pathname === pattern;
}

/** Every header whose rule matches `pathname`; later rules win. */
export function headersFor(rules, pathname) {
  const headers = {};
  for (const rule of rules) if (patternMatches(rule.pattern, pathname)) Object.assign(headers, rule.headers);
  return headers;
}

export function createStatic(root) {
  const rootPath = resolve(root);
  const headersPath = join(rootPath, "_headers");
  const rules = existsSync(headersPath) ? parseHeaders(readFileSync(headersPath, "utf8")) : [];

  return function serve(req, res, pathname) {
    let decoded;
    try {
      decoded = decodeURIComponent(pathname);
    } catch {
      res.writeHead(400);
      return res.end("bad path");
    }
    const headers = headersFor(rules, decoded);
    let file = normalize(join(rootPath, decoded));
    if (!file.startsWith(rootPath + sep) && file !== rootPath) {
      res.writeHead(403);
      return res.end("outside the root");
    }
    if (existsSync(file) && statSync(file).isDirectory()) file = join(file, "index.html");
    if (!existsSync(file)) {
      // SPA navigations get the shell; a missing asset is a 404.
      if (extname(decoded)) {
        res.writeHead(404, { "content-type": "text/plain" });
        return res.end("not found");
      }
      file = join(rootPath, "index.html");
      if (!existsSync(file)) {
        res.writeHead(404);
        return res.end("not found");
      }
    }
    const type = TYPES[extname(file).toLowerCase()] ?? "application/octet-stream";
    const out = {
      "content-type": type,
      "cache-control": "no-store",
      ...headers,
    };
    let body;
    try {
      body = readFileSync(file);
    } catch {
      res.writeHead(500);
      return res.end("unreadable");
    }
    res.writeHead(200, out);
    if (req.method === "HEAD") return res.end();
    res.end(body);
  };
}
