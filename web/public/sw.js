/* Droppen service worker (PWA). Deliberately conservative:
 *
 * - the live surfaces (`/api/**`, which carries the websockets too, and the
 *   `/embed/**` app iframes) NEVER enter the fetch handler — not calling
 *   respondWith leaves the browser's own networking untouched, which is the
 *   only safe thing to do with streaming and upgrade requests;
 * - SPA navigations are network-first: online you always get the server's
 *   current index.html (a stale shell against a fresh API is an outage), and
 *   offline you get the last cached shell instead of the browser error page;
 * - `/assets/**` is cache-first: vite writes a content hash into every one of
 *   those filenames, so they are immutable by construction;
 * - integrity: the build writes into INTEGRITY the sha256 of every file under
 *   /assets/, and into SHELL_SHA256 the shell's (web/build/service-worker.ts).
 *   They are the only thing this worker trusts — anything in Cache Storage may
 *   have been put there by a page (a page's scripts can write it) or by an
 *   earlier delivery — so an asset is cached only when its bytes have its
 *   listed sha256, and served from the cache only after they are hashed again
 *   and still do. A cached copy that does not is deleted and fetched again; a
 *   fetched one that does not is refused, never handed to the page. An asset
 *   this build does not list (a newer release's, while this worker is the
 *   older one) passes through from the network and is never cached. The
 *   offline shell is kept and served only when it is this build's with no
 *   `rh-hub` marker, or with this origin's own hub's alone — or, in a cloud
 *   build, the canonical machine origin its marker names (as
 *   scripts/verify-web.mjs accepts it) — a shell pointed at another hub would
 *   sign in there and send it everything — and it is served with headers this
 *   worker sets, never the cached ones (a page can write those too);
 * - versioning: the browser installs a new worker only when sw.js's bytes
 *   change, which the table above makes every release (sw-register.ts also
 *   calls update() on every foreground, and when the server reports a new
 *   build). Activate deletes every cache but CACHE, then every entry of CACHE
 *   this build does not list or whose bytes are not the listed ones: a release
 *   drops what it no longer has (a tab still running the build before loads
 *   its lazy chunks from the network). Bumping CACHE by hand is the kill
 *   switch — the next worker starts from an empty cache (docs/web-integrity.md);
 * - precache: the build writes the assets it boots from into PRECACHE.
 *   Installing it fetches whatever of the boot set is not cached (and good)
 *   yet — the release's new app chunk, in the background, before the reload
 *   that needs it. A page boots from its language's translations too (a
 *   chunk per language but English, LANGUAGES): those of the languages this
 *   browser shows are precached with it, and no other. A page a hub serves
 *   boots from the cloud app's chunks as well (PRECACHE_CLOUD): precached
 *   there, and only there;
 * - only real assets are kept under /assets/: never the HTML a server answers
 *   for a path it does not have, which cached under a script's name would be
 *   handed to the module loader for good.
 */

const CACHE = "rh-sw-v1";

/* Filled in by the build; empty under `vite dev`, which never registers this. */
const PRECACHE = [];
const PRECACHE_CLOUD = [];
const INTEGRITY = {};
const SHELL_SHA256 = "";
/* The CSP sources of the shell's inline scripts ('sha256-…'), as a hub allows them. */
const SHELL_SCRIPTS = [];
/* Filled in by a cloud build (`DROPPEN_WEB_MODE=cloud`): the canonical
 * machine origin its shell's marker names. Empty in a normal build, whose
 * shell marker is this origin's own. */
const HUB_ORIGIN = "";
/* Filled in by the build: each language's translations but English's (its
 * words are the keys), `{prefix, files}` — the files, and how the chunk's
 * name starts in every build (`/assets/lang-ja-`). */
const LANGUAGES = {};

/* Where the offline shell is kept: every SPA route is served the same index.html. */
const SHELL = "/";

/* What may be cached under /assets/: what vite emits, and never a page. */
const ASSET_TYPE = /^(?:text\/javascript|application\/javascript|text\/css|font\/|image\/|application\/(?:wasm|json|font))/i;
function cacheableAsset(res) {
  return res.ok && ASSET_TYPE.test(res.headers.get("content-type") ?? "");
}

