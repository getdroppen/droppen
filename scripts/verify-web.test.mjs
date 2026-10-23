// Tests for scripts/verify-web.mjs: `node --test scripts/verify-web.test.mjs`.
//
// The CLI runs against a fake hub and a fake GitHub on 127.0.0.1 (nothing
// leaves the machine). VERIFY_WEB_SCRIPT points the tests at another copy of
// the script, which is how each fix is shown to be load-bearing.

import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, before, describe, test } from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";
import { digestOf, filesOf, FORMAT, workerProblems as manifestWorkerProblems } from "./web-manifest.mjs";

const script = process.env.VERIFY_WEB_SCRIPT ?? fileURLToPath(new URL("./verify-web.mjs", import.meta.url));
const manifestScript = fileURLToPath(new URL("./web-manifest.mjs", import.meta.url));
const { checkedRedirect, markerProblem, RETIRING_WORKER, scriptPolicy, siteHubOrigin, unmark, workerProblems, workerTable } = await import(pathToFileURL(script).href);

/** `text` as a regular expression that matches it literally: every special escaped. */
const escapeRegExp = (text) => text.replace(/[\\^$.*+?()[\]{}|/-]/g, "\\$&");

const INLINE = "window.boot=1";
const INLINE_HASH = `'sha256-${createHash("sha256").update(INLINE).digest("base64")}'`;
/** The policy the hub serves the web app with (docs/hub-api/http.md). */
const HUB_CSP = `default-src 'self'; script-src 'self' ${INLINE_HASH}; style-src 'self' 'unsafe-inline'; connect-src 'self' wss://hub.example; frame-ancestors 'none'; object-src 'none'`;

// ------------------------------------------------------------------ units

describe("the rh-hub marker", () => {
  test("a site's own hub origin is wss:// on https and ws:// on http", () => {
    assert.equal(siteHubOrigin("https://droppen.app"), "wss://droppen.app");
    assert.equal(siteHubOrigin("http://127.0.0.1:8080"), "ws://127.0.0.1:8080");
  });

  test("only the site's own origin, or one passed explicitly, is accepted", () => {
    const shell = Buffer.from('<html><head><meta name="rh-hub" content="wss://evil.example" /><title>x</title></head></html>');
    const { body, origin } = unmark(shell);
    assert.equal(body.toString(), "<html><head><title>x</title></head></html>");
    assert.equal(origin, "wss://evil.example");
    assert.match(markerProblem(origin, ["wss://droppen.app"]), /pointed at the hub wss:\/\/evil\.example/);
    assert.equal(markerProblem(origin, ["wss://droppen.app", "wss://evil.example"]), null);
    assert.equal(markerProblem("wss://DROPPEN.app", ["wss://droppen.app"]), null);
    assert.equal(markerProblem(null, ["wss://droppen.app"]), null, "no marker: the page talks to no hub");
  });
});

describe("a redirect", () => {
  const sites = ["https://my.droppen.app", "https://www.droppen.app", "https://droppen.app"];
  test("to the same path on another site of the run is checked there", () => {
    assert.equal(checkedRedirect("https://www.droppen.app/", "https://droppen.app", "/", sites), "https://www.droppen.app");
    assert.equal(checkedRedirect("https://my.droppen.app/link", "https://www.droppen.app", "/link", sites), "https://my.droppen.app");
    assert.equal(checkedRedirect("https://MY.droppen.app:443/signin", "https://droppen.app", "/signin", sites), "https://my.droppen.app");
  });
  test("anywhere else is not", () => {
    for (const [location, from, path] of [
      ["https://evil.example/", "https://droppen.app", "/"],
      ["https://www.droppen.app/elsewhere", "https://droppen.app", "/"],
      ["https://www.droppen.app/?x=1", "https://droppen.app", "/"],
      ["/link/", "https://droppen.app", "/link"],
      ["https://droppen.app/", "https://droppen.app", "/"],
      ["http://my.droppen.app/link", "https://droppen.app", "/link"],
      [null, "https://droppen.app", "/"],
      ["https://[bad", "https://droppen.app", "/"],
    ]) {
      assert.equal(checkedRedirect(location, from, path, sites), null, `${from}${path} → ${location}`);
    }
    assert.equal(checkedRedirect("https://my.droppen.app/link", "https://droppen.app", "/link", ["https://droppen.app"]), null, "a site this run does not check");
  });
});

