// What the two new controls actually draw: the model picker that can be typed
// into, and the amber note under the env block.
//
// The pure rules live in `composer-model.test.ts` and `accounts-model.test.ts`;
// what is pinned here is the markup they turn into, because the bug being
// prevented is visual — a select whose value matches no option renders blank,
// which is how a session that is running a model stops naming one. What the
// list holds once opened (the order, the box at the end) is pinned in a real
// DOM in `select.test.tsx`; here is the closed control, as the server renders it.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import type { components } from "../src/api/schema.js";
import { EnvBlockField } from "../src/features/accounts/env-block-field.js";
import { parseEnvBlock } from "../src/features/accounts/env-block.js";
import { ModelSelect, resolveModel } from "../src/features/sessions/composer-controls.js";
import { makeI18n, pageDraw } from "./render.js";

type ModelInfo = components["schemas"]["ModelInfo"];

// English keys are the source strings, so an empty resource bundle renders the
// same text the untranslated UI does — and asserting on it asserts on the key.
const i18n = makeI18n({
  lng: "en",
  resources: { en: { translation: {} } }
});

const draw = pageDraw(i18n);

function model(id: string): ModelInfo {
  return { id, label: null, efforts: ["low", "medium", "high"], defaultEffort: "medium" };
}

const models = [model("opus"), model("sonnet")];

/** The words on the closed picker's trigger. */
function shown(html: string): string {
  const m = html.match(/role="combobox"[^>]*>(.*?)<\/button>/);
  assert.ok(m, "a combobox trigger");
  return m[1].replace(/<svg.*?<\/svg>/g, "").replace(/<[^>]+>/g, "");
}

test("the picker names the chosen model on a combobox, with no native select left", () => {
  const html = draw(<ModelSelect models={models} value="opus" customId={null} onChange={() => {}} />);
  assert.equal(shown(html), "opus");
  assert.match(html, /aria-label="model"/, "named for a screen reader");
  assert.doesNotMatch(html, /<select|<option/);
  assert.doesNotMatch(html, /· custom/, "and nothing is marked while a listed model is picked");
});

test("a model the list does not carry stays selected, marked, never blank", () => {
  // Exactly what the composer hands the control for a session running an id
  // the CLI never published — a self-hosted GLM, say.
  const { options, active, custom } = resolveModel(models, "GLM-5.3-CYBERSECURITY-FP8", "claude");
  assert.equal(custom, true);
  const html = draw(
    <ModelSelect
      models={options}
      value={active.id}
      customId={active.id}
      onChange={() => {}}
    />,
  );
  assert.equal(shown(html), "GLM-5.3-CYBERSECURITY-FP8 · custom", "the trigger says the model itself");
  assert.match(html, /value="GLM-5.3-CYBERSECURITY-FP8"/, "and it is the value the form holds");
  // The composer's chip has to name it too, or the toolbar reads as empty.
  assert.equal(
    shown(draw(<ModelSelect ghost models={options} value={active.id} customId={active.id} onChange={() => {}} />)),
    "GLM-5.3-CYBERSECURITY-FP8 · custom",
  );
});

test("a form's own leading entry is what an empty value says", () => {
  const html = draw(
    <ModelSelect
      models={models}
      value=""
      customId={null}
      onChange={() => {}}
      leading={{ value: "", label: "CLI default" }}
    />,
  );
  assert.equal(shown(html), "CLI default");
});

test("a saved task on an unlisted model marks it without losing it", () => {
  const html = draw(
    <ModelSelect
      models={models}
      value="GLM-4.6"
      customId="GLM-4.6"
      onChange={() => {}}
      leading={{ value: "", label: "CLI default" }}
    />,
  );
  assert.equal(shown(html), "GLM-4.6 · custom");
});

test("the /v1 base url is amber beside the box, and the box is not red", () => {
  const text = "ANTHROPIC_AUTH_TOKEN=sk-1\nANTHROPIC_BASE_URL=https://glm.example.com/v1";
  const block = parseEnvBlock(text, "claude");
  const html = draw(
    <EnvBlockField
      provider="claude"
      value={text}
      onChange={() => {}}
      issue={block.issue}
      warning={block.warning}
    />,
  );
  assert.match(html, /ANTHROPIC_BASE_URL usually should not end in \/v1/);
  assert.match(html, /text-warn/, "amber, not red");
  assert.doesNotMatch(html, /text-danger|border-danger/, "and nothing about it refuses the block");
});

test("a base url without the extra segment says nothing", () => {
  const text = "ANTHROPIC_BASE_URL=https://glm.example.com";
  const block = parseEnvBlock(text, "claude");
  const html = draw(
    <EnvBlockField
      provider="claude"
      value={text}
      onChange={() => {}}
      issue={block.issue}
      warning={block.warning}
    />,
  );
  assert.doesNotMatch(html, /text-warn|\/v1/);
});
