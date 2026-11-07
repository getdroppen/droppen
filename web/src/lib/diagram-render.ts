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
import { detectType, load, render } from "pele/lazy";
import type { RenderOptions } from "pele/lazy";
import { withoutFenceThemeVariables, withoutSeriesPalette } from "./fence-config";

export interface RawDiagram {
  svg: string;
  width: number;
  height: number;
}

/**
 * Draw `code` in this thread. `onRendering` is called after the type's chunk
 * has loaded and immediately before the synchronous render, which is what the
 * worker forwards to the client's hard timeout.
 */
export async function renderDiagram(
  code: string,
  options: RenderOptions,
  onRendering?: () => void,
): Promise<RawDiagram> {
  const source = withoutFenceThemeVariables(code);
  const type = detectType(source);
  await load(type);
  onRendering?.();
  const result = render(source, options);
  return { svg: withoutSeriesPalette(result.svg), width: result.width, height: result.height };
}
