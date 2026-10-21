// The product is Droppen. Its old internal name — "remote-hand", and "RH" for
// short — stays in the code until the open-source rename (crates, storage
// keys, `rh/v1` labels, `x-rh-*` headers, `rh-link/*` messages, `RH_*`
// variables), never on screen, in any of the four languages the app ships:
// English (the keys themselves), zh-CN, zh-TW, ja.
//
// Checked: every translation table (the app's, the download page's and the
// landing page's), the page shell (`index.html`) and the web app manifest, and
// every piece of text the interface source puts on screen (the same syntax-tree
// reading as `ui-wording.test.ts`, `screen-text.ts`): on screen, neither the
// name nor the short name and its codes (`RH1-`, `RHP1-`, `RHR1-`); in any
// other string literal, the name.
//
// What still says it is listed below, each with why. They are not branding but
// what a person types or pastes — a command, a path, a code — and they change
// only with the binary and the protocol, together with old installs (the plan:
// docs/local-aggregator.md §7.2). Each entry must still be there: one that is
// gone is taken out of the list, so the list never hides a new one.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { test } from "node:test";
import { landingBundles } from "../src/features/landing/copy.js";
import { securityBundles } from "../src/features/landing/security/security-copy.js";
import { cloudDevicesJa, cloudDevicesZhCN, cloudDevicesZhTW } from "../src/i18n/cloud-devices.js";
import { crossDeviceJa, crossDeviceZhCN, crossDeviceZhTW } from "../src/i18n/cross-device.js";
import { ja } from "../src/i18n/ja.js";
import { linkJa, linkZhCN, linkZhTW } from "../src/i18n/link.js";
import { onboardingJa, onboardingZhCN, onboardingZhTW } from "../src/i18n/onboarding.js";
import { setupJa, setupZhCN, setupZhTW } from "../src/i18n/setup.js";
import { welcomeJa, welcomeZhCN, welcomeZhTW } from "../src/i18n/welcome.js";
import { zhCN } from "../src/i18n/zh-CN.js";
import { zhTW } from "../src/i18n/zh-TW.js";
import { sourceFiles, sourceTexts } from "./screen-text.js";

/** The old name, in any spelling: remote-hand, remote_hand, Remote Hand, RemoteHand. */
const OLD_NAME = /remote[\s._-]?hand/i;
/** The short name and the codes that carry it: RH, rh-notify, RH1-…, RHP1-…, RHR1-…. */
const OLD_SHORT = /\bRH[A-Z]?\d*\b/i;
const says = (text: string, shown: boolean) => OLD_NAME.test(text) || (shown && OLD_SHORT.test(text));

/** What may still say it: a file (under `web/src`, or a table's language), the text, and why. */
interface Allowed {
  file: string;
  text: string;
  why: string;
}

const ALLOWED: Allowed[] = [
  { file: "features/landing/get-droppen.ts", text: "~/.local/share/remote-hand/bin/remote-hand-server", why: "where the installer puts the server, in the command for a device that already runs one; the data directory is never moved (§7.2)" },
  { file: "lib/droppen-command.ts", text: "remote-hand-server", why: "the command for a server too old to report droppenPath, which answers to its old name only (§7.4)" },
  { file: "lib/droppen-command.ts", text: "REMOTE_HAND_DATA_DIR", why: "the variable a second install's commands are shown with, until the rename moves it to DROPPEN_DATA_DIR (rename plan §3)" },
  { file: "lib/droppen-command.ts", text: "REMOTE_HAND_CONFIG_DIR", why: "the variable a second install's commands are shown with, until the rename moves it to DROPPEN_CONFIG_DIR (rename plan §3)" },
  { file: "features/landing/security/security.tsx", text: "rh/v1", why: "the protocol's signature and key-derivation labels, quoted byte for byte so a reader can check them against the code; they change only with the protocol (§7.2)" },
  { file: "features/landing/security/security.tsx", text: "server-rs/crates/rh-", why: "where the protocol, relay and client crates are in the source tree, for a reader checking the code; renamed with the crates (§7.2)" },
];

const TABLES: Record<string, Record<string, string>> = {
  "zh-CN": { ...onboardingZhCN, ...crossDeviceZhCN, ...linkZhCN, ...cloudDevicesZhCN, ...setupZhCN, ...welcomeZhCN, ...zhCN, ...landingBundles["zh-CN"], ...securityBundles["zh-CN"] },
  "zh-TW": { ...onboardingZhTW, ...crossDeviceZhTW, ...linkZhTW, ...cloudDevicesZhTW, ...setupZhTW, ...welcomeZhTW, ...zhTW, ...landingBundles["zh-TW"], ...securityBundles["zh-TW"] },
  ja: { ...onboardingJa, ...crossDeviceJa, ...linkJa, ...cloudDevicesJa, ...setupJa, ...welcomeJa, ...ja, ...landingBundles.ja, ...securityBundles.ja },
};

