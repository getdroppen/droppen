// A real DOM (jsdom) for the tests that drive a control the way a person does
// — pointer, keys, focus: Base UI's select needs focus management, `matches`,
// real events and a document to portal into. Still no layout: every box measures 0×0, so what is pinned
// here is behaviour and markup; a test about where a popup lands lays out the
// boxes it needs itself (`select-placement.test.tsx`).
//
// Install once per test file, before React or the components load.
import { JSDOM } from "jsdom";

/** The page's globals that libraries read, taken from the jsdom window. */
const OVERRIDE = [
  "window",
  "self",
  "document",
  "navigator",
  "location",
  "Event",
  "EventTarget",
  "CustomEvent",
  "UIEvent",
  "FocusEvent",
  "KeyboardEvent",
  "MouseEvent",
  "PointerEvent",
  "InputEvent",
  "getComputedStyle",
  "requestAnimationFrame",
  "cancelAnimationFrame",
];

export function installJsdom(): { window: Window; document: Document; container: HTMLElement } {
  const dom = new JSDOM("<!doctype html><html><head></head><body></body></html>", {
    pretendToBeVisual: true,
    url: "http://localhost/",
  });
  const win = dom.window as unknown as Window & Record<string, unknown>;
  // No media queries in jsdom: every query is false, as on a desktop with a fine pointer.
  win.matchMedia = ((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addEventListener: () => undefined,
    removeEventListener: () => undefined,
    addListener: () => undefined,
    removeListener: () => undefined,
    dispatchEvent: () => false,
  })) as unknown as Window["matchMedia"];
  (win.HTMLElement as unknown as { prototype: Record<string, unknown> }).prototype.scrollIntoView = () => undefined;

  const g = globalThis as Record<string, unknown>;
  const define = (key: string) =>
    Object.defineProperty(globalThis, key, { configurable: true, writable: true, value: win[key] });
  for (const key of Object.getOwnPropertyNames(win)) {
    if (key in g || key.startsWith("_")) continue;
    define(key);
  }
  for (const key of OVERRIDE) define(key);
  g.IS_REACT_ACT_ENVIRONMENT = true;

  const container = win.document.createElement("div");
  win.document.body.appendChild(container);
  return { window: win, document: win.document, container };
}
