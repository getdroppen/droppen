# Plugins at runtime (V1)

Droppen's web build ships **no plugin code**, and it is the same build
everywhere: the local server embeds it, droppen.app serves it, and the two
must stay byte-identical (`docs/web-integrity.md`). A plugin is therefore
**installed into an instance at runtime** and **served by that instance's own
server**, to the pages that server serves — and to nothing else:

| Where the page comes from | Plugins |
|---|---|
| The desktop app's window (`http://127.0.0.1:<port>/`) | yes |
| A browser on the server's own page (the app's *Open in Browser*, `remote-hand-server open`, a self-hosted server's address) | yes |
| droppen.app (a page a hub serves, `isCloud()`) | **no** — no code, no list, no documents |

A plugin runs with the page's own rights, unsandboxed: installing one is
trusting it as fully as a program run in that machine's terminal (§8).

This replaces the build-time *distribution* (`distribution.json`,
`DROPPEN_DISTRIBUTION`, `web/build/droppen-plugins.ts`,
`virtual:droppen/bundled-plugins`), which is removed. The plugin package
format does not change: a `pnpm pack` tarball of an ESM package whose
`package.json` carries the `droppen` manifest and whose entry default-exports
`definePlugin(...)` (see `packages/plugin-sdk/README.md`). The document store protocol
(`storage-protocol.md`) is unchanged apart from what this page says.

## 1. Installing

Per **instance**: each instance installs its own copy, at its own version,
and switches it on or off on its own.

```
remote-hand-server plugin install <file.tgz>   [--instance <name>]
remote-hand-server plugin uninstall <id>        [--instance <name>]
remote-hand-server plugin list                  [--instance <name>]   # now shows each one's build
```

and, from a page the server serves itself, Settings › Plugins › *Install from
file…* / *Uninstall* (`POST /api/plugins`, `DELETE /api/plugins/{id}`, §4).

On disk:

```
<data>/instances/<instance>/plugin-packages/<id>/   the unpacked package (its package.json at the top)
<data>/instances/<instance>/plugins/<id>/store.db   its documents — unchanged
```

- One version per plugin per instance. Installing an id that is installed
  replaces it (upgrade or downgrade) atomically: unpack and check into a
  temporary directory next to it, then swap; a failed install leaves the old
  one as it was.
- Installing turns the plugin **on** (an instance decision, as if switched
  on), unless the configuration forces it off.
- Uninstalling removes the package directory only. Its documents
  (`plugins/<id>/store.db`) and its on/off decision stay, for a later install;
  `plugin purge` is still how documents go.

### What an install checks

The tarball is a gzip'd tar whose every entry is under `package/` (what
`npm pack` / `pnpm pack` write). Refused, with a message saying why:

- any entry that is not a regular file or a directory (symlinks, hard links,
  devices), an absolute path, or a `..` segment; more than 2,000 files or
  64 MiB unpacked; a tarball over 32 MiB;
- no `package/package.json`, or one without a `droppen` object;
- `droppen.id` not matching `^[a-z][a-z0-9-]{1,31}$`; `version` not semver;
- `droppen.permissions` with anything but `storage`, `sessions:read`,
  `notify`;
- `droppen.aliases` not single lower-case segments (`^/[a-z0-9-]+$`, ≤ 64
  chars) or naming a reserved top-level path (the list in today's
  `web/build/droppen-plugins.ts` `RESERVED_PATHS`, moved to the Rust side);
- `droppen.engines.droppen` (Cargo rules, as today) not satisfied by this
  host's SDK version: an incompatible plugin is **not installed**. (One that
  an upgrade of the host makes incompatible stays installed and shows
  `compatible: false`, as today; while it is switched on, the page names it
  rather than drops it, §6.)
- the entry — `exports["."]` (a string, or its `import` / `default`), else
  `module`, else `main`, else `index.js` — not a relative `.js`/`.mjs` file
  inside the package.

Each install gets a **build id**: `<version>-<first 12 hex of the tarball's
sha256>`. It names this exact code in every URL (§2), so a reinstall of the
same version with other bytes is a new URL.

