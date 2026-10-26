// A page's own chunk that does not come (`app/page-load.tsx`, `main.tsx`):
// the page reloads once per tab, putting back first what the reload would
// lose; if the chunk does not come then either, it says so and offers to try
// again or reload, instead of staying blank.
import assert from "node:assert/strict";
import test from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import type { ReactElement, ReactNode } from "react";

const { default: i18n } = await import("../src/i18n/index.js");
await i18n.changeLanguage("en");
const { loadPage, RELOADED_KEY } = await import("../src/app/page-load.js");
type PageLoadEnv = import("../src/app/page-load.js").PageLoadEnv;

/** A tab: its storage (or none, or one that throws), and what happened in it. */
function tab(storage: "memory" | "none" | "throws" = "memory", noted = false) {
  const kept = new Map<string, string>(noted ? [[RELOADED_KEY, "1"]] : []);
  const events: string[] = [];
  const drawn: ReactNode[] = [];
  const env: PageLoadEnv = {
    storage:
      storage === "none"
        ? null
        : storage === "throws"
          ? {
              getItem: () => {
                throw new Error("denied");
              },
              setItem: () => undefined,
              removeItem: () => undefined,
            }
          : { getItem: (k) => kept.get(k) ?? null, setItem: (k, v) => void kept.set(k, v), removeItem: (k) => void kept.delete(k) },
    reload: () => void events.push("reload"),
  };
  return { env, kept, events, drawn, root: { render: (node: ReactNode) => void drawn.push(node) } };
}

const failing = () => Promise.reject(new TypeError("Failed to fetch dynamically imported module"));

/** The failure notice's buttons, as the page drew it (in `StrictMode`). */
function notice(node: ReactNode): { onRetry: () => void; onReload: () => void } {
  return ((node as ReactElement<{ children: ReactElement<{ onRetry: () => void; onReload: () => void }> }>).props.children).props;
}

test("a chunk that comes is shown, and a reload noted before is forgotten", async () => {
  const t = tab("memory", true);
  await loadPage(t.root, async () => "the page", (page) => void t.events.push(`show ${page}`), { env: t.env });
  assert.deepEqual(t.events, ["show the page"]);
  assert.equal(t.kept.has(RELOADED_KEY), false);
  assert.equal(t.drawn.length, 0);
});

test("the first failure reloads, once per tab, after putting back what the reload would lose", async () => {
  const t = tab();
  await loadPage(t.root, failing, () => void t.events.push("show"), { env: t.env, beforeReload: () => void t.events.push("put back") });
  assert.deepEqual(t.events, ["put back", "reload"]);
  assert.equal(t.kept.get(RELOADED_KEY), "1");
  assert.equal(t.drawn.length, 0, "nothing drawn: the page is going");
});

test("a failure after that reload says so, and its retry and its reload both work", async () => {
  const t = tab("memory", true);
  let comes = false;
  await loadPage(t.root, () => (comes ? Promise.resolve("the page") : failing()), (page) => void t.events.push(`show ${page}`), {
    env: t.env,
    beforeReload: () => void t.events.push("put back"),
  });
  assert.deepEqual(t.events, [], "no second reload by itself");
  assert.equal(t.drawn.length, 1);
  const html = renderToStaticMarkup(t.drawn[0] as ReactElement);
  assert.match(html, /data-testid="page-load-failed"/);
  assert.match(html, /the page could not be loaded/);
  assert.match(html, />retry</);
  assert.match(html, />reload</);
  const { onRetry, onReload } = notice(t.drawn[0]);
  // Still not there: the notice again.
  onRetry();
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(t.drawn.length, 2);
  comes = true;
  onRetry();
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.deepEqual(t.events, ["show the page"]);
  assert.equal(t.kept.has(RELOADED_KEY), false, "a good load forgets the reload");
  onReload();
  assert.deepEqual(t.events, ["show the page", "put back", "reload"]);
});

test("with no storage to note the reload in, the page never reloads by itself", async () => {
  for (const storage of ["none", "throws"] as const) {
    const t = tab(storage);
    await loadPage(t.root, failing, () => void t.events.push("show"), { env: t.env });
    assert.deepEqual(t.events, [], storage);
    assert.equal(t.drawn.length, 1, `${storage}: the notice instead`);
  }
});
