import { router } from "@/app/router";
import { ARRIVE_EVENT, internalNavPath } from "@/lib/sw-nav";

/**
 * Service-worker registration (public/sw.js).
 *
 * Production bundles only: under `vite dev` the worker's asset caching fights
 * HMR for no benefit. Registration is also allowed to fail silently — over
 * plain http on a LAN address the context is not secure and there is simply no
 * service worker to be had, which is a normal way to run this app, not an error.
 *
 * Only where the app runs: never on a hub's site host, or on its origin once
 * the app moved off it (`lib/hub/hosts.ts`), whose pages hold no keys and
 * have nothing to keep offline. In cloud mode it is called once the page
 * knows which host it is on, which may be after `load`.
 */
export function registerSw(): void {
  if (!("serviceWorker" in navigator) || !import.meta.env.PROD) return;

  const register = () => {
    navigator.serviceWorker
      .register("/sw.js")
      .then((reg) => {
        // The browser revalidates sw.js on its own, but only occasionally;
        // checking every time the app comes to the foreground keeps a
        // long-lived installed PWA from running weeks-old cached assets.
        document.addEventListener("visibilitychange", () => {
          if (document.visibilityState === "visible") void reg.update();
        });
      })
      .catch(() => undefined);
  };
  if (document.readyState === "complete") register();
  else window.addEventListener("load", register, { once: true });

  // A tapped notification routes inside the running SPA (sw.js posts this
  // rather than navigating), so the transcript and websockets are not torn down.
  // Only a same-origin internal path is honoured — see `internalNavPath`.
  navigator.serviceWorker.addEventListener("message", (e) => {
    const to = internalNavPath(e.data);
    if (to === null) return;
    void router.navigate({ to });
    // The reader is arriving at that page — which may be the one already on
    // screen, where navigating changes nothing (see `useReportSeen`).
    window.dispatchEvent(new CustomEvent(ARRIVE_EVENT, { detail: to }));
  });
}
