// The rotation notice closes itself: its close button counts down while the
// page is on screen, stands still while the pointer or focus is on it, and
// reaching zero dismisses it as a click would. The tick rule is pinned here;
// what is drawn is pinned on the first frame, which is all static markup sees.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { I18nextProvider } from "react-i18next";
import { renderToStaticMarkup } from "react-dom/server";
import { countdownTick, ROTATION_NOTICE_SECONDS, type SessionRotation } from "../src/features/accounts/session-rotation.js";
import { RotationNotice } from "../src/features/sessions/rotation-notice.js";
import { makeI18n } from "./render.js";

const i18n = makeI18n({
  lng: "en",
  resources: { en: { translation: {} } }
});

function rotation(over: Partial<SessionRotation> = {}): SessionRotation {
  return {
    from: { id: "acct-a", label: "xin@jp" },
    to: null,
    at: new Date(Date.now() - 60_000).toISOString(),
    class: "session",
    scope: "all",
    untilAt: new Date(Date.now() + 3 * 3600_000).toISOString(),
    interruptedTasks: 0,
    outcome: "no_candidate",
    trigger: "limit",
    usedPercent: null,
    ...over,
  };
}

test("the countdown ticks down to zero only while the page is shown and the button is not held", () => {
  assert.equal(countdownTick(15, { visible: true, held: false }), 14);
  assert.equal(countdownTick(1, { visible: true, held: false }), 0);
  assert.equal(countdownTick(0, { visible: true, held: false }), 0, "never below zero");
  assert.equal(countdownTick(9, { visible: false, held: false }), 9, "a hidden page does not count: nobody saw it");
  assert.equal(countdownTick(9, { visible: true, held: true }), 9, "pointer or focus on the button holds it");
});

test("the notice's close button starts at the full countdown and says it closes by itself", () => {
  const html = renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <RotationNotice source="claude" id="s-1" rotation={rotation()} />
    </I18nextProvider>,
  );
  assert.match(html, /xin@jp ran out and no account is free to switch to/);
  assert.match(html, new RegExp(`<span data-countdown="true">${ROTATION_NOTICE_SECONDS}</span>`));
  assert.match(html, new RegExp(`title="closes by itself in ${ROTATION_NOTICE_SECONDS} s"`));
  assert.match(html, /aria-label="dismiss"/, "still named for what clicking it does");
});
