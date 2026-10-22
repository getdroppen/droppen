// The phone keyboard left to WebKit (`lib/kb-native.ts`, the default mode):
// nothing is sized from the viewport, nothing scrolls the page while a field
// has focus, and once no field has focus a root still panned by the keyboard
// is put back at the top.
//
// A fake `visualViewport` and window stand in for the browser. The numbers are
// the owner's iPhone in iOS 27 Chrome, as its readout logged them: a
// 711px fixed box, the keyboard raised to a 434px visual viewport, and WebKit's
// reveal pan animated over several frames, with `innerHeight` shrinking along.
import assert from "node:assert/strict";
import test from "node:test";
import {
  type HoldEnv,
  holdInView,
  type NativeKbEnv,
  RESET_MS,
  rootResets,
  startKbNative,
  WATCH_MS,
} from "../src/lib/kb-native.js";

class FakeTarget {
  private listeners = new Map<string, Set<() => void>>();
  addEventListener(type: string, fn: () => void) {
    if (!this.listeners.has(type)) this.listeners.set(type, new Set());
    this.listeners.get(type)!.add(fn);
  }
  removeEventListener(type: string, fn: () => void) {
    this.listeners.get(type)?.delete(fn);
  }
  fire(type: string) {
    for (const fn of this.listeners.get(type) ?? []) fn();
  }
  listening(): number {
    return [...this.listeners.values()].reduce((n, set) => n + set.size, 0);
  }
}

const IOS27 = { height: 711, keyboard: 434 };
/** offsetTop, innerHeight, scrollY: WebKit's pan to the composer, frame by frame. */
const IOS27_PAN: readonly (readonly [number, number, number])[] = [
  [3, 708, 3],
  [36, 675, 36],
  [96, 615, 96],
  [167.7, 543, 168],
  [231.3, 480, 231],
  [270.3, 441, 270],
  [270.3, 434, 277],
  [277, 434, 277],
];

const field = (tagName: string) => ({ tagName, isContentEditable: false }) as unknown as Element;

function fakePage() {
  const vv = Object.assign(new FakeTarget(), { height: IOS27.height, offsetTop: 0, scale: 1 });
  const winEvents = new FakeTarget();
  const doc = new FakeTarget();
  let now = 0;
  const timers = new Map<number, { at: number; fn: () => void }>();
  let nextId = 1;
  const scrolls: [number, number, number][] = [];
  let focused: Element | null = null;
  const win = {
    innerHeight: IOS27.height,
    scrollY: 0,
    scrollTo(x: number, y: number) {
      scrolls.push([now, x, y]);
    },
    setTimeout(fn: () => void, ms: number) {
      const id = nextId++;
      timers.set(id, { at: now + ms, fn });
      return id;
    },
    clearTimeout(id: number | undefined) {
      if (id !== undefined) timers.delete(id);
    },
    addEventListener: winEvents.addEventListener.bind(winEvents),
    removeEventListener: winEvents.removeEventListener.bind(winEvents),
  };
  const env = {
    vv,
    win,
    doc,
    focused: () => focused,
  } as unknown as NativeKbEnv;
  /** Run the clock forward, firing timers as they come due. */
  const advance = (ms: number) => {
    const end = now + ms;
    for (;;) {
      const due = [...timers.entries()].filter(([, t]) => t.at <= end).sort((a, b) => a[1].at - b[1].at)[0];
      if (due === undefined) break;
      timers.delete(due[0]);
      now = due[1].at;
      due[1].fn();
    }
    now = end;
  };
  return {
    vv,
    win,
    winEvents,
    doc,
    env,
    scrolls,
    timers,
    advance,
    focus(el: Element | null) {
      if (focused !== null) doc.fire("focusout");
      focused = el;
      if (el !== null) doc.fire("focusin");
    },
    /** Focus moved with no event: the focused element taken out of the page. */
    setFocusQuietly(el: Element | null) {
      focused = el;
    },
  };
}

/** The composer focused, the keyboard up and WebKit's pan done, as the owner's phone logged it. */
function keyboardUp(page: ReturnType<typeof fakePage>) {
  page.focus(field("TEXTAREA"));
  page.vv.height = IOS27.keyboard;
  page.vv.fire("resize");
  for (const [top, inner, y] of IOS27_PAN) {
    page.advance(16);
    page.vv.offsetTop = top;
    page.win.innerHeight = inner;
    page.win.scrollY = y;
    page.winEvents.fire("scroll");
    page.vv.fire("scroll");
  }
}

