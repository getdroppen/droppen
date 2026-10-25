// The hub's hosts (`lib/hub/hosts.ts`, docs/hub-api/README.md "Hosts"): a hub
// may keep the web app on a host of its own (my.droppen.app), its public pages
// on another (www.droppen.app), and its origin (droppen.app) for devices that
// run AI and relays. It says so at `/.well-known/droppen-hub`; the page reads
// it first and runs in one of four modes.
//
// What these pin: the answer is read strictly (origins only, unknown fields
// ignored) and anything else — an older hub's 404 or shell, no network, no
// answer in time — leaves the page on one host exactly as before; each host
// gets its mode; links between the app and the public pages go to the other
// host as ordinary links, and `/` on the app host is the app; the install
// command and the "another hub" note take my. and www. for droppen.app and
// name the hub's origin; `/link` refuses every name of the hub as a device's
// page; the address the page loaded with is kept whole for the move notice.
import assert from "node:assert/strict";
import test from "node:test";
import { bootPath } from "../src/lib/hub/boot-url.js";
import { agentHub, appHost, appUrl, fetchHubHosts, hostMode, hostModeFor, HUB_HOSTS_PATH, moveOrigins, parseHubHosts, resolveHostMode, setHostMode, siteOrigin } from "../src/lib/hub/hosts.js";
import { codeInstallCommand, otherHub } from "../src/features/landing/get-droppen.js";
import { bootRoute, isFrontPath, onPublicLink, publicHref, publicLink, publicPage } from "../src/features/landing/route.js";
import { callbackTier } from "../src/features/link/handoff.js";

const SPLIT = { origin: "wss://droppen.app", app: "https://my.droppen.app", site: "https://www.droppen.app", apex_mode: "move" };
/** The canonical shape: the machine hub on a name of its own, the pages siblings under the same parent. */
const CANONICAL = { origin: "wss://api.droppen.app", app: "https://my.droppen.app", site: "https://www.droppen.app", apex_mode: "redirect" };
const MY = "https://my.droppen.app";
const hosts = parseHubHosts(SPLIT, MY)!;

const json = (body: unknown, init: { status?: number; type?: string } = {}) =>
  new Response(typeof body === "string" ? body : JSON.stringify(body), { status: init.status ?? 200, headers: { "content-type": init.type ?? "application/json" } });

test("the hub's answer is read strictly: origins only, its mode or `app`, unknown fields ignored", () => {
  assert.deepEqual(parseHubHosts(SPLIT, MY), { origin: "wss://droppen.app", app: "https://my.droppen.app", site: "https://www.droppen.app", apexMode: "move" });
  assert.deepEqual(parseHubHosts({ origin: "wss://hub.example", app: null, site: null, later: { x: 1 } }, "https://hub.example"), { origin: "wss://hub.example", app: null, site: null, apexMode: "app" });
  assert.equal(parseHubHosts({ ...SPLIT, apex_mode: "whatever" }, MY)?.apexMode, "app", "a mode it does not know: the app stays where it was");
  assert.equal(parseHubHosts({ ...SPLIT, apex_mode: "redirect" }, MY)?.apexMode, "redirect");
  assert.equal(parseHubHosts({ origin: "ws://127.0.0.1:8787", app: "http://127.0.0.1:8788" }, "http://127.0.0.1:8788")?.app, "http://127.0.0.1:8788");
  assert.equal(parseHubHosts({ ...SPLIT, app: "https://My.Droppen.app" }, MY)?.app, "https://my.droppen.app", "a host name in any case, as the URL parser writes it");
  // The canonical machine origin (`api.`) sits under its parent (`droppen.app`),
  // and the pages must be under that same root.
  assert.deepEqual(parseHubHosts(CANONICAL, MY), { origin: "wss://api.droppen.app", app: "https://my.droppen.app", site: "https://www.droppen.app", apexMode: "redirect" });
  assert.ok(parseHubHosts(CANONICAL, "https://www.droppen.app"), "the site host, a sibling of the API host");
  assert.ok(parseHubHosts(CANONICAL, "https://droppen.app"), "the apex, above the API host and the pages");
  assert.equal(parseHubHosts({ ...CANONICAL, app: "https://my.droppen.app.evil.com" }, MY), null, "still no lookalike");
  assert.equal(parseHubHosts({ ...CANONICAL, site: "https://www.evil.example" }, MY), null, "still no other domain");
  assert.equal(parseHubHosts(CANONICAL, "https://other.example"), null, "a page sharing no name with the hub");
  // A two-letter TLD's names are registrable domains themselves: `co.uk` is
  // never a root, and a hub under one may name nothing of another.
  assert.equal(parseHubHosts({ origin: "wss://api.evil.co.uk", app: "https://my.victim.co.uk", site: null }, "https://my.victim.co.uk"), null, "co.uk is no root");
  assert.equal(parseHubHosts({ origin: "wss://evil.co.uk", app: "https://victim.co.uk", site: null }, "https://victim.co.uk"), null, "another registrable name");
  assert.equal(parseHubHosts({ origin: "wss://api.evil.co.uk", app: "https://my.victim.co.uk", site: null }, "https://my.evil.co.uk"), null, "nor an app on another registrable name");
  assert.equal(parseHubHosts({ origin: "wss://api.evil.co.uk", app: "https://my.evil.co.uk", site: null }, "https://my.evil.co.uk"), null, "a two-letter TLD's name is its own root, not a parent");
  // A longer TLD's parent is the root, as the clients lane reads it.
  const deeper = { origin: "wss://api.droppen.example", app: "https://my.droppen.example", site: "https://www.droppen.example" };
  assert.equal(parseHubHosts(deeper, "https://my.droppen.example")?.app, "https://my.droppen.example");
  for (const bad of [
    null,
    "wss://droppen.app",
    {},
    { ...SPLIT, origin: "https://droppen.app" },
    { ...SPLIT, origin: "wss://droppen.app/" },
    { ...SPLIT, app: "https://my.droppen.app/" },
    { ...SPLIT, app: "https://my.droppen.app/sessions" },
    { ...SPLIT, app: "javascript:alert(1)" },
    { ...SPLIT, site: "wss://www.droppen.app" },
    { ...SPLIT, app: 42 },
  ]) {
    assert.equal(parseHubHosts(bad, MY), null, JSON.stringify(bad));
  }
});

