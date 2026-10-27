/*
 * The download tickets a page spends on the two surfaces that cannot carry a
 * bearer header: `<img src>` and `<a href download>`. The server mints one for
 * a minute, bound to the session that minted it; this keeps one tab's ticket
 * warm just before it stops being worth spending, shares each mint among the
 * callers that want it, and repairs a ticket an image reports dead.
 *
 * The token itself — who the page is, and whether this server wants a login —
 * stays in `auth.ts`; this file asks it for the token, whether a login is
 * required, and the page's home machine's client (`pageApi`).
 */
import { ApiError } from "@/api/errors";
import { authRequired, getToken, pageApi, type TicketScope } from "./auth";

// ---------------------------------------------------------------------------
// Tickets
// ---------------------------------------------------------------------------

/**
 * How long a minted download ticket is spent before a fresh one is asked for.
 * The server gives it a minute, bound to the session that minted it; stopping
 * at forty-five seconds leaves fifteen of slack for a slow request and for a
 * URL rendered just before the end. Measured on the local clock from the mint
 * rather than read from `expiresAt`, so a skewed clock cannot shorten it to
 * nothing (and mint on every render) or stretch it past the server's.
 */
export const DOWNLOAD_TICKET_TTL_MS = 45 * 1000;

export interface CachedTicket {
  ticket: string;
  /** `Date.now()` when it was minted — elapsed local time, so clock skew is moot. */
  cachedAt: number;
}

/** The cached ticket if it is still worth spending at `now`, else null. */
export function freshTicket(
  entry: CachedTicket | null,
  now: number,
  ttlMs: number = DOWNLOAD_TICKET_TTL_MS,
): string | null {
  if (!entry) return null;
  const age = now - entry.cachedAt;
  // A clock that jumped backwards makes `age` negative; that is not freshness.
  return age >= 0 && age < ttlMs ? entry.ticket : null;
}

/**
 * The one download ticket this tab shares, as a store that can be subscribed to.
 *
 * A store rather than a plain variable because of the failure this file used to
 * have. `<img src>` is built during a render, a render cannot await a mint, and
 * so a render that found the cache empty baked a URL with no ticket in it — a
 * 401 and a broken thumbnail. The mint that the miss kicked off then landed in
 * a module variable nothing was watching, so React had no reason to re-render
 * and the broken URL stayed in the DOM until something unrelated re-rendered
 * that subtree; on an idle session page, never. A backgrounded tab — which is
 * how this app is used, since iOS freezes a PWA's timers and then its whole
 * JS — hit that path on the first render after every wake. Anything that reads
 * the ticket while rendering now goes through {@link subscribeDownloadTicket}
 * and {@link downloadTicketSnapshot} with `useSyncExternalStore`, so a ticket
 * that lands late still repaints.
 *
 * Two rules keep the repaints down to the ones that mean something, because an
 * `<img src>` that changes is an `<img>` the browser fetches again — on a phone,
 * over cellular:
 *
 *  * Watchers are woken by a change of *ticket string*, not of cache entry. An
 *    entry is also rewritten in place to expire it, and an expiry is not news to
 *    an image that has already loaded.
 *  * A replacement lands before the old one is dropped, never after: there is
 *    no window in which a render bakes a URL with no ticket at all.
 *
 * A ticket lives a minute, so while anything renders it, it is replaced just
 * before it stops being worth spending ({@link scheduleTicketRefresh}). That
 * costs a picture that has loaded nothing — `HostImage` keeps the URL that
 * worked — and keeps the ones that have not loaded yet, and every `href`, on a
 * live ticket. A tab nobody is looking at mints nothing; it catches up when it
 * is looked at again.
 */
let downloadTicket: CachedTicket | null = null;
let downloadInFlight: Promise<string> | null = null;
const ticketWatchers = new Set<() => void>();

/**
 * Bumped by anything that decides the cached ticket is dead.
 *
 * A mint already in flight at that moment was very likely answered by the server
 * instance that has since gone away, so its answer is compared against the epoch
 * it started in and dropped rather than cached.
 */
let ticketEpoch = 0;

/** Replace the cache, waking watchers only when the string they render changed. */
function setDownloadTicket(next: CachedTicket | null): void {
  const before = downloadTicket?.ticket ?? null;
  downloadTicket = next;
  scheduleTicketRefresh();
  if ((next?.ticket ?? null) === before) return;
  // Copied first: a watcher is free to unsubscribe as it runs, and a `Set`
  // mutated mid-iteration would skip whoever came after it.
  for (const watcher of [...ticketWatchers]) watcher();
}

