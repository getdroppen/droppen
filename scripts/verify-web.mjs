#!/usr/bin/env node
// Check that a site serves the attested open-source web build, byte for byte
// (docs/web-integrity.md). Node >= 20, no dependencies, no credentials.
//
//   node scripts/verify-web.mjs                        droppen.app, against the release
//                                                      whose manifest has the digest
//                                                      the site reports
//   node scripts/verify-web.mjs --site URL             another hub (or a local one)
//   node scripts/verify-web.mjs --release v0.2.109     against that release's manifest
//   node scripts/verify-web.mjs --manifest FILE|URL    against a manifest you built
//                                                      yourself (scripts/web-manifest.mjs)
//
// Options:
//   --repo OWNER/NAME        where releases come from (default getdroppen/droppen)
//   --signer-repo OWNER/NAME whose .github/workflows/release.yml built and attested
//                            the manifest (default getdroppen/droppen, the source)
//   --bundle FILE            the Sigstore bundle for --manifest FILE
//   --hub-origin ORIGIN      another hub origin the page may be pointed at
//                            (default: only the site's own, wss://<its host>)
//   --require-attestation    fail unless `gh attestation verify` checked the manifest
//                            (the default for a release manifest, or with --bundle)
//   --allow-unattested       accept a release manifest whose attestation was not
//                            checked (no gh, or no bundle published)
//   --releases N             how many recent releases to search (default 30)
//
// What it does:
//   1. asks the site which build it serves (`/.well-known/droppen-build`);
//   2. takes the release manifest with that digest (or the one named), and
//      checks its attestation with `gh attestation verify --bundle` — that
//      needs `gh` installed but no GitHub login. A release without a bundle
//      of its own (getdroppen/releases, up to v0.2.126: its manifest was
//      staged by the release script, the same files but not the bytes CI
//      signed) is checked through the signer repository's release of the same
//      tag: its manifest, which must have the same digest, and that
//      manifest's bundle;
//   3. fetches every file the manifest lists, and the shell at a few routes,
//      and compares sha256s. The shell is compared after removing the one
//      thing the hub adds, `<meta name="rh-hub" content="<origin>" />` right
//      after `<head>`, whose origin — where the page signs in and sends all
//      hub traffic — must be the site's own (or a --hub-origin). Every page's
//      Content-Security-Policy may let scripts (elements and attributes) run
//      only from the site itself and the pages' own inline scripts.
//
// Exit status: 0 pass, 1 fail, 2 could not check.
//
// For tests only: DROPPEN_VERIFY_GITHUB_API and DROPPEN_VERIFY_GITHUB replace
// https://api.github.com and https://github.com.
//
// It proves what the site served to this script just now. It cannot prove what
// the site serves anyone else, or later; see docs/web-integrity.md.

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const FORMAT = "droppen-web-manifest/1";
const BUILD_INFO = "/.well-known/droppen-build";
const BUNDLE_ASSET = "web-manifest.sigstore.json";
/** Client-side routes the hub answers with the shell; each must be it too. */
const SHELL_ROUTES = ["/", "/link", "/signin"];
// Nothing about whoever runs this goes out: no cookies, no tokens, this header.
const HEADERS = { "user-agent": "droppen-verify-web/1" };
const GITHUB_API = process.env.DROPPEN_VERIFY_GITHUB_API || "https://api.github.com";
const GITHUB = process.env.DROPPEN_VERIFY_GITHUB || "https://github.com";

const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");
const bytewise = (a, b) => Buffer.compare(Buffer.from(a.path), Buffer.from(b.path));
const digestOf = (files) => `sha256:${sha256([...files].sort(bytewise).map((f) => `${f.sha256}  ${f.path}\n`).join(""))}`;

class Unchecked extends Error {}

