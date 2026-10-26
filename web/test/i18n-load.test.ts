// Each language's translations come as a chunk of their own (`i18n/load.ts`,
// `i18n/lang-*.ts`): the page waits for its language's before its first frame
// and before it changes language, an English page fetches none, and a chunk
// that does not load leaves the page in English (logged), never blank. No
// module the page boots with imports a translation table.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { relative } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import i18next from "i18next";
import { languageLoader, rootAfter, type LanguageChunk } from "../src/i18n/load.js";
import { sourceFiles } from "./screen-text.js";

/** A fresh i18next, as `i18n/index.ts` starts one: no tables, in `lng`. */
async function fresh(lng: string) {
  const i18n = i18next.createInstance();
  await i18n.init({ resources: {}, lng, fallbackLng: false, keySeparator: false, nsSeparator: false, interpolation: { escapeValue: false } });
  return i18n;
}

/** A chunk that loads when the test says so, and counts how often it was asked for. */
function chunk(tables: Record<string, string>) {
  let release!: () => void;
  let fail!: (err: Error) => void;
  const asked = { count: 0 };
  const load: LanguageChunk = () => {
    asked.count++;
    return new Promise((resolve, reject) => {
      release = () => resolve({ default: tables });
      fail = reject;
    });
  };
  return { load, asked, release: () => release(), fail: (err: Error) => fail(err) };
}

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

test("the first frame waits for the page's language, and is drawn in it", async () => {
  const i18n = await fresh("zh-CN");
  const zh = chunk({ settings: "设置" });
  const loader = languageLoader(i18n, { "zh-CN": zh.load });
  void loader.show("zh-CN");
  const drawn: string[] = [];
  const root = rootAfter({ render: (node) => void drawn.push(`${String(node)}: ${i18n.t("settings")}`), unmount: () => void drawn.push("unmounted") }, loader.shown());
  root.render("first");
  root.render("second");
  await tick();
  assert.deepEqual(drawn, [], "nothing on screen while the zh-CN chunk is on its way");
  zh.release();
  await loader.shown();
  await tick();
  assert.deepEqual(drawn, ["first: 设置", "second: 设置"], "drawn in zh-CN from the first frame, in order");
  root.unmount();
  await tick();
  assert.deepEqual(drawn.at(-1), "unmounted");
});

test("an English page fetches no translations and draws at once", async () => {
  const i18n = await fresh("en");
  const zh = chunk({});
  const ja = chunk({});
  const loader = languageLoader(i18n, { "zh-CN": zh.load, ja: ja.load });
  await loader.show("en");
  assert.equal(await loader.load("en"), true);
  assert.equal(zh.asked.count + ja.asked.count, 0);
  const drawn: string[] = [];
  rootAfter({ render: () => void drawn.push("drawn"), unmount: () => undefined }, loader.shown()).render(null);
  await tick();
  assert.deepEqual(drawn, ["drawn"]);
});

test("switching language fetches its chunk first: the page stays in its language until the words are in", async () => {
  const i18n = await fresh("en");
  const ja = chunk({ settings: "設定" });
  const loader = languageLoader(i18n, { ja: ja.load });
  const changes: string[] = [];
  i18n.on("languageChanged", (lng) => void changes.push(lng));
  const switching = loader.show("ja");
  await tick();
  assert.equal(ja.asked.count, 1);
  assert.equal(i18n.language, "en");
  assert.equal(i18n.hasResourceBundle("ja", "translation"), false);
  ja.release();
  await switching;
  assert.equal(i18n.language, "ja");
  assert.equal(i18n.t("settings"), "設定");
  assert.deepEqual(changes, ["ja"], "one change, straight to a translated ja");
  // Fetched once: back to English and to ja again, nothing is asked for.
  await loader.show("en");
  await loader.show("ja");
  assert.equal(await loader.load("ja"), true);
  assert.equal(ja.asked.count, 1);
  assert.equal(i18n.language, "ja");
});

test("only the language asked for last is shown, whichever chunk comes in first", async () => {
  for (const order of [["ja", "zh-CN"], ["zh-CN", "ja"]] as const) {
    const i18n = await fresh("en");
    const chunks = { ja: chunk({ settings: "設定" }), "zh-CN": chunk({ settings: "设置" }) };
    const loader = languageLoader(i18n, { ja: chunks.ja.load, "zh-CN": chunks["zh-CN"].load });
    const first = loader.show("ja");
    const last = loader.show("zh-CN");
    assert.equal(loader.shown(), last);
    for (const lang of order) chunks[lang].release();
    await Promise.all([first, last]);
    assert.equal(i18n.language, "zh-CN", order.join(" then "));
    assert.equal(i18n.t("settings"), "设置");
  }
});