test("the hub's names are pinned to the apex the page came from: `https://` names under it at a label boundary, no port, no address; else one host as before", () => {
  const with_ = (over: Record<string, unknown>, page = MY) => parseHubHosts({ ...SPLIT, ...over }, page);
  // Accepted: names under the apex, the apex itself as the hub's own, on any of its hosts.
  assert.ok(with_({}, "https://droppen.app") && with_({}, "https://www.droppen.app") && with_({}, MY));
  assert.equal(with_({ app: "https://a.b.droppen.app" })?.app, "https://a.b.droppen.app");
  assert.equal(with_({ app: "https://droppen.app", site: null })?.app, "https://droppen.app");
  for (const [what, over] of [
    ["a single-label host", { app: "https://app", site: null }],
    ["a lookalike suffix", { app: "https://my.droppen.app.evil.com" }],
    ["a lookalike prefix", { app: "https://evildroppen.app" }],
    ["another domain", { site: "https://www.evil.example" }],
    ["plain http", { app: "http://my.droppen.app" }],
    ["plain http for the site", { site: "http://www.droppen.app" }],
    ["a port", { app: "https://my.droppen.app:8443" }],
    ["a default port written out", { app: "https://my.droppen.app:443" }],
    ["user info", { app: "https://user@my.droppen.app" }],
    ["an address", { app: "https://203.0.113.7" }],
    ["an IPv6 address", { app: "https://[2001:db8::1]" }],
    ["a trailing dot", { app: "https://my.droppen.app." }],
    ["a loopback app for a public hub", { app: "http://127.0.0.1:8788" }],
    ["an origin with a port", { origin: "wss://droppen.app:8443" }],
    ["an origin on plain ws", { origin: "ws://droppen.app" }],
    ["an origin that is an address", { origin: "wss://203.0.113.7", app: null, site: null }],
    ["a top-level origin", { origin: "wss://app", app: "https://evil.app", site: null }],
  ] as const) {
    assert.equal(with_(over), null, what);
  }
  // A three-label origin sits under its parent: names under that parent are its own.
  assert.equal(parseHubHosts({ origin: "wss://hub.droppen.app", app: "https://droppen.app" }, "https://my.hub.droppen.app")?.app, "https://droppen.app");
  assert.equal(parseHubHosts({ origin: "wss://hub.droppen.app", app: "https://my.hub.droppen.app" }, "https://my.hub.droppen.app")?.app, "https://my.hub.droppen.app");
  assert.equal(parseHubHosts({ origin: "wss://hub.droppen.app", app: "https://my.evil.co.uk" }, "https://my.hub.droppen.app"), null, "but not another registrable name");
  // The page must be on the apex or under it: a hub answering for another name is not this page's.
  assert.equal(with_({}, "https://evil.example"), null);
  assert.equal(with_({}, "https://evildroppen.app"), null);
  assert.equal(with_({}, "http://my.droppen.app"), null, "a hub's page over plain http");
  assert.equal(parseHubHosts({ origin: "wss://droppen.app", app: "https://my.droppen.app" }, "not a url"), null);
  // A test hub on this computer's loopback: plain http and ports, loopback names only, and only for a loopback page.
  assert.equal(parseHubHosts({ origin: "ws://127.0.0.1:8787", app: "http://localhost:8788" }, "http://127.0.0.1:8788")?.app, "http://localhost:8788");
  assert.equal(parseHubHosts({ origin: "ws://127.0.0.1:8787", app: "https://my.droppen.app" }, "http://127.0.0.1:8788"), null);
  assert.equal(parseHubHosts({ origin: "ws://127.0.0.1:8787", app: "http://127.0.0.1:8788" }, MY), null);
});

