import { useEffect, useMemo } from "react";
import { HubApi } from "@/lib/hub/api";
import { appUrl } from "@/lib/hub/hosts";
import { FrontPageView } from "./front-pages";
import { frontPage, usePathname } from "./route";

/*
 * The page on a hub's site host (`www.droppen.app`, `lib/hub/hosts.ts`
 * `site`): the public pages and nothing else. No key store opens, no service
 * worker registers and no session is read: this browser's device, if any,
 * lives on the app host, so every page here reads as signed out, and every
 * way into the app ("Open app", sign in, sign up) is a link to the app host
 * (`route.ts` `publicLink`). The only requests are the pages' own (the
 * waitlist, the releases, how sign-up works: `GET /v1/signup/config`).
 */
export function SiteApp() {
  const pathname = usePathname();
  const api = useMemo(() => new HubApi(), []);
  const front = frontPage(pathname);
  useEffect(() => {
    if (!front) location.replace(appUrl(`${location.pathname}${location.search}${location.hash}`));
  }, [front]);
  return front ? <FrontPageView page={front} signedIn={false} api={api} /> : null;
}
