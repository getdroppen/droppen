// First, before any other module can read the address: a `#local-open=` token
// comes off it (`lib/local-open.ts`), and so do the browser hand-off's
// fragments — before the router or the service worker reads it.
import "./lib/local-open-boot";
// The address as it loaded (the hand-off's fragment still on it), for the
// move notice on the hub's origin to pass on whole (`lib/hub/boot-url.ts`).
// Taken here and handed to the notice: read only by the notice's own chunk,
// it would be read when that chunk runs, with the fragments gone.
import { BOOT_URL } from "./lib/hub/boot-url";
import { browserEnv, restoreReturn, takeReturn } from "./features/link/fragment-boot";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider } from "@tanstack/react-router";
import { isReturnPath } from "./features/link/handoff";
import { isCloud } from "./lib/hub/config";
import { appUrl, moveOrigins, notInstallable, resolveHostMode, siteOrigin } from "./lib/hub/hosts";
import { MOVE_PATH } from "./lib/move/types";
import { frontPage } from "./features/landing/route";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { loadPage } from "./app/page-load";
import { router } from "./app/router";
import { startBadgeSync } from "./lib/badge";
// Side-effect import: the module's `beforeinstallprompt` listener has to be in
// place before the browser fires it, which happens well before React mounts.
import "./lib/install";
import { startKbNative } from "./lib/kb-native";
import { startLocalOpen } from "./lib/auth";
import { registerSw } from "./lib/sw-register";
import { rootInLanguage } from "./i18n";
import "./index.css";

const queryClient = new QueryClient({
  defaultOptions: {
    queries: { staleTime: 5_000, retry: 1 },
  },
});

// The phone keyboard: left to WebKit's own reveal of the focused field.
startKbNative();
startBadgeSync();

// Served by a remote-hand hub (cloud mode), the app runs behind the hub's
// sign-in, device setup and machine picker, and every request it makes goes
// to the picked machine end to end (`CloudApp` opens it). Served by a server,
// it is exactly as before: the one machine is the server itself
// (`lib/machines`), and nothing in `features/cloud` runs.
//
// `/link/return` on a server is the one page that stands alone, before the
// app's router and login: droppen.app sends the system browser there with its
// answer to a sign-in, and that browser has no login here
// (`features/link/link-return.tsx`).
//
// Which of its hosts the hub serves this page on (`lib/hub/hosts.ts`) is read
// first, before anything of cloud mode starts: the app host (or the one host
// of a hub that does not say) runs the app as above; the site host only the
// public pages, with no key store and no service worker; the hub's origin,
// once the app has moved off it, the move notice (`features/hosts/legacy.tsx`).
//
// Every page but the local app is fetched by the page that runs it: the local
// app's boot carries none of cloud mode's code (`build/local-boot.ts` fails
// the build if it does). What a hub serves is one chunk (`hub-pages.ts`), and
// the shell asks for it as it loads (`cloudPreloadScript`), so it is on its
// way while the hub's hosts are read. Until a page's chunk is there, the page
// stays as blank as it is before any script runs; when it does not come, the
// page reloads once, then says so (`app/page-load.tsx`).
const hubPages = () => import("./hub-pages");
const returning = !isCloud() && isReturnPath(location.pathname);
if (!isCloud() && !returning) {
  // A page opened signed in from the device itself trades its token before
  // the router asks who it is (`lib/auth.ts`).
  startLocalOpen();
}
// Whichever page this is, its first frame waits for the reader's language
// (a chunk of its own, unless English): never English first (`i18n/index.ts`).
const root = rootInLanguage(createRoot(document.getElementById("root")!));
if (returning) {
  registerSw();
  const answer = takeReturn();
  void loadPage(
    root,
    () => import("./features/link/link-return"),
    ({ LinkReturnPage }) =>
      root.render(
        <StrictMode>
          <LinkReturnPage answer={answer} />
        </StrictMode>,
      ),
    {
      // The answer lives in this page's memory only (`fragment-boot.ts`): back
      // on the address for the reload, which takes it off again first thing.
      beforeReload: () => {
        const env = browserEnv();
        if (answer && env) restoreReturn(env, answer);
      },
    },
  );
} else if (!isCloud()) {
  registerSw();
  root.render(
    <StrictMode>
      <QueryClientProvider client={queryClient}>
        <RouterProvider router={router} />
      </QueryClientProvider>
    </StrictMode>,
  );
} else {
  void resolveHostMode().then((mode) => {
    const here = `${location.pathname}${location.search}${location.hash}`;
    if (mode === "site") {
      // Not one of the public pages: the app's, on the app host.
      if (!frontPage(location.pathname)) return location.replace(appUrl(here));
      notInstallable();
      return loadPage(root, hubPages, ({ SiteApp }) =>
        root.render(
          <StrictMode>
            <SiteApp />
          </StrictMode>,
        ),
      );
    }
    // The browser key move's popup (`features/move`): the pull's sending side on the hub's origin, the push's
    // receiving side on the app host. Nothing else of either starts in it: no notice, no app, no worker.
    // The hub's origin serves it for a pull in any of its modes (`moveOrigins`: the pinned pair, on either of them).
    const move = location.pathname === MOVE_PATH ? moveOrigins() : null;
    if (move) {
      return loadPage(root, hubPages, ({ MovePage }) =>
        root.render(
          <StrictMode>
            <MovePage origins={move} />
          </StrictMode>,
        ),
      );
    }
    if (mode === "legacy") return loadPage(root, hubPages, ({ startLegacy }) => void startLegacy(root, BOOT_URL));
    // The app host: the public pages are the site host's.
    const site = siteOrigin();
    if (site && frontPage(location.pathname)) return location.replace(`${site}${here}`);
    registerSw();
    return loadPage(root, hubPages, ({ CloudApp }) =>
      root.render(
        <StrictMode>
          <QueryClientProvider client={queryClient}>
            <CloudApp>
              <RouterProvider router={router} />
            </CloudApp>
          </QueryClientProvider>
        </StrictMode>,
      ),
    );
  });
}