## 2. Serving the code

```
GET /plugins/<key>/<id>/<build>/<path inside the package>
```

- `<key>` is an instance-wide secret: 128 random bits as hex, kept in the
  instance's `state.db` (kv scope `plugins`), created on first need, stable
  across restarts. A page cannot send its bearer header with `import()`, and
  a query string would not survive the plugin's own relative chunk imports
  (`./tasks-page-….js`), so the capability is a path segment. It reaches a
  page only inside `PluginStatus.entry`, which only a signed-in, non-cloud
  caller is given (§4, §5). Compared in constant time.
- Answered only for an installed plugin that is enabled and compatible, at
  its current build, for a file inside its package directory; everything
  else is `404` (wrong key, unknown id, old build, off, traversal, a
  directory). `GET` and `HEAD` only. Never answered to a bridged request.
- `content-type` by extension (`.js`/`.mjs` → `text/javascript`, `.css`,
  `.json`, `.svg`, `.png`, `.woff2`, …; else `application/octet-stream`),
  `x-content-type-options: nosniff`, `cache-control: private, max-age=31536000,
  immutable` (the build id is in the URL), `cross-origin-resource-policy:
  same-origin`.
- The service worker (`web/public/sw.js`) never caches or answers
  `/plugins/**`.

## 3. Sharing React and the SDK

A plugin's build leaves `react`, `react/jsx-runtime`, `react-dom` and
`@droppen/plugin-sdk` as bare imports (the todo plugin's `dist` imports
exactly the first, second and fourth). They must resolve to **the host's own
instances** — one React, and one `PluginHostContext`.

The web build emits one facade chunk per shared module, each re-exporting the
host's module with its export names intact (an emitted chunk with a strict
signature), and writes an import map naming them into `index.html`, before
any module script:

```html
<script type="importmap">{"imports":{"react":"/assets/plugin-shared-react-<hash>.js", "react/jsx-runtime":"…", "react-dom":"…", "@droppen/plugin-sdk":"…"}}</script>
```

It is part of the one build: the same bytes in every mode, an inline script
the build already hashes for the service worker's and the cloud headers' CSP,
and inert where no plugin loads. The server does not touch the shell. Import
maps need WebKit 16.4 (macOS 13.3) in the desktop app's window.

### Versions

