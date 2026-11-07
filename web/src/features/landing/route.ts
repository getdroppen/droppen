import { useSyncExternalStore } from "react";
import { appUrl, hostMode, siteOrigin } from "@/lib/hub/hosts";

/*
 * Cloud public pages live outside the app router, which mounts only when a
 * machine opens. Read their paths from `location` and use the History API;
 * TanStack's patched history methods keep the eventual app route in sync.
 * Local mode never uses this module. Split app/site hosts use ordinary links.
 */

export const DOWNLOAD_PATH = "/download";
export const SIGN_IN_PATH = "/signin";
export const SIGN_UP_PATH = "/signup";
const VERIFY_PATH = "/verify";
export const PRICING_PATH = "/pricing";
export const PRIVACY_PATH = "/privacy";
export const SECURITY_PATH = "/security";
export const TERMS_PATH = "/terms";
export const WAITLIST_HREF = `${PRICING_PATH}#waitlist`;

/** The installed app's `start_url` and the destination after sign-in. */
export const APP_PATH = "/sessions";

type PublicPage = "landing" | "download" | "pricing" | "privacy" | "security" | "terms" | "signin" | "signup" | "verify";

/* Deep links show auth in place while signed out so they can resume after sign-in. */
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

export function isLandingPath(pathname: string): boolean {
  return publicPage(pathname) === "landing";
}

/** The pages shown independently of sign-in state. */
export type FrontPage = "landing" | "download" | "pricing" | "privacy" | "security" | "terms";

const FRONT_PAGES: readonly PublicPage[] = ["landing", "download", "pricing", "privacy", "security", "terms"];

export function frontPage(pathname: string): FrontPage | null {
  const page = publicPage(pathname);
  return FRONT_PAGES.includes(page) ? (page as FrontPage) : null;
}

export function isFrontPath(pathname: string): boolean {
  return frontPage(pathname) !== null;
}

type FrontRoute = { [P in FrontPage]: { show: P } }[FrontPage];
type BootRoute = FrontRoute | { show: "verify" } | { show: "auth" } | { show: "app"; replace?: string };

export function bootRoute(pathname: string, signedIn: boolean): BootRoute {
  const front = frontPage(pathname);
  if (front) return { show: front };
  if (publicPage(pathname) === "verify") return { show: "verify" };
  if (!signedIn) return { show: "auth" };
  return isAuthPath(pathname) ? { show: "app", replace: APP_PATH } : { show: "app" };
}

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

export function usePathname(): string {
  return useSyncExternalStore(
    subscribe,
    () => location.pathname,
    () => "/",
  );
}

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

function beforeFragment(path: string): string {
  const marker = path.indexOf("#");
  const bare = marker === -1 ? path : path.slice(0, marker);
  return bare.length > 1 ? bare.replace(/\/+$/, "") : bare;
}

/** Leave same-page fragment clicks to native scrolling and history. */
function samePageFragment(path: string): boolean {
  if (!path.includes("#") || typeof location === "undefined") return false;
  return beforeFragment(path) === beforeFragment(location.pathname + location.search);
}

export function onPublicLink(path: string) {
  const elsewhere = publicHref(path) !== path;
  return (event: LinkClick) => {
    if (elsewhere || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    if (samePageFragment(path)) return;
    event.preventDefault();
    goPublic(path);
  };
}

export function publicLink(path: string): { href: string; onClick: (event: LinkClick) => void } {
  return { href: publicHref(path), onClick: onPublicLink(path) };
}
