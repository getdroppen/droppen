/*
 * Everything the app says in zh-TW, as one table: the feature tables under the
 * main one (`zh-TW.ts`), so a key the main table translates keeps that
 * translation. A chunk of its own, downloaded only by a page that shows zh-TW
 * (`index.ts`).
 */
import { cloudDevicesZhTW } from "./cloud-devices.zh-TW";
import { crossDeviceZhTW } from "./cross-device.zh-TW";
import { hostsZhTW } from "./hosts.zh-TW";
import { zhTW } from "./zh-TW";
import { linkZhTW } from "./link.zh-TW";
import { moveZhTW } from "./move.zh-TW";
import { onboardingZhTW } from "./onboarding.zh-TW";
import { setupZhTW } from "./setup.zh-TW";
import { signOutZhTW } from "./sign-out.zh-TW";
import { updatesZhTW } from "./updates.zh-TW";
import { welcomeZhTW } from "./welcome.zh-TW";

const translations: Record<string, string> = {
  ...onboardingZhTW,
  ...crossDeviceZhTW,
  ...linkZhTW,
  ...cloudDevicesZhTW,
  ...signOutZhTW,
  ...updatesZhTW,
  ...hostsZhTW,
  ...moveZhTW,
  ...setupZhTW,
  ...welcomeZhTW,
  ...zhTW,
};

export default translations;
