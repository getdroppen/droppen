/**
 * The diagram engine, fetched only when it is needed.
 *
 * A message's ```mermaid fence becomes a diagram while the device's
 * `diagrams` pref is `rendered` (the default), once the fence's text has
 * stopped moving (see `MERMAID_STABLE_MS`). Until a fence asks, nothing here
 * is imported: the worker below is created on the first fence, and
 * `vite.config.ts` keeps Pele — including the one chunk it fetches for each
 * diagram type — out of the boot chunks, so a page that never draws a diagram
 * never downloads the engine.
 *
 * Pele draws synchronously and has superlinear paths (a gantt `dateFormat` of
 * 20,000 brackets, a venn union of thousands of sets, a 48 KB `accDescr`), so
 * it runs in a module Worker, one render at a time, behind a 2 s hard timer.
 * A fence that does not finish is its code block; the worker is terminated
 * and a fresh one is made for the next diagram. The main thread only
 * serialises the returned SVG, bakes the page's theme into it and shows it as
 * a picture (`<img src="blob:…">`, see `components/mermaid-block.tsx`).
 *
 * Agent output is untrusted, and this page drives shells on the user's host,
 * so a diagram is kept at arm's length three ways: the engine is told to draw
 * no links and no images, the work it may do is capped (source, edges,
 * output), and the finished SVG is shown as a picture rather than put into the
 * page as markup — where nothing in it can be followed, fetched or reach the
 * page's styles. A fence's config cannot change any of that: the worker strips
 * its `themeVariables` before Pele reads it (see `lib/fence-config.ts`), and a
 * source over the length cap is refused here before a worker is even asked.
 */
import type { RenderOptions } from "pele/lazy";
import type { DiagramErrorCode, DiagramWorkerReply, DiagramWorkerRequest } from "./diagram-protocol";

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
 * The source cap is checked on the main thread before anything is posted, so
 * a fence cannot make Pele's quadratic preprocess run here. The rest are
 * render options, not fence config, and the worker's hard timer catches what
 * they cannot: Pele's own limits cover only some of its superlinear paths.
 */
const MAX_SOURCE_CHARS = 50_000;
const MAX_EDGES = 500;
const MAX_OUTPUT_CHARS = 2_000_000;

/**
 * The longest run of whitespace a fence may contain. Pele trims an `accDescr`
 * with a `/\s+$/gm` whose backtracking is quadratic in such a run: 48,000
 * spaces take 2.6 s in Chromium and about 1 s in WebKit. No diagram needs a
 * 32,000-character whitespace run, so it is refused here, before the worker,
 * like an over-long source. The worker's timer remains the general bound.
 *
 * Counted in one pass over the char codes, not with a `\s{N}` regular
 * expression: a regex retries from every start position after a failed match,
 * which is quadratic on exactly the inputs this cap is meant to reject.
 */
export const MAX_WHITESPACE_RUN = 32_000;

/** The code points ECMAScript's `\s` matches, as a linear scan can use them. */
export function isDiagramWhitespace(code: number): boolean {
  return (
    code === 0x09 ||
    code === 0x0a ||
    code === 0x0b ||
    code === 0x0c ||
    code === 0x0d ||
    code === 0x20 ||
    code === 0xa0 ||
    code === 0x1680 ||
    (code >= 0x2000 && code <= 0x200a) ||
    code === 0x2028 ||
    code === 0x2029 ||
    code === 0x202f ||
    code === 0x205f ||
    code === 0x3000 ||
    code === 0xfeff
  );
}

/** Whether `text` has a run of more than `max` whitespace code points. */
export function hasLongWhitespaceRun(text: string, max = MAX_WHITESPACE_RUN): boolean {
  let run = 0;
  for (let i = 0; i < text.length; i++) {
    if (isDiagramWhitespace(text.charCodeAt(i))) {
      if (++run > max) return true;
    } else {
      run = 0;
    }
  }
  return false;
}

