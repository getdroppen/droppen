// The web app's third-party notices (`build/third-party-notices.mjs`): every
// build writes `THIRD_PARTY_NOTICES.txt` beside `index.html`, and every public
// page's footer links it.
//
// What these pin: the allow-list takes permissive licences (and MPL-2.0) and
// refuses copyleft ones, alone or ANDed in; what a build renders is mapped to
// the packages it came from (code, CSS proxies, CSS banners, assets), the
// bundler's helpers to vite, and the app's own code to nothing; every package
// is listed with its licence files in full, sorted, and one without a licence
// or with a refused one fails the build; the footer links the file the build
// writes, in every language.
import * as React from "react";
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import i18next from "i18next";
import { I18nextProvider, initReactI18next } from "react-i18next";
import { renderToStaticMarkup } from "react-dom/server";
import { collect, evaluate, failureText, NOTICES_FILE, recorder, render } from "../build/third-party-notices.mjs";
import { landingBundles } from "../src/features/landing/copy.js";
import { FooterLine, NOTICES_PATH } from "../src/features/landing/page-kit.js";

test("the allow-list: permissive licences and MPL-2.0, never a copyleft one", () => {
  for (const ok of ["MIT", "(MIT OR Apache-2.0)", "Apache-2.0 WITH LLVM-exception", "MPL-2.0", "MIT OR GPL-3.0-only", "BSD-3-Clause AND ISC"])
    assert.equal(evaluate(ok).ok, true, ok);
  for (const no of ["GPL-3.0-only", "AGPL-3.0-only", "MIT AND GPL-2.0-only", "LGPL-2.1-or-later", "SSPL-1.0"]) assert.equal(evaluate(no).ok, false, no);
  assert.deepEqual(evaluate("(MIT OR CC0-1.0) AND Zlib").ids, ["MIT", "CC0-1.0", "Zlib"]);
});

test("what a build renders is mapped to its packages, listed with their licences in full, and a refused licence fails", () => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "notices-test-")));
  try {
    const web = join(root, "web");
    const pkg = (name: string, json: Record<string, unknown>, files: Record<string, string> = {}) => {
      const dir = join(web, "node_modules", name);
      mkdirSync(dir, { recursive: true });
      writeFileSync(join(dir, "package.json"), JSON.stringify({ name, version: "1.0.0", ...json }));
      for (const [file, text] of Object.entries(files)) writeFileSync(join(dir, file), text);
      return dir;
    };
    mkdirSync(web, { recursive: true });
    writeFileSync(join(web, "package.json"), JSON.stringify({ name: "web" }));
    pkg("vite", { license: "MIT" }, { LICENSE: "vite's licence\n" });
    const zed = pkg("zed", { license: "MIT", repository: { url: "git+https://example.com/zed.git" } }, { LICENSE: "zed's licence" });
    const scoped = pkg("@scope/styles", { license: "ISC" }, { "LICENSE.md": "styles' licence", NOTICE: "styles' notice" });
    const banner = pkg("banner-css", { license: "Apache-2.0" });
    const img = pkg("icons", { license: "CC0-1.0" }, { COPYING: "icons' licence" });
    pkg("unused", { license: "GPL-3.0-only" });

    const seen = recorder();
    seen.record({
      "assets/app.js": {
        type: "chunk",
        modules: {
          [join(web, "src/main.tsx")]: { renderedLength: 10 },
          [join(zed, "dist/index.js")]: { renderedLength: 10 },
          [join(zed, "dist/empty.js")]: { renderedLength: 0 },
          [`${join(scoped, "index.css")}?inline`]: { renderedLength: 0 },
          "\0commonjsHelpers.js": { renderedLength: 10 },
        },
      },
      "assets/app.css": { type: "asset", fileName: "assets/app.css", originalFileNames: [], source: "/*! banner-css v2.1.0 | Apache-2.0 */ a{}" },
      "assets/icon.svg": { type: "asset", fileName: "assets/icon.svg", originalFileNames: [join(img, "icon.svg")], source: "<svg/>" },
    } as never);
    const found = collect(web, seen);
    assert.deepEqual(
      found.entries.map((e) => `${e.label} ${e.license} ${e.texts.map((t) => t.name).join(",")}`),
      ["@scope/styles@1.0.0 ISC LICENSE.md,NOTICE", "banner-css@1.0.0 Apache-2.0 ", "icons@1.0.0 CC0-1.0 COPYING", "vite@1.0.0 MIT LICENSE", "zed@1.0.0 MIT LICENSE"],
    );
    assert.deepEqual(found.helpers, ["commonjsHelpers.js"]);
    assert.equal(failureText(found), null);
    assert.deepEqual(found.withoutText, ["banner-css@1.0.0 (Apache-2.0)"]);

    const text = render(found, ["Intro line."]);
    assert.ok(text.startsWith("npm packages bundled into the web app (5)\n\nIntro line.\nBundler runtime helpers (commonjsHelpers.js) are attributed to vite.\n\n"), text);
    assert.ok(text.includes("zed@1.0.0  MIT\nhttps://example.com/zed.git\n\n[LICENSE]\nzed's licence\n"), text);
    assert.ok(text.includes("(The package ships no licence file. Its package.json declares Apache-2.0.)"), text);
    assert.ok(!text.includes("unused"), "a package the build does not render is not listed");

    // A package with a refused licence, or with none, fails.
    const bad = pkg("copyleft", { license: "GPL-3.0-only" });
    const none = pkg("nameless", {});
    seen.record({ "assets/more.js": { type: "chunk", modules: { [join(bad, "a.js")]: { renderedLength: 1 }, [join(none, "a.js")]: { renderedLength: 1 } } } } as never);
    const failed = failureText(collect(web, seen));
    assert.match(failed ?? "", /2 npm package\(s\) bundled into the web app fail the licence check/);
    assert.match(failed ?? "", /copyleft@1\.0\.0: GPL-3\.0-only is not on the allow-list/);
    assert.match(failed ?? "", /nameless@1\.0\.0: no licence declared/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("every public page's footer links the notices the build writes, in every language", () => {
  assert.equal(NOTICES_PATH, `/${NOTICES_FILE}`);
  const i18n = i18next.createInstance();
  void i18n.use(initReactI18next).init({
    lng: "en",
    resources: {
      en: { translation: {} },
      "zh-CN": { translation: landingBundles["zh-CN"] },
      "zh-TW": { translation: landingBundles["zh-TW"] },
      ja: { translation: landingBundles.ja },
    },
    interpolation: { escapeValue: false },
    keySeparator: false,
    nsSeparator: false,
  });
  const words = { en: "Third-party licences", "zh-CN": "第三方许可证", "zh-TW": "第三方授權", ja: "サードパーティのライセンス" };
  for (const [lng, label] of Object.entries(words)) {
    void i18n.changeLanguage(lng);
    const html = renderToStaticMarkup(
      <I18nextProvider i18n={i18n}>
        <FooterLine />
      </I18nextProvider>,
    );
    assert.match(html, new RegExp(`<a href="/THIRD_PARTY_NOTICES\\.txt"[^>]*data-testid="footer-licences"[^>]*>${label}</a>`), lng);
  }
});