/* The sha256 this build lists for `path`, or undefined. */
function listedHash(path) {
  return Object.prototype.hasOwnProperty.call(INTEGRITY, path) ? INTEGRITY[path] : undefined;
}

async function sha256Hex(bytes) {
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
  let out = "";
  for (const b of digest) out += b.toString(16).padStart(2, "0");
  return out;
}

/* Whether `res` (left unread) is the asset at `path` as this build has it. */
async function assetMatches(path, res) {
  const want = listedHash(path);
  if (typeof want !== "string" || !cacheableAsset(res)) return false;
  try {
    return (await sha256Hex(await res.clone().arrayBuffer())) === want;
  } catch {
    return false;
  }
}

/* The hub this page may be pointed at: this origin's own, as the page dials
 * it — `wss://` on https, `ws://` on http (scripts/verify-web.mjs
 * `siteHubOrigin`). */
function hubHere() {
  return `${self.location.protocol === "https:" ? "wss" : "ws"}://${self.location.host}`;
}

/* The marker's origin: the cloud build's canonical machine origin when it
 * named one, else this origin's own. */
function shellHub() {
  return HUB_ORIGIN || hubHere();
}

/* `bytes` as one character per byte, so the string's indices are the bytes'. */
function latin1(bytes) {
  let text = "";
  for (let i = 0; i < bytes.length; i += 8192) text += String.fromCharCode.apply(null, bytes.subarray(i, i + 8192));
  return text;
}

/* Whether `bytes` are this build's shell: `{marked}` when they are — as built,
 * or with this origin's hub's marker, the exact tag a hub writes right after
 * the first `<head>` — else null. A marker naming any other hub (or ws:// on
 * https, or a second one, or one anywhere else) is part of the bytes, which
 * then are not the shell's. */
async function shellCheck(bytes) {
  if (!SHELL_SHA256) return null;
  const marker = `<meta name="rh-hub" content="${shellHub()}" />`;
  const head = latin1(bytes).indexOf("<head>");
  const at = head + "<head>".length;
  let built = bytes;
  if (head >= 0 && latin1(bytes.subarray(at, at + marker.length)) === marker) {
    built = new Uint8Array(bytes.length - marker.length);
    built.set(bytes.subarray(0, at));
    built.set(bytes.subarray(at + marker.length), at);
  }
  return (await sha256Hex(built)) === SHELL_SHA256 ? { marked: built !== bytes } : null;
}

/* `res` (left unread) as this build's shell: its bytes and whether a hub marked it, or null. */
async function shellOf(res) {
  if (!res.ok) return null;
  try {
    const bytes = new Uint8Array(await res.clone().arrayBuffer());
    const check = await shellCheck(bytes);
    return check && { bytes, marked: check.marked };
  } catch {
    return null;
  }
}

/* Keep `res` as the offline shell if it is this build's (its bytes only: the
 * headers it is served with are this worker's, {@link shellResponse}). */
async function keepShell(res) {
  const shell = await shellOf(res);
  if (shell) await (await caches.open(CACHE)).put(SHELL, new Response(shell.bytes, { headers: { "content-type": "text/html; charset=utf-8" } }));
}

/* The offline shell as the page gets it: its checked bytes, and the headers
 * the server that marked it sends with it — never the cached response's,
 * which a page may have written. A hub's (marked): its policy (rh-hub
 * `web.rs`), the shell's own inline scripts allowed by hash and nothing
 * inline besides. One thing differs: the relays' origins, which the hub lists
 * per response and an offline worker cannot know; `https:` and `wss:` stand
 * for them in connect-src alone, so a page that comes back online still has
 * its channels — what may run (script-src) is the hub's exactly, and that is
 * what keeps foreign code out. A local server's (unmarked): the same policy
 * on its own origin, framing by itself allowed as that server allows it. */