/**
 * How long the worker gets to draw, once its diagram type has loaded.
 *
 * The reviewer's legitimate worst cases are under 175 ms; 2 s is more than
 * ten times that, and still cuts off the hostile 2.6 s `accDescr` case. The
 * load phase (worker script, Pele's entry, the type's chunk) is asynchronous
 * and gets a separate, longer guard: a slow network must not be mistaken for
 * a slow render.
 */
const RENDER_TIMEOUT_MS = 2_000;
const LOAD_TIMEOUT_MS = 10_000;

/**
 * Raw pictures kept for the session, keyed by the fence's exact text. A theme
 * toggle only re-bakes the style onto one of these; it never asks the worker
 * again, and a fence that timed out or failed is remembered too so toggling
 * the sheet does not repeat its work. Bounded by both entry count and bytes.
 */
const MAX_CACHE_ENTRIES = 64;
const MAX_CACHE_CHARS = 8_000_000;

/**
 * The fonts the picture is measured and drawn in.
 *
 * The page's `--font-sans` starts with the self-hosted IBM Plex faces, which
 * an `<img>` cannot use: an SVG image is self-contained and cannot fetch the
 * page's webfonts. Measuring in the page with IBM Plex and drawing in the
 * picture with its fallback would size every label for one font and show
 * another, so both sides use the system families `--font-sans` falls back to
 * (the tail of that token in `index.css`). The worker measures with these
 * through OffscreenCanvas when it has one, and Pele's Arial advance table
 * otherwise. The mono stack has no webfonts to lose; the page's token is read.
 */
const DIAGRAM_FONT =
  'ui-sans-serif, system-ui, -apple-system, "PingFang SC", "Hiragino Sans GB", "Noto Sans CJK SC", "Microsoft YaHei", sans-serif';
const FALLBACK_MONO = 'ui-monospace, "SF Mono", SFMono-Regular, "JetBrains Mono", Menlo, Consolas, monospace';

