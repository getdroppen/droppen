/**
 * `droppenPlugins()` — compose a Droppen *distribution* at build time.
 *
 * Which plugins a build contains is not decided in this repository's code but
 * by a distribution manifest:
 *
 * - `distribution.json` at the repository root, which for the open build lists
 *   none — the public distribution ships no plugins;
 * - or the file `DROPPEN_DISTRIBUTION` names, which is how a private build
 *   (one person's own plugins) is made without anything private entering this
 *   repository.
 *
 * ```json
 * { "name": "my-build", "plugins": [
 *   { "tarball": "../release/droppen-plugin-x-0.1.0.tgz" },   // `pnpm pack` output
 *   { "path": "/abs/or/relative/plugin/checkout" },            // a built package directory
 *   { "package": "@scope/plugin-y", "defaultEnabled": true }   // installed in web/node_modules
 * ] }
 * ```
 *
 * Relative paths are resolved against the manifest's own directory. Each
 * plugin package carries its manifest in `package.json` → `droppen`; its
 * `engines.droppen` must accept the SDK in `packages/plugin-sdk`, or the build
 * fails here rather than at runtime.
 *
 * The result is two things:
 *
 * - the virtual module `virtual:droppen/bundled-plugins`, which the host's
 *   registry imports: each plugin's manifest plus a `load()` that dynamically
 *   imports its entry, so a plugin's code is a lazy chunk that nobody fetches
 *   until the plugin is enabled;
 * - `dist/plugins/manifest.json`, embedded in the server binary with the rest
 *   of `web/dist`, which is how the server learns what it ships without running
 *   any JavaScript.
 *
 * It also guards the one way a plugin could leak into every page: a plugin
 * module statically reachable from the entry (the vendor chunk in particular)
 * fails the build.
 */
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, realpathSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, isAbsolute, join, resolve, sep } from "node:path";
import { searchForWorkspaceRoot, type Plugin } from "vite";
import { parseVersion, satisfies, validRange } from "../../packages/plugin-sdk/src/semver";

const VIRTUAL_ID = "virtual:droppen/bundled-plugins";
const RESOLVED_VIRTUAL_ID = "\0" + VIRTUAL_ID;
const PLUGIN_ID = /^[a-z][a-z0-9-]{1,31}$/;
/** What a plugin's code must share with the host, one copy of each. */
const SHARED = ["react", "react-dom", "@droppen/plugin-sdk"];
/** The permissions the host knows (`droppen_plugin_sdk::Permission`). */
const PERMISSIONS = new Set(["storage", "sessions:read", "notify"]);
/**
 * Top-level paths the shell, the server and droppen.app already answer. A
 * plugin alias may not take one: `/settings` as an alias would hijack
 * Settings. `test/droppen-plugins.test.ts` keeps this in step with the router.
 */
export const RESERVED_PATHS = new Set([
  "p", "m", "api", "embed", "assets", "plugins", "splash",
  "login", "setup", "welcome", "sessions", "chat", "files", "terminals", "settings", "stats", "updates", "apps", "screens",
  "hub", "signin", "signup", "verify", "link",
]);

export interface DistributionEntry {
  package?: string;
  path?: string;
  tarball?: string;
  /** Overrides the package's own `droppen.defaultEnabled`. */
  defaultEnabled?: boolean;
}

export interface Distribution {
  name?: string;
  plugins: DistributionEntry[];
}

/** One plugin as the build sees it. */
export interface ResolvedPlugin {
  id: string;
  /** The package directory (with `package.json`). */
  dir: string;
  /** The entry module's absolute path. */
  entry: string;
  /** The manifest the server and the host read. */
  manifest: {
    id: string;
    version: string;
    name: unknown;
    engines: { droppen: string };
    permissions: string[];
    aliases: string[];
    defaultEnabled: boolean;
    package: string;
  };
}

export interface DroppenPluginsOptions {
  /** `web/`. */
  webRoot: string;
  /** Defaults to `DROPPEN_DISTRIBUTION`, else `<repo>/distribution.json`. */
  distribution?: string;
}

/** Which manifest this build uses. */
export function distributionPath(webRoot: string, override = process.env.DROPPEN_DISTRIBUTION): string {
  const named = override?.trim();
  return named ? resolve(process.cwd(), named) : resolve(webRoot, "..", "distribution.json");
}

function fail(message: string): never {
  throw new Error(`[droppen-plugins] ${message}`);
}

