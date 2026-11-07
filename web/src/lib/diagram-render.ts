/**
 * Rendering one fence with Pele. This module runs inside the diagram worker
 * (and in the tests that pin its behaviour); nothing here touches the page.
 *
 * The source is stripped of a fence's series palette before Pele preprocesses
 * it, and the SVG is swept for `--pele-series-*` declarations once more after
 * it is drawn. Loading is signalled so the client can start its render timer
 * only once the diagram type's chunk is in: a slow network is not a slow
 * render.
 */
import { PeleError, detectType, load, render } from "pele/lazy";
import type { RenderOptions } from "pele/lazy";
import { withoutFenceThemeVariables } from "./fence-config";

export interface RawDiagram {
  svg: string;
  width: number;
  height: number;
}

export interface RenderPhases {
  /** Just before `await load(type)`: the one gap a slow network may stretch. */
  loading?: () => void;
  /** Immediately before the synchronous render, after the type's chunk is in. */
  rendering?: () => void;
}

const SERIES_DECLARATION = /--pele-series-\d+\s*:/;

/** The root `<svg …>` start tag, quote-aware, or "" when there is none. */
function rootStartTag(svg: string): string {
  if (!/^<svg[\s>]/i.test(svg)) return "";
  let quote = "";
  for (let i = 4; i < svg.length; i++) {
    const char = svg[i]!;
    if (quote !== "") {
      if (char === quote) quote = "";
    } else if (char === '"' || char === "'") {
      quote = char;
    } else if (char === ">") {
      return svg.slice(0, i + 1);
    }
  }
  return svg;
}

/**
 * Pele writes a fence's series palette as `--pele-series-N:<colour>`
 * declarations in the root `<svg>` element's `style` attribute
 * (`paletteStyle()` in `src/svg/root.ts`). The host sets no series
 * declarations of its own, so one there can only have come from the fence.
 * A `<style>` element is checked too, in case a Pele output ever moves them.
 *
 * The check is anchored to those places, not the whole SVG: a label may say
 * `--pele-series-1: red` as text and must still draw. Refuse the whole render
 * rather than strip it: the colours also choose label ink, and a hidden slice
 * must be a code block, not a low-contrast picture. The source stripper in
 * `fence-config.ts` is the first line; this is the guarantee.
 */
export function assertNoFencePalette(svg: string): string {
  if (SERIES_DECLARATION.test(rootStartTag(svg))) {
    throw new PeleError("The diagram's own series palette is not allowed.", "semantic");
  }
  for (const style of svg.matchAll(/<style\b[^>]*>([\s\S]*?)<\/style>/gi)) {
    if (SERIES_DECLARATION.test(style[1]!)) {
      throw new PeleError("The diagram's own series palette is not allowed.", "semantic");
    }
  }
  return svg;
}

/**
 * Draw `code` in this thread. The phases bracket the only asynchronous part
 * (the chunk import), so the client's 2 s clock covers the sanitizer,
 * `detectType`, the render and the palette check on either side of it.
 *
 * `strip` is the source stripper; production uses the default. A test passes
 * the identity to put a fence palette past the stripper and reach the output
 * check, which is the guarantee.
 */
export async function renderDiagram(
  code: string,
  options: RenderOptions,
  phases: RenderPhases = {},
  strip: (source: string) => string = withoutFenceThemeVariables,
): Promise<RawDiagram> {
  const source = strip(code);
  const type = detectType(source);
  phases.loading?.();
  await load(type);
  phases.rendering?.();
  const result = render(source, options);
  return { svg: assertNoFencePalette(result.svg), width: result.width, height: result.height };
}
