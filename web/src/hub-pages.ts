/*
 * What a hub's app / migration hosts run (`lib/hub/hosts.ts`): the cloud app,
 * the key move's popup, and the hub origin's move notice. `main.tsx` imports
 * this only in cloud mode, so none of it is in the local app's boot
 * (`build/local-boot.ts`).
 *
 * The public site host (`SiteApp`) is loaded in its own chunk directly from
 * `main.tsx` so `www.droppen.app` does not pull `CloudApp` and the E2E client.
 */
export { CloudApp } from "./features/cloud/cloud-app";
export { MovePage } from "./features/move";
export { startLegacy } from "./features/hosts/legacy";