test("the keyboard rising: WebKit's pan is left alone, frame by frame and after", () => {
  const page = fakePage();
  startKbNative(page.env);
  keyboardUp(page);
  page.advance(5000);
  // The earlier watch scrolled to the top 250ms into this (its keyboard
  // "closed" when innerHeight came down with the pan), and the composer went
  // back under the keyboard. Here nothing scrolls while a field has focus.
  assert.deepEqual(page.scrolls, []);
});

test("dismissed, iOS 27: the viewport keeps the keyboard's numbers with no event, and the root goes back to the top once", () => {
  const page = fakePage();
  startKbNative(page.env);
  keyboardUp(page);
  // Done: focus leaves the composer, and every number stays where it was.
  page.focus(null);
  page.advance(RESET_MS[0] - 1);
  assert.deepEqual(page.scrolls, [], "not at once: focus may only be moving to the next field");
  page.advance(1);
  assert.equal(page.scrolls.length, 1);
  assert.deepEqual(page.scrolls[0]!.slice(1), [0, 0]);
  // iOS reads the viewport again once the page has moved: nothing more to do.
  page.vv.height = IOS27.height;
  page.vv.offsetTop = 0;
  page.win.innerHeight = IOS27.height;
  page.win.scrollY = 0;
  page.vv.fire("scroll");
  page.advance(5000);
  assert.equal(page.scrolls.length, 1);
});

test("dismissed, where WebKit puts the pan back itself (iOS 26): nothing is written", () => {
  const page = fakePage();
  startKbNative(page.env);
  keyboardUp(page);
  page.focus(null);
  // As the iOS 26.5 Simulator does it: back within a frame or two.
  page.advance(40);
  page.vv.height = IOS27.height;
  page.vv.offsetTop = 0;
  page.win.innerHeight = IOS27.height;
  page.win.scrollY = 0;
  page.vv.fire("resize");
  page.winEvents.fire("scroll");
  page.vv.fire("scroll");
  page.advance(5000);
  assert.deepEqual(page.scrolls, []);
});

test("a stale pan that iOS reports only after the first check is still put back", () => {
  const page = fakePage();
  startKbNative(page.env);
  keyboardUp(page);
  // The numbers drop back partway first (the root at the top), then iOS
  // reports the pan again 400ms on, with no field focused.
  page.focus(null);
  page.win.scrollY = 0;
  page.vv.offsetTop = 0;
  page.advance(400);
  assert.deepEqual(page.scrolls, []);
  page.win.scrollY = 277;
  page.vv.offsetTop = 277;
  page.vv.fire("scroll");
  page.advance(RESET_MS[0]);
  assert.equal(page.scrolls.length, 1);
});

test("focus moving on to the next field, or put straight back, is not a dismissal", () => {
  const page = fakePage();
  startKbNative(page.env);
  keyboardUp(page);
  page.focus(field("INPUT"));
  page.advance(5000);
  assert.deepEqual(page.scrolls, []);
  // Blur and refocus within the same moment (a script putting it back).
  page.focus(null);
  page.advance(10);
  page.focus(field("TEXTAREA"));
  page.advance(5000);
  assert.deepEqual(page.scrolls, []);
});

test("the composer tapped again right after a stale dismissal: the second keyboard is never scrolled", () => {
  const page = fakePage();
  startKbNative(page.env);
  keyboardUp(page);
  page.focus(null);
  page.advance(RESET_MS[0]);
  assert.equal(page.scrolls.length, 1);
  // The scroll has not been read back yet (the numbers are still stale) when
  // the composer is tapped again and WebKit pans to it once more.
  page.focus(field("TEXTAREA"));
  page.vv.fire("resize");
  page.vv.fire("scroll");
  page.advance(5000);
  assert.equal(page.scrolls.length, 1);
});

