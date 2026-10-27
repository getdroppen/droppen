// The app's language chunks (`lang-*.ts`) are grown by spreading the app's
// translation tables in a fixed order, so a key defined by two tables is
// silently resolved by that order: the later table's text wins and the other
// translation is dead weight nobody notices. This test makes the duplication
// impossible: every table has one owner per key, and the tables listed here are
// exactly the ones each language chunk merges, so a new table cannot slip in
// without being added below (and being checked for collisions).
//
// The public pages' copy tables (`features/landing/`) are not here: they are
// added at runtime with overwrite=false and are shared with the landing work,
// not merged by `lang-*.ts`.
import assert from "node:assert/strict";
import { test } from "node:test";
import { cloudDevicesJa, cloudDevicesZhCN, cloudDevicesZhTW } from "../src/i18n/cloud-devices.js";
import { crossDeviceJa, crossDeviceZhCN, crossDeviceZhTW } from "../src/i18n/cross-device.js";
import { hostsJa, hostsZhCN, hostsZhTW } from "../src/i18n/hosts.js";
import { ja } from "../src/i18n/ja.js";
import langJa from "../src/i18n/lang-ja.js";
import langZhCN from "../src/i18n/lang-zh-CN.js";
import langZhTW from "../src/i18n/lang-zh-TW.js";
import { linkJa, linkZhCN, linkZhTW } from "../src/i18n/link.js";
import { moveJa, moveZhCN, moveZhTW } from "../src/i18n/move.js";
import { onboardingJa, onboardingZhCN, onboardingZhTW } from "../src/i18n/onboarding.js";
import { setupJa, setupZhCN, setupZhTW } from "../src/i18n/setup.js";
import { signOutJa, signOutZhCN, signOutZhTW } from "../src/i18n/sign-out.js";
import { updatesJa, updatesZhCN, updatesZhTW } from "../src/i18n/updates.js";
import { welcomeJa, welcomeZhCN, welcomeZhTW } from "../src/i18n/welcome.js";
import { zhCN } from "../src/i18n/zh-CN.js";
import { zhTW } from "../src/i18n/zh-TW.js";

type Table = Record<string, string>;

/** Each language's tables as `lang-*.ts` merges them (link includes link-handoff). */
const TABLES: Record<"zh-CN" | "zh-TW" | "ja", Record<string, Table>> = {
  "zh-CN": {
    onboarding: onboardingZhCN,
    "cross-device": crossDeviceZhCN,
    link: linkZhCN,
    "cloud-devices": cloudDevicesZhCN,
    "sign-out": signOutZhCN,
    updates: updatesZhCN,
    hosts: hostsZhCN,
    move: moveZhCN,
    setup: setupZhCN,
    welcome: welcomeZhCN,
    main: zhCN,
  },
  "zh-TW": {
    onboarding: onboardingZhTW,
    "cross-device": crossDeviceZhTW,
    link: linkZhTW,
    "cloud-devices": cloudDevicesZhTW,
    "sign-out": signOutZhTW,
    updates: updatesZhTW,
    hosts: hostsZhTW,
    move: moveZhTW,
    setup: setupZhTW,
    welcome: welcomeZhTW,
    main: zhTW,
  },
  ja: {
    onboarding: onboardingJa,
    "cross-device": crossDeviceJa,
    link: linkJa,
    "cloud-devices": cloudDevicesJa,
    "sign-out": signOutJa,
    updates: updatesJa,
    hosts: hostsJa,
    move: moveJa,
    setup: setupJa,
    welcome: welcomeJa,
    main: ja,
  },
};

const CHUNKS = { "zh-CN": langZhCN, "zh-TW": langZhTW, ja: langJa } as const;

test("no key is defined by two translation tables", () => {
  const said: string[] = [];
  for (const [lang, tables] of Object.entries(TABLES)) {
    const owner = new Map<string, string>();
    for (const [name, table] of Object.entries(tables)) {
      for (const key of Object.keys(table)) {
        const first = owner.get(key);
        if (first === undefined) owner.set(key, name);
        else said.push(`${lang}: ${JSON.stringify(key)} in ${first} and ${name}`);
      }
    }
  }
  assert.deepEqual(said, []);
});

test("the tables listed are exactly the ones each language chunk merges", () => {
  for (const [lang, tables] of Object.entries(TABLES)) {
    const union = new Set(Object.values(tables).flatMap((table) => Object.keys(table)));
    assert.deepEqual([...union].sort(), Object.keys(CHUNKS[lang as keyof typeof CHUNKS]).sort(), lang);
  }
});

test("every table translates the same keys in all three languages", () => {
  for (const name of Object.keys(TABLES["zh-CN"])) {
    const keys = Object.keys(TABLES["zh-CN"][name]).sort();
    for (const lang of ["zh-TW", "ja"] as const) {
      assert.deepEqual(Object.keys(TABLES[lang][name]).sort(), keys, `${name}: ${lang}`);
    }
  }
});
