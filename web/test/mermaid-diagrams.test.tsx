// Pele in the transcript, behind the `diagrams` pref (Settings → message
// rendering; `rendered` unless the device's server says `source`).
//
// The bug this feature answers: an agent writes a ```mermaid flowchart in its
// reply and the transcript shows the fence as code. What is pinned here:
//   · with diagrams off (`source`, or outside a view that read the pref) a
//     mermaid fence is byte-for-byte the code block every other fence is, and
//     the engine is never imported;
//   · with them on, the fence goes through the diagram renderer and is shown
//     as an `<img>` of the SVG, themed from the page's tokens;
//   · the options the renderer relies on — no links, no images, bounded
//     source/edges/output — are what the real package is given, and a fence's
//     own init directive or frontmatter cannot change them or the theme;
//   · a fence Pele rejects is the code block again, without an error banner;
//   · inline code and every other language are untouched;
//   · a streaming fence is handed to the renderer once its text settles, and a
//     diagram already up stays up while the text moves; the per-block
//     "source" control flips only that one;
//   · a failed engine import is retried, and blocks do not wait for one
//     another (the engine is synchronous once loaded: no queue, no timer);
//   · turning diagrams off puts the code back in an open transcript.
// Where the pref comes from (the server, the settings row) is pinned in
// `crates/droppen-workbench/src/prefs` and `test/settings-message-rendering.test.tsx`.
//
// jsdom rather than `react-dom/server`, because the diagram arrives through an
// effect and a dynamic import. Pele lays nothing out in jsdom but it draws
// without a DOM, so the security and theme tests run the real package; the
// fake engine is used only where a test has to count, hold or fail a render.
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test, { after, beforeEach } from "node:test";
import type { RenderOptions } from "pele/lazy";
import { installJsdom } from "./jsdom.js";
import { makeI18n } from "./render.js";

const { window, document, container } = installJsdom();
after(() => {
  (window as unknown as { close(): void }).close();
  setTimeout(() => process.exit(), 50);
});

// The app's own tokens (`src/index.css`), as a real stylesheet so jsdom's
// computed custom properties follow the `.dark` class the theme switch sets.
const tokens = document.createElement("style");
tokens.textContent = `
:root {
  --color-bg: #ffffff;
  --color-raised: #f6f6f6;
  --color-hover: #f0f0f0;
  --color-border-strong: #a3a3a3;
  --color-fg: #1a1e23;
  --color-muted: #666666;
  --color-danger: #d42a1f;
  --radius: 0px;
  --font-sans: "IBM Plex Sans", "IBM Plex Sans SC", system-ui, sans-serif;
  --font-mono: ui-monospace, "SF Mono", Menlo, monospace;
}
.dark {
  --color-bg: #1a1e23;
  --color-raised: #23282f;
  --color-hover: #2b3138;
  --color-border-strong: #626b76;
  --color-fg: #e9ebee;
  --color-muted: #a6adb7;
  --color-danger: #ff665b;
}`;
document.head.appendChild(tokens);

const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
type Root = import("react-dom/client").Root;
const { I18nextProvider } = await import("react-i18next");
const { DiagramRendering, drawsDiagrams, Markdown } = await import("../src/components/markdown.js");
const { MERMAID_STABLE_MS, diagramPicture, diagramSettings, renderMermaidDiagram, setMermaidForTests } = await import(
  "../src/lib/mermaid-render.js"
);
const { PeleError, renderAsync } = await import("pele/lazy");

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

/** Wait for a condition the engine's dynamic import makes asynchronous. */
async function until(done: () => boolean, ms = 3000): Promise<void> {
  const start = Date.now();
  while (!done() && Date.now() - start < ms) await flush(25);
}

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
  options: RenderOptions;
  code: string;
}

/**
 * The engine, as far as `lib/mermaid-render.ts` can tell. `hold` keeps a
 * render waiting until the returned release is called; `never` makes one that
 * does not settle at all; `fail` rejects.
 */
