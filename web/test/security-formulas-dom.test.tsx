// The formulas of droppen.app/security in a real DOM, driven the way a person
// does (`features/landing/security/formula.tsx`).
//
// What is pinned: the page hydrates over its server markup without a
// mismatch; focusing a term from the keyboard lights the same value in every
// formula, inks where it is defined and outlines the calls that make it and
// take it, and shows its card; Enter pins the card, another term's card can
// sit beside it, and Esc unpins; a mouse shows a card while it points and a
// click pins it, a tap pins (a touch never "hovers"), and a press elsewhere
// unpins; "view as" hides exactly what that party cannot know, on every
// block, and moves with the arrow keys; a hidden value keeps its text, for
// copying and for screen readers, which are told it is hidden.
import * as React from "react";
import assert from "node:assert/strict";
import test, { after } from "node:test";
import { installJsdom } from "./jsdom.js";

const { window, document, container } = installJsdom();
after(() => {
  (window as unknown as { close(): void }).close();
  setTimeout(() => process.exit(), 50);
});
// The page's header follows the address with the bare `addEventListener` (`route.ts`).
Object.assign(globalThis, {
  location: window.location,
  addEventListener: window.addEventListener.bind(window),
  removeEventListener: window.removeEventListener.bind(window),
});

const { act } = await import("react");
const { hydrateRoot } = await import("react-dom/client");
const { renderToString } = await import("react-dom/server");
const i18next = (await import("i18next")).default;
const { I18nextProvider, initReactI18next } = await import("react-i18next");
const { default: SecurityPage } = await import("../src/features/landing/security/security.js");
const { BLOCKS, TERMS } = await import("../src/features/landing/security/formula-data.js");

const i18n = i18next.createInstance();
await i18n.use(initReactI18next).init({ lng: "en", resources: { en: { translation: {} } }, interpolation: { escapeValue: false }, keySeparator: false, nsSeparator: false });

const page = (
  <I18nextProvider i18n={i18n}>
    <SecurityPage />
  </I18nextProvider>
);

// Hydrate over the server's markup, as a prerendered page would be: any mismatch is an error here.
const errors: unknown[] = [];
container.innerHTML = renderToString(page);
await act(async () => {
  hydrateRoot(container, page, { onRecoverableError: (e) => errors.push(e) });
});
await act(async () => void (await new Promise((r) => setTimeout(r, 10))));

const term = (block: string, id: string, n = 0) => {
  const el = container.querySelectorAll<HTMLElement>(`[data-formula="${block}"] [data-term="${id}"]`)[n];
  assert.ok(el, `${block}: ${id} #${n}`);
  return el;
};
const named = (block: string, line: number, text: string) => {
  const code = [...container.querySelectorAll<HTMLElement>(`[data-formula="${block}"] code`)][line]!;
  const el = [...code.querySelectorAll<HTMLElement>("[data-occ]")].find((e) => e.textContent === text);
  assert.ok(el, `${block}:${line} ${text}`);
  return el;
};
const cards = () => [...document.querySelectorAll<HTMLElement>("[data-formula-popup]")];
const lit = (el: HTMLElement) => el.getAttribute("data-lit");
const key = (el: Element, k: string) => act(async () => void el.dispatchEvent(new window.KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true })));
const pointer = (el: Element, type: string, pointerType: string) =>
  act(async () => void el.dispatchEvent(new window.PointerEvent(type, { bubbles: true, cancelable: true, pointerType })));
const click = (el: Element) => act(async () => void el.dispatchEvent(new window.MouseEvent("click", { bubbles: true, cancelable: true, button: 0 })));
const focus = (el: HTMLElement) => act(async () => el.focus());
const blur = (el: HTMLElement) => act(async () => el.blur());

test("the page hydrates over its server markup, unchanged", () => {
  assert.deepEqual(errors, []);
  assert.ok(container.querySelectorAll("[data-occ]").length > 300);
});

