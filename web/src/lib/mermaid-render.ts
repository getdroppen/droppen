/**
 * The diagram engine, fetched only when it is needed.
 *
 * A message's ```mermaid fence becomes a diagram while the device's
 * `diagrams` pref is `rendered` (the default), once the fence's text has
 * stopped moving (see `MERMAID_STABLE_MS`). Until a fence asks, nothing here
 * is imported: the dynamic import below is the only reference to the package,
 * and `vite.config.ts` keeps Pele — including the one chunk it fetches for
 * each diagram type — out of the boot chunks, so a page that never draws a
 * diagram never downloads the engine.
 *
 * Agent output is untrusted, and this page drives shells on the user's host,
 * so a diagram is kept at arm's length three ways: the engine is told to draw
 * no links and no images, the work it may do is capped (source, edges,
 * output), and the finished SVG is shown as a picture (`<img src="blob:…">`,
 * see `components/mermaid-block.tsx`) rather than put into the page as markup
 * — where nothing in it can be followed, fetched or reach the page's styles.
 *
 * A fence can carry config (`%%{init: …}%%` or frontmatter `config:`), and
 * Pele reads it. Nothing in that config shares a surface with the options
 * above: limits are render arguments, theme and fonts are the `--pele-*`
 * properties baked into the picture here, and Pele reads a diagram's
 * `themeVariables` only for its own series palette — colour values, validated
 * by Pele, never fonts or page colours. Mermaid keys such as `theme`,
 * `fontFamily`, `themeCSS`, `htmlLabels` and `securityLevel` are ignored by
 * Pele, and there is nothing like Mermaid's `secure` list to add to.
 */
