/**
 * The diagram worker. One message at a time (the main thread queues them),
 * Pele drawn off the main thread, and only the raw SVG, its size or the error
 * code comes back.
 *
 * Pele has superlinear paths a size cap alone does not stop (a gantt
 * `dateFormat` of 20,000 brackets, a venn union of thousands of sets, a
 * 48 KB `accDescr`), so the client can terminate this worker on a hard timer
 * and ask a fresh one for the next diagram. Nothing here measures against the
 * page: the worker has no DOM and uses Pele's OffscreenCanvas measurer when
 * the browser provides one, its built-in advance table otherwise.
 */
import { PeleError } from "pele/lazy";
import { renderDiagram } from "./diagram-render";
import type { DiagramWorkerReply, DiagramWorkerRequest } from "./diagram-protocol";

const scope = self as unknown as {
  onmessage: ((event: MessageEvent<DiagramWorkerRequest>) => void) | null;
  postMessage: (reply: DiagramWorkerReply) => void;
};

scope.onmessage = async (event) => {
  try {
    const { svg, width, height } = await renderDiagram(event.data.code, event.data.options, () =>
      scope.postMessage({ phase: "rendering" }),
    );
    scope.postMessage({ ok: true, svg, width, height });
  } catch (error) {
    // Error messages may quote the fence; the page never shows them, and a
    // block's fallback is its code. Keep the reply small.
    const message = (error instanceof Error ? error.message : String(error)).slice(0, 500);
    scope.postMessage({ ok: false, code: error instanceof PeleError ? error.code : "error", message });
  }
};