function parseArgs(argv) {
  const opts = {
    site: "https://droppen.app",
    // The release files live in the public binaries repository; the workflow
    // that builds and attests them runs in the source repository.
    repo: "getdroppen/droppen",
    signerRepo: "getdroppen/droppen",
    releases: 30,
    requireAttestation: false,
    allowUnattested: false,
    hubOrigins: [],
  };
  const valued = {
    "--site": "site",
    "--repo": "repo",
    "--signer-repo": "signerRepo",
    "--release": "release",
    "--manifest": "manifest",
    "--bundle": "bundle",
    "--releases": "releases",
  };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a in valued) {
      if (argv[i + 1] === undefined) throw new Unchecked(`${a} needs a value`);
      opts[valued[a]] = argv[++i];
    } else if (a === "--hub-origin") {
      if (argv[i + 1] === undefined) throw new Unchecked(`${a} needs a value`);
      opts.hubOrigins.push(argv[++i].toLowerCase());
    } else if (a === "--require-attestation") opts.requireAttestation = true;
    else if (a === "--allow-unattested") opts.allowUnattested = true;
    else if (a === "-h" || a === "--help") opts.help = true;
    else throw new Unchecked(`unknown argument ${a} (see --help)`);
  }
  opts.site = opts.site.replace(/\/+$/, "");
  if (!/^https?:\/\/[^/]+$/.test(opts.site)) throw new Unchecked(`--site must be an origin like https://droppen.app, not ${opts.site}`);
  if (!/^[\w.-]+\/[\w.-]+$/.test(opts.repo)) throw new Unchecked(`--repo must be OWNER/NAME`);
  if (!/^[\w.-]+\/[\w.-]+$/.test(opts.signerRepo)) throw new Unchecked(`--signer-repo must be OWNER/NAME`);
  if (opts.release && opts.manifest) throw new Unchecked("give --release or --manifest, not both");
  if (opts.bundle && !opts.manifest) throw new Unchecked("--bundle goes with --manifest");
  if (opts.requireAttestation && opts.allowUnattested) throw new Unchecked("give --require-attestation or --allow-unattested, not both");
  for (const o of opts.hubOrigins) if (!/^wss?:\/\/[^/]+$/.test(o)) throw new Unchecked(`--hub-origin must be like wss://hub.example, not ${o}`);
  opts.releases = Number(opts.releases);
  if (!Number.isInteger(opts.releases) || opts.releases < 1 || opts.releases > 100) throw new Unchecked("--releases must be 1 to 100");
  return opts;
}

async function get(url, { json = false, optional = false } = {}) {
  let res;
  try {
    // Never follow a redirect: what is checked is what this origin serves.
    res = await fetch(url, { headers: { ...HEADERS, "cache-control": "no-cache" }, redirect: "manual" });
  } catch (error) {
    throw new Unchecked(`${url}: ${error.cause?.message ?? error.message}`);
  }
  if (optional && res.status === 404) return null;
  if (res.status !== 200) {
    const where = res.headers.get("location");
    throw Object.assign(new Error(`${url}: HTTP ${res.status}${where ? ` → ${where}` : ""}`), { status: res.status });
  }
  const body = Buffer.from(await res.arrayBuffer());
  return json ? JSON.parse(body.toString("utf8")) : { body, headers: res.headers };
}

/** GitHub release downloads redirect to a CDN; those are followed. */
async function download(url) {
  const res = await fetch(url, { headers: HEADERS, redirect: "follow" });
  if (res.status === 404) return null;
  if (!res.ok) throw new Unchecked(`${url}: HTTP ${res.status}`);
  return Buffer.from(await res.arrayBuffer());
}

function checkManifest(manifest, where) {
  const fail = (why) => {
    throw new Unchecked(`${where}: ${why}`);
  };
  if (manifest?.format !== FORMAT) fail(`not a ${FORMAT} manifest`);
  if (!Array.isArray(manifest.files) || !manifest.files.length) fail("no files");
  for (const f of manifest.files) {
    if (typeof f.path !== "string" || !/^[A-Za-z0-9._~-]+(\/[A-Za-z0-9._~-]+)*$/.test(f.path) || f.path.split("/").some((s) => s === "." || s === ".."))
      fail(`bad path ${JSON.stringify(f.path)}`);
    if (typeof f.sha256 !== "string" || !/^[0-9a-f]{64}$/.test(f.sha256)) fail(`bad sha256 for ${f.path}`);
  }
  if (!manifest.files.some((f) => f.path === "index.html")) fail("no index.html");
  if (digestOf(manifest.files) !== manifest.digest) fail("its digest does not match its own file list");
  return manifest;
}