function shellResponse(shell) {
  const hub = hubHere();
  const policy = [
    "default-src 'self'",
    ["script-src 'self'", ...SHELL_SCRIPTS].join(" "),
    "style-src 'self' 'unsafe-inline'",
    "img-src 'self' data: blob:",
    "font-src 'self' data:",
    shell.marked ? `connect-src 'self' ${hub} https: wss:` : `connect-src 'self' ${hub}`,
    "worker-src 'self' blob:",
    "manifest-src 'self'",
    shell.marked ? "frame-src 'none'" : "frame-src 'self'",
    shell.marked ? "frame-ancestors 'none'" : "frame-ancestors 'self'",
    "base-uri 'self'",
    "form-action 'self'",
    "object-src 'none'",
  ].join("; ");
  return new Response(shell.bytes, {
    status: 200,
    headers: {
      "content-type": "text/html; charset=utf-8",
      "cache-control": "no-cache",
      "content-security-policy": policy,
      "x-content-type-options": "nosniff",
      "referrer-policy": "no-referrer",
      "permissions-policy": "publickey-credentials-get=(self), publickey-credentials-create=(self)",
    },
  });
}

/* A listed asset from the network, checked. The browser's own HTTP cache
 * keeps /assets/ for a year: a bad copy there is passed by once, for the
 * network's. Returns the response when it is good (or not a 2xx at all: a
 * 404 is not a tampered asset), else null. */
async function fetchListed(req, path) {
  let res = await fetch(req);
  if (!res.ok || (await assetMatches(path, res))) return res;
  res = await fetch(req, { cache: "reload" });
  if (!res.ok || (await assetMatches(path, res))) return res;
  return null;
}

/* The files of the languages this browser shows, as much a part of its boot
 * set as PRECACHE: the one its own languages pick (what a page shows unless
 * told otherwise), one whose chunk is in the cache from an earlier build (a
 * page showed it), and one whose chunk of this build the browser's HTTP
 * cache has (a page fetched it before any worker saw its requests: the first
 * install), which `only-if-cached` asks without going to the network. A
 * language this browser has not shown is fetched when a page first shows
 * it, and kept then, as a lazy chunk is. */
async function shownLanguages(cache) {
  const kept = (await cache.keys()).map((req) => new URL(req.url).pathname);
  const own = browserLanguage();
  const out = [];
  for (const [lang, { prefix, files }] of Object.entries(LANGUAGES)) {
    if (lang === own || kept.some((path) => path.startsWith(prefix)) || (await httpCached(files[0]))) out.push(...files);
  }
  return out;
}

/* The language the browser's languages pick, by the page's rule for `auto`
 * (src/i18n/match.ts `pickLanguage`, which test/sw-integrity.test.ts holds
 * this to): the first of them the app speaks, else English. Chinese in
 * Traditional script (`zh-Hant`, else Taiwan, Hong Kong or Macao) is zh-TW,
 * any other Chinese zh-CN, `ja…` ja and `en…` en; the script decides over
 * the region. */
function browserLanguage() {
  const tags = [...(Array.isArray(navigator.languages) ? navigator.languages : []), navigator.language];
  for (const tag of tags) {
    if (typeof tag !== "string" || !tag) continue;
    const [primary, ...rest] = tag.trim().replace(/_/g, "-").toLowerCase().split("-");
    if (primary === "en" || primary === "ja") return primary;
    if (primary !== "zh") continue;
    if (rest.includes("hant")) return "zh-TW";
    if (rest.includes("hans")) return "zh-CN";
    return rest.some((part) => part === "tw" || part === "hk" || part === "mo") ? "zh-TW" : "zh-CN";
  }
  return "en";
}

async function httpCached(url) {
  try {
    return (await fetch(url, { cache: "only-if-cached", mode: "same-origin" })).ok;
  } catch {
    return false;
  }
}

/* What the page gets instead of an asset whose bytes are not this build's. */
function refused(path) {
  return new Response(`${path}: its bytes are not the ones this build lists (sha256); not served\n`, {
    status: 502,
    statusText: "Integrity check failed",
    headers: { "content-type": "text/plain; charset=utf-8", "cache-control": "no-store" },
  });
}

