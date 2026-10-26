/*
 * Everything the app says in zh-CN, as one table: the feature tables under the
 * main one (`zh-CN.ts`), so a key the main table translates keeps that
 * translation. A chunk of its own, downloaded only by a page that shows zh-CN
 * (`index.ts`).
 */
import { cloudDevicesZhCN } from "./cloud-devices.zh-CN";
import { crossDeviceZhCN } from "./cross-device.zh-CN";
import { hostsZhCN } from "./hosts.zh-CN";
import { zhCN } from "./zh-CN";
import { linkZhCN } from "./link.zh-CN";
import { moveZhCN } from "./move.zh-CN";
import { onboardingZhCN } from "./onboarding.zh-CN";
import { setupZhCN } from "./setup.zh-CN";
import { signOutZhCN } from "./sign-out.zh-CN";
import { updatesZhCN } from "./updates.zh-CN";
import { welcomeZhCN } from "./welcome.zh-CN";

const translations: Record<string, string> = {
  ...onboardingZhCN,
  ...crossDeviceZhCN,
  ...linkZhCN,
  ...cloudDevicesZhCN,
  ...signOutZhCN,
  ...updatesZhCN,
  ...hostsZhCN,
  ...moveZhCN,
  ...setupZhCN,
  ...welcomeZhCN,
  ...zhCN,
};

export default translations;