describe("the script policy", () => {
  const hashes = [INLINE_HASH];
  test("the hub's own policy passes", () => {
    assert.equal(scriptPolicy(HUB_CSP, hashes).ok, true);
  });
  test("script-src-elem counts like script-src", () => {
    const r = scriptPolicy("script-src 'self'; script-src-elem https://evil.example 'unsafe-inline'", hashes);
    assert.equal(r.ok, false);
    assert.match(r.note, /script-src-elem lets script elements come from https:\/\/evil\.example 'unsafe-inline'/);
  });
  test("script-src-attr counts like script-src", () => {
    const r = scriptPolicy("script-src 'self'; script-src-attr 'unsafe-inline'", hashes);
    assert.equal(r.ok, false);
    assert.match(r.note, /script-src-attr/);
  });
  test("another source in script-src or default-src fails", () => {
    assert.equal(scriptPolicy(`script-src 'self' ${INLINE_HASH} https://cdn.example`, hashes).ok, false);
    assert.equal(scriptPolicy("default-src 'self' 'unsafe-inline'", hashes).ok, false);
    assert.equal(scriptPolicy("img-src 'self'", hashes).ok, false, "no script restriction at all");
  });
  test("with several policies, one strict enough for each kind is enough", () => {
    assert.equal(scriptPolicy("script-src 'self' 'unsafe-inline', script-src 'self'", hashes).ok, true);
    assert.equal(scriptPolicy("script-src-elem 'self'; script-src-attr 'unsafe-inline', script-src-attr 'none'; script-src-elem https://x", hashes).ok, true);
    assert.equal(scriptPolicy("script-src 'self'; script-src 'unsafe-inline'", hashes).ok, true, "a repeated directive is ignored");
  });
});