import type { RenderOptions } from "pele/lazy";

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
 * What one diagram may cost.
 *
 * Pele draws synchronously once its type has been fetched, so a timer cannot
 * interrupt it — a `setTimeout` cannot even run while the main thread is
 * inside it. These caps are what bound the work instead, and they are render
 * options, not fence config:
 *   · 50,000 characters of source (Mermaid's own default, and Pele's);
 *   · 500 edges (Mermaid's default; Pele's own is 5,000). `A & B --> C & D`
 *     multiplies, so one short line can otherwise ask for millions;
 *   · 2,000,000 characters of SVG (Pele's default is 4,000,000): a source
 *     within the other limits can still describe a drawing of many megabytes.
 */
const MAX_SOURCE_CHARS = 50_000;
const MAX_EDGES = 500;
const MAX_OUTPUT_CHARS = 2_000_000;

/**
 * The fonts the picture is measured and drawn in.
 *
 * The page's `--font-sans` starts with the self-hosted IBM Plex faces, which
 * an `<img>` cannot use: an SVG image is self-contained and cannot fetch the
 * page's webfonts. Measuring in the page with IBM Plex and drawing in the
 * picture with its fallback would size every label for one font and show
 * another, so both sides use the system families `--font-sans` falls back to
 * (the tail of that token in `index.css`). The mono stack has no webfonts to
 * lose; the page's token is read for it.
 */
const DIAGRAM_FONT =
  'ui-sans-serif, system-ui, -apple-system, "PingFang SC", "Hiragino Sans GB", "Noto Sans CJK SC", "Microsoft YaHei", sans-serif';
const FALLBACK_MONO = 'ui-monospace, "SF Mono", SFMono-Regular, "JetBrains Mono", Menlo, Consolas, monospace';

/**
 * Ink on the two sheets, for labels Pele prints on colours the diagram source
 * itself sets. These are contrast values, not chrome: the page's theme cannot
 * say which one a diagram's own fill needs.
 */
const INK_ON_LIGHT = "#1a1e23";
const PAPER_ON_DARK = "#ffffff";

function cssToken(name: string): string {
  if (typeof document === "undefined" || typeof getComputedStyle !== "function") return "";
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

/**
 * The chart chrome of one sheet, taken from the app's design tokens
 * (`--color-*` in `index.css`) and baked into the SVG as the public
 * `--pele-*` variables. The series colours are left to Pele: those are a
 * diagram's data colours, not chrome, and its defaults read on both sheets.
 */
function themeStyle(): Record<string, string> {
  const values: Array<[string, string]> = [
    ["--pele-bg", cssToken("--color-raised")],
    ["--pele-fg", cssToken("--color-fg")],
    ["--pele-muted", cssToken("--color-muted")],
    ["--pele-line", cssToken("--color-fg")],
    ["--pele-surface", cssToken("--color-bg")],
    ["--pele-surface-alt", cssToken("--color-hover")],
    ["--pele-border", cssToken("--color-border-strong")],
    ["--pele-accent", cssToken("--color-fg")],
    ["--pele-critical", cssToken("--color-danger")],
    ["--pele-error", cssToken("--color-danger")],
    ["--pele-radius", cssToken("--radius")],
    ["--pele-fg-on-light", INK_ON_LIGHT],
    ["--pele-fg-on-dark", PAPER_ON_DARK],
  ];
  const style: Record<string, string> = {};
  for (const [property, value] of values) if (value !== "") style[property] = value;
  style["--pele-font"] = DIAGRAM_FONT;
  style["--pele-font-mono"] = cssToken("--font-mono") || FALLBACK_MONO;
  return style;
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

/** Everything one render needs: what Pele is told, and what the picture is themed with. */
export interface DiagramSettings {
  options: RenderOptions;
  style: Record<string, string>;
}

/**
 * The options every diagram is drawn with, for one sheet.
 *
 * `maxWidth`, `measurer`, `config`, `icons` and `math` are deliberately left
 * unset: the picture keeps its natural width and scrolls in the message
 * rather than adapting to the container, Pele measures with the canvas (or
 * its built-in table where there is none), nothing in the host supplies an
 * icon or typeset formula, and a fence's own config is all the config Pele
 * gets to see.
 */
export function diagramSettings(theme: DiagramTheme, idPrefix?: string): DiagramSettings {
  const style = themeStyle();
  // A diagram may name `light-dark()` colours itself; the sheet it is drawn
  // on is the one the page is on.
  style["color-scheme"] = theme;
  const options: RenderOptions = {
    links: false,
    images: false,
    limit: MAX_SOURCE_CHARS,
    maxEdges: MAX_EDGES,
    outputLimit: MAX_OUTPUT_CHARS,
    // The page's transcript body is 13px; Mermaid was drawn at the same size.
    fontSize: 13,
    // The same stack is baked into `--pele-font`, so labels fit their boxes.
    fontFamily: style["--pele-font"],
    fontFamilyMono: style["--pele-font-mono"],
    // Mermaid's diagrams were pictures at their natural size in this app.
    responsive: false,
    ...(idPrefix === undefined ? {} : { idPrefix }),
  };
  return { options, style };
}

/** A plain length in pixels (`640`, `640.5px`), or NaN — `100%` is not one. */
function pixels(value: string | null): number {
  return value !== null && /^\s*[\d.]+(px)?\s*$/.test(value) ? Number.parseFloat(value) : Number.NaN;
}

/**
 * Pele's SVG as a picture.
 *
 * Re-serialised as XML: an image decoder reads XML, not the HTML string
 * Pele returns. Sized to its natural size: the `viewBox` carries it, and the
 * attributes are both set so the picture does not fall back to the width of
 * the message. The page's `--pele-*` values are baked in here because an
 * `<img>` does not see the page's custom properties; diagrams keep that width
 * and scroll inside the message rather than being shrunk until the labels are
 * unreadable.
 *
 * Parsed in an inert document: nothing in it runs or loads.
 */
export function diagramPicture(svg: string, style: Readonly<Record<string, string>> = {}): DiagramPicture {
  const root = new DOMParser().parseFromString(svg, "text/html").querySelector("svg");
  if (!root) throw new Error("Pele returned no SVG");
  const box = (root.getAttribute("viewBox") ?? "").trim().split(/[\s,]+/).map(Number);
  const width = box[2] && box[2] > 0 ? box[2] : pixels(root.getAttribute("width"));
  const height = box[3] && box[3] > 0 ? box[3] : pixels(root.getAttribute("height"));
  if (!(width > 0 && height > 0)) throw new Error("Pele returned an SVG without a size");
  root.setAttribute("width", String(width));
  root.setAttribute("height", String(height));
  root.style.removeProperty("max-width");
  root.style.removeProperty("height");
  // Theme variables are custom properties: set on the root, they inherit
  // through the drawing, and `var(--pele-…)` resolves inside the picture.
  for (const [property, value] of Object.entries(style)) root.style.setProperty(property, value);
  const title = root.querySelector(":scope > title")?.textContent?.trim() || null;
  const xml = new XMLSerializer().serializeToString(root);
  return { blob: new Blob([xml], { type: "image/svg+xml" }), width, height, title };
}

/** The little of `pele/lazy` this file uses. */
export interface PeleEngine {
  renderAsync(code: string, options?: RenderOptions): Promise<{ svg: string; width: number; height: number }>;
}

interface Engine {
  load: () => Promise<PeleEngine>;
}

const REAL_ENGINE: Engine = {
  load: () => import("pele/lazy"),
};

let engine = REAL_ENGINE;
let loaded: Promise<PeleEngine> | null = null;

function loadEngine(): Promise<PeleEngine> {
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
 * the real import (so a test can hold, fail or count renders).
 */
export function setMermaidForTests(fake: Partial<Engine> | null): void {
  engine = fake ? { ...REAL_ENGINE, ...fake } : REAL_ENGINE;
  loaded = null;
}

let nextId = 0;

/**
 * Draw `code` and resolve to the picture.
 *
 * There is no queue: Pele's `render()` is synchronous and keeps no state
 * between calls, so two diagrams cannot interleave, and one slow render
 * cannot hold the next block behind it. There is no timeout for the same
 * reason — a timer cannot fire during a synchronous render — and the limits
 * in `diagramSettings` are what keep a hostile fence from asking for
 * unbounded work. A parse or layout failure, a limit, or `signal` aborted
 * before the result is drawn rejects.
 */
export function renderMermaidDiagram(code: string, theme: DiagramTheme, signal?: AbortSignal): Promise<DiagramPicture> {
  return (async () => {
    signal?.throwIfAborted();
    const pele = await loadEngine();
    signal?.throwIfAborted();
    // A plain id: Pele uses it for markers and link targets, and the prefix
    // keeps two diagrams on one page from naming the same elements.
    const settings = diagramSettings(theme, `droppen-diagram-${++nextId}`);
    const { svg } = await pele.renderAsync(code, settings.options);
    signal?.throwIfAborted();
    return diagramPicture(svg, settings.style);
  })();
}