test("focus from the keyboard: the same value lights everywhere, its definition is inked, its flow outlined, and its card shows", async () => {
  const use = term("seal", "seal.kek", 1);
  await focus(use);
  assert.equal(lit(term("seal", "seal.kek", 0)), "def", "where kek is defined");
  assert.equal(lit(use), "same");
  assert.equal(lit(named("seal", 0, "HKDF-SHA256")), "flow", "made by HKDF-SHA256");
  assert.equal(lit(named("seal", 1, "AES-256-GCM")), "flow", "taken by AES-256-GCM");
  assert.equal(lit(named("stream", 0, "HKDF-SHA256")), null, "another HKDF is not this value's");
  const [card] = cards();
  assert.ok(card, "a card");
  assert.ok(card.textContent!.includes(TERMS["seal.kek"]!.says[0]), "what it is");
  assert.ok(card.textContent!.includes("HKDF-SHA256 → kek → AES-256-GCM"), "where it comes from and goes");
  assert.equal(card.getAttribute("aria-hidden"), "true", "the description is the screen reader's");
  assert.equal(use.getAttribute("aria-describedby"), "fx-seal-kek");
  assert.ok(document.getElementById("fx-seal-kek")!.textContent!.includes("Made by HKDF-SHA256. Used by AES-256-GCM."));

  // The account id: every formula that has it.
  await focus(term("noise", "id.account"));
  const lit_ = [...container.querySelectorAll<HTMLElement>('[data-term="id.account"]')].map(lit);
  assert.ok(lit_.length === 10 && lit_.every((l) => l === "same"), "the account id, everywhere");
  assert.equal(lit(term("seal", "seal.kek", 1)), null, "kek no longer");
  await blur(term("noise", "id.account"));
  assert.equal(cards().length, 0);
});

test("Enter pins a card; another term's card sits beside it; Esc unpins", async () => {
  const kek = term("seal", "seal.kek", 0);
  await focus(kek);
  await key(kek, "Enter");
  assert.equal(kek.getAttribute("aria-pressed"), "true");
  assert.equal(cards().length, 1, "the pinned card stands for the focused term too");
  assert.ok(cards()[0]!.hasAttribute("data-pinned"));
  const r = term("seal", "seal.R");
  await focus(r);
  assert.equal(cards().length, 2, "pinned kek beside passing R");
  assert.equal(lit(kek), "def", "the pinned value stays lit");
  assert.equal(lit(r), "same");
  await key(r, "Escape");
  assert.equal(kek.getAttribute("aria-pressed"), "false");
  assert.equal(cards().length, 0, "unpinned, and the passing card put away");
  await blur(r);
});

test("a mouse shows a card while it points and pins it with a click; a tap pins and never hovers; a press elsewhere unpins", async () => {
  const ctx = term("seal", "seal.ctx", 0);
  await pointer(ctx, "pointerover", "mouse");
  assert.equal(cards().length, 1, "pointing");
  await pointer(ctx, "pointerout", "mouse");
  assert.equal(cards().length, 0, "pointing away");
  // A click focuses the term too, but only pins it: no second, passing card.
  await pointer(ctx, "pointerdown", "mouse");
  await focus(ctx);
  assert.equal(cards().length, 0, "focused by a click: no passing card");
  await click(ctx);
  assert.equal(ctx.getAttribute("aria-pressed"), "true");
  assert.equal(cards().length, 1, "the click pins it");
  await click(ctx);
  await blur(ctx);
  assert.equal(cards().length, 0);

  const sk = term("stream", "stream.SK", 0);
  await pointer(sk, "pointerover", "touch");
  assert.equal(cards().length, 0, "a touch does not hover");
  await click(sk);
  assert.equal(sk.getAttribute("aria-pressed"), "true", "a tap pins");
  assert.equal(cards().length, 1);
  await click(sk);
  assert.equal(sk.getAttribute("aria-pressed"), "false", "a second tap unpins");
  assert.equal(cards().length, 0);

  await click(sk);
  await pointer(document.body, "pointerdown", "touch");
  assert.equal(sk.getAttribute("aria-pressed"), "false", "a press elsewhere unpins");
  assert.equal(cards().length, 0);
});