test("each host gets its mode; a hub with no app host, or its origin still serving the app, is one host as before", () => {
  assert.equal(hostModeFor(null, "https://droppen.app"), "single");
  assert.equal(hostModeFor(hosts, "https://my.droppen.app"), "app");
  assert.equal(hostModeFor(hosts, "https://www.droppen.app"), "site");
  assert.equal(hostModeFor(hosts, "https://droppen.app"), "legacy");
  // The canonical shape: the pages keep their modes, and the apex is still
  // the hub's name (what `apex_mode` decides), not the API host's.
  const api = parseHubHosts(CANONICAL, MY)!;
  assert.equal(hostModeFor(api, "https://my.droppen.app"), "app");
  assert.equal(hostModeFor(api, "https://www.droppen.app"), "site");
  assert.equal(hostModeFor(api, "https://droppen.app"), "legacy", "apex_mode redirect");
  assert.equal(hostModeFor({ ...api, apexMode: "app" }, "https://droppen.app"), "single");
  assert.equal(hostModeFor(api, "https://api.droppen.app"), "single", "the API host serves no pages of its own");
  assert.equal(hostModeFor({ ...hosts, apexMode: "redirect" }, "https://droppen.app"), "legacy");
  assert.equal(hostModeFor({ ...hosts, apexMode: "app" }, "https://droppen.app"), "single", "the dark launch: the origin still serves the app");
  assert.equal(hostModeFor({ ...hosts, apexMode: "app" }, "https://my.droppen.app"), "app");
  // The shape a hub answers without `apex_mode` (docs/hub-api/http.md): the origin keeps the app until it says `move`.
  const plain = parseHubHosts({ origin: "wss://droppen.app", app: "https://my.droppen.app", site: "https://www.droppen.app" }, "https://droppen.app")!;
  assert.equal(plain.apexMode, "app");
  assert.equal(hostModeFor(plain, "https://droppen.app"), "single");
  assert.equal(hostModeFor(plain, "https://www.droppen.app"), "site");
  assert.equal(hostModeFor(parseHubHosts({ origin: "wss://droppen.app", app: "https://droppen.app" }, "https://droppen.app"), "https://droppen.app"), "single", "`app` on the hub's own host");
  assert.equal(hostModeFor(hosts, "https://relay-sg.droppen.app"), "single", "a name it did not list");
  const one = parseHubHosts({ origin: "wss://hub.example", app: null, site: null }, "https://hub.example")!;
  assert.equal(hostModeFor(one, "https://hub.example"), "single", "a self-hosted hub on one host");
  assert.equal(hostModeFor({ ...one, app: "https://hub.example" }, "https://hub.example"), "single", "an app host that is the origin");
  assert.equal(hostModeFor({ ...one, site: "https://www.hub.example" }, "https://www.hub.example"), "site", "a site host alone");
  setHostMode({ ...one, site: "https://www.hub.example" }, "site");
  assert.equal(appUrl("/signin"), "https://hub.example/signin", "its app is on the hub's own host");
  setHostMode(null);
});

