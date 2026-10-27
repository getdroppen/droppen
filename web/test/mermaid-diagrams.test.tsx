// Mermaid in the transcript, behind the `diagrams` pref (Settings → message
// rendering; `rendered` unless the device's server says `source`).
//
// The bug this feature answers: an agent writes a ```mermaid flowchart in its
// reply and the transcript shows the fence as code. What is pinned here:
//   · with diagrams off (`source`, or outside a view that read the pref) a
//     mermaid fence is byte-for-byte the code block every other fence is, and
//     the engine is never imported;
//   · with them on, the fence goes through the diagram renderer, with the
//     app's theme and strict security, and is shown as an `<img>` of the SVG;
//   · the fence's own config (an init directive, frontmatter) cannot loosen
//     what the app locks, and a failed render leaves nothing in the page —
//     both against the real engine;
//   · a fence Mermaid rejects is the code block again, without an error banner;
//   · inline code and every other language are untouched;
//   · a streaming fence is handed to the renderer once its text settles, and a
//     diagram already up stays up while the text moves; the per-block
//     "source" control flips only that one;
//   · a failed engine import is retried, a block that has moved on or gone is
//     skipped, and a render that never ends does not hold up the rest;
//   · turning diagrams off puts the code back in an open transcript.
// Where the pref comes from (the server, the settings row) is pinned in
// `server-rs/src/prefs` and `test/settings-message-rendering.test.tsx`.
//
// jsdom rather than `react-dom/server`, because the diagram arrives through an
// effect and a dynamic import. For the rendering itself the engine is
// replaced by `setMermaidForTests` (jsdom lays nothing out, so the real one
// cannot draw here); the config and cleanup tests load the real package.
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test, { after, beforeEach } from "node:test";
import { installJsdom } from "./jsdom.js";
import { makeI18n } from "./render.js";

const { window, document, container } = installJsdom();
after(() => {
  (window as unknown as { close(): void }).close();
  setTimeout(() => process.exit(), 50);
});

const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
type Root = import("react-dom/client").Root;
const { I18nextProvider } = await import("react-i18next");
const { DiagramRendering, drawsDiagrams, Markdown } = await import("../src/components/markdown.js");
const { MERMAID_STABLE_MS, diagramPicture, mermaidConfig, renderMermaidDiagram, setMermaidForTests } = await import(
  "../src/lib/mermaid-render.js"
);

const i18n = makeI18n({
  lng: "en",
  resources: { en: { translation: {} } }
});

const FLOWCHART = "```mermaid\nflowchart TD\n  A-->B\n  B-->C\n```";

/** A message as a session view draws it: `diagrams` is what the view read from the pref; `null` is no view at all. */
function Shell({ text, diagrams = true }: { text: string; diagrams?: boolean | null }) {
  const message = <Markdown text={text} />;
  return (
    <I18nextProvider i18n={i18n}>
      {diagrams === null ? message : <DiagramRendering.Provider value={diagrams}>{message}</DiagramRendering.Provider>}
    </I18nextProvider>
  );
}

let root: Root | null = null;

beforeEach(async () => {
  await act(async () => {
    root?.unmount();
  });
  root = null;
  container.innerHTML = "";
  setMermaidForTests(null);
  document.documentElement.classList.remove("dark");
});

const flush = (ms = 30) => act(async () => void (await new Promise((resolve) => setTimeout(resolve, ms))));

async function mount(text: string, diagrams: boolean | null = true): Promise<void> {
  root = createRoot(container);
  await act(async () => {
    root!.render(<Shell text={text} diagrams={diagrams} />);
  });
  await flush();
}

async function show(text: string, diagrams: boolean | null = true): Promise<void> {
  await act(async () => {
    root!.render(<Shell text={text} diagrams={diagrams} />);
  });
}

interface Call {
  config: Record<string, unknown>;
  id: string;
  code: string;
}

/**
 * The engine, as far as `lib/mermaid-render.ts` can tell. `initialize` is
 * followed by one `render`, so a call's config is the one set immediately
 * before it — that is how the tests see the theme and the security level.
 * `hold` keeps a render waiting until the returned release is called; `never`
 * makes one that does not settle at all.
 */
