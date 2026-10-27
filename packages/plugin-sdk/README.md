# @droppen/plugin-sdk

The contract between [Droppen](../../README.md) and its plugins.

- **Types** for the manifest (`package.json` → `droppen`), contributions (navigation,
  routes under `/p/<id>/`, split-pane content, settings blocks, strings) and the
  `PluginHost` a plugin is given.
- **`definePlugin`**, **`usePluginHost`**, **`isStorageError`** and **`satisfies`**
  (the engine-range check).
- **`@droppen/plugin-sdk/testing`**: `createTestHost()`, an in-memory host with real
  compare-and-set storage that records every file, print, copy, question and
  navigation — for a plugin's own unit tests.

A plugin depends on React and this package only (both as peer dependencies), and
does every bit of I/O through `host.*`: no `fetch` of host URLs, no
`window.confirm` / `window.print` / `<a download>`, no service worker, no host CSS
utility classes (use the host's design variables, `--color-*`, `--font-*`,
`--tracking-label`, and prefix your own classes). That is what lets one plugin
build run unchanged in the desktop app's window and in a browser on the server's
own page — and never on droppen.app, which loads no plugin code at all
([`docs/plugins/runtime.md`](../../docs/plugins/runtime.md) §5).

The page resolves four bare specifiers for a plugin — `react`,
`react/jsx-runtime`, `react-dom` and `@droppen/plugin-sdk` — through its import
map, to the host's own instances of them: one React, one `PluginHostContext`.
Any other bare import does not resolve; a plugin talks to the host through this
package.

`engines.droppen` ranges follow Cargo's rules (the host's server reads them
with the `semver` crate): comparators joined by commas, a bare version meaning
`^`. `satisfies` and `validRange` implement exactly that, checked against the
same vectors as the Rust side (`test/engine-vectors.json`).

## Versions

`SDK_VERSION` versions everything a plugin binds to, the host's React
included — a plugin's `react` *is* the host's:

| SDK | React |
|---|---|
| 0.1 | 19 |

On 0.x a minor is breaking: `^0.1.0` means `>=0.1.0, <0.2.0`, and a plugin
built for 0.1 shows as not compatible on a 0.2 host until a build for 0.2 is
installed. A React major in the host is such a break even when no SDK type
moved. `test/sdk.test.ts` fails when the web build's React and this table
part ways.

## Trust

A plugin runs in the host's page with no sandbox: it can do whatever the
signed-in user can do there, terminals included. Installing one is trusting
it as fully as a program run in that machine's terminal. `permissions` is
declared and checked at install, **not enforced** — it tells a reader what the
plugin means to use. See
[`docs/plugins/runtime.md`](../../docs/plugins/runtime.md) §8.

## Packaging and installing

A plugin is an ESM package whose `package.json` carries the manifest and whose
entry default-exports `definePlugin(...)`:

```json
{
  "name": "@droppen/plugin-example",
  "version": "0.1.0",
  "type": "module",
  "exports": { ".": "./dist/index.js" },
  "peerDependencies": { "react": "^19", "@droppen/plugin-sdk": "^0.1.0" },
  "droppen": {
    "id": "example",
    "name": { "en": "Example", "zh-CN": "示例" },
    "engines": { "droppen": "^0.1.0" },
    "permissions": ["storage"],
    "aliases": []
  }
}
```

Build it in library mode with `react`, `react/jsx-runtime`, `react-dom` and
`@droppen/plugin-sdk` external, and keep views behind `React.lazy`: a plugin's
code is fetched when it is first shown, not with the page. `pnpm pack` writes
the tarball; install it into one instance with

```sh
droppen plugin install release/droppen-plugin-example-0.1.0.tgz --instance <name>
```

or, on a page that instance serves, Settings › Plugins › *Install from file…*.
`droppen plugin list` shows each installed plugin's version and build.
Installing an id that is already installed replaces it atomically and turns it
on; uninstalling keeps its documents (`instances/<instance>/plugins/<id>/store.db`)
and its on/off decision for a later install.

Aliases are single lower-case path segments (`/tasks`), and may not take a
top-level path the shell already answers (`/settings`, `/sessions`, …): the
server refuses such a package at install. `engines.droppen` is checked the same
way — an incompatible package is not installed. A plugin that took over a
built-in feature carries its own migration and rollback steps, with the rest of
its release runbook.

The document store's HTTP protocol is in
[`docs/plugins/storage-protocol.md`](../../docs/plugins/storage-protocol.md).

## Licence

MIT OR Apache-2.0, at your option — independently of the licence of the application
that hosts plugins.