test("anything but the hub's answer leaves the page as before: an older hub's 404 or shell, no network, no answer in time", async () => {
  const asked: { url: string; init?: RequestInit }[] = [];
  const answer = (res: Response | Error | "never") => (async (url: string | URL | Request, init?: RequestInit) => {
    asked.push({ url: String(url), init });
    if (res === "never") return await new Promise<Response>((_, reject) => init?.signal?.addEventListener("abort", () => reject(new Error("aborted"))));
    if (res instanceof Error) throw res;
    return res;
  }) as typeof fetch;
  assert.deepEqual(await fetchHubHosts(answer(json(SPLIT)), undefined, MY), hosts);
  assert.equal(asked[0]!.url, HUB_HOSTS_PATH, "the page's own host, nothing else");
  assert.equal(asked[0]!.init?.credentials, "omit");
  assert.equal(await fetchHubHosts(answer(json({ error: "not_found" }, { status: 404 }))), null);
  assert.equal(await fetchHubHosts(answer(json("<!doctype html><html></html>", { type: "text/html; charset=utf-8" }))), null, "an older hub's shell for an unknown path");
  assert.equal(await fetchHubHosts(answer(json("{", {}))), null);
  assert.equal(await fetchHubHosts(answer(json({ origin: "nope" }))), null);
  assert.equal(await fetchHubHosts(answer(new TypeError("Failed to fetch"))), null);
  assert.equal(await fetchHubHosts(answer("never"), 20), null, "no answer within the wait");
});

test("a page no hub serves asks nothing; a hub's page asks once and settles its mode", async () => {
  const g = globalThis as { document?: unknown; location?: unknown };
  let asked = 0;
  const fetchFn = (async () => (asked++, json(SPLIT))) as typeof fetch;
  g.document = undefined;
  assert.equal(await resolveHostMode(fetchFn), "single");
  assert.equal(asked, 0, "local mode: nothing to ask");
  g.document = { querySelector: () => ({ getAttribute: () => "wss://my.droppen.app" }) };
  g.location = { origin: "https://my.droppen.app", host: "my.droppen.app" };
  assert.equal(await resolveHostMode(fetchFn), "app");
  assert.equal(asked, 1);
  assert.equal(hostMode(), "app");
  g.location = { origin: "https://www.droppen.app", host: "www.droppen.app" };
  assert.equal(await resolveHostMode(fetchFn), "site");
  assert.equal(await resolveHostMode((async () => json({}, { status: 404 })) as typeof fetch), "single");
  g.document = undefined;
  g.location = undefined;
  setHostMode(null);
});

test("links: from the site host every page but the public ones is the app host's; from the app host the public pages are the site's", () => {
  setHostMode(null);
  for (const path of ["/", "/download?os=linux", "/signin", "/sessions", "/pricing#waitlist"]) assert.equal(publicHref(path), path, `one host: ${path}`);
  assert.equal(publicPage("/"), "landing");

  setHostMode(hosts, "site");
  assert.equal(publicHref("/"), "/");
  assert.equal(publicHref("/download?os=linux"), "/download?os=linux");
  assert.equal(publicHref("/pricing#waitlist"), "/pricing#waitlist");
  assert.equal(publicHref("/signin"), "https://my.droppen.app/signin");
  assert.equal(publicHref("/signup"), "https://my.droppen.app/signup");
  assert.equal(publicHref("/sessions"), "https://my.droppen.app/sessions");
  assert.equal(publicHref("/devices"), "https://my.droppen.app/devices");
  assert.equal(appUrl("/verify#t=abc"), "https://my.droppen.app/verify#t=abc");
  assert.equal(siteOrigin(), null);

  setHostMode(hosts, "app");
  assert.equal(publicPage("/"), "signin", "`/` on the app host is the app: signed out, the sign-in form");
  assert.equal(isFrontPath("/"), false);
  assert.deepEqual(bootRoute("/", true), { show: "app" }, "signed in, the app's start (its router goes on to the sessions)");
  assert.deepEqual(bootRoute("/", false), { show: "auth" });
  assert.equal(publicHref("/"), "https://www.droppen.app/", "a link to the landing page");
  assert.equal(publicHref("/download?os=macos-app"), "https://www.droppen.app/download?os=macos-app");
  assert.equal(publicHref("/security"), "https://www.droppen.app/security");
  assert.equal(publicHref("/signin"), "/signin");
  assert.equal(publicHref("/sessions"), "/sessions");
  assert.equal(appUrl("/sessions"), "/sessions", "the app is here");

  setHostMode(hosts, "legacy");
  assert.equal(publicHref("/"), "https://my.droppen.app/");
  assert.equal(publicHref("/sessions"), "https://my.droppen.app/sessions");

  // A link to another host is an ordinary link: the click is the browser's.
  setHostMode(hosts, "site");
  const click = (path: string) => {
    let prevented = false;
    onPublicLink(path)({ button: 0, metaKey: false, ctrlKey: false, shiftKey: false, altKey: false, preventDefault: () => void (prevented = true) });
    return prevented;
  };
  assert.equal(click("/signin"), false);
  assert.deepEqual(Object.keys(publicLink("/signin")), ["href", "onClick"]);
  setHostMode(null);
});