/* Whether a hub serves this origin's shell (cloud mode), whose page boots from
 * PRECACHE_CLOUD too: the shell kept for offline says. A first install has
 * none yet: there a page a hub serves has just loaded those chunks, before
 * any worker saw its requests, and the browser's HTTP cache has the first of
 * them (what a hub serves), which `only-if-cached` asks without going to the
 * network; a local page never loads it. Only which listed assets the install
 * fetches ahead hangs on it; each is still checked against INTEGRITY. */
async function servedByHub(cache) {
  if (PRECACHE_CLOUD.length === 0) return false;
  try {
    const res = await cache.match(SHELL);
    if (res === undefined) return httpCached(PRECACHE_CLOUD[0]);
    const text = await res.text();
    const head = text.indexOf("<head>");
    return head >= 0 && text.startsWith('<meta name="rh-hub"', head + "<head>".length);
  } catch {
    return false;
  }
}

/* The SPA's full route set (app/router.tsx), plus cloud mode's sign-in,
 * sign-up and e-mail verification pages (features/landing/route.ts) and its
 * 「我的设备」 (features/cloud/devices-route.ts), and every other device's pages
 * under `/m/<device>/` (lib/machines/routes.ts). Only these navigations get the
 * offline shell: a `/embed/…` or download URL that fails should fail, not be
 * answered with an HTML shell that cannot possibly serve it. */
const SPA_ROUTE =
  /^\/(?:$|sessions(?:\/|$)|chat(?:\/|$)|terminals(?:\/|$)|files(?:\/|$)|settings(?:\/|$)|stats(?:\/|$)|apps\/|screens\/|signin$|signup$|verify$|devices\/?$|m\/[^/]+(?:\/|$))/;

self.addEventListener("install", (e) => {
  self.skipWaiting();
  // Best effort, and never a reason to fail the install: offline, the assets
  // are fetched on first use as before.
  e.waitUntil(
    (async () => {
      try {
        const cache = await caches.open(CACHE);
        const hub = await servedByHub(cache);
        for (const url of [...PRECACHE, ...(hub ? PRECACHE_CLOUD : []), ...(await shownLanguages(cache))]) {
          const hit = await cache.match(url);
          if (hit !== undefined && (await assetMatches(url, hit))) continue;
          const res = await fetchListed(url, url);
          if (res !== null && (await assetMatches(url, res))) await cache.put(url, res);
          else if (hit !== undefined) await cache.delete(url);
        }
      } catch {
        /* offline, or the build moved on again */
      }
    })(),
  );
});

self.addEventListener("activate", (e) => {
  e.waitUntil(
    (async () => {
      for (const k of await caches.keys()) if (k !== CACHE) await caches.delete(k);
      // Only what this build lists, with the bytes it lists: an asset it no
      // longer has, a copy that is not the asset, an older shell, and
      // anything else (an older worker's records) go.
      try {
        const cache = await caches.open(CACHE);
        for (const req of await cache.keys()) {
          const path = new URL(req.url).pathname;
          const res = await cache.match(req);
          const good =
            res !== undefined &&
            (path === SHELL ? (await shellOf(res)) !== null : path.startsWith("/assets/") && (await assetMatches(path, res)));
          if (!good) await cache.delete(req);
        }
      } catch {
        /* nothing to prune */
      }
      // Seed the offline shell. Failure is fine — the first successful
      // navigation fills it in.
      try {
        await keepShell(await fetch("/sessions"));
      } catch {
        /* offline */
      }
      await self.clients.claim();
    })(),
  );
});