test("view as: each party sees only what it can know, on every block; the text stays, and a screen reader is told", async () => {
  const before = [...container.querySelectorAll("[data-formula] code")].map((c) => c.textContent);
  const hub = container.querySelector<HTMLElement>('[data-formula="seal"] [role="radio"][data-party="hub"]')!;
  await click(hub);
  assert.equal(hub.getAttribute("aria-checked"), "true");
  for (const id of ["seal.kek", "seal.e-priv", "seal.S-priv", "key.key"]) assert.equal(term("seal", id).getAttribute("data-hidden-from"), "hub", id);
  for (const id of ["seal.R", "seal.e-pub", "seal.S-pub", "seal.ctx", "key.commit", "id.account"]) assert.equal(term("seal", id).getAttribute("data-hidden-from"), null, id);
  // Every block follows the one choice.
  assert.equal(term("stream", "stream.SK").getAttribute("data-hidden-from"), "hub");
  assert.equal(term("head", "host.key").getAttribute("data-hidden-from"), "hub");
  assert.equal(term("stream", "stream.id").getAttribute("data-hidden-from"), null);
  assert.equal(container.querySelectorAll('[data-formula="labels"] [data-hidden-from]').length, 0, "labels are public");
  // Hidden, not removed: the text copies as before, and the description says so.
  assert.deepEqual([...container.querySelectorAll("[data-formula] code")].map((c) => c.textContent), before);
  assert.equal(term("seal", "seal.kek").getAttribute("aria-describedby"), "fx-seal-kek fx-hidden-hub");
  assert.equal(document.getElementById("fx-hidden-hub")!.textContent, "Droppen Hub cannot know this value.");
  // Each hidden token is exactly a term the Hub cannot know.
  for (const el of container.querySelectorAll<HTMLElement>("[data-hidden-from]")) assert.ok(!TERMS[el.dataset.term!]!.known.includes("hub"), el.dataset.term);
  assert.ok(container.querySelector('[data-formula="seal"]')!.textContent!.includes("Hidden here: 5"));

  // The arrow keys move the choice: the relay knows the ids a channel's prologue binds, not a direct path's.
  await key(hub, "ArrowRight");
  const relay = container.querySelector<HTMLElement>('[data-formula="noise"] [role="radio"][data-party="relay"]')!;
  assert.equal(relay.getAttribute("aria-checked"), "true");
  assert.equal(term("noise", "noise.prologue").getAttribute("data-hidden-from"), null);
  assert.equal(term("direct", "direct.prologue").getAttribute("data-hidden-from"), "relay");
  // Your device, as the sender, cannot know the recipient's side; the recipient cannot know the sender's one-off key.
  await click(container.querySelector<HTMLElement>('[data-formula="seal"] [role="radio"][data-party="other"]')!);
  assert.equal(term("seal", "seal.e-priv").getAttribute("data-hidden-from"), "other");
  assert.equal(term("seal", "seal.kek").getAttribute("data-hidden-from"), null);
  assert.ok(container.querySelector('[data-formula="seal"]')!.textContent!.includes("the recipient"));
  await click(container.querySelector<HTMLElement>('[data-formula="seal"] [role="radio"][data-party="all"]')!);
  assert.equal(container.querySelectorAll("[data-hidden-from]").length, 0);
});

test("the legend opens beside the switch, and names every kind", async () => {
  const button = container.querySelector<HTMLElement>('[data-formula="chain"] [aria-expanded]')!;
  assert.equal(button.getAttribute("aria-expanded"), "false");
  await click(button);
  assert.equal(button.getAttribute("aria-expanded"), "true");
  const legend = document.getElementById(button.getAttribute("aria-controls")!)!;
  for (const name of ["Secret key", "Derived secret", "Public key", "Public value", "Label or constant", "Function", "Operator"]) assert.ok(legend.textContent!.includes(name), name);
  assert.equal(legend.hidden, false);
  await click(button);
  assert.equal(legend.hidden, true);
  assert.ok(Object.keys(BLOCKS).length > 10);
});

test("one tab stop per formula: the arrow keys move between its parts and lines, and the stop follows", async () => {
  const seal = container.querySelector<HTMLElement>('[data-formula="seal"]')!;
  const parts = [...seal.querySelectorAll<HTMLElement>("[data-occ]")];
  assert.equal(parts.filter((p) => p.tabIndex === 0).length, 1, "one stop in the formula");
  await act(async () => void document.dispatchEvent(new window.KeyboardEvent("keydown", { key: "Tab", bubbles: true })));
  await focus(parts[0]!);
  await key(parts[0]!, "ArrowRight");
  assert.equal(document.activeElement, parts[1]);
  assert.equal(parts[1]!.tabIndex, 0, "the stop moves with the focus");
  assert.equal(parts[0]!.tabIndex, -1);
  await key(parts[1]!, "ArrowDown");
  const line2 = seal.querySelectorAll("code")[1]!;
  assert.equal(document.activeElement, line2.querySelector("[data-occ]"), "the next line's first part");
  await key(document.activeElement!, "End");
  assert.equal(document.activeElement, parts[parts.length - 1]);
  await key(document.activeElement!, "Home");
  assert.equal(document.activeElement, parts[0]);
  await key(parts[0]!, "Escape");
  await blur(parts[0]!);
  assert.equal(cards().length, 0);
});

test("one live region for the whole page says what a view hides", async () => {
  assert.equal(container.querySelectorAll("[data-formula] [aria-live]").length, 0, "none per block");
  const announcer = container.querySelector<HTMLElement>("[data-formula-announcer]")!;
  assert.equal(announcer.getAttribute("aria-live"), "polite");
  await click(container.querySelector<HTMLElement>('[data-formula="stream"] [role="radio"][data-party="relay"]')!);
  const hidden = container.querySelectorAll("[data-hidden-from]").length;
  assert.equal(announcer.textContent, `Viewing as Relay: ${hidden} values on this page are hidden.`);
  await click(container.querySelector<HTMLElement>('[data-formula="stream"] [role="radio"][data-party="all"]')!);
  assert.equal(announcer.textContent, "Showing every value.");
});

