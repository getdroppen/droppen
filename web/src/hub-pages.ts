/*
 * What a page a hub serves runs, whichever of its hosts it is on
 * (`lib/hub/hosts.ts`): the app, the site host's public pages, the key move's
 * popup, the hub origin's move notice. `main.tsx` imports this only in cloud
 * mode, so none of it is in the local app's boot (`build/local-boot.ts`).
 *
 * One chunk for the four rather than a chunk each: they share most of the hub
 * client, and split along those lines the same code is spread over some
 * twenty files that compress worse, which a page a hub serves would pay for
 * at every load.
 */
export { CloudApp } from "./features/cloud/cloud-app";
export { SiteApp } from "./features/landing/site-app";
export { MovePage } from "./features/move";
export { startLegacy } from "./features/hosts/legacy";
