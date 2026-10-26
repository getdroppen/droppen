/**
 * The diagram engine, fetched only when it is needed.
 *
 * A message's ```mermaid fence becomes a diagram while the device's
 * `diagrams` pref is `rendered` (the default), once the fence's text has
 * stopped moving (see `MERMAID_STABLE_MS`). Until a fence asks, nothing here
 * is imported: the dynamic import below is the only reference to the package,
 * and `vite.config.ts` keeps Mermaid and everything only it imports (d3,
 * DOMPurify, cytoscape, …) out of the boot chunks, so a page that never draws
 * a diagram never downloads the engine.
 *
 * Agent output is untrusted, and this page drives shells on the user's host,
 * so a diagram is kept at arm's length three ways: Mermaid's strict security
 * level, a config the fence's own text cannot loosen (`LOCKED_KEYS`), and the
 * finished SVG shown as a picture (`<img src="blob:…">`, see
 * `components/mermaid-block.tsx`) rather than put into the page as markup —
 * where a link in it cannot be followed and its styles cannot reach the page.
 */
import type { Mermaid, MermaidConfig } from "mermaid";

export type DiagramTheme = "light" | "dark";

/**
 * How long a fence's text has to stand still before it is drawn.
 *
 * A reply streams into the transcript token by token, so the renderer sees a
 * half-written fence many times a second; parsing it each time is both wasted
 * work and a row of transient parse errors. 300 ms is long enough to cover a
 * pause between tokens and short enough that a diagram in a finished message
 * appears as soon as the page does.
 */
export const MERMAID_STABLE_MS = 300;

/**
 * How long one diagram may take. Diagrams are drawn one at a time, so a
 * render that never settles would hold up every block after it; past this,
 * its block shows the code and the next one gets its turn.
 */
const RENDER_TIMEOUT_MS = 10_000;

/**
 * The page's own system font stack (`--font-sans` in `index.css`), spelled
 * out. Mermaid sizes every box by measuring its text in the page, and the
 * picture is then drawn inside an `<img>`, which neither the page's custom
 * properties nor any font the page loaded can reach; system fonts are the
 * same in both, so the text fits the boxes it was measured for.
 */
const DIAGRAM_FONT =
  'ui-sans-serif, system-ui, -apple-system, "PingFang SC", "Hiragino Sans GB", "Noto Sans CJK SC", "Microsoft YaHei", sans-serif';

/**
 * Config a fence's own text may not set.
 *
 * A fence can carry config — a `%%{init: …}%%` directive or a frontmatter
 * `config:` block — and Mermaid merges both over ours, except for the keys in
 * `secure`. What is given here is added to Mermaid's own list (`secure`,
 * `securityLevel`, `startOnLoad`, `maxTextSize`, `suppressErrorRendering`,
 * `maxEdges`), at any depth of the fence's config:
 *   · `htmlLabels`: HTML in a `foreignObject` instead of SVG text;
 *   · `themeCSS`: raw CSS, whose `@keyframes` or `@font-face` are not scoped
 *     to the diagram while Mermaid measures it in the page;
 *   · `theme`, `themeVariables`, `darkMode`: the page decides light or dark,
 *     and a diagram that ignored it would be unreadable on the other sheet;
 *   · `fontFamily`, `altFontFamily`: text has to be drawn in the font it was
 *     measured in (`DIAGRAM_FONT`);
 *   · `layout`, `look`: ELK is not in this build (`lib/elk-stub.ts`), and the
 *     hand-drawn look is the app's choice to make, not a reply's.
 */
const LOCKED_KEYS = [
  "htmlLabels",
  "themeCSS",
  "theme",
  "themeVariables",
  "darkMode",
  "fontFamily",
  "altFontFamily",
  "layout",
  "look",
];

/**
 * Mermaid takes a `layout` inside a diagram's own section too, though its
 * types list it only for some. A mindmap reads any layout set anywhere as the
 * user's choice, so the global dagre below would replace its own default.
 */
const MINDMAP: NonNullable<MermaidConfig["mindmap"]> & { layout: string } = { layout: "cose-bilkent" };

/** Everything Mermaid is initialised with, for one sheet. */
export function mermaidConfig(theme: DiagramTheme): MermaidConfig {
  return {
    startOnLoad: false,
    // Labels encoded and sanitised, click handlers off. Never configurable.
    securityLevel: "strict",
    // A failed render leaves nothing behind; without this, Mermaid 12 draws
    // its "Syntax error" picture into a node in <body> and throws before it
    // removes it.
    suppressErrorRendering: true,
    secure: LOCKED_KEYS,
    theme: theme === "dark" ? "dark" : "default",
    fontFamily: DIAGRAM_FONT,
    themeVariables: { fontSize: "13px" },
    // No HTML in labels: SVG text needs no `foreignObject`, and leaves the
    // strict sanitiser less to do. At the top level, which in Mermaid 12 wins
    // over the deprecated `flowchart.htmlLabels` — still read directly in a
    // place or two, so that is set too.
    htmlLabels: false,
    flowchart: { htmlLabels: false },
    // Mermaid 12's default is ELK, which this build does not carry.
    layout: "dagre",
    mindmap: MINDMAP,
  };
}

/** A drawn diagram, ready for an `<img>`. */
export interface DiagramPicture {
  /** The SVG, as well-formed XML. */
  blob: Blob;
  /** Its natural size, in CSS pixels. */
  width: number;
  height: number;
  /** The diagram's own accessible title (`accTitle`), when it has one. */
  title: string | null;
}