function cssToken(name: string): string {
  if (typeof document === "undefined" || typeof getComputedStyle !== "function") return "";
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

/**
 * The chart chrome of one sheet, taken from the app's design tokens
 * (`--color-*` in `index.css`) and baked into the SVG as the public
 * `--pele-*` variables. The series colours are left to Pele: those are a
 * diagram's data colours, not chrome, and its defaults read on both sheets.
 *
 * The ink for labels on colours the diagram source sets is the other sheet's
 * canvas (light text on the dark sheet, dark text on the light one), derived
 * from the same tokens rather than a second palette.
 */
export function diagramStyle(theme: DiagramTheme): Record<string, string> {
  const fg = cssToken("--color-fg");
  const bg = cssToken("--color-bg");
  const values: Array<[string, string]> = [
    ["--pele-bg", cssToken("--color-raised")],
    ["--pele-fg", fg],
    ["--pele-muted", cssToken("--color-muted")],
    ["--pele-line", fg],
    ["--pele-surface", bg],
    ["--pele-surface-alt", cssToken("--color-hover")],
    ["--pele-border", cssToken("--color-border-strong")],
    ["--pele-accent", fg],
    ["--pele-critical", cssToken("--color-danger")],
    ["--pele-error", cssToken("--color-danger")],
    ["--pele-radius", cssToken("--radius")],
    ["--pele-fg-on-light", theme === "light" ? fg : bg],
    ["--pele-fg-on-dark", theme === "light" ? bg : fg],
  ];
  const style: Record<string, string> = {};
  for (const [property, value] of values) if (value !== "") style[property] = value;
  style["--pele-font"] = DIAGRAM_FONT;
  style["--pele-font-mono"] = cssToken("--font-mono") || FALLBACK_MONO;
  style["color-scheme"] = theme;
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
 * rather than adapting to the container, the worker measures with
 * OffscreenCanvas or Pele's table, nothing in the host supplies an icon or
 * typeset formula, and a fence's own config is all the config Pele gets.
 */
export function diagramSettings(theme: DiagramTheme, idPrefix?: string): DiagramSettings {
  const style = diagramStyle(theme);
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
 * Parsed in an inert document: nothing in it runs or loads. The worker has
 * already removed a fence's series palette from the string.
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

/** The little of a dedicated worker this client uses. */
export interface DiagramWorker {
  postMessage(message: DiagramWorkerRequest): void;
  terminate(): void;
  addEventListener(type: "message", listener: (event: MessageEvent<DiagramWorkerReply>) => void): void;
  addEventListener(type: "error" | "messageerror", listener: (event: Event) => void): void;
  removeEventListener(type: "message", listener: (event: MessageEvent<DiagramWorkerReply>) => void): void;
  removeEventListener(type: "error" | "messageerror", listener: (event: Event) => void): void;
}

export type DiagramWorkerFactory = () => DiagramWorker;

interface Engine {
  createWorker: DiagramWorkerFactory;
  loadTimeoutMs: number;
  renderTimeoutMs: number;
}

const REAL_ENGINE: Engine = {
  createWorker: () =>
    new Worker(new URL("./diagram-worker.ts", import.meta.url), {
      type: "module",
      name: "droppen-diagram",
    }) as unknown as DiagramWorker,
  loadTimeoutMs: LOAD_TIMEOUT_MS,
  renderTimeoutMs: RENDER_TIMEOUT_MS,
};

/** A diagram that could not be drawn, with enough detail for a test. */
export class DiagramRenderError extends Error {
  readonly code: DiagramErrorCode;
  /**
   * Whether a rerun of the same fence cannot differ. Pele's own errors and
   * the 2 s render timeout are; a load failure, a load timeout, a worker
   * crash and the source caps are not (the next attempt may succeed, and the
   * caps are cheap to re-check).
   */
  readonly cacheable: boolean;

  constructor(code: DiagramErrorCode, message: string, cacheable = false) {
    super(message);
    this.name = "DiagramRenderError";
    this.code = code;
    this.cacheable = cacheable;
  }
}

interface RawDiagram {
  svg: string;
  width: number;
  height: number;
}

type CacheEntry =
  | { ok: true; raw: RawDiagram }
  | { ok: false; code: DiagramErrorCode; message: string };

/** Pele's own refusals: a rerun of the same fence cannot differ. */
const PELE_ERROR_CODES = new Set<DiagramErrorCode>(["unsupported-diagram", "syntax", "semantic", "limit"]);

let engine = REAL_ENGINE;
let worker: DiagramWorker | null = null;
let queue: Promise<unknown> = Promise.resolve();
let nextId = 0;
let epoch = 0;

const rawCache = new Map<string, CacheEntry>();
let rawCacheChars = 0;

function rawCacheGet(code: string): CacheEntry | undefined {
  const entry = rawCache.get(code);
  if (entry === undefined) return undefined;
  // Refresh its place in insertion order: the map is the LRU list.
  rawCache.delete(code);
  rawCache.set(code, entry);
  return entry;
}

function rawCacheSet(code: string, entry: CacheEntry): void {
  const old = rawCache.get(code);
  if (old !== undefined) {
    rawCache.delete(code);
    rawCacheChars -= old.ok ? old.raw.svg.length : 0;
  }
  rawCache.set(code, entry);
  rawCacheChars += entry.ok ? entry.raw.svg.length : 0;
  while (rawCache.size > MAX_CACHE_ENTRIES || rawCacheChars > MAX_CACHE_CHARS) {
    const oldest = rawCache.keys().next().value;
    if (oldest === undefined) break;
    const evicted = rawCache.get(oldest)!;
    rawCache.delete(oldest);
    rawCacheChars -= evicted.ok ? evicted.raw.svg.length : 0;
  }
}

function clearRawCache(): void {
  rawCache.clear();
  rawCacheChars = 0;
}

function workerFor(): DiagramWorker {
  worker ??= engine.createWorker();
  return worker;
}

function retire(current: DiagramWorker): void {
  current.terminate();
  if (worker === current) worker = null;
}

/**
 * Test seam only (`web/test/mermaid-diagrams.test.tsx`): a fake worker
 * factory and shorter timers in place of the real import (jsdom has no
 * Worker, and a test must be able to hold or fail a message).
 */
export function setMermaidForTests(fake: Partial<Engine> | null): void {
  if (worker !== null) {
    worker.terminate();
    worker = null;
  }
  queue = Promise.resolve();
  epoch++;
  clearRawCache();
  engine = fake ? { ...REAL_ENGINE, ...fake } : REAL_ENGINE;
}

function enqueue<T>(work: () => Promise<T>): Promise<T> {
  const run = queue.then(work, work);
  queue = run.then(
    () => undefined,
    () => undefined,
  );
  return run;
}

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function abortReason(signal: AbortSignal): unknown {
  return signal.reason ?? new DOMException("Aborted", "AbortError");
}

/**
 * Post one render to the worker. The load guard covers the worker script and
 * the diagram type's chunk; the worker says `rendering` after both, and the
 * hard render timer starts there. A timeout, an abort or an error event
 * terminates the worker; the next call makes a new one.
 */
function renderInWorker(
  code: string,
  options: RenderOptions,
  signal?: AbortSignal,
): Promise<RawDiagram> {
  if (signal?.aborted) return Promise.reject(abortReason(signal));
  return new Promise((resolve, reject) => {
    let current: DiagramWorker;
    try {
      current = workerFor();
    } catch (error) {
      reject(new DiagramRenderError("worker", messageOf(error)));
      return;
    }
    let loadTimer: ReturnType<typeof setTimeout> | undefined;
    let renderTimer: ReturnType<typeof setTimeout> | undefined;
    // The render clock starts at the worker's `started` message, pauses only
    // around `await load(type)`, and counts the synchronous work on either
    // side (the sanitizer, detectType, the render, the palette check).
    let renderClockStarted = 0;
    let renderElapsed = 0;
    let phase: "start" | "loading" | "rendering" = "start";
    let settled = false;

    const cleanup = () => {
      if (loadTimer !== undefined) clearTimeout(loadTimer);
      if (renderTimer !== undefined) clearTimeout(renderTimer);
      current.removeEventListener("message", onMessage);
      current.removeEventListener("error", onError);
      current.removeEventListener("messageerror", onError);
      signal?.removeEventListener("abort", onAbort);
    };

    const fail = (error: unknown, stopWorker: boolean) => {
      if (settled) return;
      settled = true;
      cleanup();
      if (stopWorker) retire(current);
      reject(error);
    };

    const succeed = (raw: RawDiagram) => {
      if (settled) return;
      settled = true;
      cleanup();
      resolve(raw);
    };

    const pauseRenderClock = () => {
      if (renderClockStarted !== 0) {
        renderElapsed += performance.now() - renderClockStarted;
        renderClockStarted = 0;
      }
      if (renderTimer !== undefined) {
        clearTimeout(renderTimer);
        renderTimer = undefined;
      }
    };

    const startRenderClock = () => {
      if (renderClockStarted !== 0 || settled) return;
      renderClockStarted = performance.now();
      const remaining = engine.renderTimeoutMs - renderElapsed;
      if (remaining <= 0) {
        fail(new DiagramRenderError("timeout", `no diagram after ${engine.renderTimeoutMs} ms`, true), true);
        return;
      }
      renderTimer = setTimeout(
        () => fail(new DiagramRenderError("timeout", `no diagram after ${engine.renderTimeoutMs} ms`, true), true),
        remaining,
      );
    };

    const onMessage = (event: MessageEvent<DiagramWorkerReply>) => {
      const reply = event.data;
      if (reply === null || typeof reply !== "object") return;
      if ("phase" in reply) {
        if (reply.phase === "started") {
          phase = "start";
          startRenderClock();
        } else if (reply.phase === "loading") {
          phase = "loading";
          // The chunk import is the one thing a slow network may stretch;
          // everything before and after it counts against the render clock.
          if (renderClockStarted === 0) startRenderClock();
          pauseRenderClock();
        } else if (reply.phase === "rendering") {
          phase = "rendering";
          if (loadTimer !== undefined) {
            clearTimeout(loadTimer);
            loadTimer = undefined;
          }
          if (renderClockStarted === 0) startRenderClock();
        }
        return;
      }
      if (reply.ok) {
        succeed({ svg: reply.svg, width: reply.width, height: reply.height });
      } else {
        // Pele refused the fence: the worker is healthy and stays for reuse.
        // A load-phase failure is not remembered; the next attempt may work.
        fail(new DiagramRenderError(reply.code, reply.message, phase !== "loading" && PELE_ERROR_CODES.has(reply.code)), false);
      }
    };

    const onError = () => fail(new DiagramRenderError("worker", "the diagram worker failed"), true);

    const onAbort = () => fail(abortReason(signal!), true);

    current.addEventListener("message", onMessage);
    current.addEventListener("error", onError);
    current.addEventListener("messageerror", onError);
    signal?.addEventListener("abort", onAbort, { once: true });
    loadTimer = setTimeout(
      () => fail(new DiagramRenderError("timeout", `the worker did not start within ${engine.loadTimeoutMs} ms`), true),
      engine.loadTimeoutMs,
    );
    try {
      current.postMessage({ code, options });
    } catch (error) {
      fail(new DiagramRenderError("worker", messageOf(error)), true);
    }
  });
}

function cachedPicture(entry: CacheEntry, theme: DiagramTheme): DiagramPicture {
  if (!entry.ok) throw new DiagramRenderError(entry.code, entry.message);
  return diagramPicture(entry.raw.svg, diagramStyle(theme));
}

/**
 * Draw `code` and resolve to the picture.
 *
 * The raw cache is consulted before the source is touched: a theme change
 * over a cached fence (even an adversarial one) only re-bakes the style. On a
 * miss, the length and whitespace caps are checked before a worker exists or
 * a message is posted — Pele's own limit is checked only after its quadratic
 * preprocess, so a four-hundred-thousand-newline fence must never get there.
 * A source over a cap is the code block, like any other rejection.
 *
 * One render runs at a time: the queue keeps hostile fences from piling their
 * work into one long task and keeps a single worker's message order simple.
 */
export async function renderMermaidDiagram(
  code: string,
  theme: DiagramTheme,
  signal?: AbortSignal,
): Promise<DiagramPicture> {
  // The cache is consulted before the source is scanned: a theme toggle over
  // a cached 50 KB fence must not walk it again, and an adversarial fence in
  // the cache is exactly what the round-2 probes exercise.
  const early = rawCacheGet(code);
  if (early !== undefined) {
    signal?.throwIfAborted();
    return cachedPicture(early, theme);
  }
  if (code.length > MAX_SOURCE_CHARS || hasLongWhitespaceRun(code)) {
    throw new DiagramRenderError("limit", `Diagram source is longer than the limit of ${MAX_SOURCE_CHARS} characters, or has a whitespace run over ${MAX_WHITESPACE_RUN}.`);
  }
  signal?.throwIfAborted();
  const started = epoch;
  return enqueue(async () => {
    signal?.throwIfAborted();
    const hit = rawCacheGet(code);
    if (hit !== undefined) return cachedPicture(hit, theme);
    const settings = diagramSettings(theme, `droppen-diagram-${++nextId}`);
    try {
      const raw = await renderInWorker(code, settings.options, signal);
      if (started === epoch) rawCacheSet(code, { ok: true, raw });
      signal?.throwIfAborted();
      return diagramPicture(raw.svg, settings.style);
    } catch (error) {
      if (started === epoch && error instanceof DiagramRenderError && error.cacheable) {
        rawCacheSet(code, { ok: false, code: error.code, message: error.message });
      }
      throw error;
    }
  });
}
