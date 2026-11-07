// Types for worker-notices.mjs (plain JavaScript, so scripts/notices can run
// it with Node alone).
import type { Plugin } from "vite";

export function recordWorkerModules(): Plugin;
export function workerNotices(): Set<string>;
