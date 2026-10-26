/*
 * Adding a device and seeing devices side by side, without anyone to walk
 * you through it: droppen.app's 「添加可运行 AI 的设备」 guide
 * (`features/cloud/add-computer.tsx`), the hints while the account has one
 * device that runs AI and the one-time side-by-side tip
 * (`features/cloud/onboarding-hints.tsx`), 「我的设备」's first-run card after
 * signing up (`features/cloud/first-host.tsx`), and the local page's first-run
 * card (`features/link/first-run.tsx`).
 * Kept apart from the main tables and merged under them in `lang-*.ts`; keys
 * are the English source strings, as everywhere. The interface says 「设备」 /
 * 裝置 / デバイス, never 「机器」 or 「电脑」 (`test/ui-wording.test.ts`); the
 * buttons it names are named as the app names them (「登录 Droppen」,
 * 「加入这台设备」, 「添加浏览器或手机」).
 */

export { onboardingZhCN } from "./onboarding.zh-CN";
export { onboardingZhTW } from "./onboarding.zh-TW";
export { onboardingJa } from "./onboarding.ja";