/** Unpack a `pnpm pack` tarball once, keyed by its content. */
function unpack(tarball: string, webRoot: string): string {
  if (!existsSync(tarball)) fail(`tarball not found: ${tarball}`);
  const digest = createHash("sha256").update(readFileSync(tarball)).digest("hex").slice(0, 16);
  const into = join(webRoot, "node_modules", ".droppen-plugins", digest);
  const done = join(into, ".unpacked");
  if (!existsSync(done)) {
    mkdirSync(into, { recursive: true });
    execFileSync("tar", ["-xzf", tarball, "-C", into]);
    writeFileSync(done, tarball + "\n");
  }
  const dir = join(into, "package");
  if (!existsSync(join(dir, "package.json"))) fail(`${tarball} has no package/package.json (not a pnpm/npm pack tarball?)`);
  return dir;
}

function entryOf(dir: string, pkg: Record<string, unknown>): string {
  const exp = pkg.exports as unknown;
  let rel: unknown = undefined;
  if (typeof exp === "string") rel = exp;
  else if (exp && typeof exp === "object") {
    const dot = (exp as Record<string, unknown>)["."] ?? exp;
    rel = typeof dot === "string" ? dot : (dot as Record<string, unknown>)?.import ?? (dot as Record<string, unknown>)?.default;
  }
  rel ??= pkg.module ?? pkg.main ?? "index.js";
  if (typeof rel !== "string") fail(`${dir}/package.json: cannot tell the entry module from "exports"`);
  const entry = resolve(dir, rel);
  if (!existsSync(entry)) fail(`${entry} does not exist — is the plugin built?`);
  return entry;
}

/** Read and check the distribution: every plugin resolved, unique and compatible. */
export function resolveDistribution(webRoot: string, file: string, sdkVersion: string): { name: string; plugins: ResolvedPlugin[] } {
  if (!existsSync(file)) fail(`distribution manifest not found: ${file}`);
  let parsed: Distribution;
  try {
    parsed = JSON.parse(readFileSync(file, "utf8")) as Distribution;
  } catch (error) {
    fail(`${file} is not JSON: ${(error as Error).message}`);
  }
  if (!parsed || !Array.isArray(parsed.plugins)) fail(`${file} needs a "plugins" array`);
  const base = dirname(file);
  const require = createRequire(join(webRoot, "package.json"));
  const seen = new Set<string>();
  const plugins = parsed.plugins.map((entry, index): ResolvedPlugin => {
    const sources = [entry.package, entry.path, entry.tarball].filter((s) => typeof s === "string");
    if (sources.length !== 1) fail(`${file} plugins[${index}]: give exactly one of "package", "path" or "tarball"`);
    const found = entry.tarball
      ? unpack(isAbsolute(entry.tarball) ? entry.tarball : resolve(base, entry.tarball), webRoot)
      : entry.path
        ? resolve(base, entry.path)
        : dirname(require.resolve(`${entry.package}/package.json`));
    // Module ids are real paths; a symlinked checkout must still be recognised as plugin code.
    const dir = existsSync(found) ? realpathSync(found) : found;
    const pkgFile = join(dir, "package.json");
    if (!existsSync(pkgFile)) fail(`${pkgFile} not found`);
    const pkg = JSON.parse(readFileSync(pkgFile, "utf8")) as Record<string, unknown>;
    const droppen = pkg.droppen as Record<string, unknown> | undefined;
    const where = `${pkg.name ?? dir}`;
    if (!droppen || typeof droppen !== "object") fail(`${where}: package.json has no "droppen" manifest`);
    const id = droppen.id;
    if (typeof id !== "string" || !PLUGIN_ID.test(id)) fail(`${where}: "${String(id)}" is not a plugin id`);
    if (seen.has(id)) fail(`plugin id "${id}" appears twice in ${file}`);
    seen.add(id);
    // The same rules the server applies when it reads dist/plugins/manifest.json
    // (`droppen_plugin_sdk::Manifest`): what passes here, the server accepts.
    const engines = (droppen.engines as { droppen?: unknown } | undefined)?.droppen;
    if (typeof engines !== "string" || !validRange(engines))
      fail(
        `${where}: engines.droppen ${JSON.stringify(engines)} is not a range the host reads — comparators joined by commas, a bare version meaning ^ (Cargo's rules), e.g. "^0.1.0" or ">=0.1.0, <0.3.0"`,
      );
    if (!satisfies(sdkVersion, engines))
      fail(`${where} needs SDK ${engines}, but this tree has @droppen/plugin-sdk ${sdkVersion} — rebuild the plugin against it`);
    if (typeof pkg.version !== "string" || !parseVersion(pkg.version))
      fail(`${where}: package.json version ${JSON.stringify(pkg.version)} is not a semver version (major.minor.patch)`);
    const permissions = droppen.permissions ?? [];
    if (!Array.isArray(permissions) || permissions.some((p) => typeof p !== "string" || !PERMISSIONS.has(p)))
      fail(`${where}: permissions must be some of ${[...PERMISSIONS].join(", ")}`);
    const name = droppen.name ?? id;
    const nameOk =
      (typeof name === "string" && name.trim() !== "") ||
      (typeof name === "object" &&
        name !== null &&
        !Array.isArray(name) &&
        typeof (name as Record<string, unknown>).en === "string" &&
        Object.values(name).every((text) => typeof text === "string" && text.trim() !== ""));
    if (!nameOk) fail(`${where}: name must be a string or {"en": …, "zh-CN"?: …} of strings`);
    if (droppen.defaultEnabled !== undefined && typeof droppen.defaultEnabled !== "boolean")
      fail(`${where}: defaultEnabled must be true or false`);
    const aliases = (droppen.aliases ?? []) as unknown[];
    if (!Array.isArray(aliases) || aliases.some((a) => typeof a !== "string" || a.length > 64 || !/^\/[a-z0-9-]+$/.test(a)))
      fail(`${where}: aliases must be single lower-case path segments such as "/tasks"`);
    const taken = (aliases as string[]).find((alias) => RESERVED_PATHS.has(alias.slice(1)));
    if (taken) fail(`${where}: alias "${taken}" would shadow the shell's own ${taken} — pick another path`);
    return {
      id,
      dir,
      entry: entryOf(dir, pkg),
      manifest: {
        id,
        version: pkg.version,
        name,
        engines: { droppen: engines },
        permissions: permissions as string[],
        aliases: aliases as string[],
        defaultEnabled: entry.defaultEnabled ?? droppen.defaultEnabled === true,
        package: String(pkg.name ?? id),
      },
    };
  });
  resolvedDirs = plugins.map((p) => p.dir);
  return { name: parsed.name ?? "unnamed", plugins };
}

