/*
 * Everything the app says in ja, as one table: the feature tables under the
 * main one (`ja.ts`), so a key the main table translates keeps that
 * translation. A chunk of its own, downloaded only by a page that shows ja
 * (`index.ts`).
 */
import { cloudDevicesJa } from "./cloud-devices.ja";
import { crossDeviceJa } from "./cross-device.ja";
import { hostsJa } from "./hosts.ja";
import { ja } from "./ja";
import { linkJa } from "./link.ja";
import { moveJa } from "./move.ja";
import { onboardingJa } from "./onboarding.ja";
import { setupJa } from "./setup.ja";
import { signOutJa } from "./sign-out.ja";
import { updatesJa } from "./updates.ja";
import { welcomeJa } from "./welcome.ja";

const translations: Record<string, string> = {
  ...onboardingJa,
  ...crossDeviceJa,
  ...linkJa,
  ...cloudDevicesJa,
  ...signOutJa,
  ...updatesJa,
  ...hostsJa,
  ...moveJa,
  ...setupJa,
  ...welcomeJa,
  ...ja,
};

export default translations;
