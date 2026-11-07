// Pele in the transcript, behind the `diagrams` pref (Settings → message
// rendering; `rendered` unless the device's server says `source`).
//
// The bug this feature answers: an agent writes a ```mermaid flowchart in its
// reply and the transcript shows the fence as code. What is pinned here:
//   · with diagrams off (`source`, or outside a view that read the pref) a
//     mermaid fence is byte-for-byte the code block every other fence is, and
//     no worker is made;
//   · with them on, the fence goes through the worker renderer and is shown
//     as an `<img>` of the SVG, themed from the page's tokens;
//   · the options the renderer relies on — no links, no images, bounded
//     source/edges/output — are what the real package is given; a fence's own
//     init directive or frontmatter cannot change them or the theme, and its
//     series palette is stripped before Pele sees it;
//   · a fence Pele rejects, a worker that times out, and a source over the
//     length cap are the code block again, without an error banner;
//   · the client queues one render at a time, terminates a timed-out worker,
//     makes a fresh one for the next diagram, and caches both pictures and
//     failures by fence text (a theme toggle never re-renders);
//   · inline code and every other language are untouched;
//   · a streaming fence is handed to the renderer once its text settles, and a
//     diagram already up stays up while the text moves; the per-block
//     "source" control flips only that one;
//   · turning diagrams off puts the code back in an open transcript.
// Where the pref comes from (the server, the settings row) is pinned in
// `crates/droppen-workbench/src/prefs` and `test/settings-message-rendering.test.tsx`.
//
// jsdom rather than `react-dom/server`, because the diagram arrives through an
// effect and a dynamic import. jsdom has no Worker, so the client's queue,
// timeout and message mapping are exercised through `setMermaidForTests`; the
// security and palette tests run the real package through `renderDiagram`.
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
const {
  MERMAID_STABLE_MS,
  DiagramRenderError,
  diagramPicture,
  diagramSettings,
  hasLongWhitespaceRun,
  isDiagramWhitespace,
  renderMermaidDiagram,
  setMermaidForTests,
} = await import("../src/lib/mermaid-render.js");
const { assertNoFencePalette, renderDiagram } = await import("../src/lib/diagram-render.js");
const { withoutFenceThemeVariables } = await import("../src/lib/fence-config.js");
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
const tick = () => new Promise<void>((resolve) => setImmediate(resolve));

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

/* ── the worker seam ────────────────────────────────────────────────────── */

type Behavior = (worker: FakeWorker, message: import("../src/lib/diagram-protocol.js").DiagramWorkerRequest) => void;

class FakeWorker {
  readonly posted: import("../src/lib/diagram-protocol.js").DiagramWorkerRequest[] = [];
  terminated = false;
  private readonly messages = new Set<(event: MessageEvent) => void>();
  private readonly errors = new Set<(event: Event) => void>();

  constructor(private readonly behavior: Behavior) {}

  postMessage(message: import("../src/lib/diagram-protocol.js").DiagramWorkerRequest): void {
    this.posted.push(message);
    this.behavior(this, message);
  }

  terminate(): void {
    this.terminated = true;
  }

  addEventListener(type: string, listener: (event: never) => void): void {
    (type === "message" ? this.messages : this.errors).add(listener as never);
  }

  removeEventListener(type: string, listener: (event: never) => void): void {
    (type === "message" ? this.messages : this.errors).delete(listener as never);
  }

  reply(reply: import("../src/lib/diagram-protocol.js").DiagramWorkerReply): void {
    for (const listener of [...this.messages]) listener({ data: reply } as MessageEvent);
  }

  fail(): void {
    for (const listener of [...this.errors]) listener({} as Event);
  }
}

function fakeSvg(code: string): string {
  const safe = code.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  return `<svg width="120" height="80" xmlns="http://www.w3.org/2000/svg" style="max-width: 120px; height: auto;" viewBox="0 0 120 80"><title>${safe}</title><text>${safe}</text></svg>`;
}

function success(worker: FakeWorker, message: import("../src/lib/diagram-protocol.js").DiagramWorkerRequest): void {
  queueMicrotask(() => {
    worker.reply({ phase: "started" });
    worker.reply({ phase: "loading" });
    worker.reply({ phase: "rendering" });
    worker.reply({ ok: true, svg: fakeSvg(message.code), width: 120, height: 80 });
  });
}

function failure(code: "syntax" | "limit" | "semantic" | "unsupported-diagram" | "error") {
  return (worker: FakeWorker): void => {
    queueMicrotask(() => {
      worker.reply({ phase: "started" });
      worker.reply({ phase: "loading" });
      worker.reply({ phase: "rendering" });
      worker.reply({ ok: false, code, message: `cannot draw (${code})` });
    });
  };
}