function fakeMermaid(options: { fail?: (code: string) => boolean; hold?: (code: string) => boolean; never?: (code: string) => boolean } = {}) {
  const calls: Call[] = [];
  const held: (() => void)[] = [];
  let config: Record<string, unknown> = {};
  let loads = 0;
  const api = {
    initialize(next: Record<string, unknown>) {
      config = next;
    },
    async render(id: string, code: string) {
      calls.push({ config, id, code });
      if (options.never?.(code)) await new Promise(() => undefined);
      if (options.hold?.(code)) await new Promise<void>((resolve) => held.push(resolve));
      if (options.fail?.(code)) throw new Error("cannot parse this diagram");
      const safe = code.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
      // The shape Mermaid gives with `useMaxWidth`: a relative width, the size in the viewBox.
      return {
        svg: `<svg id="${id}" width="100%" xmlns="http://www.w3.org/2000/svg" style="max-width: 120px;" viewBox="0 0 120 80"><text>${safe}</text></svg>`,
        diagramType: "flowchart-v2",
      };
    },
  };
  return {
    calls,
    release: () => held.splice(0).forEach((resolve) => resolve()),
    get loads() {
      return loads;
    },
    load: () => {
      loads += 1;
      return Promise.resolve(api);
    },
  };
}

/** Hand `lib/mermaid-render.ts` a fake engine (and, optionally, a shorter time limit). */
function useFake(fake: ReturnType<typeof fakeMermaid>, timeoutMs?: number) {
  setMermaidForTests({ load: fake.load as never, ...(timeoutMs === undefined ? {} : { timeoutMs }) });
}

const diagramImage = () => container.querySelector<HTMLImageElement>("[data-mermaid-image] img");

/* ── off: today's rendering, byte for byte ──────────────────────────────── */

for (const [name, diagrams] of [
  ["set to source", false],
  ["outside a view that read the pref", null],
] as const) {
  test(`with diagrams off (${name}) a mermaid fence is the code block any fence is, and nothing is loaded`, async () => {
    const fake = fakeMermaid();
    useFake(fake);
    await mount(`${FLOWCHART}\n\n${FLOWCHART.replace("mermaid", "rust")}`, diagrams);
    await flush(MERMAID_STABLE_MS + 100);

    assert.equal(fake.loads, 0, "the engine is never imported");
    assert.equal(fake.calls.length, 0, "and never asked to draw");
    const [mermaidBlock, rustBlock] = [...container.querySelectorAll("pre")].map((pre) => pre.outerHTML);
    assert.ok(mermaidBlock && rustBlock);
    assert.equal(mermaidBlock.replaceAll("language-mermaid", "language-rust"), rustBlock, "only the info string differs");
    assert.match(mermaidBlock, /^<pre class="my-1\.5 overflow-x-auto border border-border bg-raised px-2\.5 py-2"><code class="block font-mono text-2xs leading-\[18px\] language-mermaid">/);
    assert.match(mermaidBlock, /flowchart TD/);
    assert.equal(container.querySelector("[data-mermaid-diagram]"), null);
  });
}

/* ── what the engine is told ────────────────────────────────────────────── */

