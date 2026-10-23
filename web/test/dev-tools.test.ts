// Developer pages are no part of a build (`build/dev-tools.ts`): the
// direct-path probe page used to sit in `public/`, so droppen.app served it
// from the app's own origin. Now `vite dev` alone serves it.
import assert from "node:assert/strict";
import { existsSync, readdirSync } from "node:fs";
import test from "node:test";
import { devTools } from "../build/dev-tools.js";

test("the probe page is not in public/ (copied into every build), only in dev/, served by the dev server alone", () => {
  const pub = readdirSync(new URL("../public/", import.meta.url));
  assert.deepEqual(pub.filter((f) => f.startsWith("p2p-probe")), []);
  for (const f of ["p2p-probe.html", "p2p-probe.js"]) assert.ok(existsSync(new URL(`../dev/${f}`, import.meta.url)), f);
  const plugin = devTools("/nowhere");
  assert.equal(plugin.apply, "serve", "never applied to a build");
});
