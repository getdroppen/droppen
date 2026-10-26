// What of cloud mode the app's own views draw (`app/cloud-parts.ts`): nothing
// until the cloud app's module loads, then every slot, with the parts
// themselves — the bar, the two onboarding hints, the copy-window line.
import assert from "node:assert/strict";
import test, { after } from "node:test";
import { installJsdom } from "./jsdom.js";

const { window } = installJsdom();
after(() => {
  (window as unknown as { close(): void }).close();
  setTimeout(() => process.exit(), 50);
});

const { cloudParts } = await import("../src/app/cloud-parts.js");

test("the cloud app fills every slot as it loads, and nothing is there before", async () => {
  assert.equal(cloudParts(), null, "local mode: the views draw none of them");
  await import("../src/features/cloud/cloud-app.js");
  const { CloudBar } = await import("../src/features/cloud/cloud-bar.js");
  const { AddDeviceHint, SideBySideTip } = await import("../src/features/cloud/onboarding-hints.js");
  const { CopyWindowNotice } = await import("../src/features/cloud/plan.js");
  assert.deepEqual(cloudParts(), { CloudBar, SideBySideTip, AddDeviceHint, CopyWindowNotice });
});
