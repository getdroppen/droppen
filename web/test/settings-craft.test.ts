// The settings page's craft pass (getdroppen/droppen#133): one skeleton for
// every section, one control vocabulary, no accent tints and no `bg-raised`
// chips. The measurements themselves live in `features/settings/layout.tsx`
// (the section/row components) and the shared field and button; this pins that
// the sections keep using them and that the old ad-hoc classes do not come back.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import test from "node:test";

const src = fileURLToPath(new URL("../src", import.meta.url));
const read = (rel: string) => readFileSync(`${src}/${rel}`, "utf8");

const SETTINGS = [
  "features/settings/settings-view.tsx",
  "features/settings/accounts-section.tsx",
  "features/settings/schedule-section.tsx",
  "features/settings/security-section.tsx",
  "features/settings/account-wizards.tsx",
  "features/settings/limit-events-panel.tsx",
  "features/settings/desktop-app-section.tsx",
  "features/setup/settings-section.tsx",
  "features/link/link-settings.tsx",
];

test("the settings skeleton is one shared set of measurements", () => {
  const layout = read("features/settings/layout.tsx");
  assert.match(layout, /max-w-3xl/, "one 48rem content column");
  assert.match(layout, /grid-cols-\[16rem_14rem\]/, "label 16rem / control 14rem");
  assert.match(layout, /gap-4/, "16px between them");
  assert.match(layout, /min-h-14/, "56px rows with a hint");
  const title = read("components/ui/section-title.tsx");
  assert.match(title, /mb-2/, "8px under a section title");
  assert.match(title, /px-3/, "sections at 12px");
  const field = read("components/ui/input.tsx");
  assert.match(field, /h-8/, "32px fields");
  assert.match(field, /max-md:h-10/, "40px fields on a phone");
  assert.match(field, /max-md:text-base/, "16px fields on a phone");
  const button = read("components/ui/button.tsx");
  assert.match(button, /h-7/, "28px actions");
  assert.match(button, /text-xs/, "sans 12");
  assert.match(button, /font-medium/, "500 weight");
  assert.doesNotMatch(button, /font-mono/, "not hand-written mono 11");
});

test("the page title and a section title start on the same 12px edge", () => {
  const view = read("features/settings/settings-view.tsx");
  assert.match(view, /<header[^>]*px-3/, "the page title at 12px");
  assert.match(read("components/ui/section-title.tsx"), /px-3/, "sections at 12px");
});

test("four or more options are a select: font size and the five languages are not segmented", () => {
  const view = read("features/settings/settings-view.tsx");
  assert.match(view, /import \{ SelectField \} from "@\/components\/ui\/select"/);
  assert.match(view, /aria-label=\{t\("font size"\)\}[\s\S]{0,400}value: "xlarge"/);
  assert.match(view, /aria-label=\{t\("language"\)\}[\s\S]{0,400}label: "日本語"/);
  assert.doesNotMatch(view, /w-56|w-72/);
});

test("no settings section keeps an accent tint or a raised chip", () => {
  for (const file of SETTINGS) {
    const source = read(file);
    assert.doesNotMatch(source, /hover:bg-accent\//, `${file}: accent tint on hover`);
    assert.doesNotMatch(source, /bg-accent\/(10|15)/, `${file}: accent tint`);
    // `bg-raised` is only a well (a `pre` or a code line) or a disabled field.
    for (const line of source.split("\n")) {
      if (!/bg-raised/.test(line)) continue;
      assert.ok(/<(pre|code)|disabled:bg-raised|const command/.test(line), `${file}: raised chip: ${line.trim()}`);
    }
  }
});

test("focus is never disabled: fields and pickers do not opt out of the ink ring", () => {
  for (const file of ["components/ui/input.tsx", "components/ui/combobox.tsx", "components/ui/select.tsx", "components/ui/tabs.tsx", "features/machines/device-picker.tsx"]) {
    const source = read(file);
    assert.doesNotMatch(source, /focus:outline-none/, `${file}: focus ring removed`);
  }
  assert.doesNotMatch(read("components/ui/tabs.tsx"), /-outline-offset/, "no ring drawn inside the ink fill");
});