describe("the service worker's integrity table", () => {
  const files = [
    { path: "index.html", sha256: "a".repeat(64) },
    { path: "assets/app-1.js", sha256: "b".repeat(64) },
    { path: "assets/app-1.css", sha256: "c".repeat(64) },
    { path: "sw.js", sha256: "d".repeat(64) },
    { path: "p2p-probe.js", sha256: "e".repeat(64) },
  ];
  const sw = (assets, shell = "a".repeat(64), scripts = [INLINE_HASH]) =>
    `const PRECACHE = [];\nconst INTEGRITY = ${JSON.stringify(assets)};\nconst SHELL_SHA256 = "${shell}";\nconst SHELL_SCRIPTS = ${JSON.stringify(scripts)};\n`;
  const html = Buffer.from(`<html><head><script>${INLINE}</script><script type="module" src="/assets/app-1.js"></script></head></html>`);
  const good = { "/assets/app-1.css": "c".repeat(64), "/assets/app-1.js": "b".repeat(64) };

  test("one that lists every asset and the shell, as built, agrees (in both scripts)", () => {
    assert.deepEqual(workerProblems(workerTable(sw(good)), files, html), []);
    assert.deepEqual(manifestWorkerProblems(files, sw(good), html), []);
  });

  test("a missing, extra or different asset, or another shell, is a disagreement (in both scripts)", () => {
    const cases = [
      [{ "/assets/app-1.js": "b".repeat(64) }, undefined, /does not list \/assets\/app-1\.css/],
      [{ ...good, "/assets/evil.js": "f".repeat(64) }, undefined, /lists \/assets\/evil\.js, which/],
      [{ ...good, "/assets/app-1.js": "f".repeat(64) }, undefined, /lists \/assets\/app-1\.js as f{64}/],
      [good, "f".repeat(64), /the shell as f{64}/],
      [good, "", /the shell as \(none\)/],
    ];
    for (const [assets, shell, why] of cases) {
      const text = sw(assets, shell);
      assert.match(workerProblems(workerTable(text), files).join("\n"), why);
      assert.match(manifestWorkerProblems(files, text).join("\n"), why);
    }
  });

  test("the shell's inline scripts the worker allows must be the shell's own (in both scripts)", () => {
    for (const [scripts, why] of [
      [[], /allows the shell's inline scripts \(none\), index\.html has 'sha256-/],
      [[INLINE_HASH, "'unsafe-inline'"], /allows the shell's inline scripts 'sha256-\S+ 'unsafe-inline'/],
      [["'sha256-AAAA'"], /allows the shell's inline scripts 'sha256-AAAA'/],
    ]) {
      assert.match(workerProblems(workerTable(sw(good, undefined, scripts)), files, html).join("\n"), why);
      assert.match(manifestWorkerProblems(files, sw(good, undefined, scripts), html).join("\n"), why);
    }
    assert.equal(workerTable(sw(good).replace(/^const SHELL_SCRIPTS.*$/m, "")), null, "a table without them is none");
    assert.equal(workerTable(sw(good, undefined, [1])), null, "not a list of strings");
  });

  test("a worker with no table (an older build) has none; web-manifest.mjs refuses it", () => {
    assert.equal(workerTable("const CACHE = 1;\nconst ASSETS = [];\n"), null);
    assert.equal(workerTable('const INTEGRITY = {"/assets/a.js": 1};\nconst SHELL_SHA256 = "";\n'), null, "not a table of sha256 strings");
    assert.match(manifestWorkerProblems(files, "const ASSETS = [];\n").join(), /no integrity table/);
  });
});

// ------------------------------------------------------------- end to end

const root = mkdtempSync(join(tmpdir(), "verify-web-test-"));
const sha = (text) => createHash("sha256").update(text).digest("hex");
const SHELL = `<!doctype html><html><head><title>Droppen</title><script>${INLINE}</script></head><body><script type="module" src="/assets/app-1.js"></script></body></html>`;

/**
 * A build in `root/<name>`: its shell, one asset, the p2p probe, and a
 * `sw.js` whose integrity table is right — or `table(assets, shell, scripts)`
 * makes another (`null`: a worker with no table) — and its manifest
 * (`<name>.json`).
 */
function makeBuild(name, table = (assets, shell, scripts) => ({ assets, shell, scripts })) {
  const dist = join(root, name);
  mkdirSync(join(dist, "assets"), { recursive: true });
  const APP = "console.log('the app')";
  writeFileSync(join(dist, "index.html"), SHELL);
  writeFileSync(join(dist, "assets", "app-1.js"), APP);
  writeFileSync(join(dist, "p2p-probe.html"), '<!doctype html><script src="/p2p-probe.js"></script>');
  writeFileSync(join(dist, "p2p-probe.js"), "probe()");
  const t = table({ "/assets/app-1.js": sha(APP) }, sha(SHELL), [INLINE_HASH]);
  writeFileSync(
    join(dist, "sw.js"),
    t === null
      ? `const CACHE = "x";\nconst ASSETS = [];\n`
      : `const CACHE = "x";\nconst PRECACHE = [];\nconst INTEGRITY = ${JSON.stringify(t.assets)};\nconst SHELL_SHA256 = ${JSON.stringify(t.shell)};\nconst SHELL_SCRIPTS = ${JSON.stringify(t.scripts ?? [INLINE_HASH])};\n`,
  );
  const files = filesOf(dist);
  const manifest = { format: FORMAT, commit: "0123abc", ref: "refs/tags/v9.9.9", distribution: "public", digest: digestOf(files), files };
  const manifestFile = join(root, `${name}.json`);
  writeFileSync(manifestFile, JSON.stringify(manifest));
  return { dist, files, manifest, manifestFile };
}

const build = makeBuild("dist");
const { dist, files, manifest, manifestFile } = build;

/**
 * A hub that serves a build (`dist` by default) the way rh-hub does.
 * `marker`: the origin written into the shell (`self` for the site's own);
 * `csp(path)`: the policy header; `redirect(path)`: a `308` there instead
 * (`null`: none); `worker(req)`: another `/sw.js` (`null`: the build's).
 */
function fakeHub({ marker = "self", csp = () => HUB_CSP, serves = build, redirect = () => null, worker = () => null } = {}) {
  const server = createServer((req, res) => {
    const path = new URL(req.url, "http://x").pathname.slice(1);
    const sw = path === "sw.js" ? worker(req) : null;
    if (sw !== null) {
      res.setHeader("content-type", "text/javascript");
      return res.end(sw);
    }
    const location = redirect(`/${path}`);
    if (location) {
      res.writeHead(308, { location });
      return res.end();
    }
    if (path === ".well-known/droppen-build") {
      res.setHeader("content-type", "application/json");
      return res.end(JSON.stringify({ commit: "0123abc", web: { digest: serves.manifest.digest, files: serves.files.length } }));
    }
    const file = serves.files.find((f) => f.path === path && path !== "index.html");
    res.setHeader("content-security-policy", csp(`/${path}`));
    if (file) return res.end(readFileSync(join(serves.dist, file.path)));
    if (path.startsWith("assets/")) {
      res.statusCode = 404;
      return res.end("not found");
    }
    const origin = marker === "self" ? `ws://${req.headers.host}` : marker;
    res.end(SHELL.replace("<head>", `<head><meta name="rh-hub" content="${origin}" />`));
  });
  return listen(server);
}

/**
 * What release.yml attaches to the signer repository's release: the manifest
 * CI built (the same files and digest as the release's, other bytes: it also
 * names the repository and the build time) and a bundle that signs it. The
 * stand-in `gh` below accepts a bundle only for the bytes, repository,
 * workflow and tag it names, as the real one checks them.
 */
const signedManifest = Buffer.from(JSON.stringify({ ...manifest, repository: "s/r", source_date_epoch: 1700000000 }));
const sha256Hex = (bytes) => createHash("sha256").update(bytes).digest("hex");
const bundleFor = (bytes, repo = "s/r") =>
  JSON.stringify({ subject: sha256Hex(bytes), repo, workflow: `${repo}/.github/workflows/release.yml`, ref: "refs/tags/v9.9.9" });
/** Another build's manifest under the same tag. */
const otherManifest = (() => {
  const files2 = [...files, { path: "extra.js", size: 1, sha256: "0".repeat(64) }];
  return Buffer.from(JSON.stringify({ ...manifest, digest: digestOf(files2), files: files2 }));
})();

/**
 * A GitHub with one release in o/r whose manifest is ours and no Sigstore
 * bundle, and signer repositories with a release of the same tag:
 *   s/r  the signed manifest and its bundle;
 *   x/r  the signed manifest and a bundle over other bytes (the release's own manifest);
 *   d/r  another build's manifest, signed;
 *   e/r  an error (HTTP 500) for anything;
 *   anything else: nothing (404).
 */
function fakeGitHub() {
  const server = createServer((req, res) => {
    if (req.url.startsWith("/repos/o/r/releases")) {
      res.setHeader("content-type", "application/json");
      return res.end(JSON.stringify([{ tag_name: "v9.9.9", draft: false, assets: [{ name: "web-manifest.json" }] }]));
    }
    if (req.url === "/o/r/releases/download/v9.9.9/web-manifest.json") return res.end(JSON.stringify(manifest));
    const signer = /^\/([sxde])\/r\/releases\/download\/v9\.9\.9\/(web-manifest\.json|web-manifest\.sigstore\.json)$/.exec(req.url);
    if (signer) {
      const [, who, asset] = signer;
      if (who === "e") {
        res.statusCode = 500;
        return res.end("oops");
      }
      const signed = who === "d" ? otherManifest : signedManifest;
      if (asset === "web-manifest.json") return res.end(signed);
      return res.end(bundleFor(who === "x" ? Buffer.from(JSON.stringify(manifest)) : signed, `${who}/r`));
    }
    res.statusCode = 404;
    res.end("not found");
  });
  return listen(server);
}

/**
 * A stand-in `gh` on PATH: `--version` answers; `attestation verify <file>
 * --bundle <b> --repo <r> --signer-workflow <w> --source-ref <ref>` passes
 * only when the bundle names the file's sha256 and that repository, workflow
 * and tag. It records the bytes it was asked about.
 */
function standInGh(dir) {
  const js = join(dir, "gh.mjs");
  writeFileSync(
    js,
    `import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
const args = process.argv.slice(2);
if (args[0] === "--version") process.exit(0);
const flag = (name) => args[args.indexOf(name) + 1];
const file = readFileSync(args[2]);
writeFileSync(process.env.GH_SEEN, file);
const b = JSON.parse(readFileSync(flag("--bundle"), "utf8"));
const subject = createHash("sha256").update(file).digest("hex");
if (b.subject !== subject) { console.error("verification failed: the bundle signs other bytes"); process.exit(1); }
if (b.repo !== flag("--repo") || b.workflow !== flag("--signer-workflow") || b.ref !== flag("--source-ref")) { console.error("verification failed: another signer"); process.exit(1); }
`,
  );
  writeFileSync(join(dir, "gh"), `#!/bin/sh\nexec "${process.execPath}" "${js}" "$@"\n`, { mode: 0o755 });
  return { PATH: `${dir}:${process.env.PATH}`, GH_SEEN: join(dir, "seen") };
}

function listen(server) {
  return new Promise((resolve) => server.listen(0, "127.0.0.1", () => resolve({ server, url: `http://127.0.0.1:${server.address().port}` })));
}

/** Run the CLI; `{code, out}`. Asynchronous: the fake servers live in this process. */
function verify(args, env = {}) {
  return new Promise((resolve) => {
    execFile(process.execPath, [script, ...args], { env: { ...process.env, ...env } }, (error, stdout, stderr) =>
      resolve({ code: error ? error.code : 0, out: stdout + stderr }),
    );
  });
}

describe("verify-web.mjs against a hub", () => {
  const servers = [];
  const hub = async (options) => {
    const s = await fakeHub(options);
    servers.push(s.server);
    return s.url;
  };
  let github;
  before(async () => {
    const s = await fakeGitHub();
    servers.push(s.server);
    github = s.url;
  });
  after(() => {
    for (const s of servers) s.close();
    rmSync(root, { recursive: true, force: true });
  });

  test("the attested build with its own marker passes", async () => {
    const site = await hub();
    const r = await verify(["--site", site, "--manifest", manifestFile]);
    assert.equal(r.code, 0, r.out);
    assert.match(r.out, /PASS/);
    assert.match(r.out, /worker: +sw\.js lists 1 assets and the shell, as the manifest has them/);
  });

  test("an attested worker whose integrity table disagrees with the manifest fails", async () => {
    const odd = makeBuild("odd", (assets, shell) => ({ assets: { ...assets, "/assets/vendor-old.js": "f".repeat(64) }, shell }));
    const site = await hub({ serves: odd });
    const r = await verify(["--site", site, "--manifest", odd.manifestFile]);
    assert.equal(r.code, 1, r.out);
    assert.match(r.out, /sw\.js lists \/assets\/vendor-old\.js, which the manifest does not/);
  });

  test("an attested worker with no integrity table fails: its cache would be served unchecked", async () => {
    const old = makeBuild("old-worker", () => null);
    const site = await hub({ serves: old });
    const r = await verify(["--site", site, "--manifest", old.manifestFile]);
    assert.equal(r.code, 1, r.out);
    assert.match(r.out, /sw\.js has no integrity table: the service worker would serve whatever is in the browser's cache, unchecked/);
  });

  test("an attested worker that allows other inline scripts than the shell's fails", async () => {
    const odd = makeBuild("odd-scripts", (assets, shell) => ({ assets, shell, scripts: [INLINE_HASH, "'sha256-AAAA'"] }));
    const site = await hub({ serves: odd });
    const r = await verify(["--site", site, "--manifest", odd.manifestFile]);
    assert.equal(r.code, 1, r.out);
    assert.match(r.out, /sw\.js allows the shell's inline scripts/);
  });

  test("web-manifest.mjs writes no manifest for such a build, and --check reports it", async () => {
    const odd = makeBuild("odd-shell", (assets) => ({ assets, shell: "f".repeat(64) }));
    mkdirSync(join(odd.dist, "plugins"));
    writeFileSync(join(odd.dist, "plugins", "manifest.json"), JSON.stringify({ distribution: "public", plugins: [] }));
    const run = (args) =>
      new Promise((resolve) => execFile(process.execPath, [manifestScript, ...args], (error, stdout, stderr) => resolve({ code: error ? error.code : 0, out: stdout + stderr })));
    const write = await run([odd.dist]);
    assert.equal(write.code, 2, write.out);
    assert.match(write.out, /integrity table does not match the build:\n +sw\.js has the shell as f{64}, index\.html is [0-9a-f]{64}/);
    const check = await run([odd.dist, "--check", odd.manifestFile]);
    assert.equal(check.code, 1, check.out);
    assert.match(check.out, /worker +sw\.js has the shell as f{64}/);
    const fine = await run([dist, "--check", manifestFile]);
    assert.equal(fine.code, 0, fine.out);
  });

  test("a byte-identical page pointed at another hub fails", async () => {
    const site = await hub({ marker: "wss://evil.example" });
    const r = await verify(["--site", site, "--manifest", manifestFile]);
    assert.equal(r.code, 1, r.out);
    assert.match(r.out, /pointed at the hub wss:\/\/evil\.example/);
  });

  test("another hub origin passes only when named with --hub-origin", async () => {
    const site = await hub({ marker: "wss://hub.example" });
    const r = await verify(["--site", site, "--manifest", manifestFile, "--hub-origin", "wss://hub.example"]);
    assert.equal(r.code, 0, r.out);
  });

  test("the policy is checked on every page, not only /", async () => {
    for (const bad of ["/link", "/signin", "/p2p-probe.html"]) {
      const site = await hub({ csp: (path) => (path === bad ? `${HUB_CSP}; script-src-elem 'self' https://evil.example` : HUB_CSP) });
      const r = await verify(["--site", site, "--manifest", manifestFile]);
      assert.equal(r.code, 1, `${bad}: ${r.out}`);
      assert.match(r.out, new RegExp(`${bad.replace(".", "\\.")}: Content-Security-Policy: script-src-elem`));
    }
  });

  test("both scripts run when started through a symlink (macOS /tmp is one)", async () => {
    const links = mkdtempSync(join(tmpdir(), "verify-web-links-"));
    try {
      for (const name of ["verify-web.mjs", "web-manifest.mjs"]) {
        const target = name === "verify-web.mjs" ? script : fileURLToPath(new URL(`./${name}`, import.meta.url));
        symlinkSync(target, join(links, name));
        const r = await new Promise((resolve) =>
          execFile(process.execPath, [join(links, name), "--help"], (error, stdout) => resolve({ code: error ? error.code : 0, out: stdout })),
        );
        assert.equal(r.code, 0, name);
        assert.match(r.out, /usage|Check that a site/, `${name} printed nothing: its entry check missed`);
      }
    } finally {
      rmSync(links, { recursive: true, force: true });
    }
  });

  describe("droppen.app's three hosts", () => {
    // As the hub serves them once the app moved: my. holds the app; www. the
    // site, sending app paths to my.; the apex sends the site's paths to www.,
    // the app's own to my. (or shows its interstitial, the shell), and
    // retires the worker. Each host is a port of its own on 127.0.0.1.
    const site = new Set(["/", "/security", "/download"]);
    const app = new Set(["/link", "/signin", "/sessions", "/move"]);
    const hosts = {};
    const to = (name, path) => `${hosts[name]}${path}`;
    const hostsUp = async (overrides = {}) => {
      hosts.my = await hub({ redirect: (p) => (["/security", "/download"].includes(p) ? to("www", p) : null), ...overrides.my });
      hosts.www = await hub({ redirect: (p) => (app.has(p) ? to("my", p) : null), worker: () => RETIRING_WORKER, ...overrides.www });
      hosts.apex = await hub({ redirect: (p) => (site.has(p) ? to("www", p) : null), worker: () => RETIRING_WORKER, ...overrides.apex });
      return [hosts.my, hosts.www, hosts.apex];
    };
    const sites = (urls) => urls.flatMap((u) => ["--site", u]);

    test("each passes on its own: own marker, own policy, redirects to the others checked there, the retiring worker", async () => {
      const urls = await hostsUp();
      const r = await verify([...sites(urls), "--manifest", manifestFile]);
      assert.equal(r.code, 0, r.out);
      assert.equal(r.out.match(/^PASS: /gm)?.length, 3, r.out);
      for (const u of urls) assert.match(r.out, new RegExp(`marker: +rh-hub ${escapeRegExp(siteHubOrigin(u))}$`, "m"));
      assert.match(r.out, new RegExp(`files: +\\d+ of \\d+ match, 4 redirect to ${escapeRegExp(hosts.my)}, checked there`));
      assert.match(r.out, /\/sw\.js is the retiring worker/);
      assert.match(r.out, /summary:\n +PASS +http:\/\/127\.0\.0\.1:\d+\n +PASS/);
    });

    test("one host's policy, worker or marker fails that host, and the run", async () => {
      const cases = [
        [{ www: { csp: (p) => (p === "/security" ? `${HUB_CSP}; script-src-elem 'self' https://evil.example` : HUB_CSP) } }, "www", /\/security: Content-Security-Policy: script-src-elem/],
        [{ apex: { worker: () => "self.addEventListener('fetch', () => {});\n" } }, "apex", /\/sw\.js: sha256 [0-9a-f]{64}, attested/],
        [{ apex: { worker: (req) => (req.headers["service-worker"] === "script" ? "evil()\n" : RETIRING_WORKER) } }, "apex", /\/sw\.js \(Service-Worker: script\): sha256/],
        [{ www: { marker: "ws://127.0.0.1:1" } }, "www", /pointed at the hub ws:\/\/127\.0\.0\.1:1,/],
      ];
      for (const [overrides, bad, why] of cases) {
        const urls = await hostsUp(overrides);
        const r = await verify([...sites(urls), "--manifest", manifestFile]);
        assert.equal(r.code, 1, r.out);
        assert.match(r.out, why);
        assert.match(r.out, new RegExp(`FAIL: ${escapeRegExp(hosts[bad])} does not serve`));
        assert.equal(r.out.match(/^PASS: /gm)?.length, 2, r.out);
      }
    });

    test("a host that sends everything on to another of the run passes through it", async () => {
      hosts.apex = await hub();
      const www = await hub({ redirect: (p) => to("apex", p) });
      const r = await verify(["--site", www, "--site", hosts.apex, "--manifest", manifestFile]);
      assert.equal(r.code, 0, r.out);
      assert.match(r.out, new RegExp(`droppen-build redirects to ${escapeRegExp(hosts.apex)}: the build that one serves`));
      assert.match(r.out, new RegExp(`files: +0 of 13 match, 13 redirect to ${escapeRegExp(hosts.apex)}, checked there`));
      const alone = await verify(["--site", www, "--manifest", manifestFile]);
      assert.equal(alone.code, 1, alone.out);
    });

    test("a redirect to a host the run does not check, or to another path, fails", async () => {
      const urls = await hostsUp();
      const alone = await verify(["--site", hosts.apex, "--manifest", manifestFile]);
      assert.equal(alone.code, 1, alone.out);
      assert.match(alone.out, /\/: .*HTTP 308 → .*\/ \(not the same path on another site this run checks\)/);
      const elsewhere = await hostsUp({ apex: { redirect: (p) => (p === "/" ? to("www", "/download") : null) } });
      const r = await verify([...sites(elsewhere), "--manifest", manifestFile]);
      assert.equal(r.code, 1, r.out);
      assert.match(r.out, /\/: .*HTTP 308 → .*\/download \(not the same path/);
      assert.ok(urls.length === 3);
    });

    test("a host that cannot be reached is not checked; the others are", async () => {
      const up = await hub();
      const closed = await new Promise((resolve) => {
        const probe = createServer().listen(0, "127.0.0.1", () => {
          const url = `http://127.0.0.1:${probe.address().port}`;
          probe.close(() => resolve(url));
        });
      });
      const r = await verify([...sites([up, closed]), "--manifest", manifestFile]);
      assert.equal(r.code, 2, r.out);
      assert.match(r.out, new RegExp(`could not check ${escapeRegExp(closed)}: `));
      assert.match(r.out, new RegExp(`summary:\\n +PASS +${escapeRegExp(up)}\\n +could not check +${escapeRegExp(closed)}`));
    });
  });

  test("releases come from getdroppen/droppen unless --repo names another", async () => {
    const site = await hub();
    const env = { DROPPEN_VERIFY_GITHUB_API: github, DROPPEN_VERIFY_GITHUB: github };
    // The fake GitHub only knows o/r: the default repository is asked, and is not there.
    const r = await verify(["--site", site], env);
    assert.equal(r.code, 2, r.out);
    assert.match(r.out, /\/repos\/getdroppen\/droppen\/releases\?per_page=30: HTTP 404/);
  });

  test("--signer-repo must be OWNER/NAME", async () => {
    const r = await verify(["--signer-repo", "not a repo"]);
    assert.equal(r.code, 2, r.out);
    assert.match(r.out, /--signer-repo must be OWNER\/NAME/);
  });

  test("a release manifest must be attested unless --allow-unattested", async () => {
    const site = await hub();
    const env = { DROPPEN_VERIFY_GITHUB_API: github, DROPPEN_VERIFY_GITHUB: github };
    const strict = await verify(["--site", site, "--repo", "o/r"], env);
    assert.equal(strict.code, 1, strict.out);
    assert.match(strict.out, /attestation not checked: no Sigstore bundle published with this release, nor a signed manifest with getdroppen\/droppen's release v9\.9\.9 \(--allow-unattested accepts that\)/);
    const lax = await verify(["--site", site, "--repo", "o/r", "--allow-unattested"], env);
    assert.equal(lax.code, 0, lax.out);
    assert.match(lax.out, /release v9\.9\.9 of o\/r/);
  });

  describe("a release without a bundle of its own is checked through the signer repository's release of the tag", () => {
    let bin, env;
    before(() => {
      bin = mkdtempSync(join(tmpdir(), "verify-web-gh-"));
      env = { DROPPEN_VERIFY_GITHUB_API: github, DROPPEN_VERIFY_GITHUB: github, ...standInGh(bin) };
    });
    after(() => rmSync(bin, { recursive: true, force: true }));
    const run = async (signer, ...more) => verify(["--site", await hub(), "--repo", "o/r", "--signer-repo", signer, ...more], env);

    test("its signed manifest and that manifest's bundle pass", async () => {
      const r = await run("s/r");
      assert.equal(r.code, 0, r.out);
      assert.match(r.out, /manifest: +release v9\.9\.9 of s\/r, the signed manifest of release v9\.9\.9 of o\/r/);
      assert.match(r.out, /verified: built by s\/r\/\.github\/workflows\/release\.yml at v9\.9\.9/);
      assert.deepEqual(readFileSync(env.GH_SEEN), signedManifest, "the signed bytes are what gh checks, not the release's own manifest");
    });

    test("a bundle over other bytes fails, --allow-unattested or not", async () => {
      for (const more of [[], ["--allow-unattested"]]) {
        const r = await run("x/r", ...more);
        assert.equal(r.code, 1, r.out);
        assert.match(r.out, /attestation FAILED: verification failed: the bundle signs other bytes/);
      }
    });

    test("a signed manifest of another build under the tag fails, --allow-unattested or not", async () => {
      for (const more of [[], ["--allow-unattested"]]) {
        const r = await run("d/r", ...more);
        assert.equal(r.code, 1, r.out);
        assert.match(r.out, /the signed manifest of d\/r's release v9\.9\.9 is sha256:[0-9a-f]{64}, not the release's sha256:/);
      }
    });

    test("none there, or the signer repository failing, is a missing attestation", async () => {
      for (const [signer, why] of [["n/r", /nor a signed manifest with n\/r's release v9\.9\.9/], ["e/r", /e\/r's release v9\.9\.9 could not be read \(.*HTTP 500\)/]]) {
        const strict = await run(signer);
        assert.equal(strict.code, 1, strict.out);
        assert.match(strict.out, why);
        assert.match(strict.out, /\(--allow-unattested accepts that\)/);
        const lax = await run(signer, "--allow-unattested");
        assert.equal(lax.code, 0, lax.out);
        assert.match(lax.out, /release v9\.9\.9 of o\/r \(attestation not checked\)/);
      }
    });
  });
});
