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
