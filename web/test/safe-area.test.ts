// The page asks to be drawn edge to edge (`viewport-fit=cover`), so the shell
// must keep its own content out of the safe-area insets: the status bar, the
// notch and the home indicator.
//
// The top inset is the one that bit. With the "default" status bar style
// (`index.html`) iOS 26 lays a home-screen web app out below the status bar
// and reports a top inset of 0, and the shell padded only its sides and foot.
// Newer iOS releases open the same installed app under the status bar and
// report its height as the inset, and the shell's first line sat under the
// clock. The body now pads all of it; a layer fixed over the whole window is
// outside that padding and pads itself.
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

const read = (path: string) => readFileSync(new URL(path, import.meta.url), "utf8");
const css = read("../src/index.css").replace(/\/\*[\s\S]*?\*\//g, "");
const rule = (selector: string) => {
  const m = css.match(new RegExp(`(?:^|\\})\\s*${selector.replace(/[.]/g, "\\.")}\\s*\\{([^}]*)\\}`));
  assert.ok(m, `a ${selector} rule`);
  return m[1]!;
};

test("the page is drawn edge to edge and never asks for the deprecated translucent status bar", () => {
  const html = read("../index.html").replace(/<!--[\s\S]*?-->/g, "");
  assert.match(html, /name="viewport"\s+content="[^"]*viewport-fit=cover/);
  assert.match(html, /<meta name="apple-mobile-web-app-status-bar-style" content="default" \/>/);
  assert.doesNotMatch(html, /black-translucent/);
});

test("the body pads the shell out of the top, left and right insets; the bottom-most chrome pads the bottom", () => {
  const body = rule("body");
  for (const side of ["top", "left", "right"]) {
    assert.match(body, new RegExp(`padding-${side}:\\s*env\\(safe-area-inset-${side},\\s*0px\\)`), side);
  }
  assert.match(rule(".pb-safe"), /padding-bottom:\s*env\(safe-area-inset-bottom,\s*0px\)/);
  assert.match(rule(".pt-safe"), /padding-top:\s*env\(safe-area-inset-top,\s*0px\)/);
  assert.match(rule(".px-safe"), /padding-left:\s*env\(safe-area-inset-left,\s*0px\);\s*padding-right:\s*env\(safe-area-inset-right,\s*0px\)/);
});

test("every layer fixed over the whole window, other than a backdrop, pads itself out of the insets", () => {
  const root = fileURLToPath(new URL("../src/", import.meta.url));
  const layers: string[] = [];
  for (const file of readdirSync(root, { recursive: true }) as string[]) {
    if (!file.endsWith(".tsx")) continue;
    for (const line of readFileSync(join(root, file), "utf8").split("\n")) {
      if (/Backdrop/.test(line)) continue;
      for (const [, classes] of line.matchAll(/className="([^"]*)"/g)) {
        const list = classes!.split(/\s+/);
        if (!list.includes("fixed") || !list.includes("inset-0")) continue;
        layers.push(file);
        assert.ok(list.includes("pt-safe") && list.includes("px-safe"), `${file}: "${classes}" is not padded out of the insets`);
      }
    }
  }
  // The scan still finds them: the recovery-words pages and the image lightbox.
  assert.ok(layers.length >= 3, `found ${layers.join(", ")}`);
});

test("a dialog stays below the top inset: centred in what is left, and no taller than that", () => {
  const dialog = read("../src/components/ui/dialog.tsx");
  assert.match(dialog, /top-\[calc\(50%\+env\(safe-area-inset-top,0px\)\/2\)\]/);
  assert.match(dialog, /max-h-\[calc\(100dvh-2rem-env\(safe-area-inset-top,0px\)\)\]/);
});
