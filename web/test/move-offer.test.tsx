// The app host's way in to the key move (`features/move/bring-it-over.tsx`):
// the sign-in screen's "Used Droppen at droppen.app in this browser? Bring it
// over" — on its sign-in tab only, shown while this origin holds no device, the pull started from its
// click, how each outcome leaves it — and the other tabs of the origin, which
// reload when a device arrives (and only on another page's word).
//
// React renders into a stand-in DOM (`fake-dom.ts`) where effects run.
import * as React from "react";
import assert from "node:assert/strict";
import test, { after } from "node:test";
import { installFakeDom, type FakeNode } from "./fake-dom.js";

Object.assign(globalThis, { location: { origin: "https://my.droppen.test", pathname: "/signin", search: "", hash: "", reload: () => undefined, replace: () => undefined } });
const container = installFakeDom();
after(() => void setTimeout(() => process.exit(), 50));

const { createRoot } = await import("react-dom/client");
const { I18nextProvider } = await import("react-i18next");
const { default: i18n } = await import("../src/i18n/index.js");
const { AuthScreen } = await import("../src/features/cloud/cloud-app.js");
const { BringItOver, afterPull, reloadOnArrival } = await import("../src/features/move/bring-it-over.js");
const { pageTabs } = await import("../src/lib/move/env.js");
type MoveOutcome = import("../src/features/move/index.js").MoveOutcome;
type MoveEvent = import("../src/lib/move/types.js").MoveEvent;

const flush = (ms = 20) => new Promise((resolve) => setTimeout(resolve, ms));
async function until(done: () => boolean, ms = 5_000): Promise<void> {
  const end = Date.now() + ms;
  while (!done()) {
    if (Date.now() > end) throw new Error("timed out waiting");
    await flush(5);
  }
}
function find(node: FakeNode, match: (n: FakeNode) => boolean): FakeNode | null {
  if (match(node)) return node;
  for (const c of node.childNodes) {
    const hit = find(c, match);
    if (hit) return hit;
  }
  return null;
}
const byTestId = (id: string) => find(container, (n) => n.getAttribute?.("data-testid") === id);
const click = (id: string) => {
  const node = byTestId(id);
  assert.ok(node, id);
  const handlers = Object.entries(node as unknown as Record<string, unknown>).find(([k]) => k.startsWith("__reactProps$"))?.[1] as Record<string, (e: unknown) => void>;
  handlers.onClick({});
};

test("the sign-in screen offers to bring the device over, and a pull from its click goes on into the app", async () => {
  await i18n.changeLanguage("en");
  const pulls: { legacy: string }[] = [];
  let answer!: (o: MoveOutcome) => void;
  let moved = 0;
  const pull = async (req: { legacy: string }) => {
    pulls.push(req);
    return new Promise<MoveOutcome>((resolve) => (answer = resolve));
  };
  const api = { signupConfig: async () => ({ signup: "open", inviteRequired: false, turnstile: null, verification: "off", passkeys: null }) };
  const root = createRoot(container as never);
  root.render(
    <I18nextProvider i18n={i18n}>
      <AuthScreen api={api as never} onSignedIn={() => undefined} above={<BringItOver legacy="https://droppen.test" pull={pull as never} onMoved={() => moved++} />} />
    </I18nextProvider>,
  );
  try {
    // No device here: the offer is there, above the form.
    await until(() => byTestId("bring-it-over") !== null && byTestId("auth-email") !== null);
    assert.match((container as FakeNode).textContent, /Used Droppen at droppen\.test in this browser\?/);
    click("bring-it-over");
    assert.deepEqual(pulls, [{ legacy: "https://droppen.test" }], "the pull starts from the click, for the hub's origin");
    answer({ kind: "failed", message: "droppen.test could not sign in" });
    await until(() => byTestId("bring-it-over-error") !== null);
    assert.match((container as FakeNode).textContent, /droppen\.test could not sign in/);
    assert.equal(moved, 0);
    click("bring-it-over");
    answer({ kind: "moved" });
    await until(() => moved === 1);
  } finally {
    root.unmount();
  }
});

