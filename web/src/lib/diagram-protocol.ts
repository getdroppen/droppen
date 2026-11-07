import type { RenderOptions } from "pele/lazy";

/** What the main thread hands the diagram worker. */
export interface DiagramWorkerRequest {
  code: string;
  options: RenderOptions;
}

/**
 * Why a diagram could not be drawn. Pele's own four, plus the ways this app's
 * worker client can fail before or around it.
 */
export type DiagramErrorCode =
  | "unsupported-diagram"
  | "syntax"
  | "semantic"
  | "limit"
  | "timeout"
  | "worker"
  | "error";

/** What the diagram worker says back: a render phase, a picture, or a failure. */
export type DiagramWorkerReply =
  | { phase: "rendering" }
  | { ok: true; svg: string; width: number; height: number }
  | { ok: false; code: DiagramErrorCode; message: string };