`SDK_VERSION` (`droppen-plugin-sdk` and `@droppen/plugin-sdk`, one value,
checked by the SDK's tests) is what a plugin's `engines.droppen` ranges over.
It versions everything a plugin binds to — **React included**: a plugin's bare
`react` is the host's React, so the web build's React major is as much a part
of the contract as any SDK type.

- On 0.x, Cargo's rules make `^0.1.0` mean `>=0.1.0, <0.2.0`: a **patch**
  (0.1.x) may only add or fix, a **minor** (0.2.0) is breaking. From 1.0, the
  usual major and minor.
- A React major in the web build (19 → 20) is a breaking SDK change even when
  no SDK type moved: it takes the next breaking `SDK_VERSION`, and the SDK's
  `react` peer range follows. `packages/plugin-sdk/test/sdk.test.ts` keeps the
  table of which React each SDK version lends, and fails when the web's React
  and `SDK_VERSION` move apart.
- A breaking bump leaves every installed plugin `compatible: false` on the
  next start, and the desktop app updates itself without asking. A plugin's
  build for the new SDK is therefore ready before the host release, and
  installed once the host has updated (an install is checked against the
  running host). In between, the page names each switched-on plugin that no
  longer runs (§6).

## 4. HTTP API changes

`PluginStatus` (`droppen-plugin-sdk`, `packages/plugin-sdk`, `web/src/plugins/types.ts`)
gains:

```ts
interface PluginStatus {
  // … as in storage-protocol.md, plus:
  build: string;          // "<version>-<12 hex>"
  entry: string | null;   // "/plugins/<key>/<id>/<build>/<entry>" when enabled and compatible; else null
  aliases: string[];
  permissions: string[];  // declared, not enforced (§8)
  switchedOn: boolean;    // on by the configuration or the instance, whether or not compatible
}
```

| Method and path | Body | Answer |
|---|---|---|
| `GET /api/plugins` | — | `200 { plugins: PluginStatus[] }` — every plugin **installed** in this instance |
| `POST /api/plugins` | the tarball's bytes (`application/gzip` or `application/octet-stream`), ≤ 32 MiB | `200 PluginStatus`; `400 { error: "invalid_package", message }`; `413 too_large` |
| `DELETE /api/plugins/{id}` | — | `204`; `404 plugin_not_found` |
| the rest of `storage-protocol.md` | | unchanged |

After an install, an uninstall or a switch, open (non-cloud) clients get a
`plugins` frame, as after a switch today.

## 5. Not to the cloud

A request that came in over the end-to-end bridge (identity `cloud:<device>`)
gets none of it:

- **the bridge's allowlist** (`server-rs/src/agent/bridge.rs`) drops every
  `/api/plugins…` route, so droppen.app can neither list, switch, install nor
  read or write a document;
- **the handlers** refuse a `cloud:` identity as well (`404 plugin_not_found`),
  so a route the allowlist ever lets through by mistake still answers nothing;
- **the feed**: a `cloud:` subscriber's `settings` frame carries
  `plugins: []`, and it is sent no `plugins` and no `plugin-doc` frames;
- **the hub copy** (what the machine mirrors to the hub for offline reading)
  carries no plugin status and no plugin document;
- **the page**: with `isCloud()` the registry reads no status, imports
  nothing, and the shell shows no plugin nav entry, page, pane, alias or
  Settings › Plugins section.

## 6. In the page

- The registry (`web/src/plugins/registry.tsx`) takes its list from the
  statuses (the feed's `settings` / `plugins` frames, else
  `GET /api/plugins`): every status with an `entry` is loaded with
  `import(/* @vite-ignore */ entry)`, its `definition.manifest.id` checked
  against the status's id, and activated once per (machine, plugin, build), as
  today.
- Aliases come from the statuses, so they are known only once the list is:
  an alias path opened directly (`/tasks`) goes to `/p/<id>/` as soon as the
  list says a plugin has it.
- A plugin that is switched on but cannot run here (`switchedOn` and not
  `compatible`, typically after a host update, §3) is not loaded, and not
  dropped either: the rail keeps a plain entry for it marked as needing an
  update, and its page, its aliases and any pane of it say so, with the
  status's `reason`. Switching it off in Settings › Plugins takes the entry
  away.
- A module cannot be unloaded. After an install, upgrade or uninstall from
  Settings › Plugins the page reloads; one made elsewhere (the CLI, another
  tab) shows a *Reload to finish* prompt in Settings › Plugins and on the
  plugin's page.

## 7. Moving off the build-time distribution

Documents stay where they were (`plugins/<id>/store.db`) and so do on/off
decisions; only the code's way in changes. An instance that ran a private
distribution build runs the public build plus `plugin install <tarball>`.
Before the first such deploy, install the tarball into each instance with the
new binary (`--instance`), then start it.

## 8. Trust and permissions

Installing a plugin is **trusting it fully**, as with a program run in that
machine's terminal:

- Its code runs in the host's page, at the host's origin, with no sandbox. It
  can read whatever the page can — the signed-in user's token in
  `localStorage` included — and call every API the page can (terminals,
  sessions, files) as that user.
- `droppen.permissions` (`storage`, `sessions:read`, `notify`) is
  **declared, not enforced**. The install checks the names; nothing at run
  time holds a plugin to them. They tell a reader what the plugin means to
  use, and keep the field's place for a host that may enforce it later.
- So install only what you would run in a terminal there. Settings › Plugins
  says this and asks before an install; `plugin install` is already run from
  one.

droppen.app loads no plugin code at all (§5), so a plugin never runs on a
page the hosted service serves.