/** The release manifest to compare with: by tag, or the newest with the site's digest. */
async function releaseManifest(opts, digest) {
  const base = `${GITHUB}/${opts.repo}/releases/download`;
  const fetchOne = async (tag) => {
    const bytes = await download(`${base}/${encodeURIComponent(tag)}/web-manifest.json`);
    return bytes && { tag, bytes, manifest: JSON.parse(bytes.toString("utf8")) };
  };
  if (opts.release) {
    const found = await fetchOne(opts.release);
    if (!found) throw new Unchecked(`release ${opts.release} of ${opts.repo} has no web-manifest.json`);
    return found;
  }
  if (!digest) throw new Unchecked(`the site does not say which build it serves; name one with --release or --manifest`);
  const api = `${GITHUB_API}/repos/${opts.repo}/releases?per_page=${opts.releases}`;
  const res = await fetch(api, { headers: { ...HEADERS, accept: "application/vnd.github+json" } });
  if (!res.ok) throw new Unchecked(`${api}: HTTP ${res.status} (is ${opts.repo} public? otherwise name a manifest with --manifest)`);
  for (const release of await res.json()) {
    if (release.draft || !release.assets?.some((a) => a.name === "web-manifest.json")) continue;
    const found = await fetchOne(release.tag_name);
    if (found?.manifest?.digest === digest) return found;
  }
  return null;
}

/**
 * The signed copy of a release's manifest, for a release that carries no
 * Sigstore bundle of its own. The files of getdroppen/releases (where releases
 * were published up to v0.2.126) were staged by scripts/release-public.sh on
 * the release Mac, whose manifest lists the same files as the one
 * release.yml builds and attests in the signer repository, but is not the
 * same bytes (CI's also names the repository and the build time); a bundle
 * signs bytes. So both are taken from the signer repository's release of the
 * same tag, the manifest and its bundle, and the manifest must have the
 * release's digest: the site is then compared with the signed manifest.
 *
 * `{manifestBytes, bundleBytes}`; `{failure}` when the signer repository
 * signs another build under that tag (or a manifest that is not one);
 * `{missing}` when it has no signed copy to give (none published, or its
 * release could not be read), which is a missing attestation, as when the
 * release has no bundle.
 */
async function signedCopy(opts, tag, digest) {
  const at = (asset) => `${GITHUB}/${opts.signerRepo}/releases/download/${encodeURIComponent(tag)}/${asset}`;
  let manifestBytes, bundleBytes;
  try {
    [manifestBytes, bundleBytes] = await Promise.all([download(at("web-manifest.json")), download(at(BUNDLE_ASSET))]);
  } catch (error) {
    return { missing: `no Sigstore bundle published with this release, and ${opts.signerRepo}'s release ${tag} could not be read (${error.message})` };
  }
  if (!manifestBytes || !bundleBytes)
    return { missing: `no Sigstore bundle published with this release, nor a signed manifest with ${opts.signerRepo}'s release ${tag}` };
  const where = `the signed manifest of ${opts.signerRepo}'s release ${tag}`;
  let signed;
  try {
    signed = checkManifest(JSON.parse(manifestBytes.toString("utf8")), where);
  } catch (error) {
    return { failure: error instanceof Unchecked ? error.message : `${where}: ${error.message}` };
  }
  if (signed.digest !== digest) return { failure: `${where} is ${signed.digest}, not the release's ${digest}` };
  return { manifestBytes, bundleBytes };
}

