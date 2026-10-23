/**
 * The page's address as it loaded, before any other module reads it: the
 * first import of `main.tsx`, ahead of the boot modules that take fragments
 * off it (`lib/local-open-boot.ts`, `features/link/fragment-boot.ts`).
 *
 * The move notice on the hub's origin (`features/hosts/legacy.tsx`) sends
 * the browser on to the app host with exactly this path, query and fragment,
 * so a browser hand-off (`/link#droppen-link=…`) and a verification link
 * (`/verify#t=…`) arrive there whole. Kept in memory only.
 */
export interface BootUrl {
  pathname: string;
  search: string;
  hash: string;
}

export const BOOT_URL: BootUrl | null = typeof location === "undefined" ? null : { pathname: location.pathname, search: location.search, hash: location.hash };

/** The path, query and fragment the page loaded with (`/` where there is no page). */
export function bootPath(url: BootUrl | null = BOOT_URL): string {
  return url ? `${url.pathname}${url.search}${url.hash}` : "/";
}
