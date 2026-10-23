import { useSyncExternalStore } from "react";
import { appUrl, hostMode, siteOrigin } from "@/lib/hub/hosts";
import { DOWNLOAD_PATH } from "./get-droppen";

/*
 * Cloud mode's public pages have no router: the landing page at `/` (signed
 * in or not), the download page at `/download`, the pricing page at
 * `/pricing`, the privacy notice at `/privacy`, the security design at
 * `/security` and the terms of service at `/terms` (each signed in or not), sign-in /
 * sign-up at their own paths, and `/verify`, where the verification mail's
 * link lands (signed in or not). The app's router only
 * mounts once a machine is open (`features/cloud/cloud-app.tsx`), so these
 * few paths are read straight off `location` and moved between with the
 * History API; TanStack's history (patched onto `pushState` /
 * `replaceState`) follows along, so the app later boots from whatever path
 * is current.
 *
 * Local mode (a remote-hand server's own shell) never reaches any of this.
 *
 * Where the hub serves the app and the public pages on hosts of their own
 * (`lib/hub/hosts.ts`), a link between them is an ordinary link to the other
 * host ({@link publicLink}), and `/` on the app host is the app.
 */

export const SIGN_IN_PATH = "/signin";
export const SIGN_UP_PATH = "/signup";
/** The verification mail's link: `/verify#t=<token>` (the token in the fragment). */
export const VERIFY_PATH = "/verify";
/** The plans, and the waitlist (`#waitlist`). */
export const PRICING_PATH = "/pricing";
/** The privacy notice. */
export const PRIVACY_PATH = "/privacy";
/** How the encryption works (`security/security.tsx`). */
export const SECURITY_PATH = "/security";
/** The terms of service (`terms/terms.tsx`). */
export const TERMS_PATH = "/terms";
/** The waitlist's form, on the pricing page. */
export const WAITLIST_HREF = `${PRICING_PATH}#waitlist`;

/**
 * Where the app starts: the machine list, or the sessions of the machine
 * last opened here. "Open app" on the landing page goes here, sign-in lands
 * here, and the installed app starts here (`start_url` in
 * `public/manifest.webmanifest`), so it never opens on the landing page.
 */
export const APP_PATH = "/sessions";

/** What a signed-out visitor sees at `pathname`. */
export type PublicPage = "landing" | "download" | "pricing" | "privacy" | "security" | "terms" | "signin" | "signup" | "verify";

/**
 * Only the bare root is the landing page. Sign-in and sign-up have their own
 * paths; every other path is a deep link into the app (a session, a terminal),
 * which keeps showing the sign-in form in place so the link still opens once
 * the visitor is in. On the app host of a hub that has one (`hostMode()`
 * `app`), the root is the app too: signed in, its start; signed out, the
 * sign-in form.
 */
export function publicPage(pathname: string): PublicPage {
  const path = pathname.replace(/\/+$/, "") || "/";
  if (path === "/") return hostMode() === "app" ? "signin" : "landing";
  if (path === DOWNLOAD_PATH) return "download";
  if (path === PRICING_PATH) return "pricing";
  if (path === PRIVACY_PATH) return "privacy";
  if (path === SECURITY_PATH) return "security";
  if (path === TERMS_PATH) return "terms";
  if (path === SIGN_UP_PATH) return "signup";
  if (path === VERIFY_PATH) return "verify";
  return "signin";
}

/** Whether `pathname` is the landing page. */
export function isLandingPath(pathname: string): boolean {
  return publicPage(pathname) === "landing";
}

/** The pages that are pages of their own whether or not this browser is signed in. */
export type FrontPage = "landing" | "download" | "pricing" | "privacy" | "security" | "terms";

const FRONT_PAGES: readonly PublicPage[] = ["landing", "download", "pricing", "privacy", "security", "terms"];

/** `pathname`'s page if it is one of the {@link FrontPage}s. */
export function frontPage(pathname: string): FrontPage | null {
  const page = publicPage(pathname);
  return FRONT_PAGES.includes(page) ? (page as FrontPage) : null;
}

