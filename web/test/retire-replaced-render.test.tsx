// "The new device d-new says it replaces d-old. Confirm that one can no
// longer be used, and remove it?" (`features/cloud/devices.tsx`
// `RetireReplacedPrompt`), driven in a real DOM: put as the new device's
// claim, with what tells the old one apart (its browser, when it was added
// and last seen, its fingerprint); the ordinary removal runs only on the
// person's tap, once, and says what it rotated as the list's removal does —
// alarms included; "not now" leaves the device alone; a removal that fails
// says why and can be tried again.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test, { after, afterEach } from "node:test";
import { installJsdom } from "./jsdom.js";

const { window, container } = installJsdom();
after(() => {
  (window as unknown as { close(): void }).close();
  setTimeout(() => process.exit(), 50);
});

const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { I18nextProvider } = await import("react-i18next");
const { default: i18n } = await import("./i18n-all.js");
const { RetireReplacedPrompt } = await import("../src/features/cloud/devices.js");
type Report = Awaited<ReturnType<React.ComponentProps<typeof RetireReplacedPrompt>["onRemove"]>>;

type Root = ReturnType<typeof createRoot>;
let root: Root | null = null;
afterEach(async () => {
  await act(async () => root?.unmount());
  root = null;
  await i18n.changeLanguage("en");
});

async function render(node: React.ReactNode): Promise<void> {
  root = createRoot(container);
  await act(async () => root!.render(<I18nextProvider i18n={i18n}>{node}</I18nextProvider>));
}

const byTestId = (id: string) => container.querySelector<HTMLElement>(`[data-testid="${id}"]`);
const click = (el: HTMLElement | null) =>
  act(async () => {
    assert.ok(el, "the control is on the page");
    el.dispatchEvent(new window.MouseEvent("click", { bubbles: true, cancelable: true }));
  });

const offer = { id: "d-old", label: "iPhone · Chrome", addedMs: Date.now() - 3 * 86_400_000, lastSeenMs: Date.now() - 3 * 3_600_000 };
/** What the ordinary removal reports: a new user key sent, and a machine whose new key the keyring did not take (an alarm). */
const report: Report = {
  ukEpoch: 2,
  sealedTo: ["d-new"],
  failedTo: [],
  failedWhy: {},
  recovery: false,
  machines: [{ machine: "m-1", mkEpoch: 3, result: { via: "channel" }, keyring: "keyring_full" }],
} as Report;

test("the claim is the new device's, with what tells the old one apart, and it is removed only on the person's tap, once", async () => {
  await i18n.changeLanguage("zh-CN");
  let removed = 0;
  await render(<RetireReplacedPrompt added="d-new" offer={offer} fingerprint="ABCD-EFGH-IJKL-MNOP" onRemove={async () => (removed++, report)} />);
  assert.equal(removed, 0, "nothing on its own");
  const prompt = byTestId("retire-replaced");
  assert.ok(prompt);
  assert.equal(prompt.getAttribute("role"), "alertdialog");
  assert.match(prompt.textContent ?? "", /新设备 d-new 说它替换了 d-old。确认这台已经用不了了，顺手移除？/, "the new device's claim, not a fact");
  const facts = byTestId("retire-replaced-facts")?.textContent ?? "";
  assert.match(facts, /iPhone · Chrome/);
  assert.match(facts, /添加于 /);
  assert.match(facts, /最近在线 /);
  assert.match(prompt.textContent ?? "", /ABCD/, "its fingerprint");
  await click(byTestId("retire-replaced-remove"));
  assert.equal(removed, 1);
  const done = byTestId("retire-replaced-done");
  assert.ok(done);
  assert.match(done.textContent ?? "", /已移除 d-old/);
  assert.equal(byTestId("retire-replaced-remove"), null, "and asked no more");
  // What the removal rotated, as the list's removal says it — the keyring alarm loudly.
  const alarm = done.querySelector('[data-signal="alarm"]');
  assert.ok(alarm, "the alarm is shown");
  assert.match(alarm.textContent ?? "", /m-1/);
  assert.ok(done.querySelectorAll("span").length > 2, "every line of the report");
});

test("“not now” leaves the old device alone; a removal that fails says why and can be tried again", async () => {
  await i18n.changeLanguage("en");
  let removed = 0;
  await render(<RetireReplacedPrompt added="d-new" offer={offer} fingerprint={null} onRemove={async () => (removed++, report)} />);
  const notNow = [...container.querySelectorAll("button")].find((b) => b.textContent === "not now") as HTMLElement;
  await click(notNow);
  assert.equal(byTestId("retire-replaced"), null);
  assert.equal(removed, 0);
  await act(async () => root?.unmount());

  let tries = 0;
  root = null;
  await render(
    <RetireReplacedPrompt
      added="d-new"
      offer={offer}
      fingerprint={null}
      onRemove={async () => {
        tries++;
        if (tries === 1) throw new Error("the hub said no");
        return report;
      }}
    />,
  );
  await click(byTestId("retire-replaced-remove"));
  assert.match(container.textContent ?? "", /the hub said no/);
  assert.ok(byTestId("retire-replaced-remove"), "still offered");
  await click(byTestId("retire-replaced-remove"));
  assert.equal(tries, 2);
  assert.ok(byTestId("retire-replaced-done"));
});