/**
 * Why a plugin module may not import `source`, or `null` when it may: the
 * core's `@/…` alias, a root-relative `/src/…`, or any path that lands in
 * `web/src`.
 */
export function coreImport(source: string, importer: string, webRoot: string): string | null {
  const reason = "plugins use the host only through @droppen/plugin-sdk";
  if (source.startsWith("@/") || source === "@") return reason;
  if (source.startsWith("/src/")) return reason;
  const src = resolve(webRoot, "src") + sep;
  const bare = importer.split("?")[0]!;
  const target = isAbsolute(source) ? source : source.startsWith(".") ? resolve(dirname(bare), source) : null;
  if (target && (target + sep).startsWith(src)) return reason;
  return null;
}

/**
 * Whether a resolved module id is the core's own code: a file under
 * `web/src`, by its real path (so a symlink or an absolute path through one
 * does not hide it).
 */
export function inCore(id: string, webRoot: string): boolean {
  if (id.startsWith("\0") || id.startsWith("virtual:")) return false;
  const file = id.split("?")[0]!;
  if (!isAbsolute(file)) return false;
  const real = (path: string) => (existsSync(path) ? realpathSync(path) : path);
  return (real(file) + sep).startsWith(real(resolve(webRoot, "src")) + sep);
}

/** The package directories of the distribution last resolved (see {@link isPluginModule}). */
let resolvedDirs: string[] = [];

/** Whether a module id belongs to a bundled plugin's own code. */
export function isPluginModule(id: string): boolean {
  return /\/node_modules\/(?:@droppen\/plugin-(?!sdk\b)|\.droppen-plugins\/)/.test(id) || resolvedDirs.some((dir) => id.startsWith(dir + sep));
}