/** Watch the download ticket; the returned function stops watching. */
export function subscribeDownloadTicket(onChange: () => void): () => void {
  ticketWatchers.add(onChange);
  scheduleTicketRefresh();
  return () => {
    ticketWatchers.delete(onChange);
    scheduleTicketRefresh();
  };
}

let refreshTimer: ReturnType<typeof setTimeout> | null = null;
let watchingVisibility = false;

/** Whether the page is hidden, where that can be known. */
function pageHidden(): boolean {
  return typeof document !== "undefined" && document.visibilityState === "hidden";
}

/**
 * Replace the cached ticket when it stops being worth spending, as long as
 * something renders it — a picture still to load, a download link — and the
 * page is being looked at.
 *
 * At most one timer, re-armed on every change of the cache and of who is
 * watching, so there is nothing to leak. A mint that fails is not retried
 * from here: the next thing that needs a ticket asks again, so a server that
 * is down cannot be turned into a loop. A hidden page mints nothing; when it
 * is shown again a stale ticket is replaced at once.
 */
function scheduleTicketRefresh(): void {
  if (refreshTimer !== null) {
    clearTimeout(refreshTimer);
    refreshTimer = null;
  }
  if (!downloadTicket || ticketWatchers.size === 0) return;
  if (!watchingVisibility && typeof document !== "undefined" && typeof document.addEventListener === "function") {
    watchingVisibility = true;
    document.addEventListener("visibilitychange", () => {
      if (!pageHidden() && ticketWatchers.size > 0 && downloadTicket && freshDownloadTicket() === null) void primeDownloadTicket();
    });
  }
  const due = downloadTicket.cachedAt + DOWNLOAD_TICKET_TTL_MS - Date.now();
  refreshTimer = setTimeout(
    () => {
      refreshTimer = null;
      if (ticketWatchers.size === 0 || pageHidden()) return;
      void primeDownloadTicket();
    },
    Math.max(0, due),
  );
  // Never what keeps a process alive (tests run this module under node).
  (refreshTimer as { unref?: () => void }).unref?.();
}

/**
 * The last download ticket this tab was handed, expired or not, or null.
 *
 * The `getSnapshot` half of the store, so it has to be free of side effects and
 * has to keep answering the same thing until a watcher has been woken — which
 * rules out both the "start a mint on a miss" its predecessor did and any
 * freshness check against the clock. Last known rather than live is also the
 * right answer for a URL: a ticket past its forty-five seconds is usually
 * still good (the server keeps it for sixty) and is about to be replaced, and
 * when it is not, the `<img>` that finds out says so. Callers that must have a
 * live one await {@link fetchTicket}.
 */
export function downloadTicketSnapshot(): string | null {
  return downloadTicket?.ticket ?? null;
}

/** The cached ticket if it is still worth spending right now, minting nothing. */
export function freshDownloadTicket(): string | null {
  return freshTicket(downloadTicket, Date.now());
}

async function mintTicket(scope: TicketScope): Promise<string> {
  const res = await pageApi().POST("/api/auth/ticket", { body: { scope } });
  if (!res.data) throw new ApiError(res.error);
  return res.data.ticket;
}

/** Mint a download ticket into the cache, sharing one request among callers. */
function mintDownloadTicket(): Promise<string> {
  if (downloadInFlight) return downloadInFlight;
  const epoch = ticketEpoch;
  const flight = mintTicket("download").then((ticket) => {
    if (epoch === ticketEpoch) setDownloadTicket({ ticket, cachedAt: Date.now() });
    return ticket;
  });
  // Cleared by identity rather than unconditionally: an invalidation clears this
  // slot itself, and the mint it abandoned must not then clear the one that
  // replaced it and leave two mints running with no dedup between them.
  const settle = () => {
    if (downloadInFlight === flight) downloadInFlight = null;
  };
  void flight.then(settle, settle);
  downloadInFlight = flight;
  return flight;
}

/**
 * A ticket for `scope`.
 *
 * `ws` tickets are single-use — redeeming one destroys it — so they are never
 * cached and every reconnect mints its own. `download` tickets are reusable for
 * a minute, so one is kept and shared: a transcript full of images would
 * otherwise mint a ticket per `<img>` and walk straight into the server's
 * live-ticket cap. Per click is the other side of that line and is fine — a
 * click is rare, and it is the one moment at which we know a URL is about to be
 * spent rather than merely rendered.
 */
export async function fetchTicket(scope: TicketScope): Promise<string> {
  if (scope !== "download") return mintTicket(scope);
  return freshDownloadTicket() ?? mintDownloadTicket();
}

/**
 * Warm the cache, so the first paint has a ticket to build `<img src>` from.
 *
 * Resolves either way — a failure only means the next URL goes out unticketed,
 * and the image that gets a 401 for it repairs the cache — which is why the
 * router guard can await this without a broken server hanging the app.
 */