test("the app's host name for the page's words, and the hub address a device that runs AI is given", () => {
  setHostMode(null);
  assert.equal(appHost(), "droppen.app", "no hub answer and no page: droppen.app");
  assert.equal(agentHub("wss://hub.example"), "https://hub.example");
  setHostMode(hosts, "site");
  assert.equal(appHost(), "my.droppen.app");
  assert.equal(agentHub("wss://my.droppen.app"), "https://droppen.app", "the hub's origin, not the app host the page is on");
  setHostMode(parseHubHosts({ origin: "wss://hub.example" }, "https://hub.example"), "single");
  assert.equal(appHost(), "hub.example");
  setHostMode(null);
  // droppen.app's other names are droppen.app: no DROPPEN_HUB in the command.
  for (const name of ["https://droppen.app", "https://api.droppen.app", "https://my.droppen.app", "https://www.droppen.app", "https://my.droppen.app/"]) assert.equal(otherHub(name), false, name);
  assert.equal(codeInstallCommand("RH1-X", "https://api.droppen.app"), codeInstallCommand("RH1-X"), "the machine hub needs no DROPPEN_HUB");
  for (const name of ["https://hub.example", "http://127.0.0.1:61135", "https://evil.droppen.app.example", "https://relay.droppen.app"]) assert.equal(otherHub(name), true, name);
  assert.equal(codeInstallCommand("RH1-X", "https://my.droppen.app"), codeInstallCommand("RH1-X"));
});

test("`/link` refuses every name of the hub as a device's page: its origin, the app and site hosts, anything under its origin", () => {
  const app = "https://my.droppen.app";
  for (const cb of ["https://droppen.app", "https://my.droppen.app", "https://www.droppen.app", "http://www.droppen.app:8080", "https://verify.droppen.app", "https://a.b.droppen.app"]) {
    assert.equal(callbackTier(cb, app, hosts), "refuse", cb);
  }
  assert.equal(callbackTier("https://droppen.app.example", app, hosts), "public", "not under the hub's name");
  assert.equal(callbackTier("https://evildroppen.app", app, hosts), "public");
  assert.equal(callbackTier("http://127.0.0.1:8700", app, hosts), "local");
  assert.equal(callbackTier("http://192.168.1.5:8700", app, hosts), "warn");
  // Without the hub's answer: only the page's own host, as before.
  assert.equal(callbackTier("https://www.droppen.app", "https://droppen.app"), "public");
  assert.equal(callbackTier("https://droppen.app", "https://droppen.app"), "refuse");
  // A test hub on loopback shares it with the device: still "local", unless it is the page's own origin.
  const local = parseHubHosts({ origin: "ws://127.0.0.1:8787", app: "http://127.0.0.1:8788" }, "http://127.0.0.1:8788")!;
  assert.equal(callbackTier("http://127.0.0.1:8700", "http://127.0.0.1:8788", local), "local");
  assert.equal(callbackTier("http://127.0.0.1:8788", "http://127.0.0.1:8788", local), "refuse");
});

test("the move's two ends exist whenever the hub names an app host of its own, whatever the origin does with the app's pages", () => {
  for (const apexMode of ["app", "move", "redirect"] as const) {
    setHostMode({ ...hosts, apexMode }, "single");
    assert.deepEqual(moveOrigins("https://droppen.app"), { legacy: "https://droppen.app", app: MY }, apexMode);
    assert.deepEqual(moveOrigins(MY), { legacy: "https://droppen.app", app: MY }, apexMode);
    assert.equal(moveOrigins("https://www.droppen.app"), null, `${apexMode}: not on the site host`);
  }
  setHostMode({ ...hosts, app: null }, "single");
  assert.equal(moveOrigins("https://droppen.app"), null, "no app host");
  setHostMode({ ...hosts, app: "https://droppen.app" }, "single");
  assert.equal(moveOrigins("https://droppen.app"), null, "the app on the hub's own host");
  setHostMode(null);
  assert.equal(moveOrigins("https://droppen.app"), null, "no answer");
});

test("the address the page loaded with is passed on whole: path, query and fragment", () => {
  assert.equal(bootPath({ pathname: "/link", search: "", hash: "#droppen-link=eyJ9" }), "/link#droppen-link=eyJ9");
  assert.equal(bootPath({ pathname: "/verify", search: "?x=1", hash: "#t=abc" }), "/verify?x=1#t=abc");
  assert.equal(bootPath(null), "/");
});