test("the offer is on the sign-in tab only: sign-up makes a new account, which has nothing to bring over", async () => {
  await i18n.changeLanguage("en");
  const api = { signupConfig: async () => ({ signup: "open", inviteRequired: false, turnstile: null, verification: "off", passkeys: null }) };
  const pull = async () => ({ kind: "cancelled" }) as MoveOutcome;
  // A deep link's sign-in form (the tabs do not move the address there: this stand-in has no history).
  const place = globalThis.location as { pathname: string };
  const was = place.pathname;
  place.pathname = "/devices";
  const root = createRoot(container as never);
  root.render(
    <I18nextProvider i18n={i18n}>
      <AuthScreen api={api as never} onSignedIn={() => undefined} initialMode="signup" above={<BringItOver legacy="https://droppen.test" pull={pull as never} />} />
    </I18nextProvider>,
  );
  const tab = (label: string) => find(container, (n) => n.getAttribute?.("role") === "tab" && n.textContent === label);
  const press = (node: FakeNode | null) => {
    assert.ok(node);
    const handlers = Object.entries(node as unknown as Record<string, unknown>).find(([k]) => k.startsWith("__reactProps$"))?.[1] as Record<string, (e: unknown) => void>;
    handlers.onClick({});
  };
  try {
    await until(() => byTestId("auth-email") !== null);
    await flush(50);
    assert.equal(byTestId("bring-it-over"), null, "not on sign-up");
    assert.doesNotMatch((container as FakeNode).textContent, /Used Droppen at/);
    // Over to sign-in: there it is.
    press(tab("sign in"));
    await until(() => byTestId("bring-it-over") !== null);
    assert.match((container as FakeNode).textContent, /Used Droppen at droppen\.test in this browser\?/);
    // And back to sign-up: gone again.
    press(tab("sign up"));
    await until(() => byTestId("bring-it-over") === null);
  } finally {
    root.unmount();
    place.pathname = was;
  }
});

test("the offer in every language", async () => {
  for (const lng of ["zh-CN", "zh-TW", "ja"]) {
    await i18n.changeLanguage(lng);
    for (const key of ["Used Droppen at {{legacy}} in this browser?", "Bring it over", "This browser cannot move its keys here."]) {
      assert.notEqual(i18n.t(key, { legacy: "droppen.app" }), key.replace("{{legacy}}", "droppen.app"), `${lng}: ${key}`);
    }
  }
  await i18n.changeLanguage("en");
});

test("what each pull outcome leaves the offer showing", () => {
  const t = (key: string) => key;
  assert.equal(afterPull({ kind: "moved" }, t), null);
  assert.deepEqual(afterPull({ kind: "cancelled" }, t), { kind: "idle" });
  assert.equal(afterPull({ kind: "blocked" }, t)?.kind, "failed");
  assert.equal(afterPull({ kind: "unavailable" }, t)?.kind, "failed");
  assert.deepEqual(afterPull({ kind: "failed", message: "no" }, t), { kind: "failed", message: "no" });
});

test("another tab reloads when a device arrives, unless it has an account open", () => {
  let hear: ((e: MoveEvent) => void) | null = null;
  const watch = (fn: (e: MoveEvent) => void) => {
    hear = fn;
    return () => void (hear = null);
  };
  let open = false;
  let reloads = 0;
  const stop = reloadOnArrival({ isOpen: () => open, reload: () => reloads++, watch: watch as never });
  hear!({ t: "moving" });
  hear!({ t: "moved", to: "https://my.droppen.test" });
  assert.equal(reloads, 0);
  hear!({ t: "arrived", account: "acct-1" });
  assert.equal(reloads, 1);
  open = true;
  hear!({ t: "arrived", account: "acct-1" });
  assert.equal(reloads, 1, "an account open here stays open");
  stop();
  assert.equal(hear, null);
});

test("the tabs channel: another page hears a move, the page that said it does not", async () => {
  const mine = pageTabs()!;
  const theirs = pageTabs()!;
  const heardHere: MoveEvent[] = [];
  const heardThere: MoveEvent[] = [];
  const stopHere = mine.subscribe((e) => heardHere.push(e));
  const stopThere = theirs.subscribe((e) => heardThere.push(e));
  mine.post({ t: "arrived", account: "acct-1" });
  await until(() => heardThere.length === 1);
  await flush(50);
  assert.deepEqual(heardThere, [{ t: "arrived", account: "acct-1" }]);
  assert.deepEqual(heardHere, [], "a page does not hear itself");
  stopHere();
  stopThere();
});