export function droppenPlugins(options: DroppenPluginsOptions): Plugin {
  const webRoot = options.webRoot;
  const file = options.distribution ?? distributionPath(webRoot);
  const sdkPackage = JSON.parse(readFileSync(resolve(webRoot, "../packages/plugin-sdk/package.json"), "utf8")) as { version: string };
  const sdkVersion = sdkPackage.version;
  let resolved: { name: string; plugins: ResolvedPlugin[] } | null = null;
  const load = () => {
    resolved ??= resolveDistribution(webRoot, file, sdkVersion);
    return resolved;
  };
  const appEntry = join(webRoot, "src", "main.tsx");

  return {
    name: "droppen-plugins",
    enforce: "pre",
    config() {
      const dist = load();
      return {
        resolve: { dedupe: SHARED },
        server: { fs: { allow: [searchForWorkspaceRoot(webRoot), ...dist.plugins.map((p) => p.dir)] } },
      };
    },
    configResolved(config) {
      const dist = load();
      config.logger.info(
        `[droppen-plugins] distribution "${dist.name}" (${file}): ${dist.plugins.length ? dist.plugins.map((p) => `${p.id}@${p.manifest.version}`).join(", ") : "no plugins"}`,
      );
    },
    async resolveId(source, importer, opts) {
      if (source === VIRTUAL_ID) return RESOLVED_VIRTUAL_ID;
      if (!importer || !isPluginModule(importer)) return null;
      // A plugin gets the host through the SDK and nothing else: no reaching
      // into the core's own code, which is not the plugin's to link (or
      // license). Fails the build rather than bundling it — by the specifier
      // as written, and by the real path it resolves to.
      const why = coreImport(source, importer, webRoot);
      if (why) this.error(`[droppen-plugins] ${importer} imports "${source}" — ${why}`);
      // A plugin's own `react` must be the host's `react`: two copies would
      // mean two hook dispatchers and a host context the plugin cannot see.
      if (SHARED.some((s) => source === s || source.startsWith(s + "/"))) {
        return this.resolve(source, appEntry, { ...opts, skipSelf: true });
      }
      const resolved = await this.resolve(source, importer, { ...opts, skipSelf: true });
      if (resolved && !resolved.external && inCore(resolved.id, webRoot))
        this.error(`[droppen-plugins] ${importer} imports "${source}", which is ${resolved.id} — plugins use the host only through @droppen/plugin-sdk`);
      return resolved;
    },
    load(id) {
      if (id !== RESOLVED_VIRTUAL_ID) return null;
      const dist = load();
      const rows = dist.plugins.map(
        (p) => `  { id: ${JSON.stringify(p.id)}, manifest: ${JSON.stringify(p.manifest)}, load: () => import(${JSON.stringify(p.entry)}) },`,
      );
      return `export const distribution = ${JSON.stringify(dist.name)};\nexport const bundledPlugins = [\n${rows.join("\n")}\n];\n`;
    },
    generateBundle(_options, bundle) {
      const dist = load();
      // Whatever path an import took, nothing a plugin module imports may be
      // the core's own code.
      for (const id of this.getModuleIds()) {
        if (!isPluginModule(id)) continue;
        const info = this.getModuleInfo(id);
        for (const imported of [...(info?.importedIds ?? []), ...(info?.dynamicallyImportedIds ?? [])]) {
          if (inCore(imported, webRoot))
            fail(`${id} imports ${imported} — plugins use the host only through @droppen/plugin-sdk`);
        }
      }
      // Everything the entry loads before anything is clicked must be plugin-free.
      const boot = new Set<string>();
      const visit = (fileName: string) => {
        const chunk = bundle[fileName];
        if (!chunk || chunk.type !== "chunk" || boot.has(fileName)) return;
        boot.add(fileName);
        for (const imported of chunk.imports) visit(imported);
      };
      for (const chunk of Object.values(bundle)) if (chunk.type === "chunk" && chunk.isEntry) visit(chunk.fileName);
      for (const fileName of boot) {
        const chunk = bundle[fileName];
        if (chunk?.type !== "chunk") continue;
        const leaked = chunk.moduleIds.filter(isPluginModule);
        if (leaked.length) fail(`plugin code is in the boot chunk ${fileName}: ${leaked.slice(0, 3).join(", ")} — check vendorChunk in build/vendor-chunks.ts`);
      }
      this.emitFile({
        type: "asset",
        fileName: "plugins/manifest.json",
        source: JSON.stringify({ sdkVersion, distribution: dist.name, plugins: dist.plugins.map((p) => p.manifest) }, null, 2) + "\n",
      });
    },
  };
}