function fakeEngine(options: { fail?: (code: string) => boolean; hold?: (code: string) => boolean } = {}) {
  const calls: Call[] = [];
  const held: (() => void)[] = [];
  let loads = 0;
  const api = {
    async renderAsync(code: string, next: RenderOptions = {}) {
      calls.push({ code, options: next });
      if (options.hold?.(code)) await new Promise<void>((resolve) => held.push(resolve));
      if (options.fail?.(code)) throw new Error("cannot parse this diagram");
      const safe = code.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
      // The shape Pele gives: absolute width/height, the same box in the viewBox.
      return {
        svg: `<svg id="${next.idPrefix ?? "d"}" width="120" height="80" xmlns="http://www.w3.org/2000/svg" style="max-width: 120px; height: auto;" viewBox="0 0 120 80"><title>${safe}</title><text>${safe}</text></svg>`,
        width: 120,
        height: 80,
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

/** Hand `lib/mermaid-render.ts` a fake engine. */
function useFake(fake: ReturnType<typeof fakeEngine>) {
  setMermaidForTests({ load: fake.load as never });
}

const diagramImage = () => container.querySelector<HTMLImageElement>("[data-mermaid-image] img");

/* ── off: today's rendering, byte for byte ──────────────────────────────── */

for (const [name, diagrams] of [
  ["set to source", false],
  ["outside a view that read the pref", null],
] as const) {
  test(`with diagrams off (${name}) a mermaid fence is the code block any fence is, and nothing is loaded`, async () => {
    const fake = fakeEngine();
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

test("every diagram is drawn without links or images, within fixed limits, in the page's font", () => {
  const { options, style } = diagramSettings("light", "droppen-diagram-7");
  assert.equal(options.links, false, "no link a reader could follow");
  assert.equal(options.images, false, "nothing fetched when the picture is shown");
  assert.equal(options.limit, 50_000, "source cap");
  assert.equal(options.maxEdges, 500, "edge cap");
  assert.equal(options.outputLimit, 2_000_000, "output cap");
  assert.equal(options.fontSize, 13, "the transcript's body size");
  assert.equal(options.responsive, false, "natural size, scrolled in the message");
  assert.equal(options.idPrefix, "droppen-diagram-7");
  assert.equal(options.maxWidth, undefined, "no container adaptation");
  assert.equal(options.measurer, undefined, "Pele measures with the canvas, or its table");
  assert.equal(options.config, undefined, "a fence's config is the only config");
  assert.equal(options.icons, undefined, "no icon resolver");
  assert.equal(options.math, undefined, "no typesetting of $$…$$");
  assert.equal(options.fontFamily, style["--pele-font"]);
  assert.equal(options.fontFamilyMono, style["--pele-font-mono"]);
  assert.match(options.fontFamily ?? "", /system-ui/, "the families an <img> can resolve");
  assert.doesNotMatch(options.fontFamily ?? "", /IBM Plex/, "the page's webfont cannot be fetched inside the picture");
  assert.equal(style["--pele-fg"], "#1a1e23");
  assert.equal(style["--pele-muted"], "#666666");
  assert.equal(style["--pele-line"], "#1a1e23");
  assert.equal(style["--pele-surface"], "#ffffff");
  assert.equal(style["--pele-surface-alt"], "#f0f0f0");
  assert.equal(style["--pele-border"], "#a3a3a3");
  assert.equal(style["--pele-accent"], "#1a1e23");
  assert.equal(style["--pele-critical"], "#d42a1f");
  assert.equal(style["--pele-radius"], "0px");
  assert.equal(style["color-scheme"], "light");
});

test("the picture's colours are the dark sheet's when the page is dark", () => {
  document.documentElement.classList.add("dark");
  try {
    const { style } = diagramSettings("dark");
    assert.equal(style["--pele-bg"], "#23282f");
    assert.equal(style["--pele-fg"], "#e9ebee");
    assert.equal(style["--pele-muted"], "#a6adb7");
    assert.equal(style["--pele-border"], "#626b76");
    assert.equal(style["--pele-critical"], "#ff665b");
    assert.equal(style["color-scheme"], "dark");
  } finally {
    document.documentElement.classList.remove("dark");
  }
});

test("the real engine drops links and images under these options, and draws them only when allowed", async () => {
  const { options } = diagramSettings("light");
  const click = 'flowchart LR\n  A[Start] --> B[End]\n  click A "https://example.com/click" "tip"';
  const bare = await renderAsync(click, options);
  assert.doesNotMatch(bare.svg, /<a\b|href=/, "no link in the picture");
  assert.deepEqual(bare.links, []);
  const linked = await renderAsync(click, { ...options, links: true });
  assert.match(linked.svg, /<a\b/, "the same fence would draw a link if links were on");
  assert.match(linked.svg, /href="https:\/\/example\.com\/click"/);
  assert.equal(linked.links.length, 1);

  const image = 'flowchart LR\n  A@{ img: "https://example.com/pixel.png", label: "pic" } --> B';
  const noImage = await renderAsync(image, options);
  assert.doesNotMatch(noImage.svg, /<image\b|href=/, "nothing to fetch when the picture is shown");
  const withImage = await renderAsync(image, { ...options, images: true });
  assert.match(withImage.svg, /<image\b/, "the same fence would draw an image if images were on");
  assert.match(withImage.svg, /href="https:\/\/example\.com\/pixel\.png"/);
});

test("a fence's init directive or frontmatter cannot change theme, fonts or limits (the real engine)", async () => {
  const { options } = diagramSettings("light");
  const base = await renderAsync("flowchart TD\n  A-->B", options);
  const directive = [
    '%%{init:{"theme":"forest","fontFamily":"Comic Sans MS","themeCSS":"@keyframes spin{}",',
    '"htmlLabels":true,"securityLevel":"loose","secure":[],"fontSize":30,"layout":"elk",',
    '"look":"handDrawn","maxEdges":99999,"limit":99999,',
    '"themeVariables":{"primaryColor":"#ff0000","fontSize":"30px"}}}%%',
    "\nflowchart TD\n  A-->B",
  ].join("");
  const frontmatter = [
    "---",
    "config:",
    "  theme: forest",
    "  fontFamily: Comic Sans MS",
    "  themeCSS: '@keyframes spin{}'",
    "  htmlLabels: true",
    "  securityLevel: loose",
    "  fontSize: 30",
    "  layout: elk",
    "  look: handDrawn",
    "  maxEdges: 99999",
    "  limit: 99999",
    "  themeVariables:",
    '    primaryColor: "#ff0000"',
    "    fontSize: 30px",
    "---",
    "flowchart TD\n  A-->B",
  ].join("\n");
  for (const [name, fence] of [
    ["init directive", directive],
    ["frontmatter", frontmatter],
  ] as const) {
    const rendered = await renderAsync(fence, options);
    assert.equal(rendered.svg, base.svg, `${name}: the picture is the one the options ask for`);
    assert.equal(rendered.width, base.width, `${name}: same font metrics`);
    assert.equal(rendered.height, base.height, `${name}: same font metrics`);
  }
});

test("a fence past the source, edge or output limit is a limit error (the real engine)", async () => {
  const { options } = diagramSettings("light");
  const long = `flowchart TD\n  A["${"x".repeat(50_000)}"] --> B`;
  await assert.rejects(renderAsync(long, options), (error: unknown) => error instanceof PeleError && error.code === "limit");

  const edges = `%%{init:{"maxEdges":99999}}%%\nflowchart TD\n${Array.from({ length: 501 }, (_, i) => `  A${i} --> A${i + 1}`).join("\n")}`;
  await assert.rejects(renderAsync(edges, options), (error: unknown) => error instanceof PeleError && error.code === "limit");
});

test("a fence Pele cannot read is a syntax error, and a render leaves nothing in the page (the real engine)", async () => {
  await assert.rejects(
    renderAsync("flowchart TD\n  A--", diagramSettings("light").options),
    (error: unknown) => error instanceof PeleError && error.code === "syntax",
  );
  const before = document.body.innerHTML;
  await assert.rejects(renderMermaidDiagram("flowchart TD\n  A--", "light"));
  await renderMermaidDiagram("flowchart TD\n  A-->B", "light");
  assert.equal(document.body.innerHTML, before, "no error picture, no scratch node");
});

/* ── the picture ────────────────────────────────────────────────────────── */

test("the picture is the SVG as well-formed XML at its natural size, titled and themed", async () => {
  const picture = diagramPicture(
    '<svg id="d1" width="100%" height="auto" xmlns="http://www.w3.org/2000/svg" style="max-width: 640.5px; height: auto;" viewBox="-8 -8 640.5 200">' +
      '<title id="chart-title-d1">Request flow</title><style>#d1{fill:#333;}</style>' +
      '<g class="pele-node"><text>Request flow</text></g></svg>',
    { "--pele-fg": "#1a1e23", "--pele-font": "system-ui" },
  );
  assert.equal(picture.width, 640.5);
  assert.equal(picture.height, 200);
  assert.equal(picture.title, "Request flow");
  assert.equal(picture.blob.type, "image/svg+xml");
  const xml = await picture.blob.text();
  const parsed = new DOMParser().parseFromString(xml, "image/svg+xml");
  assert.equal(parsed.querySelector("parsererror"), null, "an image decoder can read it");
  const svg = parsed.documentElement;
  assert.equal(svg.getAttribute("width"), "640.5");
  assert.equal(svg.getAttribute("height"), "200");
  assert.doesNotMatch(svg.getAttribute("style") ?? "", /max-width/);
  assert.doesNotMatch(svg.getAttribute("style") ?? "", /(^|;)height:/);
  assert.match(svg.getAttribute("style") ?? "", /--pele-fg: ?#1a1e23/);
  assert.match(svg.getAttribute("style") ?? "", /--pele-font: ?system-ui/);

  // Both sides in pixels and no viewBox: the attributes are the size.
  const sized = diagramPicture('<svg xmlns="http://www.w3.org/2000/svg" width="300" height="150"></svg>');
  assert.deepEqual([sized.width, sized.height, sized.title], [300, 150, null]);
  assert.throws(() => diagramPicture('<svg xmlns="http://www.w3.org/2000/svg" width="100%"></svg>'), /without a size/);
});

/* ── on: the diagram renderer ───────────────────────────────────────────── */

test("with diagrams on, the fence goes through the renderer and is shown as a picture", async () => {
  const fake = fakeEngine();
  useFake(fake);
  // A finished message: drawn straight away, without waiting for its text to settle.
  await mount(FLOWCHART);

  assert.equal(fake.calls.length, 1);
  assert.equal(fake.calls[0]!.code.trim(), "flowchart TD\n  A-->B\n  B-->C");
  assert.equal(fake.calls[0]!.options.links, false, "agent output is untrusted, never linked");
  const img = diagramImage();
  assert.ok(img, "the diagram is on the page");
  assert.match(img.src, /^blob:/, "as a picture from a blob URL");
  assert.equal(img.getAttribute("width"), "120", "at its natural width, from the viewBox");
  assert.equal(img.getAttribute("height"), "80");
  assert.equal(img.alt, "flowchart TD\n  A-->B\n  B-->C", "the fake's title stands in for accTitle");
  assert.equal(container.querySelector("[data-mermaid-image] svg"), null, "no SVG markup in the page");
  assert.equal(container.querySelector("pre"), null, "the source is replaced, not shown under it");
});

test("the real engine draws a finished fence without a DOM and it is shown as a picture", async () => {
  await mount(FLOWCHART);
  await until(() => diagramImage() !== null);
  const img = diagramImage();
  assert.ok(img, "the diagram is on the page");
  assert.match(img.src, /^blob:/);
  assert.equal(img.alt, "Mermaid diagram", "no accTitle, so the shared label");
  assert.equal(container.querySelector("[data-mermaid-image] svg"), null);
  assert.equal(container.querySelector("pre"), null);
});

test("a fence the real engine cannot read is the plain code block, with no error banner", async () => {
  await mount("```mermaid\nflowchart TD\n  A--\n```");
  // The dynamic import and the failed render have both finished when the
  // block is still its code and no engine work remains; a fixed wait would be
  // either slow or flaky, so wait for the code to have been there a moment.
  await flush(500);

  assert.match(container.innerHTML, /language-mermaid/);
  assert.match(container.innerHTML, /flowchart TD/);
  assert.equal(container.querySelector("[data-mermaid-diagram]"), null);
  assert.doesNotMatch(container.textContent ?? "", /cannot parse|error|failed/i);
});

test("a diagram with an accTitle is titled by it (the real engine)", async () => {
  await mount("```mermaid\nsequenceDiagram\n  accTitle: Request flow\n  Client->>Server: GET /x\n```");
  await until(() => diagramImage()?.alt === "Request flow");
  assert.equal(diagramImage()?.alt, "Request flow");
});

test("the diagram follows a theme change, and a picture no longer shown is released", async () => {
  const fake = fakeEngine();
  useFake(fake);
  const pictures: Blob[] = [];
  const revoked: string[] = [];
  const create = URL.createObjectURL;
  const revoke = URL.revokeObjectURL;
  URL.createObjectURL = (blob: Blob) => {
    pictures.push(blob);
    return create(blob);
  };
  URL.revokeObjectURL = (url: string) => {
    revoked.push(url);
    revoke(url);
  };
  try {
    document.documentElement.classList.add("dark");
    await mount(FLOWCHART);
    assert.equal(fake.calls.length, 1);
    assert.match(await pictures.at(-1)!.text(), /--pele-fg:\s*#e9ebee/, "the dark sheet's ink");
    const dark = diagramImage()!.src;

    await act(async () => {
      document.documentElement.classList.remove("dark");
    });
    await flush();
    assert.equal(fake.calls.length, 2, "the diagram is drawn again for the other sheet");
    assert.match(await pictures.at(-1)!.text(), /--pele-fg:\s*#1a1e23/, "the light sheet's ink");
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
    URL.createObjectURL = create;
    URL.revokeObjectURL = revoke;
  }
});

test("a fence the engine cannot read is the plain code block, with no error banner", async () => {
  const fake = fakeEngine({ fail: () => true });
  useFake(fake);
  await mount("```mermaid\nflowchart TD\n  A--\n```");

  assert.match(container.innerHTML, /language-mermaid/);
  assert.match(container.innerHTML, /flowchart TD/);
  assert.equal(container.querySelector("[data-mermaid-diagram]"), null);
  assert.doesNotMatch(container.textContent ?? "", /cannot parse|error|failed/i);
});

test("inline code and other fenced languages stay exactly as they were, even with diagrams on", async () => {
  const fake = fakeEngine();
  useFake(fake);
  await mount("`flowchart TD` and then\n\n```js\nconst x = 1;\n```\n");

  assert.equal(fake.calls.length, 0);
  assert.match(container.innerHTML, /<code class="bg-raised px-1 py-px font-mono text-\[0\.9em\] text-fg shadow-\[inset_0_0_0_1px_var\(--color-border\)\]">flowchart TD<\/code>/);
  assert.match(container.innerHTML, /language-js/);
  assert.equal(container.querySelector("[data-mermaid-diagram]"), null);
});

/* ── the per-block source control ───────────────────────────────────────── */

test("the source control flips one block to its code and back", async () => {
  const fake = fakeEngine();
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
  const fake = fakeEngine({ fail: (code) => code.includes("BROKEN") });
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

test("a result whose block has moved on is not drawn", async () => {
  const fake = fakeEngine({ hold: (code) => code.includes("SLOW") });
  useFake(fake);
  const slow = "```mermaid\nflowchart TD\n  SLOW-->A\n```";
  await mount(slow);
  assert.equal(fake.calls.length, 1);
  assert.equal(diagramImage(), null, "the held render has not finished");

  await show("```mermaid\nflowchart TD\n  FAST-->B\n```");
  await flush(MERMAID_STABLE_MS + 100);
  assert.equal(fake.calls.length, 2, "the new text is not queued behind the old render");
  assert.equal(diagramImage()?.alt, "flowchart TD\n  FAST-->B");

  await act(async () => fake.release());
  await flush();
  assert.equal(diagramImage()?.alt, "flowchart TD\n  FAST-->B", "the superseded picture never replaces it");
});

test("a block that left the page while its render was running is not drawn", async () => {
  const fake = fakeEngine({ hold: (code) => code.includes("SLOW") });
  useFake(fake);
  await mount("```mermaid\nflowchart TD\n  SLOW-->A\n```");
  assert.equal(fake.calls.length, 1);

  await act(async () => {
    root?.unmount();
  });
  root = null;
  await act(async () => fake.release());
  await flush();
  assert.equal(container.querySelector("[data-mermaid-image]"), null);
});

test("one block's render does not hold up another block (there is no queue)", async () => {
  const fake = fakeEngine({ hold: (code) => code.includes("SLOW") });
  useFake(fake);
  await mount(`\`\`\`mermaid\nflowchart TD\n  SLOW-->A\n\`\`\`\n\n${FLOWCHART}`);
  await flush();

  assert.equal(fake.calls.length, 2, "both renders started");
  const blocks = [...container.children[0]!.children];
  assert.equal(blocks.length, 2);
  assert.equal(blocks[0]!.tagName, "PRE", "the held one is its code");
  assert.match(blocks[0]!.textContent ?? "", /SLOW-->A/);
  assert.ok(blocks[1]!.querySelector("[data-mermaid-image] img"), "the second one is already drawn");

  await act(async () => fake.release());
  await flush();
  const finished = [...container.children[0]!.children];
  assert.ok(finished[0]!.querySelector("[data-mermaid-image] img"), "and the first follows when it finishes");
});

/* ── the engine ─────────────────────────────────────────────────────────── */

test("an engine that failed to load is fetched again for the next diagram", async () => {
  const fake = fakeEngine();
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

/* ── the pref ───────────────────────────────────────────────────────────── */

test("turning diagrams off returns an open transcript to the code block", async () => {
  const fake = fakeEngine();
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