interface FakeEngine {
  workers: FakeWorker[];
  factory: () => FakeWorker;
  release: () => void;
}

/**
 * A fake worker factory. `"hold"` keeps every reply until `release()` (the
 * load phase's `rendering` too); a behavior replies itself.
 */
function fakeEngine(behavior: Behavior | "hold"): FakeEngine {
  const workers: FakeWorker[] = [];
  const held: (() => void)[] = [];
  const run: Behavior = (worker, message) => {
    if (behavior === "hold") {
      held.push(() => {
        worker.reply({ phase: "started" });
        worker.reply({ phase: "loading" });
        worker.reply({ phase: "rendering" });
        worker.reply({ ok: true, svg: fakeSvg(message.code), width: 120, height: 80 });
      });
      return;
    }
    behavior(worker, message);
  };
  return {
    workers,
    factory: () => {
      const worker = new FakeWorker(run);
      workers.push(worker);
      return worker;
    },
    release: () => held.splice(0).forEach((send) => send()),
  };
}

function installWorker(engine: FakeEngine, timers: { loadTimeoutMs?: number; renderTimeoutMs?: number } = {}): void {
  setMermaidForTests({ createWorker: engine.factory, ...timers });
}

const diagramImage = () => container.querySelector<HTMLImageElement>("[data-mermaid-image] img");

/* ── off: today's rendering, byte for byte ──────────────────────────────── */

for (const [name, diagrams] of [
  ["set to source", false],
  ["outside a view that read the pref", null],
] as const) {
  test(`with diagrams off (${name}) a mermaid fence is the code block any fence is, and no worker is made`, async () => {
    const engine = fakeEngine(success);
    installWorker(engine);
    await mount(`${FLOWCHART}\n\n${FLOWCHART.replace("mermaid", "rust")}`, diagrams);
    await flush(MERMAID_STABLE_MS + 100);

    assert.equal(engine.workers.length, 0, "the worker is never made");
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
  assert.equal(options.measurer, undefined, "the worker measures with OffscreenCanvas, or Pele's table");
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
  assert.equal(style["--pele-fg-on-light"], "#1a1e23", "the light sheet's ink");
  assert.equal(style["--pele-fg-on-dark"], "#ffffff", "the light sheet's paper");
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
    assert.equal(style["--pele-fg-on-light"], "#1a1e23", "the dark sheet's canvas is the dark ink");
    assert.equal(style["--pele-fg-on-dark"], "#e9ebee", "and its ink the light ink");
    assert.equal(style["color-scheme"], "dark");
  } finally {
    document.documentElement.classList.remove("dark");
  }
});

test("the real engine drops links and images under these options, and draws them only when allowed", async () => {
  const { options } = diagramSettings("light");
  const click = 'flowchart LR\n  A[Start] --> B[End]\n  click A "https://example.com/click" "tip"';
  const bare = await renderDiagram(click, options);
  assert.doesNotMatch(bare.svg, /<a\b|href=/, "no link in the picture");
  const linked = await renderDiagram(click, { ...options, links: true });
  assert.match(linked.svg, /<a\b/, "the same fence would draw a link if links were on");
  assert.match(linked.svg, /href="https:\/\/example\.com\/click"/);

  const image = 'flowchart LR\n  A@{ img: "https://example.com/pixel.png", label: "pic" } --> B';
  const noImage = await renderDiagram(image, options);
  assert.doesNotMatch(noImage.svg, /<image\b|href=/, "nothing to fetch when the picture is shown");
  const withImage = await renderDiagram(image, { ...options, images: true });
  assert.match(withImage.svg, /<image\b/, "the same fence would draw an image if images were on");
  assert.match(withImage.svg, /href="https:\/\/example\.com\/pixel\.png"/);
});

test("a fence's init directive or frontmatter cannot change theme, fonts or limits (the real engine)", async () => {
  const { options } = diagramSettings("light");
  const base = await renderDiagram("flowchart TD\n  A-->B", options);
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
    const rendered = await renderDiagram(fence, options);
    assert.equal(rendered.svg, base.svg, `${name}: the picture is the one the options ask for`);
    assert.equal(rendered.width, base.width, `${name}: same font metrics`);
    assert.equal(rendered.height, base.height, `${name}: same font metrics`);
  }
});

test("a fence past the source, edge or output limit is a limit error (the real engine)", async () => {
  const { options } = diagramSettings("light");
  const long = `flowchart TD\n  A["${"x".repeat(50_000)}"] --> B`;
  await assert.rejects(renderDiagram(long, options), (error: unknown) => error instanceof PeleError && error.code === "limit");

  const edges = `%%{init:{"maxEdges":99999}}%%\nflowchart TD\n${Array.from({ length: 501 }, (_, i) => `  A${i} --> A${i + 1}`).join("\n")}`;
  await assert.rejects(renderDiagram(edges, options), (error: unknown) => error instanceof PeleError && error.code === "limit");

  // A 20-character source that draws an SVG past 2,000,000 characters.
  await assert.rejects(
    renderDiagram('packet\n0-320000: "x"', options),
    (error: unknown) => error instanceof PeleError && error.code === "limit",
  );
});

