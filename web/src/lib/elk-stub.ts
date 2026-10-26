/**
 * What `elkjs` resolves to in this build (`resolve.alias` in `vite.config.ts`).
 *
 * Mermaid ships its ELK layouts as a lazily loaded part of itself, and that
 * part imports elkjs, which is EPL-2.0: not on the licence allow-list in
 * `build/third-party-notices.mjs`, so it is not bundled at all. Every diagram
 * is configured for dagre (`lib/mermaid-render.ts`) and a fence cannot ask
 * for another layout, so nothing should reach this; if something does, it
 * refuses, the render fails, and the block stays its code.
 */
export default class ELK {
  layout(): Promise<never> {
    return Promise.reject(new Error("the ELK layout is not part of this build"));
  }
}