test("a chunk that does not load leaves the page in English, logged, and is fetched again next time", async (t) => {
  const logged = t.mock.method(console, "error", () => undefined);
  const i18n = await fresh("ja");
  const ja = chunk({ settings: "設定" });
  const loader = languageLoader(i18n, { ja: ja.load });
  const booting = loader.show("ja");
  const drawn: string[] = [];
  rootAfter({ render: () => void drawn.push(i18n.language), unmount: () => undefined }, loader.shown()).render(null);
  ja.fail(new Error("Failed to fetch dynamically imported module"));
  await booting;
  await tick();
  assert.equal(i18n.language, "en");
  assert.deepEqual(drawn, ["en"], "the page is drawn, in English: never blank");
  assert.equal(logged.mock.callCount(), 1);
  assert.match(String(logged.mock.calls[0]!.arguments[0]), /ja translations did not load/);
  assert.match(String(logged.mock.calls[0]!.arguments[1]), /Failed to fetch/);

  // Asked again (the language menu, the server's pref): fetched again, and shown once it loads.
  const again = loader.show("ja");
  assert.equal(ja.asked.count, 2);
  ja.release();
  await again;
  assert.equal(i18n.language, "ja");
  assert.equal(i18n.t("settings"), "設定");
});

test("a language switch whose chunk does not load lands on English", async (t) => {
  t.mock.method(console, "error", () => undefined);
  const i18n = await fresh("en");
  const zh = chunk({ settings: "设置" });
  const ja = chunk({ settings: "設定" });
  const loader = languageLoader(i18n, { "zh-CN": zh.load, ja: ja.load });
  const toZh = loader.show("zh-CN");
  zh.release();
  await toZh;
  assert.equal(i18n.language, "zh-CN");
  const toJa = loader.show("ja");
  ja.fail(new Error("offline"));
  await toJa;
  assert.equal(i18n.language, "en");
  assert.equal(i18n.t("settings"), "settings");
});

test("the app's tables win over what a page added to the language before them, and leave what only it has", async () => {
  const i18n = await fresh("en");
  // As the public pages add theirs (`features/landing`): never overwriting.
  i18n.addResourceBundle("ja", "translation", { settings: "せってい", "only the page's": "ページだけ" }, true, false);
  const ja = chunk({ settings: "設定" });
  const loader = languageLoader(i18n, { ja: ja.load });
  const showing = loader.show("ja");
  await tick();
  assert.equal(ja.asked.count, 1, "fetched although the language has a table already");
  assert.equal(i18n.language, "en");
  ja.release();
  await showing;
  assert.equal(i18n.t("settings"), "設定");
  assert.equal(i18n.t("only the page's"), "ページだけ");
});

test("no module the page boots with imports a translation table: each language comes as its own chunk", () => {
  const src = fileURLToPath(new URL("../src/", import.meta.url));
  const imports = (file: string) => [...readFileSync(file, "utf8").matchAll(/\bfrom\s+["']([^"']+)["']|\bimport\s*\(\s*["']([^"']+)["']\s*\)|^\s*import\s+["']([^"']+)["']/gm)].map((m) => ({ spec: (m[1] ?? m[2] ?? m[3])!, lazy: m[2] !== undefined }));
  const wrong: string[] = [];
  for (const file of sourceFiles(src)) {
    const at = relative(src, file);
    for (const { spec, lazy } of imports(file)) {
      if (at.startsWith("i18n/")) {
        // The module itself: i18next, the loader and the matcher, and each language by `import()` only.
        if (at === "i18n/index.ts" && spec.startsWith("./") && !["./load", "./match"].includes(spec) && !(lazy && /^\.\/lang-[\w-]+$/.test(spec))) wrong.push(`${at}: ${spec}`);
        continue;
      }
      // Elsewhere: the module (`@/i18n`) and the matcher, never a table.
      if (/^@\/i18n\//.test(spec) && spec !== "@/i18n/match") wrong.push(`${at}: ${spec}`);
      if (/(^|\/)i18n\/(?!index$|match$)[\w.-]+$/.test(spec) && !spec.startsWith("@/")) wrong.push(`${at}: ${spec}`);
    }
  }
  assert.deepEqual(wrong, []);
});