self.addEventListener("fetch", (e) => {
  const req = e.request;
  if (req.method !== "GET") return;
  const url = new URL(req.url);
  if (url.origin !== self.location.origin) return;
  // `/v1/` is a Droppen hub's own API (cloud mode): never cached either.
  if (url.pathname.startsWith("/api/") || url.pathname.startsWith("/embed") || url.pathname.startsWith("/v1/")) return;

  if (req.mode === "navigate") {
    if (!SPA_ROUTE.test(url.pathname)) return;
    e.respondWith(
      (async () => {
        try {
          const res = await fetch(req);
          // Refresh the offline shell as we go, with this build's only: a
          // newer release's waits for its own worker. Served as it came
          // either way — an older worker must never stand in a release's way.
          if (res.ok) e.waitUntil(keepShell(res.clone()).catch(() => undefined));
          return res;
        } catch (err) {
          const cache = await caches.open(CACHE);
          const hit = await cache.match(SHELL);
          if (hit !== undefined) {
            const shell = await shellOf(hit);
            if (shell) return shellResponse(shell);
            await cache.delete(SHELL);
          }
          throw err;
        }
      })(),
    );
    return;
  }

  if (url.pathname.startsWith("/assets/")) {
    const path = url.pathname;
    e.respondWith(
      (async () => {
        const cache = await caches.open(CACHE);
        const listed = listedHash(path) !== undefined;
        const hit = await cache.match(req);
        if (hit !== undefined) {
          if (listed && (await assetMatches(path, hit))) return hit;
          // Not this build's asset, or not its bytes: never served from here.
          await cache.delete(req);
        }
        // A file this build does not list: the network's, never kept.
        if (!listed) return fetch(req);
        const res = await fetchListed(req, path);
        if (res === null) return refused(path);
        if (res.ok) {
          const copy = res.clone();
          e.waitUntil(cache.put(req, copy));
        }
        return res;
      })(),
    );
  }
});

/* Web Push arrival. Payload (server-rs push module): {title, body, tag, url}.
 * tag is the session key, so a chatty session collapses into one notification
 * rather than a column of them.
 *
 * The worker's badge is a flag, not a count: it has no idea how much of this
 * the user has already read. The page puts its own unread count (summed over
 * every device) back the moment the app is looked at (lib/badge.ts).
 * setAppBadge is absent in Firefox and may reject even where it exists, so it
 * gets its own waitUntil — a rejection there must not fail the push event and
 * lose the notification. */
self.addEventListener("push", (e) => {
  let data = {};
  try {
    data = e.data !== null ? e.data.json() : {};
  } catch {
    data = { body: e.data !== null ? e.data.text() : "" };
  }
  e.waitUntil(
    self.registration.showNotification(
      typeof data.title === "string" ? data.title : "Droppen",
      {
        body: typeof data.body === "string" ? data.body : "",
        tag: typeof data.tag === "string" ? data.tag : undefined,
        data: { url: typeof data.url === "string" ? data.url : "/sessions" },
        icon: "/icon-192.png",
        badge: "/icon-192.png",
      },
    ),
  );
  if ("setAppBadge" in navigator) {
    e.waitUntil(navigator.setAppBadge().catch(() => undefined));
  }
});

/* Where a tapped notification may lead: a path of this origin only, as
 * src/lib/sw-nav.ts `internalNavPath` takes it — the URL comes from a push
 * payload. A string that starts with one "/" and, resolved here, stays on
 * this origin (to the URL parser "/\host", or "/" then a tab then "/host",
 * is another host). Anything else is null. */
function internalPath(url) {
  if (typeof url !== "string" || !url.startsWith("/") || url.startsWith("//")) return null;
  try {
    const to = new URL(url, self.location.origin);
    return to.origin === self.location.origin ? `${to.pathname}${to.search}${to.hash}` : null;
  } catch {
    return null;
  }
}

/* Tapping a notification: focus an existing window and let the SPA route
 * internally (sw-register.ts listens for {type:"nav"}), so the transcript and
 * every open websocket survive. With no window open, launch straight at it.
 * Either way only to a path of this origin ({@link internalPath}). */
self.addEventListener("notificationclick", (e) => {
  e.notification.close();
  const url = internalPath(e.notification.data?.url) ?? "/sessions";
  e.waitUntil(
    (async () => {
      const wins = await self.clients.matchAll({ type: "window", includeUncontrolled: true });
      const win = wins.find((c) => "focus" in c);
      if (win !== undefined) {
        await win.focus();
        win.postMessage({ type: "nav", url });
      } else {
        await self.clients.openWindow(url);
      }
    })(),
  );
});