/** `gh attestation verify` against the bundle; `{ok, note}`. */
function attest(opts, manifestBytes, bundleBytes, tag) {
  if (!bundleBytes)
    return { ok: false, note: opts.manifest ? "not checked: add --bundle to check this manifest's attestation" : "not checked: no Sigstore bundle published with this release" };
  try {
    execFileSync("gh", ["--version"], { stdio: "ignore" });
  } catch {
    return { ok: false, note: "not checked: install the GitHub CLI (gh) to verify the attestation" };
  }
  const dir = mkdtempSync(join(tmpdir(), "droppen-verify-"));
  try {
    writeFileSync(join(dir, "web-manifest.json"), manifestBytes);
    writeFileSync(join(dir, BUNDLE_ASSET), bundleBytes);
    // The attestation names the repository whose workflow signed it, which is
    // the source repository, not the one the files were downloaded from.
    const args = ["attestation", "verify", join(dir, "web-manifest.json"), "--bundle", join(dir, BUNDLE_ASSET), "--repo", opts.signerRepo,
      "--signer-workflow", `${opts.signerRepo}/.github/workflows/release.yml`];
    if (tag) args.push("--source-ref", `refs/tags/${tag}`);
    execFileSync("gh", args, { stdio: ["ignore", "pipe", "pipe"] });
    return { ok: true, note: `verified: built by ${opts.signerRepo}/.github/workflows/release.yml${tag ? ` at ${tag}` : ""} (gh attestation verify)` };
  } catch (error) {
    const said = `${error.stderr ?? ""}${error.stdout ?? ""}`.trim().split("\n").slice(-3).join(" / ");
    return { ok: false, failed: true, note: `FAILED: ${said || error.message}` };
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

/**
 * The shell as built: the served bytes without the hub's marker. Only the
 * exact tag the hub writes, in the one place it writes it, with an origin for
 * a value — anything else stays and fails the comparison.
 */
export function unmark(html) {
  const text = html.toString("latin1");
  const m = /^([\s\S]*?<head>)<meta name="rh-hub" content="(wss?:\/\/[A-Za-z0-9.:[\]-]+)" \/>/.exec(text);
  if (!m || m[1].indexOf("<head>") !== m[1].length - "<head>".length) return { body: html, origin: null };
  return { body: Buffer.concat([html.subarray(0, m[1].length), html.subarray(m[0].length)]), origin: m[2] };
}

/** The hub origin a site's own pages name: `wss://` for https, `ws://` for http. */
export function siteHubOrigin(site) {
  const url = new URL(site);
  return `${url.protocol === "https:" ? "wss" : "ws"}://${url.host}`.toLowerCase();
}

/**
 * Why the marker's origin is not acceptable, or `null`. The marker decides
 * where the page signs in and sends every hub request, so a byte-identical
 * page pointed at another hub is not the page it claims to be. No marker is
 * fine: that page talks to no hub at all.
 */
export function markerProblem(origin, accepted) {
  if (origin === null || accepted.includes(origin.toLowerCase())) return null;
  return `the page is pointed at the hub ${origin}, not ${accepted.join(" or ")} (pass --hub-origin ${origin} if that is intended)`;
}

/** The shell's inline scripts' CSP hashes, found as the hub finds them. */
function inlineHashes(html) {
  const out = [];
  let rest = html.toString("utf8");
  for (;;) {
    const start = rest.indexOf("<script");
    if (start < 0) break;
    const openEnd = rest.indexOf(">", start);
    if (openEnd < 0) break;
    const close = rest.indexOf("</script>", openEnd + 1);
    if (close < 0) break;
    if (!rest.slice(start, openEnd).includes("src=")) {
      out.push(`'sha256-${createHash("sha256").update(rest.slice(openEnd + 1, close)).digest("base64")}'`);
    }
    rest = rest.slice(close + "</script>".length);
  }
  return out;
}

/**
 * Which directive governs each kind of script, most specific first (CSP 3):
 * `<script>` elements and inline event-handler attributes.
 */
const SCRIPT_DIRECTIVES = {
  "script elements": ["script-src-elem", "script-src", "default-src"],
  "script attributes": ["script-src-attr", "script-src", "default-src"],
};

/**
 * Whether the policy (one header, possibly several comma-separated policies)
 * lets a page run anything but the site's own files and the pages' own inline
 * scripts. Every policy is enforced, so for each kind of script one policy
 * strict enough is enough.
 */
export function scriptPolicy(header, allowedHashes) {
  if (!header) return { ok: true, note: "no Content-Security-Policy header (nothing beyond the files above is checked)" };
  const allowed = (source) => ["'self'", "'none'"].includes(source.toLowerCase()) || allowedHashes.includes(source);
  const restricted = new Set();
  const problems = new Set();
  for (const policy of header.split(",")) {
    // A repeated directive is ignored: the first one counts.
    const directives = new Map();
    for (const [name, ...values] of policy.split(";").map((d) => d.trim().split(/\s+/)).filter((d) => d[0])) {
      if (!directives.has(name.toLowerCase())) directives.set(name.toLowerCase(), values);
    }
    for (const [kind, chain] of Object.entries(SCRIPT_DIRECTIVES)) {
      const name = chain.find((n) => directives.has(n));
      if (!name) {
        problems.add(`a policy leaves ${kind} unrestricted`);
        continue;
      }
      const extra = directives.get(name).filter((source) => !allowed(source));
      if (extra.length) problems.add(`${name} lets ${kind} come from ${extra.join(" ")}`);
      else restricted.add(kind);
    }
  }
  if (Object.keys(SCRIPT_DIRECTIVES).every((kind) => restricted.has(kind)))
    return { ok: true, note: `scripts run only from 'self' and the pages' ${allowedHashes.length} inline script(s)` };
  return { ok: false, note: [...problems].join("; ") };
}

async function pool(items, limit, work) {
  const results = new Array(items.length);
  let next = 0;
  await Promise.all(
    Array.from({ length: Math.min(limit, items.length) }, async () => {
      while (next < items.length) {
        const i = next++;
        results[i] = await work(items[i]);
      }
    }),
  );
  return results;
}

async function main(argv) {
  const opts = parseArgs(argv);
  if (opts.help) {
    console.log(readFileSync(new URL(import.meta.url), "utf8").split("\n").filter((l) => l.startsWith("//")).map((l) => l.slice(3)).join("\n"));
    return 0;
  }
  const failures = [];
  const notes = [];
  console.log(`droppen web integrity check`);
  console.log(`  site:        ${opts.site}`);

  // 1. Which build the site says it serves.
  let info = null;
  try {
    info = await get(`${opts.site}${BUILD_INFO}`, { json: true, optional: true });
  } catch (error) {
    if (error instanceof Unchecked) throw error;
    notes.push(`${BUILD_INFO}: ${error.message}`);
  }
  const served = typeof info?.web?.digest === "string" ? info.web.digest : null;
  console.log(`  serves:      ${served ? `${served} (${info.web.files} files)` : "(not reported)"}${info?.commit ? `, built from ${info.commit}` : ""}`);

  // 2. The attested manifest to compare with.
  let manifestBytes, bundleBytes, source;
  let tag = null;
  let missing = null;
  if (opts.manifest) {
    manifestBytes = /^https?:\/\//.test(opts.manifest) ? await download(opts.manifest) : readFileSync(opts.manifest);
    if (!manifestBytes) throw new Unchecked(`${opts.manifest}: not found`);
    bundleBytes = opts.bundle ? readFileSync(opts.bundle) : null;
    source = opts.manifest;
  } else {
    const found = await releaseManifest(opts, served);
    if (!found) {
      console.log(`  manifest:    none of the last ${opts.releases} releases of ${opts.repo} has ${served}`);
      console.log(
        `\nFAIL: the site serves a build no release attests.${info?.commit ? ` To check it anyway, build ${info.commit} yourself (docs/web-integrity.md) and pass --manifest.` : ""}`,
      );
      return 1;
    }
    ({ tag, bytes: manifestBytes } = found);
    bundleBytes = await download(`${GITHUB}/${opts.repo}/releases/download/${encodeURIComponent(tag)}/${BUNDLE_ASSET}`);
    source = `release ${tag} of ${opts.repo}`;
    // No bundle of its own: the signed copy of its manifest, from the signer repository.
    if (!bundleBytes && opts.signerRepo !== opts.repo) {
      const copy = await signedCopy(opts, tag, found.manifest?.digest);
      if (copy.failure) failures.push(copy.failure);
      else if (copy.missing) missing = copy.missing;
      else {
        ({ manifestBytes, bundleBytes } = copy);
        source = `release ${tag} of ${opts.signerRepo}, the signed manifest of ${source}`;
      }
    }
  }
  const manifest = checkManifest(JSON.parse(manifestBytes.toString("utf8")), source);
  console.log(`  manifest:    ${source}: ${manifest.digest}${manifest.commit ? `, commit ${manifest.commit}` : ""}`);
  if (served && served !== manifest.digest) failures.push(`the site says it serves ${served}, not the manifest's ${manifest.digest}`);
  if (info?.commit && manifest.commit && !manifest.commit.startsWith(info.commit) && !info.commit.startsWith(manifest.commit))
    notes.push(`the site was built from ${info.commit}, the manifest from ${manifest.commit}: the same web app if the digests agree`);

  // A release manifest came anonymously from a public repository, where every
  // release is attested: not checking it is a failure unless asked otherwise.
  const requireAttestation = opts.requireAttestation || (!opts.allowUnattested && (!opts.manifest || Boolean(opts.bundle)));
  const attestation = attest(opts, manifestBytes, bundleBytes, tag);
  if (!bundleBytes && missing) attestation.note = `not checked: ${missing}`;
  console.log(`  attestation: ${attestation.note}`);
  if (attestation.failed) failures.push(`attestation ${attestation.note}`);
  else if (requireAttestation && !attestation.ok)
    failures.push(`attestation ${attestation.note}${opts.manifest ? "" : " (--allow-unattested accepts that)"}`);

  // 3. Every file, as served now.
  const shell = manifest.files.find((f) => f.path === "index.html");
  const checks = [
    ...manifest.files.map((f) => ({ url: `/${f.path.split("/").map(encodeURIComponent).join("/")}`, want: f, isShell: f.path === "index.html" })),
    ...SHELL_ROUTES.map((route) => ({ url: route, want: shell, isShell: true, route: true })),
  ];
  const accepted = [siteHubOrigin(opts.site), ...opts.hubOrigins];
  const policies = [];
  const attestedPages = [];
  const markerFailures = [];
  const origins = new Set();
  const results = await pool(checks, 6, async (c) => {
    let got;
    try {
      got = await get(`${opts.site}${c.url}`);
    } catch (error) {
      if (error instanceof Unchecked) return `unreachable ${c.url}: ${error.message}`;
      // A route a static server does not answer is not a different shell.
      if (c.route && error.status === 404) {
        notes.push(`${c.url}: 404, not checked`);
        return null;
      }
      return `${c.url}: ${error.message}`;
    }
    let body = got.body;
    const page = c.isShell || c.want.path.endsWith(".html");
    if (page) policies.push({ url: c.url, header: got.headers.get("content-security-policy") });
    let wrongHub = null;
    if (c.isShell) {
      const unmarked = unmark(body);
      body = unmarked.body;
      if (unmarked.origin) origins.add(unmarked.origin);
      wrongHub = markerProblem(unmarked.origin, accepted);
    }
    const hash = sha256(body);
    const same = hash === c.want.sha256;
    if (same && page) attestedPages.push(body);
    if (wrongHub) markerFailures.push(`${c.url}: ${wrongHub}`);
    return same ? null : `${c.url}: sha256 ${hash}, attested ${c.want.sha256}${c.isShell ? " (after removing the rh-hub marker)" : ""}`;
  });
  const differing = results.filter(Boolean);
  failures.push(...differing, ...markerFailures.sort());
  console.log(`  files:       ${checks.length - differing.length} of ${checks.length} match (${manifest.files.length} files, ${SHELL_ROUTES.length} shell routes)`);
  if (origins.size) console.log(`  marker:      rh-hub ${[...origins].join(", ")}`);

  // Every page's policy, read against the attested pages' inline scripts
  // (unknown when no page matched, and then the pages have failed already).
  if (attestedPages.length) {
    const hashes = [...new Set(attestedPages.flatMap(inlineHashes))];
    const verdicts = policies.sort((a, b) => a.url.localeCompare(b.url)).map((p) => ({ ...p, ...scriptPolicy(p.header, hashes) }));
    for (const v of verdicts.filter((v) => !v.ok)) failures.push(`${v.url}: Content-Security-Policy: ${v.note}`);
    const good = verdicts.filter((v) => v.ok);
    console.log(`  policy:      ${good.length} of ${verdicts.length} pages${good.length ? `: ${[...new Set(good.map((v) => v.note))].join("; ")}` : ""}`);
  } else console.log(`  policy:      not checked: no served page matched`);

  for (const n of notes) console.log(`  note:        ${n}`);
  if (failures.length) {
    console.log(`\nFAIL: ${opts.site} does not serve the attested build:`);
    for (const f of failures) console.log(`  - ${f}`);
    return 1;
  }
  console.log(
    `\nPASS: every file ${opts.site} served just now is byte-identical to ${source}${attestation.ok ? ", whose attestation checks out" : " (attestation not checked)"}.`,
  );
  return 0;
}

// Run as a program (through a symlink too, e.g. macOS /tmp), not when imported.
if (process.argv[1] && realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url))) {
  main(process.argv.slice(2)).then(
    (code) => (process.exitCode = code),
    (error) => {
      console.error(`\ncould not check: ${error.message}`);
      process.exitCode = 2;
    },
  );
}
