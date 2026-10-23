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
 *   offline shell is kept and served only when it is this build's, the hub's
 *   `rh-hub` marker aside (as scripts/verify-web.mjs compares it);
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
 *   that needs it;
 * - only real assets are kept under /assets/: never the HTML a server answers
 *   for a path it does not have, which cached under a script's name would be
 *   handed to the module loader for good.
 */

const CACHE = "rh-sw-v1";

/* Filled in by the build; empty under `vite dev`, which never registers this. */
const PRECACHE = [];
const INTEGRITY = {};
const SHELL_SHA256 = "";

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

/* The marker a hub writes right after the shell's first `<head>`. */
const MARKER = /^<meta name="rh-hub" content="wss?:\/\/[A-Za-z0-9.:[\]-]+" \/>/;

/* The shell as built: without the hub's marker, the exact tag in the one place
 * it is written, as scripts/verify-web.mjs `unmark` takes it out. */
function unmarked(bytes) {
  // One character per byte, so the string's indices are the bytes'.
  let text = "";
  for (let i = 0; i < bytes.length; i += 8192) text += String.fromCharCode.apply(null, bytes.subarray(i, i + 8192));
  const head = text.indexOf("<head>");
  if (head < 0) return bytes;
  const at = head + "<head>".length;
  const m = MARKER.exec(text.slice(at));
  if (m === null) return bytes;
  const out = new Uint8Array(bytes.length - m[0].length);
  out.set(bytes.subarray(0, at));
  out.set(bytes.subarray(at + m[0].length), at);
  return out;
}

/* Whether `res` (left unread) is this build's shell. */
async function shellMatches(res) {
  if (!SHELL_SHA256 || !res.ok) return false;
  try {
    return (await sha256Hex(unmarked(new Uint8Array(await res.clone().arrayBuffer())))) === SHELL_SHA256;
  } catch {
    return false;
  }
}

/* Keep `res` as the offline shell if it is this build's. */
async function keepShell(res) {
  if (await shellMatches(res)) await (await caches.open(CACHE)).put(SHELL, res);
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

/* What the page gets instead of an asset whose bytes are not this build's. */
function refused(path) {
  return new Response(`${path}: its bytes are not the ones this build lists (sha256); not served\n`, {
    status: 502,
    statusText: "Integrity check failed",
    headers: { "content-type": "text/plain; charset=utf-8", "cache-control": "no-store" },
  });
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
        for (const url of PRECACHE) {
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
            (path === SHELL ? await shellMatches(res) : path.startsWith("/assets/") && (await assetMatches(path, res)));
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
            if (await shellMatches(hit)) return hit;
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