test("a fence Pele cannot read is a syntax error, and a render leaves nothing in the page (the real engine)", async () => {
  const { options } = diagramSettings("light");
  await assert.rejects(renderDiagram("flowchart TD\n  A--", options), (error: unknown) => error instanceof PeleError && error.code === "syntax");
  await assert.rejects(renderDiagram("this is not a diagram", options), (error: unknown) => error instanceof PeleError && error.code === "unsupported-diagram");
  const before = document.body.innerHTML;
  await renderDiagram("flowchart TD\n  A-->B", options);
  assert.equal(document.body.innerHTML, before, "no error picture, no scratch node");
});

/* ── a fence's series palette ────────────────────────────────────────────── */

test("themeVariables is stripped from init directives and frontmatter, leaving other config alone", () => {
  const init = withoutFenceThemeVariables(
    '%%{init: {"themeVariables":{"pie1":"transparent"},"flowchart":{"curve":"step"}}}%%\npie title P\n  "A": 1',
  );
  assert.doesNotMatch(init, /themeVariables/);
  assert.match(init, /"curve":"step"/);

  const block = withoutFenceThemeVariables(
    "---\ntitle: P\nconfig:\n  themeVariables:\n    pie1: transparent\n  gantt:\n    displayMode: compact\n---\npie title P\n  \"A\": 1",
  );
  assert.doesNotMatch(block, /themeVariables|transparent/);
  assert.match(block, /title: P/);
  assert.match(block, /gantt:\n    displayMode: compact/);

  const flow = withoutFenceThemeVariables(
    "---\nconfig: {themeVariables: {pie1: transparent}, gantt: {displayMode: compact}}\n---\npie title P\n  \"A\": 1",
  );
  assert.doesNotMatch(flow, /themeVariables|transparent/);
  assert.match(flow, /gantt: \{displayMode: compact\}/);

  const quoted = withoutFenceThemeVariables('---\nconfig:\n  "themeVariables":\n    pie1: transparent\n---\npie\n"A": 1');
  assert.doesNotMatch(quoted, /themeVariables|transparent/);
  assert.match(quoted, /^---\nconfig:\n---/);

  // Not a config key: a block scalar's text is untouched.
  const scalar = withoutFenceThemeVariables("---\ntitle: |\n  themeVariables: keep me\n---\npie\n\"A\": 1");
  assert.match(scalar, /themeVariables: keep me/);

  const legit = '%%{init: {"flowchart":{"curve":"step"}}}%%\nflowchart TD\n  A-->B';
  assert.equal(withoutFenceThemeVariables(legit), legit);
});

