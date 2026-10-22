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
import { digestOf, filesOf, FORMAT } from "./web-manifest.mjs";

const script = process.env.VERIFY_WEB_SCRIPT ?? fileURLToPath(new URL("./verify-web.mjs", import.meta.url));
const { markerProblem, scriptPolicy, siteHubOrigin, unmark } = await import(pathToFileURL(script).href);

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

// ------------------------------------------------------------- end to end

const root = mkdtempSync(join(tmpdir(), "verify-web-test-"));
const dist = join(root, "dist");
mkdirSync(join(dist, "assets"), { recursive: true });
const SHELL = `<!doctype html><html><head><title>Droppen</title><script>${INLINE}</script></head><body><script type="module" src="/assets/app-1.js"></script></body></html>`;
writeFileSync(join(dist, "index.html"), SHELL);
writeFileSync(join(dist, "assets", "app-1.js"), "console.log('the app')");
writeFileSync(join(dist, "p2p-probe.html"), '<!doctype html><script src="/p2p-probe.js"></script>');
writeFileSync(join(dist, "p2p-probe.js"), "probe()");
const files = filesOf(dist);
const manifest = { format: FORMAT, commit: "0123abc", ref: "refs/tags/v9.9.9", distribution: "public", digest: digestOf(files), files };
const manifestFile = join(root, "web-manifest.json");
writeFileSync(manifestFile, JSON.stringify(manifest));

/**
 * A hub that serves `dist` the way rh-hub does. `marker`: the origin written
 * into the shell (`self` for the site's own); `csp(path)`: the policy header.
 */
function fakeHub({ marker = "self", csp = () => HUB_CSP } = {}) {
  const server = createServer((req, res) => {
    const path = new URL(req.url, "http://x").pathname.slice(1);
    if (path === ".well-known/droppen-build") {
      res.setHeader("content-type", "application/json");
      return res.end(JSON.stringify({ commit: "0123abc", web: { digest: manifest.digest, files: files.length } }));
    }
    const file = files.find((f) => f.path === path && path !== "index.html");
    res.setHeader("content-security-policy", csp(`/${path}`));
    if (file) return res.end(readFileSync(join(dist, file.path)));
    if (path.startsWith("assets/")) {
      res.statusCode = 404;
      return res.end("not found");
    }
    const origin = marker === "self" ? `ws://${req.headers.host}` : marker;
    res.end(SHELL.replace("<head>", `<head><meta name="rh-hub" content="${origin}" />`));
  });
  return listen(server);
}

/** The Sigstore bundle the signer repository s/r attached to its release v9.9.9. */
const SIGNER_BUNDLE = '{"bundle":"from s/r"}';

/**
 * A GitHub with one release in o/r whose manifest is ours and no Sigstore
 * bundle, and the signer repository s/r's release of the same tag, which has
 * the bundle.
 */
function fakeGitHub() {
  const server = createServer((req, res) => {
    if (req.url.startsWith("/repos/o/r/releases")) {
      res.setHeader("content-type", "application/json");
      return res.end(JSON.stringify([{ tag_name: "v9.9.9", draft: false, assets: [{ name: "web-manifest.json" }] }]));
    }
    if (req.url === "/o/r/releases/download/v9.9.9/web-manifest.json") return res.end(JSON.stringify(manifest));
    if (req.url === "/s/r/releases/download/v9.9.9/web-manifest.sigstore.json") return res.end(SIGNER_BUNDLE);
    res.statusCode = 404;
    res.end("not found");
  });
  return listen(server);
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

  test("releases come from getdroppen/releases unless --repo names another", async () => {
    const site = await hub();
    const env = { DROPPEN_VERIFY_GITHUB_API: github, DROPPEN_VERIFY_GITHUB: github };
    // The fake GitHub only knows o/r: the default repository is asked, and is not there.
    const r = await verify(["--site", site], env);
    assert.equal(r.code, 2, r.out);
    assert.match(r.out, /\/repos\/getdroppen\/releases\/releases\?per_page=30: HTTP 404/);
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
    assert.match(strict.out, /attestation not checked: no Sigstore bundle published with this release \(--allow-unattested accepts that\)/);
    const lax = await verify(["--site", site, "--repo", "o/r", "--allow-unattested"], env);
    assert.equal(lax.code, 0, lax.out);
    assert.match(lax.out, /release v9\.9\.9 of o\/r/);
  });

  test("a release without its own bundle is checked with the signer repository's bundle for the same tag", async () => {
    const site = await hub();
    // A stand-in `gh` that accepts any attestation and records what it was given.
    const bin = mkdtempSync(join(tmpdir(), "verify-web-gh-"));
    try {
      writeFileSync(
        join(bin, "gh"),
        [
          "#!/bin/sh",
          '[ "$1" = --version ] && exit 0',
          'printf "%s\\n" "$@" > "$GH_ARGS"',
          'while [ $# -gt 0 ]; do [ "$1" = --bundle ] && cp "$2" "$GH_BUNDLE"; shift; done',
          "exit 0",
          "",
        ].join("\n"),
        { mode: 0o755 },
      );
      const env = {
        DROPPEN_VERIFY_GITHUB_API: github,
        DROPPEN_VERIFY_GITHUB: github,
        PATH: `${bin}:${process.env.PATH}`,
        GH_ARGS: join(bin, "args"),
        GH_BUNDLE: join(bin, "bundle"),
      };
      const r = await verify(["--site", site, "--repo", "o/r", "--signer-repo", "s/r"], env);
      assert.equal(r.code, 0, r.out);
      assert.match(r.out, /verified: built by s\/r\/\.github\/workflows\/release\.yml at v9\.9\.9/);
      assert.equal(readFileSync(join(bin, "bundle"), "utf8"), SIGNER_BUNDLE);
      const args = readFileSync(join(bin, "args"), "utf8").trim().split("\n");
      for (const [flag, value] of [["--repo", "s/r"], ["--signer-workflow", "s/r/.github/workflows/release.yml"], ["--source-ref", "refs/tags/v9.9.9"]])
        assert.equal(args[args.indexOf(flag) + 1], value, flag);
      // Without a signer repository of its own, nothing else is asked.
      const same = await verify(["--site", site, "--repo", "o/r", "--signer-repo", "o/r"], env);
      assert.equal(same.code, 1, same.out);
      assert.match(same.out, /no Sigstore bundle published with this release/);
    } finally {
      rmSync(bin, { recursive: true, force: true });
    }
  });
});