test("the focused composer taken out of the page (a view that navigates on send): no focusout, and the pan is still put back", () => {
  const page = fakePage();
  startKbNative(page.env);
  keyboardUp(page);
  page.advance(2000);
  assert.deepEqual(page.scrolls, []);
  // WebKit moves focus to the body with no event when the focused element is
  // removed, and iOS 27 may leave the keyboard's pan with no event either.
  page.setFocusQuietly(null);
  page.advance(WATCH_MS + RESET_MS[0]);
  assert.equal(page.scrolls.length, 1);
  // Nothing left panned once iOS reads the scroll back: no more checks.
  page.win.scrollY = 0;
  page.vv.offsetTop = 0;
  page.advance(5000);
  assert.equal(page.scrolls.length, 1);
  assert.equal(page.timers.size, 0);
});

test("a field inside an embedded app's frame: the rise is left alone, and the pan is put back once focus leaves the frame", () => {
  const page = fakePage();
  startKbNative(page.env);
  // code-server in /embed/…: this document sees only the IFRAME as focused.
  page.focus(field("IFRAME"));
  page.vv.height = IOS27.keyboard;
  page.vv.fire("resize");
  for (const [top, inner, y] of IOS27_PAN) {
    page.advance(16);
    page.vv.offsetTop = top;
    page.win.innerHeight = inner;
    page.win.scrollY = y;
    page.winEvents.fire("scroll");
    page.vv.fire("scroll");
  }
  page.advance(5000);
  assert.deepEqual(page.scrolls, [], "no scroll while the frame has focus");
  // Focus leaves the frame, and the numbers stay at the keyboard's: back to
  // the top (the watch on the frame and the first check may both find it so
  // within the same 100ms; scrolling to the top twice is still the top).
  page.focus(null);
  page.advance(RESET_MS[0]);
  assert.ok(page.scrolls.length >= 1);
  assert.ok(page.scrolls.every(([, x, y]) => x === 0 && y === 0));
  const made = page.scrolls.length;
  page.win.scrollY = 0;
  page.vv.offsetTop = 0;
  page.vv.fire("scroll");
  page.advance(5000);
  assert.equal(page.scrolls.length, made);
});

test("pinch-zoom is the reader's pan, and is left alone", () => {
  const page = fakePage();
  startKbNative(page.env);
  page.vv.scale = 2;
  page.vv.offsetTop = 300;
  page.win.scrollY = 300;
  page.vv.fire("scroll");
  page.advance(5000);
  assert.deepEqual(page.scrolls, []);
});

test("a page brought back from the background with a pan left over goes back to the top", () => {
  const page = fakePage();
  startKbNative(page.env);
  page.vv.offsetTop = 277;
  page.win.scrollY = 277;
  page.doc.fire("visibilitychange");
  page.advance(RESET_MS[0]);
  assert.equal(page.scrolls.length, 1);
});

test("desktop and Android: the root never scrolls, so nothing is ever written", () => {
  const page = fakePage();
  const before = rootResets();
  startKbNative(page.env);
  page.focus(field("TEXTAREA"));
  // Android's keyboard shrinks the layout viewport itself: no pan.
  page.vv.height = 400;
  page.vv.fire("resize");
  page.focus(null);
  page.vv.height = IOS27.height;
  page.vv.fire("resize");
  page.winEvents.fire("pageshow");
  page.advance(5000);
  assert.deepEqual(page.scrolls, []);
  assert.equal(rootResets(), before);
});

test("the root is put back at the top when the page starts with a pan left over", () => {
  const page = fakePage();
  page.vv.offsetTop = 120;
  page.win.scrollY = 120;
  startKbNative(page.env);
  page.advance(RESET_MS[0]);
  assert.equal(page.scrolls.length, 1);
});

test("stopping removes every listener and pending check", () => {
  const page = fakePage();
  const stop = startKbNative(page.env);
  // The keyboard up and panned: checks pending, and the watch on the field.
  keyboardUp(page);
  page.advance(RESET_MS[0]);
  assert.ok(page.timers.size > 0);
  stop();
  assert.equal(page.vv.listening() + page.winEvents.listening() + page.doc.listening(), 0);
  assert.equal(page.timers.size, 0);
});

// The terminal (`holdInView`): xterm's textarea is transparent, so WebKit
// never pans to it, and the box xterm fits itself to is held above the bottom
// of the visual viewport while it has focus. The numbers are the iOS 26.5
// Simulator's: the terminal's box from 38px down, a 714px screen, the keyboard
// leaving a 404px visual viewport and no pan.

