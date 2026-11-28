/**
 * Fetches the pre-bundled plugin frame runtime script emitted by
 * `web/build/plugin-frame.ts` once per page load so every sandboxed
 * `/plugin-frame.html` iframe can boot from the same cached code string.
 *
 * `web/build/plugin-frame.ts` replaces the placeholder string below during
 * Vite builds and dev serving, keeping this module plain TypeScript so Node
 * test runners (`tsx`) can import it without a Vite virtual-module loader.
 */
const RUNTIME_PLACEHOLDER = "__DROPPEN_PLUGIN_FRAME_RUNTIME_URL__";
const RUNTIME_FALLBACK = "/assets/plugin-frame-runtime.js";

function pluginFrameRuntimeUrl(): string {
  return RUNTIME_PLACEHOLDER.startsWith("__DROPPEN_") ? RUNTIME_FALLBACK : RUNTIME_PLACEHOLDER;
}

let cachedRuntime: Promise<string> | null = null;

export function fetchFrameRuntimeCode(): Promise<string> {
  if (!cachedRuntime) {
    cachedRuntime = (async () => {
      const res = await fetch(pluginFrameRuntimeUrl(), { credentials: "same-origin" });
      if (!res.ok) {
        throw new Error(`failed to load plugin frame runtime (HTTP ${res.status})`);
      }
      return await res.text();
    })().catch((error) => {
      cachedRuntime = null;
      throw error;
    });
  }
  return cachedRuntime;
}