const used = new Set<Allowed>();
const allowed = (file: string, text: string) => {
  const entry = ALLOWED.find((a) => a.file === file && text.includes(a.text));
  if (entry) used.add(entry);
  return entry !== undefined;
};

/** Each string in `source` that says it, as `line: text`. */
function sayings(file: string, source: string): string[] {
  return sourceTexts(file, source)
    .filter(({ text, shown }) => says(text, shown) && !allowed(file, text))
    .map(({ line, text }) => `${line}: ${text.trim().slice(0, 100)}`);
}

test("no translation says remote-hand or RH, in English (the key) or in its own language", () => {
  const said: string[] = [];
  for (const [lang, table] of Object.entries(TABLES)) {
    for (const [key, value] of Object.entries(table)) {
      // The English is the key, allowed as the source file that renders it is.
      if (says(key, true) && !ALLOWED.some((a) => a.file.includes("/") && key.includes(a.text))) said.push(`en: ${key}`);
      if (says(value, true) && !allowed(lang, value)) said.push(`${lang}: ${value}`);
    }
  }
  assert.deepEqual(said, []);
});

test("no interface source puts remote-hand or RH on screen", () => {
  const src = fileURLToPath(new URL("../src", import.meta.url));
  const files = sourceFiles(src);
  assert.ok(files.length > 100);
  const said: string[] = [];
  for (const file of files) {
    const rel = file.slice(src.length + 1);
    // The tables are checked as tables above.
    if (rel.startsWith("i18n/")) continue;
    for (const s of sayings(rel, readFileSync(file, "utf8"))) said.push(`${rel}:${s}`);
  }
  assert.deepEqual(said, []);
});

test("the page shell and the app manifest name Droppen, never remote-hand", () => {
  for (const file of ["../index.html", "../public/manifest.webmanifest"]) {
    const text = readFileSync(new URL(file, import.meta.url), "utf8");
    assert.doesNotMatch(text, OLD_NAME, file);
    // The boot script's storage keys (`rh.theme`) are code, and comments are not shown.
    const shown = text.replace(/<script[\s\S]*?<\/script>/g, "").replace(/<!--[\s\S]*?-->/g, "");
    assert.doesNotMatch(shown, OLD_SHORT, file);
  }
  const manifest = JSON.parse(readFileSync(new URL("../public/manifest.webmanifest", import.meta.url), "utf8")) as { name: string; short_name: string };
  assert.equal(manifest.name, "Droppen");
  assert.equal(manifest.short_name, "Droppen");
  assert.match(readFileSync(new URL("../index.html", import.meta.url), "utf8"), /<meta name="apple-mobile-web-app-title" content="Droppen" \/>/);
});

test("every exception is still needed", () => {
  // Runs after the scans above (node:test runs a file's tests in order).
  assert.deepEqual(
    ALLOWED.filter((a) => !used.has(a)).map((a) => `${a.file}: ${a.text}`),
    [],
    "an exception nothing says any more: take it out of ALLOWED",
  );
});

test("the scan catches each spelling and each way text reaches the screen, and nothing that is code", () => {
  const hits = (source: string) => sayings("probe.tsx", source).length;
  assert.equal(hits(`const a = <span>remote-hand</span>;`), 1, "JSX text");
  assert.equal(hits(`const a = <span>Remote Hand</span>;`), 1, "with a space");
  assert.equal(hits(`const a = <span>RemoteHand</span>;`), 1, "run together");
  assert.equal(hits(`const a = t("Open remote_hand");`), 1, "a t() key");
  assert.equal(hits(`const a = <b title="RH server" />;`), 1, "RH in a title");
  assert.equal(hits(`const a = <b aria-label="paste the RH1- code" />;`), 1, "an enrollment code");
  assert.equal(hits(`const a = <b placeholder="RHR1-…" />;`), 1, "a relay token");
  assert.equal(hits("const a = <b>{`${n} on rh-notify`}</b>;"), 1, "a template literal in JSX");
  assert.equal(hits(`toast("remote-hand restarted");`), 1, "a toast");
  assert.equal(hits(`const p = "~/.config/remote-hand";`), 1, "the name in any string");
  // Never: comments, identifiers, the short name in code, Droppen itself.
  assert.equal(
    hits(
      '// remote-hand\nconst rh = 1; /* RH */ const k = "rh.auth.token"; const m = { t: "rh-link/ready" }; const l = "rh/v1 page"; const a = <b data-testid="rh-x" className="rh-search-flash">Droppen</b>;',
    ),
    0,
  );
  assert.equal(hits(`const a = <span>rhythm, thRHee</span>;`), 0, "a word that only contains the letters");
});
