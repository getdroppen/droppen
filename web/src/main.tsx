// First, before any other module can read the address: a `#local-open=` token
// comes off it (`lib/local-open.ts`), and so do the browser hand-off's
// fragments — before the router or the service worker reads it.
import "./lib/local-open-boot";
// The address as it loaded (the hand-off's fragment still on it), for the
// move notice on the hub's origin to pass on whole (`lib/hub/boot-url.ts`).
import "./lib/hub/boot-url";
import { takeReturn } from "./features/link/fragment-boot";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider } from "@tanstack/react-router";
import { CloudApp } from "./features/cloud/cloud-app";
import { isReturnPath } from "./features/link/handoff";
import { LinkReturnPage } from "./features/link/link-return";
import { isCloud } from "./lib/hub/config";
import { appUrl, moveOrigins, notInstallable, resolveHostMode, siteOrigin } from "./lib/hub/hosts";
import { MOVE_PATH } from "./lib/move/types";
import { MovePage } from "./features/move";
import { frontPage } from "./features/landing/route";
import { SiteApp } from "./features/landing/site-app";
import { startLegacy } from "./features/hosts/legacy";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { router } from "./app/router";
import { startBadgeSync } from "./lib/badge";
// Side-effect import: the module's `beforeinstallprompt` listener has to be in
// place before the browser fires it, which happens well before React mounts.
import "./lib/install";
import { startKbWatch } from "./lib/kb-inset";
import { kbMode } from "./lib/kb-mode";
import { startKbNative } from "./lib/kb-native";
import { startVvDebug } from "./lib/vv-debug";
import { startLocalOpen } from "./lib/auth";
import { startLocalShell } from "./lib/machines/shells/local";
import { registerSw } from "./lib/sw-register";
import "./i18n";
import "./index.css";

const queryClient = new QueryClient({
  defaultOptions: {
    queries: { staleTime: 5_000, retry: 1 },
  },
});

// The phone keyboard: left to WebKit's own reveal of the focused field, or
// (`?kb=legacy`) measured and the shell sized above it (`lib/kb-mode.ts`).
if (kbMode() === "legacy") startKbWatch();
else startKbNative();
// Off unless asked for (`?vvdebug=1`): the keyboard's numbers, live on the phone.
startVvDebug();
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
const returning = !isCloud() && isReturnPath(location.pathname);
if (!isCloud() && !returning) {
  startLocalShell();
  // A page opened signed in from the device itself trades its token before
  // the router asks who it is (`lib/auth.ts`).
  startLocalOpen();
}
const root = createRoot(document.getElementById("root")!);
if (returning) {
  registerSw();
  root.render(
    <StrictMode>
      <LinkReturnPage answer={takeReturn()} />
    </StrictMode>,
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
      return root.render(
        <StrictMode>
          <SiteApp />
        </StrictMode>,
      );
    }
    // The browser key move's popup (`features/move`): the pull's sending side on the hub's origin, the push's
    // receiving side on the app host. Nothing else of either starts in it: no notice, no app, no worker.
    // The hub's origin serves it for a pull in any of its modes (`moveOrigins`: the pinned pair, on either of them).
    const move = location.pathname === MOVE_PATH ? moveOrigins() : null;
    if (move) {
      return root.render(
        <StrictMode>
          <MovePage origins={move} />
        </StrictMode>,
      );
    }
    if (mode === "legacy") return startLegacy(root);
    // The app host: the public pages are the site host's.
    const site = siteOrigin();
    if (site && frontPage(location.pathname)) return location.replace(`${site}${here}`);
    registerSw();
    root.render(
      <StrictMode>
        <QueryClientProvider client={queryClient}>
          <CloudApp>
            <RouterProvider router={router} />
          </CloudApp>
        </QueryClientProvider>
      </StrictMode>,
    );
  });
}
