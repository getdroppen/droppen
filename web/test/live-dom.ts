// The live-DOM tests' shared harness: install a real DOM (jsdom) once per
// file, let the process go when the file's tests have run, and read the tree
// React drew into it. React and the app's modules are imported by the test
// file *after* `installLiveDom`, as they read the page when they load.
//
// The stand-in a page sets before installing (`location`, mostly) is kept:
// jsdom's own is a real `Location`, but these pages are not at jsdom's URL.
import assert from "node:assert/strict";
import { after } from "node:test";
import { installJsdom } from "./jsdom.js";

/** The nodes these tests walk (jsdom elements). */
export type FakeNode = HTMLElement;

/**
 * Install the page and, once every test in the file has run, exit: query
 * caches, feeds and React's scheduler keep timers the file does not own.
 * `act` sets `IS_REACT_ACT_ENVIRONMENT` for the files whose renders go
 * through React's `act`.
 */
export function installLiveDom({ act = false } = {}): FakeNode {
  const stubLocation = (globalThis as { location?: unknown }).location;
  // Node's BroadcastChannel dispatches Node events: its `Event` is what the
  // checks see, not the page's (jsdom's) one.
  const nodeEvents = { Event: globalThis.Event, EventTarget: globalThis.EventTarget };
  const { container } = installJsdom();
  for (const [name, value] of Object.entries(nodeEvents)) Object.defineProperty(globalThis, name, { value, configurable: true, writable: true });
  if (stubLocation) Object.defineProperty(globalThis, "location", { value: stubLocation, configurable: true, writable: true });
  (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = act;
  after(() => void setTimeout(() => process.exit(), 50));
  return container;
}

/** The elements below `node` that carry `attribute` (the old `FakeNode.findAll`). */
export function findAll(node: Node, attribute: string): Element[] {
  return [...(node as Element).querySelectorAll(`[${attribute}]`)];
}

/** Every element below `node`, depth first (a live DOM has text nodes too; the tests read elements). */
export function walk(node: Node, out: Element[] = []): Element[] {
  for (const child of node.childNodes) {
    if (child.nodeType === 1) out.push(child as Element);
    walk(child, out);
  }
  return out;
}

/** `all`/`first` over one file's container, so a test reads `first((n) => …)`. */
export function domTree(container: Node) {
  const all = () => walk(container);
  const first = (match: (node: Element) => boolean) => all().find(match);
  return { all, first };
}

/** The first node in `node`'s subtree (not `node` itself) that `match` accepts. */
export function find(node: Node, match: (node: Element) => boolean): Element | null {
  for (const child of node.childNodes) {
    if (child.nodeType !== 1) continue;
    if (match(child as Element)) return child as Element;
    const hit = find(child, match);
    if (hit) return hit;
  }
  return null;
}

/** The props React gave an element, so a test can do what a browser's event would. */
export function propsOf(node: Element | undefined): Record<string, (...args: unknown[]) => unknown> {
  assert.ok(node, "the element is on the page");
  const key = Object.keys(node).find((k) => k.startsWith("__reactProps$"));
  assert.ok(key, "rendered by React");
  return (node as unknown as Record<string, Record<string, (...args: unknown[]) => unknown>>)[key];
}

const pause = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/**
 * Wait for `check`, naming what is waited for, then fail if it never holds.
 * The budget defaults to the most patient copy this replaced (10 s:
 * `new-session-render`'s 500 × 20 ms); the other copies waited 2 s or 3 s.
 */
export async function until(what: string, check: () => boolean, budgetMs = 10_000): Promise<void> {
  for (let i = 0; i < Math.ceil(budgetMs / 20); i++) {
    if (check()) return;
    await pause(20);
  }
  assert.fail(`timed out waiting for ${what}`);
}

/** Wait for `done` up to `ms`, sampling every 5ms. */
export async function waitFor(done: () => boolean, ms = 5_000): Promise<void> {
  const end = Date.now() + ms;
  while (!done()) {
    if (Date.now() > end) throw new Error("timed out waiting");
    await pause(5);
  }
}

/** Wait for `check` up to `ms`: true once it holds, false at the timeout (no throw). */
export async function eventually(check: () => boolean, ms = 5000): Promise<boolean> {
  const end = Date.now() + ms;
  while (!check()) {
    if (Date.now() > end) return false;
    await pause(50);
  }
  return true;
}