test("a fence's series palette cannot reach the output (the real engine)", async () => {
  const { options } = diagramSettings("light");
  const fences = [
    '%%{init: {"themeVariables":{"pie1":"transparent","pie2":"var(--pele-bg)"}}}%%\npie title P\n  "A": 10\n  "B": 5',
    "---\nconfig:\n  themeVariables:\n    pie1: transparent\n    pie2: var(--pele-bg)\n---\npie title P\n  \"A\": 10\n  \"B\": 5",
    "---\nconfig: {themeVariables: {pie1: transparent, pie2: var(--pele-bg)}}\n---\npie title P\n  \"A\": 10\n  \"B\": 5",
  ];
  for (const fence of fences) {
    const picture = await renderDiagram(fence, options);
    assert.doesNotMatch(picture.svg, /--pele-series-\d+:/, "no palette declaration");
    assert.doesNotMatch(picture.svg, /transparent/, "nothing hidden");
    assert.match(picture.svg, /--pele-series-1,#4c78a8/, "Pele's own first series colour");
  }

  // The output check, for a config form the source stripper does not know.
  const raw = await renderAsync(
    '%%{init: {"themeVariables":{"pie1":"transparent"}}}%%\npie\n"A": 1\n"B": 2',
    options,
  );
  assert.match(raw.svg, /--pele-series-1:transparent/);
  assert.throws(
    () => assertNoFencePalette(raw.svg),
    (error: unknown) => error instanceof PeleError && error.code === "semantic",
    "a surviving fence palette fails the render, it is not stripped",
  );
});

test("every round-2 themeVariables form is stripped or refused (the real engine)", async () => {
  const { options } = diagramSettings("light");
  const body = 'pie\n"a": 1\n"b": 2\n';
  const fm = (open: string, config: string, close = "---") => `${open}\n${config}\n${close}\n${body}`;
  const forms: Array<[string, string]> = [
    ["baseline init", `%%{init: {"themeVariables": {"pie1": "#abcdef"}}}%%\n${body}`],
    ["baseline frontmatter", fm("---", 'config:\n  themeVariables:\n    pie1: "#abcdef"')],
    ["BOM-indented frontmatter", "﻿---\n﻿config:\n﻿  themeVariables:\n﻿    pie1: \"#abcdef\"\n﻿---\n" + body],
    ["opening ---<NBSP>", fm("--- ", 'config:\n  themeVariables:\n    pie1: "#abcdef"')],
    ["closing ---<NBSP>", fm("---", 'config:\n  themeVariables:\n    pie1: "#abcdef"', "--- ")],
    ['quoted "config" key', fm("---", '"config":\n  themeVariables:\n    pie1: "#abcdef"')],
    ["flow map on next line", fm("---", 'config:\n  {themeVariables: {pie1: "#abcdef"}}')],
    ["%%{ in frontmatter title", `---\ntitle: "%%{x"\n---\n%%{init: {"themeVariables": {"pie1": "#abcdef"}}}%%\n${body}`],
    ["unclosed trailing init", `${body}%%{init: {"themeVariables": {"pie1": "#abcdef"}}}`],
    ["escaped key \\x56", fm("---", 'config:\n  "theme\\x56ariables":\n    pie1: "#abcdef"')],
    ["escaped key \\u0056 in flow", fm("---", 'config: {"theme\\u0056ariables": {pie1: "#abcdef"}}')],
    ["comment in flow map", fm("---", 'config: { # c\n  themeVariables: {pie1: "#abcdef"}}')],
    ["quoted 'config' single", fm("---", "'config':\n  themeVariables:\n    pie1: '#abcdef'")],
    ["config with trailing NBSP", fm("---", 'config: \n  themeVariables:\n    pie1: "#abcdef"')],
    ["tab-indented frontmatter", fm("\t---", '\tconfig:\n\t  themeVariables:\n\t    pie1: "#abcdef"', "\t---")],
    ["init+initialize twice", `%%{initialize: {"themeVariables": {"pie1": "#abcdef"}}}%%\n%%{init: {"x":1}}%%\n${body}`],
    ["directive with }%% in string", `%%{init: {"a":"}%%", "themeVariables": {"pie1": "#abcdef"}}}%%\n${body}`],
    ["themeVariables after a comment line", fm("---", 'config:\n  # c\n  themeVariables:\n    pie1: "#abcdef"')],
    ["xychart plotColorPalette", `%%{init: {"themeVariables": {"xyChart": {"plotColorPalette": "#abcdef"}}}}%%\nxychart\nx-axis [a,b]\nbar [1,2]\n`],
    ["unclosed initialize after frontmatter", `---\ntitle: t\n---\n${body}%%{initialize: {"themeVariables": {"pie1": "#abcdef"}}}`],
    ["explicit key ?", fm("---", 'config:\n  ? themeVariables\n  : {pie1: "#abcdef"}')],
    ["config: !!map", fm("---", 'config: !!map\n  themeVariables:\n    pie1: "#abcdef"')],
  ];
  for (const [name, source] of forms) {
    try {
      const picture = await renderDiagram(source, options);
      assert.doesNotMatch(picture.svg, /--pele-series-\d+\s*:/, `${name}: no fence palette`);
      assert.doesNotMatch(picture.svg, /#abcdef/i, `${name}: no fence colour`);
    } catch (error) {
      assert.ok(error instanceof PeleError, `${name}: a refusal is a PeleError, got ${String(error)}`);
    }
  }
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

test("with diagrams on, the fence goes through a worker and is shown as a picture", async () => {
  const engine = fakeEngine(success);
  installWorker(engine);
  // A finished message: drawn straight away, without waiting for its text to settle.
  await mount(FLOWCHART);

  assert.equal(engine.workers.length, 1);
  assert.equal(engine.workers[0]!.posted.length, 1);
  const message = engine.workers[0]!.posted[0]!;
  assert.equal(message.code.trim(), "flowchart TD\n  A-->B\n  B-->C");
  assert.equal(message.options.links, false, "agent output is untrusted, never linked");
  assert.equal(message.options.images, false);
  assert.match(message.options.idPrefix ?? "", /^droppen-diagram-\d+$/);
  const img = diagramImage();
  assert.ok(img, "the diagram is on the page");
  assert.match(img.src, /^blob:/, "as a picture from a blob URL");
  assert.equal(img.getAttribute("width"), "120", "at its natural width, from the viewBox");
  assert.equal(img.getAttribute("height"), "80");
  assert.equal(img.alt, "flowchart TD\n  A-->B\n  B-->C", "the fake's title stands in for accTitle");
  assert.equal(container.querySelector("[data-mermaid-image] svg"), null, "no SVG markup in the page");
  assert.equal(container.querySelector("pre"), null, "the source is replaced, not shown under it");
});

test("a theme switch re-bakes the picture, releases the old one, and does not render again", async () => {
  const engine = fakeEngine(success);
  installWorker(engine);
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
    await mount(FLOWCHART);
    assert.equal(engine.workers[0]!.posted.length, 1);
    assert.match(await pictures.at(-1)!.text(), /--pele-fg:\s*#1a1e23/);
    const light = diagramImage()!.src;

    await act(async () => {
      document.documentElement.classList.add("dark");
    });
    await flush();
    assert.equal(engine.workers[0]!.posted.length, 1, "the cache is re-baked, not rendered again");
    assert.match(await pictures.at(-1)!.text(), /--pele-fg:\s*#e9ebee/);
    const dark = diagramImage()!.src;
    assert.notEqual(dark, light);
    await flush(1100);
    assert.ok(revoked.includes(light), "the replaced picture is released");
    assert.ok(!revoked.includes(dark), "the one on screen is not");
  } finally {
    URL.createObjectURL = create;
    URL.revokeObjectURL = revoke;
  }
});

test("the client queues one render at a time, and a held render delays the next", async () => {
  const engine = fakeEngine("hold");
  installWorker(engine);
  const first = renderMermaidDiagram("flowchart TD\n A-->B", "light");
  const second = renderMermaidDiagram("flowchart TD\n C-->D", "light");
  await tick();
  assert.equal(engine.workers[0]!.posted.length, 1, "the second render waits its turn");
  engine.release();
  await first;
  await tick();
  assert.equal(engine.workers[0]!.posted.length, 2, "and starts once the first has settled");
  engine.release();
  await second;
  assert.equal(engine.workers.length, 1, "both use the same worker");
});

test("a timed-out render terminates its worker and the next render gets a fresh one", async () => {
  let hang = false;
  const engine = fakeEngine((worker, message) => {
    if (hang) {
      worker.reply({ phase: "started" });
      worker.reply({ phase: "loading" });
      worker.reply({ phase: "rendering" });
      return;
    }
    success(worker, message);
  });
  installWorker(engine, { renderTimeoutMs: 20, loadTimeoutMs: 100 });
  hang = true;
  await assert.rejects(
    renderMermaidDiagram("flowchart TD\n HANG-->A", "light"),
    (error: unknown) => error instanceof DiagramRenderError && error.code === "timeout",
  );
  assert.equal(engine.workers.length, 1);
  assert.equal(engine.workers[0]!.terminated, true, "the worker was killed");

  // The timeout is cached: a theme toggle must not run it again.
  await assert.rejects(
    renderMermaidDiagram("flowchart TD\n HANG-->A", "dark"),
    (error: unknown) => error instanceof DiagramRenderError && error.code === "timeout",
  );
  assert.equal(engine.workers.length, 1);

  hang = false;
  await renderMermaidDiagram("flowchart TD\n A-->B", "light");
  assert.equal(engine.workers.length, 2, "a fresh worker is made for the next diagram");
  assert.equal(engine.workers[1]!.posted.length, 1);
});

test("a worker that never starts is timed out, terminated and not cached", async () => {
  let silent = false;
  const engine = fakeEngine((worker, message) => {
    if (!silent) success(worker, message);
  });
  installWorker(engine, { renderTimeoutMs: 100, loadTimeoutMs: 20 });
  silent = true;
  const code = "flowchart TD\n SLOW-->A";
  await assert.rejects(
    renderMermaidDiagram(code, "light"),
    (error: unknown) => error instanceof DiagramRenderError && error.code === "timeout" && error.cacheable === false,
  );
  assert.equal(engine.workers[0]!.terminated, true);
  // A load timeout may be a slow network, not a broken fence: the same code
  // is tried again and gets a fresh worker.
  silent = false;
  await renderMermaidDiagram(code, "light");
  assert.equal(engine.workers.length, 2);
});

test("a load failure is not cached, and the same fence retries with a fresh worker", async () => {
  let broken = true;
  const engine = fakeEngine((worker, message) => {
    if (broken) {
      worker.reply({ phase: "started" });
      worker.reply({ phase: "loading" });
      worker.reply({ ok: false, code: "error", message: "chunk failed" });
      return;
    }
    success(worker, message);
  });
  installWorker(engine);
  const code = "flowchart TD\n A-->B";
  await assert.rejects(
    renderMermaidDiagram(code, "light"),
    (error: unknown) => error instanceof DiagramRenderError && error.code === "error" && error.cacheable === false,
  );
  assert.equal(engine.workers[0]!.terminated, false, "the worker is reused for a load failure");
  broken = false;
  await renderMermaidDiagram(code, "light");
  assert.equal(engine.workers[0]!.posted.length, 2, "the same fence is retried");
  assert.ok(await renderMermaidDiagram(code, "dark"));
});

test("the render clock pauses around the type chunk load, and counts the work before it", async () => {
  // A 50 ms render clock; the load phase takes 100 ms and must not count.
  const slowLoad = fakeEngine((worker, message) => {
    worker.reply({ phase: "started" });
    setTimeout(() => {
      worker.reply({ phase: "loading" });
      setTimeout(() => {
        worker.reply({ phase: "rendering" });
        worker.reply({ ok: true, svg: fakeSvg(message.code), width: 120, height: 80 });
      }, 100);
    }, 10);
  });
  installWorker(slowLoad, { renderTimeoutMs: 50, loadTimeoutMs: 300 });
  await renderMermaidDiagram("flowchart TD\n A-->B", "light");
  assert.equal(slowLoad.workers.length, 1);

  // A 30 ms synchronous phase before `loading` is 30 ms against a 20 ms
  // clock: the worker is killed before the load even starts.
  const slowStart = fakeEngine((worker) => {
    worker.reply({ phase: "started" });
    setTimeout(() => worker.reply({ phase: "loading" }), 30);
  });
  installWorker(slowStart, { renderTimeoutMs: 20, loadTimeoutMs: 200 });
  await assert.rejects(
    renderMermaidDiagram("flowchart TD\n C-->D", "light"),
    (error: unknown) => error instanceof DiagramRenderError && error.code === "timeout" && error.cacheable === true,
  );
  assert.equal(slowStart.workers[0]!.terminated, true);
});

test("a worker's error event terminates it, is not cached, and a fresh one draws the same fence", async () => {
  let broken = false;
  const engine = fakeEngine((worker, message) => {
    if (broken) {
      worker.reply({ phase: "started" });
      worker.reply({ phase: "loading" });
      worker.reply({ phase: "rendering" });
      worker.fail();
      return;
    }
    success(worker, message);
  });
  installWorker(engine);
  broken = true;
  const code = "flowchart TD\n A-->B";
  await assert.rejects(
    renderMermaidDiagram(code, "light"),
    (error: unknown) => error instanceof DiagramRenderError && error.code === "worker" && error.cacheable === false,
  );
  assert.equal(engine.workers[0]!.terminated, true);
  broken = false;
  await renderMermaidDiagram(code, "light");
  assert.equal(engine.workers.length, 2);
  assert.equal(engine.workers[1]!.posted.length, 1);
});

test("a Pele error is mapped to its code, and the worker is kept for the next render", async () => {
  let fail = true;
  const engine = fakeEngine((worker, message) => {
    if (fail) {
      fail = false;
      failure("syntax")(worker);
      return;
    }
    success(worker, message);
  });
  installWorker(engine);
  await assert.rejects(
    renderMermaidDiagram("flowchart TD\n A--", "light"),
    (error: unknown) => error instanceof DiagramRenderError && error.code === "syntax",
  );
  assert.equal(engine.workers[0]!.terminated, false, "the worker itself is healthy");
  await renderMermaidDiagram("flowchart TD\n A-->B", "light");
  assert.equal(engine.workers.length, 1, "and reused");
  assert.equal(engine.workers[0]!.posted.length, 2);
});

test("a source over the length cap, or with a huge whitespace run, is refused before a worker is made", async () => {
  const engine = fakeEngine(success);
  installWorker(engine);
  await assert.rejects(
    renderMermaidDiagram("x".repeat(50_001), "light"),
    (error: unknown) => error instanceof DiagramRenderError && error.code === "limit",
  );
  // 32,001 spaces is under the source cap but Pele's accDescr trim is
  // quadratic in it; the shape cap refuses it on this thread.
  await assert.rejects(
    renderMermaidDiagram("flowchart TD\n" + " ".repeat(32_001) + "A-->B", "light"),
    (error: unknown) => error instanceof DiagramRenderError && error.code === "limit",
  );
  assert.equal(engine.workers.length, 0, "no worker, no Pele preprocess");
});

test("the whitespace guard is one linear pass on 50k adversarial sources", () => {
  const exact = " ".repeat(32_000) + "x" + " ".repeat(17_900);
  const cases: Array<[string, boolean]> = [
    [exact, false],
    ["\n".repeat(32_000) + "x" + "\n".repeat(17_900), false],
    [Array.from({ length: 25 }, () => " ".repeat(1_999)).join("x"), false],
    ["　\t\n ".repeat(8_000) + "x" + " ".repeat(17_900), false],
    [" x".repeat(25_000), false],
    [" ".repeat(50_000), true],
  ];
  for (const [source, expected] of cases) {
    const start = performance.now();
    const found = hasLongWhitespaceRun(source);
    const ms = performance.now() - start;
    assert.equal(found, expected, `${source.length} chars`);
    assert.ok(ms < 100, `${source.length} chars took ${ms.toFixed(1)} ms`);
  }
  // The scan's set is exactly `\s`, not an approximation.
  for (let code = 0; code <= 0xffff; code++) {
    assert.equal(isDiagramWhitespace(code), /\s/.test(String.fromCharCode(code)), `U+${code.toString(16)}`);
  }
});

test("a cached adversarial fence is not re-scanned on a theme toggle", async () => {
  const engine = fakeEngine(success);
  installWorker(engine);
  const probe = 'pie\n"a": 1\n%%' + " ".repeat(32_000) + "x" + " ".repeat(17_900);
  await renderMermaidDiagram(probe, "light");
  assert.equal(engine.workers[0]!.posted.length, 1);
  const start = performance.now();
  await renderMermaidDiagram(probe, "dark");
  const ms = performance.now() - start;
  assert.equal(engine.workers[0]!.posted.length, 1, "the cache hit does not post");
  assert.ok(ms < 100, `the cached theme change took ${ms.toFixed(1)} ms`);
});

test("an aborted render terminates its worker, and a queued aborted render never posts", async () => {
  const engine = fakeEngine("hold");
  installWorker(engine);
  const live = new AbortController();
  const first = renderMermaidDiagram("flowchart TD\n A-->B", "light", live.signal);
  await tick();
  assert.equal(engine.workers[0]!.posted.length, 1);

  const queued = new AbortController();
  const second = renderMermaidDiagram("flowchart TD\n C-->D", "light", queued.signal);
  const third = renderMermaidDiagram("flowchart TD\n E-->F", "light", queued.signal);
  queued.abort();
  live.abort();
  await assert.rejects(first, (error: Error) => error.name === "AbortError");
  await assert.rejects(second, (error: Error) => error.name === "AbortError");
  await assert.rejects(third, (error: Error) => error.name === "AbortError");
  assert.equal(engine.workers[0]!.terminated, true);
  assert.equal(engine.workers.length, 1, "the queued renders never made a worker");
  assert.equal(engine.workers[0]!.posted.length, 1);
});

test("the picture is cached by fence text, so a theme change never renders again", async () => {
  const engine = fakeEngine(success);
  installWorker(engine);
  document.documentElement.classList.add("dark");
  const dark = await renderMermaidDiagram("flowchart TD\n A-->B", "dark");
  assert.equal(engine.workers[0]!.posted.length, 1);
  assert.match(await dark.blob.text(), /--pele-fg:\s*#e9ebee/);
  assert.match(await dark.blob.text(), /--pele-fg-on-light:\s*#1a1e23/);
  assert.match(await dark.blob.text(), /--pele-fg-on-dark:\s*#e9ebee/);

  document.documentElement.classList.remove("dark");
  const light = await renderMermaidDiagram("flowchart TD\n A-->B", "light");
  assert.equal(engine.workers[0]!.posted.length, 1, "the theme toggle is a re-bake only");
  assert.match(await light.blob.text(), /--pele-fg:\s*#1a1e23/);
});

test("a failed fence is cached too: changing the theme does not ask the worker again", async () => {
  const engine = fakeEngine(failure("syntax"));
  installWorker(engine);
  await assert.rejects(renderMermaidDiagram("flowchart TD\n A--", "light"));
  assert.equal(engine.workers[0]!.posted.length, 1);
  await assert.rejects(
    renderMermaidDiagram("flowchart TD\n A--", "dark"),
    (error: unknown) => error instanceof DiagramRenderError && error.code === "syntax",
  );
  assert.equal(engine.workers[0]!.posted.length, 1);
});

/* ── the component and its fallback ─────────────────────────────────────── */

test("a fence the worker cannot draw is the plain code block, with no error banner", async () => {
  const engine = fakeEngine(failure("syntax"));
  installWorker(engine);
  await mount("```mermaid\nflowchart TD\n  A--\n```");

  assert.match(container.innerHTML, /language-mermaid/);
  assert.match(container.innerHTML, /flowchart TD/);
  assert.equal(container.querySelector("[data-mermaid-diagram]"), null);
  assert.doesNotMatch(container.textContent ?? "", /cannot draw|error|failed/i);
});

test("inline code and other fenced languages stay exactly as they were, even with diagrams on", async () => {
  const engine = fakeEngine(success);
  installWorker(engine);
  await mount("`flowchart TD` and then\n\n```js\nconst x = 1;\n```\n");

  assert.equal(engine.workers.length, 0);
  assert.match(container.innerHTML, /<code class="bg-raised px-1 py-px font-mono text-\[0\.9em\] text-fg shadow-\[inset_0_0_0_1px_var\(--color-border\)\]">flowchart TD<\/code>/);
  assert.match(container.innerHTML, /language-js/);
  assert.equal(container.querySelector("[data-mermaid-diagram]"), null);
});

test("the source control flips one block to its code and back", async () => {
  const engine = fakeEngine(success);
  installWorker(engine);
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
  let broken = false;
  const engine = fakeEngine((worker, message) => {
    queueMicrotask(() => {
      worker.reply({ phase: "rendering" });
      if (broken) worker.reply({ ok: false, code: "syntax", message: "broken" });
      else worker.reply({ ok: true, svg: fakeSvg(message.code), width: 120, height: 80 });
    });
  });
  installWorker(engine);
  const start = "```mermaid\nflowchart TD\n  A-->B";
  await mount(start);
  assert.equal(engine.workers[0]!.posted.length, 1, "the first frame is drawn straight away");
  const first = diagramImage()?.src;
  assert.ok(first);

  await show(`${start}\n  B-->C`);
  await show(`${start}\n  B-->C\n  C-->D`);
  await flush(100);
  assert.equal(engine.workers[0]!.posted.length, 1, "tokens inside the wait are not rendered one by one");
  assert.equal(diagramImage()?.src, first, "the last diagram stays up, rather than flipping to code");
  assert.equal(container.querySelector("pre"), null);

  await flush(MERMAID_STABLE_MS + 150);
  assert.equal(engine.workers[0]!.posted.length, 2);
  assert.match(engine.workers[0]!.posted[1]!.code, /C-->D/, "the settled text is what is drawn");
  assert.notEqual(diagramImage()?.src, first, "and replaces the picture");

  // Text that does not draw: the diagram stays while it may still be arriving…
  broken = true;
  await show(`${start}\n  B-->C\n  C-->D\n  BROKEN`);
  await flush(100);
  assert.ok(diagramImage(), "still the last good diagram");
  // …and gives way to the code once it has stood still and failed.
  await flush(MERMAID_STABLE_MS + 150);
  assert.equal(engine.workers[0]!.posted.length, 3);
  assert.equal(container.querySelector("[data-mermaid-diagram]"), null);
  assert.match(container.innerHTML, /language-mermaid/);
  assert.match(container.textContent ?? "", /BROKEN/);
});

test("a result whose block has moved on is not drawn", async () => {
  const engine = fakeEngine("hold");
  installWorker(engine);
  const slow = "```mermaid\nflowchart TD\n  SLOW-->A\n```";
  await mount(slow);
  await tick();
  assert.equal(engine.workers[0]!.posted.length, 1);
  assert.equal(diagramImage(), null, "the held render has not finished");

  await show("```mermaid\nflowchart TD\n  FAST-->B\n```");
  await flush(MERMAID_STABLE_MS + 100);
  assert.equal(engine.workers[0]!.terminated, true, "the superseded render's worker is stopped");
  assert.equal(engine.workers.length, 2, "the new text gets a fresh worker");
  assert.equal(engine.workers[1]!.posted.length, 1);
  assert.equal(diagramImage(), null);

  engine.release();
  await flush();
  assert.equal(diagramImage()?.alt, "flowchart TD\n  FAST-->B", "the superseded picture never replaces it");
});

test("a block that left the page while its render was running is not drawn", async () => {
  const engine = fakeEngine("hold");
  installWorker(engine);
  await mount("```mermaid\nflowchart TD\n  SLOW-->A\n```");
  await tick();
  assert.equal(engine.workers[0]!.posted.length, 1);

  await act(async () => {
    root?.unmount();
  });
  root = null;
  assert.equal(engine.workers[0]!.terminated, true, "its worker is stopped");
  engine.release();
  await flush();
  assert.equal(container.querySelector("[data-mermaid-image]"), null);
});

test("an engine that failed to start is retried for the next diagram", async () => {
  const engine = fakeEngine(success);
  let attempts = 0;
  setMermaidForTests({
    createWorker: () => {
      attempts += 1;
      if (attempts === 1) throw new Error("chunk failed to load");
      return engine.factory();
    },
  });
  await mount(FLOWCHART);
  assert.equal(attempts, 1);
  assert.equal(container.querySelector("[data-mermaid-diagram]"), null, "the first block stays its code");

  await show(`${FLOWCHART}\n\n${FLOWCHART.replace("B-->C", "C-->D")}`);
  await flush();
  assert.equal(attempts, 2, "the failure was not kept");
  assert.equal(engine.workers[0]!.posted.length, 1);
  assert.match(engine.workers[0]!.posted[0]!.code, /C-->D/);
  assert.ok(diagramImage(), "the new block is drawn");
});

/* ── the pref ───────────────────────────────────────────────────────────── */

test("turning diagrams off returns an open transcript to the code block", async () => {
  const engine = fakeEngine(success);
  installWorker(engine);
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
