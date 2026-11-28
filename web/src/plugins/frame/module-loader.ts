/**
 * Loads an unpacked plugin package inside the sandboxed frame via `blob:` URLs
 * and a runtime `<script type="importmap">` (`docs/plugins/cloud.md` §3).
 */
import type { PluginDefinition } from "@droppen/plugin-sdk";
import type { FramePackageData } from "./protocol";
import { PKG_SPECIFIER_PREFIX, rewriteRelativeImports } from "./rewrite-imports";

function base64ToBytes(b64: string): Uint8Array<ArrayBuffer> {
  const bin = atob(b64);
  const out = new Uint8Array(new ArrayBuffer(bin.length));
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

/**
 * Create a tiny facade `blob:` module that re-exports every key of a shared
 * host module stored on `globalThis.__droppenFrameShared[specifier]`.
 */
function sharedFacadeBlobUrl(specifier: string, mod: Record<string, unknown>): string {
  const keys = Object.keys(mod).filter((k) => k !== "default" && /^[A-Za-z_$][\w$]*$/.test(k));
  const lines = [
    `const m = globalThis.__droppenFrameShared[${JSON.stringify(specifier)}];`,
    ...keys.map((k) => `export const ${k} = m[${JSON.stringify(k)}];`),
    `export default m.default ?? m;`,
  ];
  return URL.createObjectURL(new Blob([lines.join("\n")], { type: "text/javascript" }));
}

function appendStylesheet(href: string): void {
  const link = document.createElement("link");
  link.rel = "stylesheet";
  link.href = href;
  document.head.appendChild(link);
}

/**
 * Turn `packageData`'s files into `blob:` URLs, install an import map that
 * lends React and `@droppen/plugin-sdk` from `shared`, and dynamically import
 * the plugin's entry module.
 */
export async function loadPluginPackage(
  packageData: FramePackageData,
  shared: Record<string, Record<string, unknown>>,
): Promise<PluginDefinition> {
  (globalThis as unknown as { __droppenFrameShared: Record<string, Record<string, unknown>> }).__droppenFrameShared = shared;

  const imports: Record<string, string> = {};
  for (const [specifier, mod] of Object.entries(shared)) {
    imports[specifier] = sharedFacadeBlobUrl(specifier, mod);
  }

  const fileIndex = new Set(packageData.files.map((f) => f.path));

  for (const file of packageData.files) {
    const isJs = file.path.endsWith(".js") || file.path.endsWith(".mjs");
    const isCss = file.path.endsWith(".css");
    if (isJs) {
      const source =
        file.encoding === "utf8"
          ? file.data
          : new TextDecoder().decode(base64ToBytes(file.data));
      const rewritten = rewriteRelativeImports(source, file.path, (resolved) => {
        // Resolve extensionless relative imports if needed (e.g. `./foo` -> `./foo.js`).
        const matched = fileIndex.has(resolved)
          ? resolved
          : fileIndex.has(`${resolved}.js`)
            ? `${resolved}.js`
            : fileIndex.has(`${resolved}.mjs`)
              ? `${resolved}.mjs`
              : resolved;
        return `${PKG_SPECIFIER_PREFIX}${matched}`;
      });
      const blobUrl = URL.createObjectURL(new Blob([rewritten], { type: "text/javascript" }));
      imports[`${PKG_SPECIFIER_PREFIX}${file.path}`] = blobUrl;
    } else if (isCss) {
      const bytes =
        file.encoding === "utf8" ? file.data : base64ToBytes(file.data);
      const cssUrl = URL.createObjectURL(new Blob([bytes], { type: "text/css" }));
      // Always apply packaged stylesheets and also allow JS chunks to `import "./style.css"`.
      appendStylesheet(cssUrl);
      const cssStub = URL.createObjectURL(
        new Blob([`export default ${JSON.stringify(cssUrl)};`], { type: "text/javascript" }),
      );
      imports[`${PKG_SPECIFIER_PREFIX}${file.path}`] = cssStub;
    } else if (file.path.endsWith(".json")) {
      const text =
        file.encoding === "utf8"
          ? file.data
          : new TextDecoder().decode(base64ToBytes(file.data));
      const jsonStub = URL.createObjectURL(
        new Blob([`export default ${text};`], { type: "text/javascript" }),
      );
      imports[`${PKG_SPECIFIER_PREFIX}${file.path}`] = jsonStub;
    } else {
      const bytes =
        file.encoding === "utf8"
          ? new TextEncoder().encode(file.data)
          : base64ToBytes(file.data);
      const assetUrl = URL.createObjectURL(new Blob([bytes], { type: file.contentType || "application/octet-stream" }));
      const assetStub = URL.createObjectURL(
        new Blob([`export default ${JSON.stringify(assetUrl)};`], { type: "text/javascript" }),
      );
      imports[`${PKG_SPECIFIER_PREFIX}${file.path}`] = assetStub;
    }
  }

  const mapScript = document.createElement("script");
  mapScript.type = "importmap";
  mapScript.textContent = JSON.stringify({ imports });
  document.head.appendChild(mapScript);

  const entrySpec = `${PKG_SPECIFIER_PREFIX}${packageData.entry}`;
  const mod = (await import(/* @vite-ignore */ entrySpec)) as { default?: PluginDefinition };
  const definition = mod?.default;
  if (!definition || typeof definition.activate !== "function") {
    throw new Error("plugin entry module has no default definePlugin(...) export");
  }
  if (definition.manifest?.id !== packageData.id) {
    throw new Error(`package declares id "${definition.manifest?.id}", expected "${packageData.id}"`);
  }
  return definition;
}