/**
 * Whether `pathname` is a page of its own whether or not this browser is
 * signed in — the landing page, the download page, the pricing page, the
 * privacy notice, the security design or the terms — which starts nothing of the app
 * until the visitor goes on into it.
 */
export function isFrontPath(pathname: string): boolean {
  return frontPage(pathname) !== null;
}

/**
 * What a page loading at `pathname` shows: the landing page at `/`, the
 * download page at `/download`, the pricing page at `/pricing`, the privacy
 * notice at `/privacy`, the security design at `/security` and the terms at
 * `/terms` whether or not
 * a device session is kept here;
 * signed out, the sign-in / sign-up forms (in place on a deep link); signed
 * in, the app — from its start rather than from a sign-in page (`replace`),
 * and from a deep link where it points. The verification link's page shows
 * signed in or not.
 */
export function bootRoute(
  pathname: string,
  signedIn: boolean,
): { [P in FrontPage]: { show: P } }[FrontPage] | { show: "verify" } | { show: "auth" } | { show: "app"; replace?: string } {
  const front = frontPage(pathname);
  if (front) return { show: front } as { [P in FrontPage]: { show: P } }[FrontPage];
  if (publicPage(pathname) === "verify") return { show: "verify" };
  if (!signedIn) return { show: "auth" };
  return isAuthPath(pathname) ? { show: "app", replace: APP_PATH } : { show: "app" };
}

/** Whether `pathname` is one of the sign-in / sign-up pages (not a route the app knows). */
export function isAuthPath(pathname: string): boolean {
  const path = pathname.replace(/\/+$/, "");
  return path === SIGN_IN_PATH || path === SIGN_UP_PATH;
}

const listeners = new Set<() => void>();

function notify(): void {
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  addEventListener("popstate", listener);
  return () => {
    listeners.delete(listener);
    removeEventListener("popstate", listener);
  };
}

/** The current path, re-read on back / forward and on {@link goPublic}. */
export function usePathname(): string {
  return useSyncExternalStore(
    subscribe,
    () => location.pathname,
    () => "/",
  );
}

/** Move between the public pages without a reload (a new history entry unless `replace`). */
export function goPublic(path: string, { replace = false }: { replace?: boolean } = {}): void {
  if (location.pathname === path) return;
  if (replace) history.replaceState(history.state, "", path);
  else history.pushState(null, "", path);
  notify();
}

/**
 * Where a link to `path` (with any query and fragment) goes from this page:
 * the path itself, or the same path on the other host where the hub keeps
 * the app and the public pages apart (`lib/hub/hosts.ts`) — from the site
 * host, every page but the public ones is the app's; from the app host, the
 * public pages are the site's; from the hub's origin after the move,
 * everything is the app's.
 */
export function publicHref(path: string): string {
  const bare = path.replace(/[?#].*$/, "") || "/";
  switch (hostMode()) {
    case "site":
      return isFrontPath(bare) ? path : appUrl(path);
    case "legacy":
      return appUrl(path);
    default: {
      // On the app host a link to `/` is one to the landing page (the app's start is `APP_PATH`).
      const site = siteOrigin();
      return site && (bare === "/" || isFrontPath(bare)) ? `${site}${path}` : path;
    }
  }
}

type LinkClick = { button: number; metaKey: boolean; ctrlKey: boolean; shiftKey: boolean; altKey: boolean; preventDefault(): void };

/**
 * A link's click, done in place: a plain left click moves without a reload;
 * a modified click (new tab, new window) is left to the browser, and so is
 * any click on a link to another host ({@link publicHref}).
 */
export function onPublicLink(path: string) {
  const elsewhere = publicHref(path) !== path;
  return (event: LinkClick) => {
    if (elsewhere || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    event.preventDefault();
    goPublic(path);
  };
}

/** A public link's `href` and click, for `<a {...publicLink(path)}>`. */
export function publicLink(path: string): { href: string; onClick: (event: LinkClick) => void } {
  return { href: publicHref(path), onClick: onPublicLink(path) };
}