test("the engine is always strict, keeps failures out of the page, draws SVG text with dagre, and locks what a fence could loosen", () => {
  for (const theme of ["light", "dark"] as const) {
    const config = mermaidConfig(theme);
    assert.equal(config.securityLevel, "strict");
    assert.equal(config.startOnLoad, false);
    assert.equal(config.suppressErrorRendering, true, "no error picture left in <body>");
    assert.equal(config.htmlLabels, false, "top level: wins over flowchart.htmlLabels in Mermaid 12");
    assert.equal(config.flowchart?.htmlLabels, false);
    assert.equal(config.layout, "dagre", "Mermaid 12 defaults to ELK, which is not in the build");
    for (const key of ["htmlLabels", "themeCSS", "theme", "themeVariables", "darkMode", "fontFamily", "altFontFamily", "layout", "look"])
      assert.ok(config.secure?.includes(key), `${key} is locked`);
    // The SVG is drawn inside an <img>, where the page's custom properties do not resolve.
    assert.doesNotMatch(config.fontFamily ?? "", /var\(/);
  }
  assert.equal(mermaidConfig("light").theme, "default");
  assert.equal(mermaidConfig("dark").theme, "dark");
});

/** The real package, initialised the way the app does it. */
async function realMermaid() {
  const mermaid = (await import("mermaid")).default;
  mermaid.initialize(mermaidConfig("light"));
  return mermaid;
}

test("a fence's init directive or frontmatter config cannot change the locked keys (the real engine)", async () => {
  const mermaid = await realMermaid();
  const effective = async (fence: string) => {
    await mermaid.parse(fence, { suppressErrors: true });
    return mermaid.mermaidAPI.getConfig();
  };
  const base = await effective("flowchart TD\n  A-->B");
  assert.equal(base.htmlLabels, false);
  assert.equal(base.layout, "dagre");
  assert.equal(base.securityLevel, "strict");

  const directive = [
    '%%{init:{"htmlLabels":true,"layout":"elk","themeCSS":"@keyframes spin{}","theme":"forest","look":"handDrawn",',
    '"fontFamily":"Comic Sans MS","securityLevel":"loose","secure":[],"themeVariables":{"primaryColor":"#ff0000"},',
    '"flowchart":{"htmlLabels":true,"curve":"stepBefore"}}}%%',
    "\nflowchart TD\n  A-->B",
  ].join("");
  const frontmatter = [
    "---",
    "config:",
    "  htmlLabels: true",
    "  layout: elk",
    "  theme: forest",
    "  look: handDrawn",
    '  themeCSS: "@keyframes spin{}"',
    "  fontFamily: Comic Sans MS",
    "  securityLevel: loose",
    "  themeVariables:",
    '    primaryColor: "#ff0000"',
    "  flowchart:",
    "    htmlLabels: true",
    "    curve: stepBefore",
    "---",
    "flowchart TD\n  A-->B",
  ].join("\n");

  for (const [name, fence] of [
    ["init directive", directive],
    ["frontmatter", frontmatter],
  ] as const) {
    const config = await effective(fence);
    assert.equal(config.flowchart?.curve, "stepBefore", `${name}: the fence's config is read (an unlocked key takes)`);
    assert.equal(config.htmlLabels, false, `${name}: htmlLabels`);
    assert.equal(config.flowchart?.htmlLabels, false, `${name}: flowchart.htmlLabels`);
    assert.equal(config.layout, "dagre", `${name}: layout`);
    assert.equal(config.themeCSS, base.themeCSS, `${name}: themeCSS`);
    assert.equal(config.theme, base.theme, `${name}: theme`);
    assert.equal(config.look, base.look, `${name}: look`);
    assert.equal(config.fontFamily, base.fontFamily, `${name}: fontFamily`);
    assert.equal(config.themeVariables?.primaryColor, base.themeVariables?.primaryColor, `${name}: themeVariables`);
    assert.equal(config.securityLevel, "strict", `${name}: securityLevel`);
    assert.deepEqual(config.secure, base.secure, `${name}: secure`);
  }
});

test("a render that fails leaves nothing behind in the page (the real engine)", async () => {
  await realMermaid();
  const before = document.body.innerHTML;
  await assert.rejects(renderMermaidDiagram("flowchart TD\n  A-->", "light"));
  await assert.rejects(renderMermaidDiagram("this is not a diagram", "dark"));
  // Mermaid's stylesheet step throws outside its own cleanup.
  await renderMermaidDiagram("flowchart TD\n  classDef a/b fill:red\n  A-->B", "light").catch(() => undefined);
  assert.equal(document.body.innerHTML, before, "no error picture, no leftover render node");
});

/* ── on: the diagram renderer ───────────────────────────────────────────── */

test("with diagrams on, the fence goes through the renderer under the page's theme and is shown as a picture", async () => {
  const fake = fakeMermaid();
  useFake(fake);
  // A finished message: drawn straight away, without waiting for its text to settle.
  await mount(FLOWCHART);

  assert.equal(fake.calls.length, 1);
  assert.equal(fake.calls[0]!.code.trim(), "flowchart TD\n  A-->B\n  B-->C");
  assert.equal(fake.calls[0]!.config.securityLevel, "strict", "agent output is untrusted, always strict");
  assert.equal(fake.calls[0]!.config.theme, "default", "the light sheet");
  const img = diagramImage();
  assert.ok(img, "the diagram is on the page");
  assert.match(img.src, /^blob:/, "as a picture from a blob URL");
  assert.equal(img.getAttribute("width"), "120", "at its natural width, from the viewBox");
  assert.equal(img.getAttribute("height"), "80");
  assert.equal(img.alt, "Mermaid diagram");
  assert.equal(container.querySelector("[data-mermaid-image] svg"), null, "no SVG markup in the page");
  assert.equal(container.querySelector("pre"), null, "the source is replaced, not shown under it");
});

test("the picture is the SVG as well-formed XML at its natural size, titled by the diagram's own title", async () => {
  const picture = diagramPicture(
    '<svg id="d1" width="100%" xmlns="http://www.w3.org/2000/svg" style="max-width: 640.5px;" viewBox="-8 -8 640.5 200">' +
      '<title id="chart-title-d1">Request flow</title><style>#d1{fill:#333;}</style>' +
      '<g><foreignObject width="10" height="10"><div xmlns="http://www.w3.org/1999/xhtml">a<br>b</div></foreignObject></g></svg>',
  );
  assert.equal(picture.width, 640.5);
  assert.equal(picture.height, 200);
  assert.equal(picture.title, "Request flow");
  assert.equal(picture.blob.type, "image/svg+xml");
  const xml = await picture.blob.text();
  const parsed = new DOMParser().parseFromString(xml, "image/svg+xml");
  assert.equal(parsed.querySelector("parsererror"), null, "an image decoder can read it");
  assert.equal(parsed.querySelector("div")?.namespaceURI, "http://www.w3.org/1999/xhtml", "a label's HTML is still HTML");
  const svg = parsed.documentElement;
  assert.equal(svg.getAttribute("width"), "640.5");
  assert.equal(svg.getAttribute("height"), "200");
  assert.doesNotMatch(svg.getAttribute("style") ?? "", /max-width/);

  // Both sides in pixels and no viewBox: the attributes are the size.
  const sized = diagramPicture('<svg xmlns="http://www.w3.org/2000/svg" width="300" height="150"></svg>');
  assert.deepEqual([sized.width, sized.height, sized.title], [300, 150, null]);
  assert.throws(() => diagramPicture('<svg xmlns="http://www.w3.org/2000/svg" width="100%"></svg>'), /without a size/);
});

test("the diagram follows a theme change, and a picture no longer shown is released", async () => {
  const fake = fakeMermaid();
  useFake(fake);
  const revoked: string[] = [];
  const revoke = URL.revokeObjectURL;
  URL.revokeObjectURL = (url: string) => {
    revoked.push(url);
    revoke(url);
  };
  try {
      document.documentElement.classList.add("dark");
    await mount(FLOWCHART);
    assert.equal(fake.calls.at(-1)!.config.theme, "dark");
    const dark = diagramImage()!.src;

    await act(async () => {
      document.documentElement.classList.remove("dark");
    });
    await flush();
    assert.equal(fake.calls.length, 2, "the diagram is drawn again for the other sheet");
    assert.equal(fake.calls.at(-1)!.config.theme, "default");
    const light = diagramImage()!.src;
    assert.notEqual(light, dark);
    await flush(1100);
    assert.ok(revoked.includes(dark), "the replaced picture is released");
    assert.ok(!revoked.includes(light), "the one on screen is not");

    await act(async () => {
      root?.unmount();
    });
    root = null;
    await flush(1100);
    assert.ok(revoked.includes(light), "and the last one once the block is gone");
  } finally {
    URL.revokeObjectURL = revoke;
  }
});

test("a fence Mermaid cannot read is the plain code block, with no error banner", async () => {
  const fake = fakeMermaid({ fail: () => true });
  useFake(fake);
  await mount("```mermaid\nflowchart TD\n  A--\n```");

  assert.match(container.innerHTML, /language-mermaid/);
  assert.match(container.innerHTML, /flowchart TD/);
  assert.equal(container.querySelector("[data-mermaid-diagram]"), null);
  assert.doesNotMatch(container.textContent ?? "", /cannot parse|error|failed/i);
});

test("inline code and other fenced languages stay exactly as they were, even with diagrams on", async () => {
  const fake = fakeMermaid();
  useFake(fake);
  await mount("`flowchart TD` and then\n\n```js\nconst x = 1;\n```\n");

  assert.equal(fake.calls.length, 0);
  assert.match(container.innerHTML, /<code class="bg-raised px-1 py-px font-mono text-\[0\.9em\] text-fg shadow-\[inset_0_0_0_1px_var\(--color-border\)\]">flowchart TD<\/code>/);
  assert.match(container.innerHTML, /language-js/);
  assert.equal(container.querySelector("[data-mermaid-diagram]"), null);
});

/* ── the per-block source control ───────────────────────────────────────── */

test("the source control flips one block to its code and back", async () => {
  const fake = fakeMermaid();
  useFake(fake);
  await mount(FLOWCHART);

  const button = () => container.querySelector<HTMLButtonElement>("[data-mermaid-diagram] button");
  assert.equal(button()?.textContent, "source");
  assert.equal(button()?.title, "show source");

  await act(async () => void button()!.dispatchEvent(new window.MouseEvent("click", { bubbles: true, cancelable: true })));
  assert.equal(button()?.textContent, "diagram");
  assert.equal(button()?.title, "show diagram");
  assert.equal(diagramImage(), null);
  assert.match(container.textContent ?? "", /flowchart TD/);
  assert.match(container.textContent ?? "", /A-->B/, "the code can be read and copied");

  await act(async () => void button()!.dispatchEvent(new window.MouseEvent("click", { bubbles: true, cancelable: true })));
  assert.ok(diagramImage());
});

/* ── streaming ──────────────────────────────────────────────────────────── */

test("a streaming fence is drawn once its text settles, and the diagram stays up while the text moves", async () => {
  const fake = fakeMermaid({ fail: (code) => code.includes("BROKEN") });
  useFake(fake);
  const start = "```mermaid\nflowchart TD\n  A-->B";
  await mount(start);
  assert.equal(fake.calls.length, 1, "the first frame is drawn straight away");
  const first = diagramImage()?.src;
  assert.ok(first);

  await show(`${start}\n  B-->C`);
  await show(`${start}\n  B-->C\n  C-->D`);
  await flush(100);
  assert.equal(fake.calls.length, 1, "tokens inside the wait are not rendered one by one");
  assert.equal(diagramImage()?.src, first, "the last diagram stays up, rather than flipping to code");
  assert.equal(container.querySelector("pre"), null);

  await flush(MERMAID_STABLE_MS + 150);
  assert.equal(fake.calls.length, 2);
  assert.match(fake.calls[1]!.code, /C-->D/, "the settled text is what is drawn");
  assert.notEqual(diagramImage()?.src, first, "and replaces the picture");

  // Text that does not draw: the diagram stays while it may still be arriving…
  await show(`${start}\n  B-->C\n  C-->D\n  BROKEN`);
  await flush(100);
  assert.ok(diagramImage(), "still the last good diagram");
  // …and gives way to the code once it has stood still and failed.
  await flush(MERMAID_STABLE_MS + 150);
  assert.equal(fake.calls.length, 3);
  assert.equal(container.querySelector("[data-mermaid-diagram]"), null);
  assert.match(container.innerHTML, /language-mermaid/);
  assert.match(container.textContent ?? "", /BROKEN/);
});

/* ── the engine and the queue ───────────────────────────────────────────── */

test("an engine that failed to load is fetched again for the next diagram", async () => {
  const fake = fakeMermaid();
  let attempts = 0;
  setMermaidForTests({
    load: (() => {
      attempts += 1;
      return attempts === 1 ? Promise.reject(new Error("chunk failed to load")) : fake.load();
    }) as never,
  });
  await mount(FLOWCHART);
  assert.equal(attempts, 1);
  assert.equal(container.querySelector("[data-mermaid-diagram]"), null, "the first block stays its code");

  await show(`${FLOWCHART}\n\n${FLOWCHART.replace("B-->C", "C-->D")}`);
  await flush();
  assert.equal(attempts, 2, "the failure was not kept");
  assert.equal(fake.calls.length, 1);
  assert.match(fake.calls[0]!.code, /C-->D/);
  assert.ok(diagramImage(), "the new block is drawn");
});

test("a block whose text moved on while it waited its turn is skipped, not drawn", async () => {
  const fake = fakeMermaid({ hold: (code) => code.includes("SLOW") });
  useFake(fake);
  const slow = "```mermaid\nflowchart TD\n  SLOW-->A\n```";
  const streaming = "```mermaid\nflowchart TD\n  X-->Y";
  await mount(`${slow}\n\n${streaming}`);
  assert.equal(fake.calls.length, 1, "the second block waits behind the first");
  assert.match(fake.calls[0]!.code, /SLOW-->A/);

  await show(`${slow}\n\n${streaming}\n  Y-->Z`);
  await flush(MERMAID_STABLE_MS + 150);
  await act(async () => fake.release());
  await flush();
  assert.equal(fake.calls.length, 2, "the superseded text was never handed to the engine");
  assert.match(fake.calls[1]!.code, /Y-->Z/);
  assert.equal(container.querySelectorAll("[data-mermaid-image] img").length, 2);
});

test("a block that left the page while it waited its turn is skipped", async () => {
  const fake = fakeMermaid({ hold: (code) => code.includes("SLOW") });
  useFake(fake);
  await mount(`\`\`\`mermaid\nflowchart TD\n  SLOW-->A\n\`\`\`\n\n${FLOWCHART}`);
  assert.equal(fake.calls.length, 1);

  await act(async () => {
    root?.unmount();
  });
  root = null;
  await act(async () => fake.release());
  await flush();
  assert.equal(fake.calls.length, 1, "nothing is drawn for a block that is gone");
});

test("a render that never finishes gives up after the time limit and does not hold up the next block", async () => {
  const fake = fakeMermaid({ never: (code) => code.includes("HANG") });
  useFake(fake, 50);
  await mount(`\`\`\`mermaid\nflowchart TD\n  HANG-->A\n\`\`\`\n\n${FLOWCHART}`);
  await flush(150);

  assert.equal(fake.calls.length, 2, "the second block got its turn");
  const blocks = [...container.children[0]!.children];
  assert.equal(blocks.length, 2);
  assert.equal(blocks[0]!.tagName, "PRE", "the one that hung is its code");
  assert.match(blocks[0]!.textContent ?? "", /HANG-->A/);
  assert.ok(blocks[1]!.querySelector("[data-mermaid-image] img"), "the next one is drawn");
});

/* ── the pref ───────────────────────────────────────────────────────────── */

test("turning diagrams off returns an open transcript to the code block", async () => {
  const fake = fakeMermaid();
  useFake(fake);
  await mount(FLOWCHART);
  assert.ok(diagramImage());

  await show(FLOWCHART, false);
  assert.match(container.innerHTML, /language-mermaid/);
  assert.equal(container.querySelector("[data-mermaid-diagram]"), null);
  assert.equal(diagramImage(), null);
});

test("a view draws diagrams only once its prefs are in, and not when they say source", () => {
  assert.equal(drawsDiagrams(undefined), false, "prefs still loading, or unreadable");
  assert.equal(drawsDiagrams({ diagrams: "source" }), false);
  assert.equal(drawsDiagrams({ diagrams: "rendered" }), true);
  assert.equal(drawsDiagrams({}), true, "a server from before the pref");
});
