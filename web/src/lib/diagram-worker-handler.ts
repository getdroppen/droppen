/**
 * One worker message, with the worker globals left outside so the tests can
 * drive it with a fake `post` recorder and the real `renderDiagram`.
 *
 * The phase order is the contract the client's clocks depend on: `started`
 * first, then the source stripper and `detectType` (synchronous, under the
 * 2 s clock), `loading` before the one awaited chunk import, `rendering` after
 * it, then the synchronous render and the palette check. Moving `loading`
 * before the stripper would pause the clock over work a hostile source can
 * stretch, and removing the palette check would let a fence palette through.
 */
import { PeleError } from "pele/lazy";
import { renderDiagram } from "./diagram-render";
import { withoutFenceThemeVariables } from "./fence-config";
import type { DiagramWorkerReply, DiagramWorkerRequest } from "./diagram-protocol";

export async function handleDiagramMessage(
  message: DiagramWorkerRequest,
  post: (reply: DiagramWorkerReply) => void,
  strip: (source: string) => string = withoutFenceThemeVariables,
): Promise<void> {
  post({ phase: "started" });
  try {
    const { svg, width, height } = await renderDiagram(
      message.code,
      message.options,
      {
        loading: () => post({ phase: "loading" }),
        rendering: () => post({ phase: "rendering" }),
      },
      strip,
    );
    post({ ok: true, svg, width, height });
  } catch (error) {
    // Error messages may quote the fence; the page never shows them, and a
    // block's fallback is its code. Keep the reply small.
    const text = (error instanceof Error ? error.message : String(error)).slice(0, 500);
    post({ ok: false, code: error instanceof PeleError ? error.code : "error", message: text });
  }
}