test("a mention of a device that runs AI explains itself: every mention lights, and its card says what it is", async () => {
  const mentions = [...container.querySelectorAll<HTMLElement>('main [data-concept="host"]')];
  const mention = mentions.find((m) => m.closest("#overview"))!;
  assert.ok(mention && mentions.length > 40);
  await act(async () => void document.dispatchEvent(new window.KeyboardEvent("keydown", { key: "Tab", bubbles: true })));
  await focus(mention);
  assert.ok(mentions.every((m) => m.getAttribute("data-lit") === "same"), "every mention lights");
  assert.equal(container.querySelector('[data-concept="client"][data-lit]'), null, "not the other word");
  const [card] = cards();
  assert.ok(card!.textContent!.includes("A device that runs AI"));
  assert.ok(card!.textContent!.includes("it is never a full device"));
  await key(mention, "Enter");
  assert.equal(mention.getAttribute("aria-pressed"), "true");
  await key(mention, "Escape");
  assert.equal(mention.getAttribute("aria-pressed"), "false");
  assert.equal(cards().length, 0);
  // The arrow keys move to the next mention in the section, whichever word.
  const inOverview = [...container.querySelectorAll<HTMLElement>("#overview [data-concept]")];
  await focus(inOverview[0]!);
  await key(inOverview[0]!, "ArrowRight");
  assert.equal(document.activeElement, inOverview[1]);
  await key(inOverview[1]!, "Escape");
  await blur(inOverview[1]!);
  assert.ok(document.getElementById("fx-c-host")!.textContent!.startsWith("A device that runs AI: "));
});

test("a card stays inside the viewport: above its block when there is room, below it when not, moved in from the edge", async () => {
  type Box = { x: number; y: number; w: number; h: number };
  const rect = ({ x, y, w, h }: Box) => ({ x, y, left: x, top: y, width: w, height: h, right: x + w, bottom: y + h, toJSON: () => ({}) }) as DOMRect;
  const layout = { anchor: { x: 0, y: 0, w: 0, h: 0 } as Box, block: { x: 0, y: 0, w: 0, h: 0 } as Box, card: { w: 300, h: 80 } };
  const kek = term("seal", "seal.kek", 1);
  const proto = window.Element.prototype as unknown as { getBoundingClientRect: () => DOMRect };
  const original = proto.getBoundingClientRect;
  Object.defineProperty(window.Element.prototype, "getBoundingClientRect", {
    configurable: true,
    value(this: Element) {
      if (this === kek) return rect(layout.anchor);
      if (this.getAttribute("data-formula") === "seal") return rect(layout.block);
      return rect({ x: 0, y: 0, w: 0, h: 0 });
    },
  });
  const sizes = ["offsetWidth", "offsetHeight"] as const;
  for (const prop of sizes) {
    Object.defineProperty(window.HTMLElement.prototype, prop, {
      configurable: true,
      get(this: HTMLElement) {
        return this.hasAttribute("data-formula-popup") ? (prop === "offsetWidth" ? layout.card.w : layout.card.h) : 0;
      },
    });
  }
  for (const [prop, value] of [
    ["clientWidth", 400],
    ["clientHeight", 600],
  ] as const) {
    Object.defineProperty(document.documentElement, prop, { configurable: true, get: () => value });
  }
  const place = async () => {
    await click(kek);
    await act(async () => void (await new Promise((r) => setTimeout(r, 10))));
    const card = cards()[0]!;
    const at = { left: parseFloat(card.style.left), top: parseFloat(card.style.top) };
    await click(kek);
    return at;
  };
  try {
    // Near the right edge of a phone: moved in so all of it shows, 8px from the edge.
    layout.block = { x: 16, y: 300, w: 368, h: 60 };
    layout.anchor = { x: 360, y: 320, w: 20, h: 16 };
    assert.deepEqual(await place(), { left: 400 - 300 - 8, top: 300 - 6 - 80 }, "above the block, in from the edge");
    // The block at the top of the screen: no room above, so below the block, never over the formula.
    layout.block = { x: 16, y: 20, w: 368, h: 60 };
    layout.anchor = { x: 40, y: 40, w: 20, h: 16 };
    assert.deepEqual(await place(), { left: 40, top: 20 + 60 + 6 });
  } finally {
    Object.defineProperty(window.Element.prototype, "getBoundingClientRect", { configurable: true, value: original });
  }
});