/** A box that takes focus events with a `relatedTarget`, as an element does. */
class FakeEl {
  private listeners = new Map<string, Set<(e: { relatedTarget: unknown }) => void>>();
  readonly props = new Map<string, string>();
  readonly style = {
    setProperty: (k: string, v: string) => void this.props.set(k, v),
    removeProperty: (k: string) => void this.props.delete(k),
  };
  readonly inside = { tagName: "TEXTAREA" };
  top = 38;
  addEventListener(type: string, fn: (e: { relatedTarget: unknown }) => void) {
    if (!this.listeners.has(type)) this.listeners.set(type, new Set());
    this.listeners.get(type)!.add(fn);
  }
  removeEventListener(type: string, fn: (e: { relatedTarget: unknown }) => void) {
    this.listeners.get(type)?.delete(fn);
  }
  fire(type: string, relatedTarget: unknown = null) {
    for (const fn of this.listeners.get(type) ?? []) fn({ relatedTarget });
  }
  listening(): number {
    return [...this.listeners.values()].reduce((n, set) => n + set.size, 0);
  }
  contains(node: unknown) {
    return node === this.inside;
  }
  getBoundingClientRect() {
    return { top: this.top };
  }
}

function terminalPage() {
  const vv = Object.assign(new FakeTarget(), { height: 714, offsetTop: 0, scale: 1 });
  const win = new FakeTarget();
  const shell = { top: 0, getBoundingClientRect: () => ({ top: shell.top }) };
  const el = new FakeEl();
  const env = { vv, win, shell } as unknown as HoldEnv;
  const unhold = holdInView(el as unknown as HTMLElement, env);
  return { vv, win, shell, el, unhold, maxHeight: () => el.props.get("max-height") ?? null };
}

test("the terminal focused under the keyboard: its box ends at the bottom of the screen, and goes back once focus leaves", () => {
  const t = terminalPage();
  assert.equal(t.maxHeight(), null, "nothing until it has focus");
  t.el.fire("focusin");
  t.vv.height = 404;
  t.vv.fire("resize");
  assert.equal(t.maxHeight(), "366px");
  // A taller keyboard (the emoji one) moves the bound with it.
  t.vv.height = 360;
  t.vv.fire("resize");
  assert.equal(t.maxHeight(), "322px");
  // Focus moving within the terminal keeps it.
  t.el.fire("focusout", t.el.inside);
  assert.equal(t.maxHeight(), "322px");
  // Focus leaving it (Done, or the composer in the next pane): the full box,
  // whatever the viewport still says.
  t.el.fire("focusout", null);
  assert.equal(t.maxHeight(), null);
  t.vv.fire("resize");
  assert.equal(t.maxHeight(), null);
});

test("the terminal's bound reads the same whether client rects follow a pan (WebKit) or not (Chrome)", () => {
  const webkit = terminalPage();
  webkit.el.fire("focusin");
  webkit.vv.height = 404;
  webkit.vv.offsetTop = 100;
  webkit.shell.top = -100;
  webkit.el.top = 38 - 100;
  webkit.vv.fire("scroll");
  const chrome = terminalPage();
  chrome.el.fire("focusin");
  chrome.vv.height = 404;
  chrome.vv.offsetTop = 100;
  chrome.vv.fire("scroll");
  assert.equal(webkit.maxHeight(), "466px");
  assert.equal(chrome.maxHeight(), "466px");
});

test("the terminal with nothing over it (desktop, Android) is bounded below its own box: no change", () => {
  const t = terminalPage();
  t.el.fire("focusin");
  // The whole window is on screen: the bound is the box's own bottom.
  assert.equal(t.maxHeight(), "676px");
});

test("the terminal's bound is lifted under pinch-zoom, and never shrinks it past a couple of rows", () => {
  const t = terminalPage();
  t.el.fire("focusin");
  t.vv.scale = 2;
  t.vv.height = 357;
  t.vv.fire("resize");
  assert.equal(t.maxHeight(), null);
  t.vv.scale = 1;
  t.el.top = 500;
  t.vv.height = 404;
  t.vv.fire("resize");
  assert.equal(t.maxHeight(), "48px");
});

test("unholding the terminal removes its listeners and its bound", () => {
  const t = terminalPage();
  t.el.fire("focusin");
  t.vv.height = 404;
  t.vv.fire("resize");
  t.unhold();
  assert.equal(t.maxHeight(), null);
  assert.equal(t.el.listening() + t.vv.listening() + t.win.listening(), 0);
});