export function primeDownloadTicket(): Promise<void> {
  if (!authRequired() || !getToken()) return Promise.resolve();
  return fetchTicket("download").then(
    () => undefined,
    () => undefined,
  );
}

/**
 * Treat the cached ticket as dead and go and get another.
 *
 * Expired in place rather than dropped: everything that *renders* it keeps the
 * string it already has — which is worth trying, and is sitting in a DOM
 * attribute either way — while everything that *spends* it sees a stale entry
 * and mints. When the replacement lands the watchers wake exactly once, with a
 * ticket that works. Dropping it instead would repaint every image with a URL
 * carrying no ticket at all, which is a guaranteed 401 and then a second
 * repaint when the mint returns.
 */
export function invalidateDownloadTicket(): void {
  ticketEpoch += 1;
  downloadInFlight = null;
  if (downloadTicket) setDownloadTicket({ ...downloadTicket, cachedAt: 0 });
  void primeDownloadTicket();
}

/** Forget the ticket outright — for a token that no longer exists. */
export function forgetDownloadTicket(): void {
  ticketEpoch += 1;
  downloadInFlight = null;
  setDownloadTicket(null);
}

/**
 * How long after a repair the next broken image is ignored.
 *
 * An image that is still broken after being given a brand new ticket is broken
 * for a reason a ticket cannot fix, and the repair costs a mint and a repaint
 * of every picture that has not loaded yet. Twenty seconds is far longer than
 * the round trip and well inside the ticket's minute, so a page of dead images
 * cannot turn into a mint loop against the server's live-ticket cap.
 */
export const HEAL_COOLDOWN_MS = 20 * 1000;

/** When the cached ticket was last replaced because an image said it was dead. */
let lastHealAt = 0;

/**
 * What an `<img>` that failed to load does about it.
 *
 * A broken image is the only report we ever get that a download ticket has
 * died, and it is a poor one: an error event carries no status code, so a 401
 * from a dead ticket and a 404 from a deleted file arrive here looking
 * identical. So this is deliberately cheap and self-limiting, in three ways
 * that between them make a loop impossible:
 *
 *  * Nothing happens unless `used` — the ticket that was in the URL that
 *    failed — is the one currently worth spending. A gallery whose images all
 *    fail at once therefore costs one mint: the first image condemns the ticket
 *    and the rest find it already condemned and only join the mint in flight.
 *  * A repair inside {@link HEAL_COOLDOWN_MS} of the last one is refused, so a
 *    file that is simply gone cannot mint over and over as it is repainted.
 *  * And the caller latches: one repair per mounted image, ever.
 */
export function healDownloadTicket(used: string | null): void {
  if (!authRequired() || !getToken()) return;
  const spendable = freshDownloadTicket();
  // No live ticket to condemn: either one has never been minted, or somebody
  // else has already condemned this one and a mint is on its way — which
  // `primeDownloadTicket` joins rather than duplicates.
  if (spendable === null) {
    void primeDownloadTicket();
    return;
  }
  // The failed URL was built with a ticket that has since been replaced, so
  // the picture is about to repaint with the new one on its own.
  if (used !== spendable) return;
  const now = Date.now();
  const since = now - lastHealAt;
  // Negative is a clock that jumped backwards, which is not a cooldown — the
  // same reading `freshTicket` takes of the same problem.
  if (since >= 0 && since < HEAL_COOLDOWN_MS) return;
  lastHealAt = now;
  invalidateDownloadTicket();
}

/**
 * Whether a `settings` frame means we are talking to a server that never minted
 * the ticket we are holding.
 *
 * Tickets live in a `HashMap` in the server's memory, so a restart forgets every
 * one of them while the browser goes on believing its copy is good — and nothing
 * on the client notices, because the websockets reconnect and mint themselves
 * fresh `ws` tickets while every download quietly 401s. `serverStartedAt`, which
 * the feed sends in a `settings` frame on every connect, is what tells the two
 * cases apart: a reconnect carrying a different start time is a restart, and one
 * carrying the same start time is a dropped connection that cost us nothing.
 * Comparing it rather than reacting to the socket opening is what stops a blip
 * on a phone's flaky connection from throwing away a good ticket and repainting
 * every image on the page.
 *
 * The first frame of a page load proves nothing and never counts. A server that
 * will not identify its boot at all (recorded as `""`) counts as a restart on
 * every frame after the first — the blunter, older behaviour, and the safe
 * direction to be wrong in.
 */
export function serverRestarted(prev: string | null, next: string | null | undefined): boolean {
  if (prev === null) return false;
  if (!prev || !next) return true;
  return prev !== next;
}

