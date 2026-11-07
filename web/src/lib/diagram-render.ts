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

/**
 * Pele writes a fence's series palette as `--pele-series-N:<colour>`
 * declarations on the SVG root. The host sets no series declarations of its
 * own, so one here can only have come from the fence. Refuse the whole render
 * rather than strip it: the colours also choose label ink, and a hidden slice
 * must be a code block, not a low-contrast picture. The source stripper in
 * `fence-config.ts` is the first line; this is the guarantee.
 */
export function assertNoFencePalette(svg: string): string {
  if (/--pele-series-\d+\s*:/.test(svg)) {
    throw new PeleError("The diagram's own series palette is not allowed.", "semantic");
  }
  return svg;
}

/**
 * Draw `code` in this thread. The phases bracket the only asynchronous part
 * (the chunk import), so the client's 2 s clock covers the sanitizer,
 * `detectType`, the render and the palette check on either side of it.
 */
export async function renderDiagram(
  code: string,
  options: RenderOptions,
  phases: RenderPhases = {},
): Promise<RawDiagram> {
  const source = withoutFenceThemeVariables(code);
  const type = detectType(source);
  phases.loading?.();
  await load(type);
  phases.rendering?.();
  const result = render(source, options);
  return { svg: assertNoFencePalette(result.svg), width: result.width, height: result.height };
}
