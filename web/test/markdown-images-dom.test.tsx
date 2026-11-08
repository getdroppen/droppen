// A picture from the web in an agent's markdown loads when the reader clicks
// "load image", and not before.
//
// What is pinned here: before the click the page has no `<img>` with the
// picture's URL; the click draws it, with no referrer, together with every
// other copy of the same URL on the page and no other picture; and a message
// mounted again afterwards (a row the virtualised transcript scrolled out and
// back in) draws it straight away. A real DOM (jsdom), because this is about
// the click; `markdown-images.test.tsx` has the rest.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test, { after, afterEach } from "node:test";
import { installJsdom } from "./jsdom.js";
import { makeI18n } from "./render.js";

const { window, container } = installJsdom();
after(() => {
  (window as unknown as { close(): void }).close();
  setTimeout(() => process.exit(), 50);
});

const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { QueryClient, QueryClientProvider } = await import("@tanstack/react-query");
const { I18nextProvider } = await import("react-i18next");
const { Markdown } = await import("../src/components/markdown.js");

const i18n = makeI18n({
  lng: "en",
  resources: { en: { translation: {} } },
  keySeparator: false,
  nsSeparator: false
});

let root: ReturnType<typeof createRoot> | null = null;
afterEach(async () => {
  await act(async () => root?.unmount());
  root = null;
});

async function render(text: string) {
  await act(async () => root?.unmount());
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  root = createRoot(container);
  await act(async () =>
    root!.render(
      <I18nextProvider i18n={i18n}>
        <QueryClientProvider client={client}>
          <Markdown text={text} />
        </QueryClientProvider>
      </I18nextProvider>,
    ),
  );
}

const images = (url: string) => [...container.querySelectorAll("img")].filter((img) => img.getAttribute("src") === url);
const loadButtons = () => [...container.querySelectorAll("button")].filter((button) => button.textContent === "load image");

test("the picture is drawn when the reader asks, everywhere it is on the page, and again after a remount", async () => {
  const chart = "https://images.example.com/chart.png?q=report";
  const other = "https://images.example.com/other.png";
  const text = `![chart](${chart})\n\nand again ![chart](${chart}), and ![other](${other})`;
  await render(text);
  assert.equal(images(chart).length, 0, "nothing fetches it before the click");
  assert.equal(container.querySelectorAll("img").length, 0);
  assert.equal(loadButtons().length, 3);

  await act(async () => loadButtons()[0]!.dispatchEvent(new window.MouseEvent("click", { bubbles: true, cancelable: true })));
  const drawn = images(chart);
  assert.equal(drawn.length, 2, "both copies of the URL load");
  for (const img of drawn) assert.equal(img.getAttribute("referrerpolicy"), "no-referrer");
  assert.equal(images(other).length, 0, "another URL still waits");
  assert.equal(loadButtons().length, 1);

  await render(text);
  assert.equal(images(chart).length, 2, "a new mount draws it straight away");
  assert.equal(images(other).length, 0);
});