const XHTML = "http://www.w3.org/1999/xhtml";

/** A plain length in pixels (`640`, `640.5px`), or NaN — `100%` is not one. */
function pixels(value: string | null): number {
  return value !== null && /^\s*[\d.]+(px)?\s*$/.test(value) ? Number.parseFloat(value) : Number.NaN;
}

/**
 * Mermaid's SVG as a picture.
 *
 * Re-serialised as XML: Mermaid hands back `innerHTML`, which an image
 * decoder refuses at the first HTML-only construct (an unclosed `<br>` in a
 * label). And sized to its natural size: each diagram type either sets its
 * width to `100%` with a `max-width` (its `useMaxWidth`, on by default in
 * nearly every type) or sets both sides in pixels, and some have no config
 * section to turn the first off at all; the `viewBox` is the natural size in
 * either case. Diagrams keep that width and scroll inside the message rather
 * than being shrunk until the labels are unreadable.
 *
 * Parsed in an inert document: nothing in it runs or loads.
 */
export function diagramPicture(svg: string): DiagramPicture {
  const root = new DOMParser().parseFromString(svg, "text/html").querySelector("svg");
  if (!root) throw new Error("Mermaid returned no SVG");
  const box = (root.getAttribute("viewBox") ?? "").trim().split(/[\s,]+/).map(Number);
  const width = box[2] && box[2] > 0 ? box[2] : pixels(root.getAttribute("width"));
  const height = box[3] && box[3] > 0 ? box[3] : pixels(root.getAttribute("height"));
  if (!(width > 0 && height > 0)) throw new Error("Mermaid returned an SVG without a size");
  root.setAttribute("width", String(width));
  root.setAttribute("height", String(height));
  root.style.removeProperty("max-width");
  // Mermaid marks the HTML in a `foreignObject` with an `xmlns` attribute; the
  // serialiser writes the element's namespace itself, and twice is an error.
  for (const element of root.querySelectorAll("*")) {
    if (element.namespaceURI === XHTML) element.removeAttribute("xmlns");
  }
  const title = root.querySelector(":scope > title")?.textContent?.trim() || null;
  const xml = new XMLSerializer().serializeToString(root);
  return { blob: new Blob([xml], { type: "image/svg+xml" }), width, height, title };
}

/** The little of the engine this file uses. */
export type MermaidEngine = Pick<Mermaid, "initialize" | "render">;

interface Engine {
  load: () => Promise<MermaidEngine>;
  timeoutMs: number;
}

const REAL_ENGINE: Engine = {
  load: () => import("mermaid").then((module) => module.default),
  timeoutMs: RENDER_TIMEOUT_MS,
};

let engine = REAL_ENGINE;
let loaded: Promise<MermaidEngine> | null = null;

function loadMermaid(): Promise<MermaidEngine> {
  loaded ??= engine.load().catch((error: unknown) => {
    // A fetch that failed (offline, a release that replaced the chunk) is not
    // kept: the next diagram asks again.
    loaded = null;
    throw error;
  });
  return loaded;
}

/**
 * Test seam only (`web/test/mermaid-diagrams.test.tsx`): an engine in place of
 * the real import (jsdom lays nothing out), and a shorter time limit.
 */
export function setMermaidForTests(fake: Partial<Engine> | null): void {
  engine = fake ? { ...REAL_ENGINE, ...fake } : REAL_ENGINE;
  loaded = null;
}

function within<T>(work: Promise<T>, ms: number): Promise<T> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`no diagram after ${ms} ms`)), ms);
    work.then(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      (error: unknown) => {
        clearTimeout(timer);
        reject(error);
      },
    );
  });
}

/**
 * `initialize` is global state and Mermaid measures while it renders, so one
 * diagram is prepared and drawn at a time. Mermaid 12 serialises `render`
 * itself; the queue here is what also keeps each one's `initialize` (and so
 * its theme) next to its own render.
 */
let queue: Promise<unknown> = Promise.resolve();
let nextId = 0;

/**
 * Draw `code` and resolve to the picture. A parse or layout failure, a render
 * past the time limit, or `signal` aborted before its turn comes, rejects.
 */
export function renderMermaidDiagram(code: string, theme: DiagramTheme, signal?: AbortSignal): Promise<DiagramPicture> {
  const next = queue.then(async () => {
    // A block that has changed its text or its theme, or left the page, while
    // this waited its turn: nothing to draw.
    signal?.throwIfAborted();
    const mermaid = await loadMermaid();
    signal?.throwIfAborted();
    mermaid.initialize(mermaidConfig(theme));
    // A plain id: Mermaid puts it in a CSS selector, so no React `useId` colons.
    const id = `droppen-diagram-${++nextId}`;
    try {
      const { svg } = await within(mermaid.render(id, code), engine.timeoutMs);
      return diagramPicture(svg);
    } catch (error) {
      // Mermaid draws in a scratch node, `#d<id>` in <body>, and one failure
      // throws before its own cleanup: a `classDef` name its stylesheet cannot
      // take, raised between parsing and drawing.
      document.getElementById(`d${id}`)?.remove();
      throw error;
    }
  });
  queue = next.then(
    () => undefined,
    () => undefined,
  );
  return next;
}
