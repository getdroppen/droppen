# Plugins on droppen.app

**Status:** §1, the switch, is built. §2–§7 are the design for what follows
it, and none of that is built yet. Until it is, droppen.app shows no plugin,
whatever the switch says ([runtime.md](runtime.md) §5).

## Why not the way a local page does it

A page the instance serves imports a plugin's code into itself
([runtime.md](runtime.md) §6). The plugin runs with that page's rights, which
are the rights of the user on that one machine (§8).

A page on droppen.app holds much more:

- this browser's device keys (WebCrypto's non-extractable keys stop export,
  not use);
- the hub session;
- a channel to every machine of the account;
- on an admin device, the power to change the device chain.

A plugin imported there could control every other machine of the account and
add a device to it. And the code comes from a machine where an AI agent runs
with a shell: an agent talked into `plugin install` is enough to put it there.

The app's CSP allows only `'self'` scripts (`web/build/cloud-mode.ts`), and
`scripts/verify-web.mjs` holds every page to that ([web-integrity.md](../web-integrity.md)).
That does not change.

So droppen.app runs a plugin only inside a frame that holds none of this.
The page does the few things `host.*` offers on the plugin's behalf, one
checked message at a time.

## 1. The switch: show on droppen.app

The switch is per plugin and per instance, and it is **off** until someone
switches it on. It is a decision made on the machine, and neither the hub nor
droppen.app can change it.

- `PluginStatus.cloud` is the instance's decision. It takes effect only while
  the plugin is `enabled`.
- `PUT /api/plugins/{id}/cloud` with `{ cloud: boolean }` answers
  `200 PluginStatus`, or `404 plugin_not_found`. Like every `/api/plugins…`
  route it refuses a bridged (`cloud:`) caller, and the bridge refuses the path
  before it reaches the server.
- `remote-hand-server plugin cloud <id> on|off [--instance <name>]` sets it
  from the CLI, and `plugin list` shows it.
- It is kept in the instance's `state.db` next to the on/off decision, and it
  outlives an upgrade or an uninstall in the same way. The configuration's
  `[plugins] enable/disable` pins on/off only, not this switch.

Settings › Plugins gets the switch together with the frame (§5). Until
droppen.app can show a plugin, a switch that changes nothing visible would
only mislead.

## 2. The frame

droppen.app draws each plugin page and each pane in an
`<iframe sandbox="allow-scripts" allow="clipboard-write">` whose source is
`/plugin-frame.html`, a static page of the same reproducible build.

- The frame never gets `allow-same-origin`, so it is an opaque origin. It
  cannot reach my.droppen.app's localStorage, IndexedDB, cookies or keys, or
  the parent's DOM. Every frame is an opaque origin of its own, so one plugin
  cannot reach another, including a plugin from another machine.
- It gets none of `allow-popups`, `allow-top-navigation`, `allow-forms`,
  `allow-modals` or `allow-downloads`.
- The page is served with its own CSP, so it is just as closed when it is
  opened directly:
  `sandbox allow-scripts; default-src 'none'; script-src 'unsafe-inline' blob:; style-src 'unsafe-inline' blob:; img-src blob: data:; font-src blob: data:; connect-src 'none'; frame-ancestors <app origin>`.
  With `connect-src 'none'` the frame cannot fetch or open a socket. Its only
  way out is its message port. Inline scripts and `blob:` are allowed in this
  one page and nowhere else: the import map that the frame writes at run time
  counts as an inline script, and the frame runs untrusted code by design. It
  holds nothing for that code to take, and it has no network to send anything
  over.
- The app's own CSP gains `frame-src 'self'`, and nothing else.

## 3. Getting the code into the frame

- A new route, `GET /api/plugins/{id}/package`, answers with the installed
  package's files at the current build. It answers only for a plugin that is
  enabled, compatible and `cloud`, and only within the bridge's 64 MiB body
  limit. It travels the end-to-end channel like every bridged call, so the hub
  never sees it. The page keeps the files in memory, keyed by build.
- Everything the frame runs arrives over its `MessageChannel` as bytes: the
  plugin's files, and the frame's runtime. The page reads the runtime from its
  own `/assets/`, where the manifest and the service worker's table cover it.
  The runtime holds React, `react-dom` and the SDK, built by the web build at
  the versions [runtime.md](runtime.md) §3 lends.
- The frame's bootstrap is the only script in `plugin-frame.html`. It turns
  each file into a `blob:` URL. It rewrites every relative import (`./x.js`)
  into a bare specifier with `es-module-lexer`, because a `blob:` URL cannot
  resolve a relative import, and chunks may import each other in a cycle. It
  writes one import map that sends those specifiers, plus `react`,
  `react/jsx-runtime`, `react-dom` and `@droppen/plugin-sdk`, to their blobs.
  Then it imports the entry. A CSS file goes in as a `blob:` stylesheet.

## 4. `host.*` through the page

The frame's `PluginHost` sends each call over its port. The page answers for
the machine, plugin and build that it opened the frame for, which the frame
has no way to change.

| `host.*` | On droppen.app |
|---|---|
| `storage` | That machine's `/api/plugins/{id}/docs…` and `/batch`, over the bridge, and nothing else. `watch` follows that machine's `plugin-doc` frames for that plugin. |
| `ui.confirm` | The page's own dialog, which names the plugin and the machine. |
| `ui.Dialog`, `ui.Spinner` | Drawn inside the frame by the runtime, from the host's design variables. |
| `files.save` | The page offers the bytes as a download. |
| `files.open` | A file input inside the frame. |
| `clipboard` | Written by the frame itself (`allow="clipboard-write"`), which gives no read. |
| `print` | `capabilities.print` is `false`, so the plugin hides it. |
| `navigation.navigate` | The page's router, within that plugin's pages only. |
| `navigation.useLeaveGuard` | The frame reports the guard and its message, and the page asks before leaving. |
| `i18n` | The page passes its language at start and on every change. |

This makes `permissions` hold on droppen.app for the first time: whatever a
manifest declares, a frame can reach only its own documents on its own
machine.

## 5. In the page

- `activate()` runs in a frame without UI. The frame reports what it
  contributes as data: nav labels and paths, route paths, pane ids and labels,
  settings titles. Icons are components and cannot cross the port, so v1
  shows a generic plugin icon.
- A plugin page is a page of its machine. Each page and each pane is a frame
  of its own, and the page pushes the theme's variables (`--color-*`,
  `--font-*`) to it at start and on every change.
- Outside every frame, the page says which plugin it is and which machine it
  came from. A frame never covers the app's own controls.
- Aliases are not offered: they are top-level paths of a local page.
- A machine's Settings › Plugins (on its own page) and the CLI are the only
  places to switch `cloud`.

## 6. Server, feed and hub

- **The bridge** (`crates/droppen-agent/src/bridge.rs`) lets through
  `GET /api/plugins`, `GET /api/plugins/{id}/package`, a document's `GET` and
  `PUT`, `POST …/batch` and `GET …/history`. Installing, uninstalling,
  `…/enabled`, `…/cloud` and notice dismissal stay `not_from_cloud`.
- **The handlers** serve a `cloud:` caller only for a plugin that is `enabled`
  and `cloud`. For every other plugin they answer `404 plugin_not_found`, the
  same as for an unknown id. The list they give such a caller holds only those
  plugins, with `entry: null`, because the entry URL carries the instance's key.
- **The asset route** `/plugins/<key>/…` is still never bridged.
- **The feed** gives a `cloud:` subscriber that same filtered list in its
  `settings` and `plugins` frames, and `plugin-doc` frames for those plugins
  only.
- **The hub copy** still carries no plugin status or document. While a machine
  is offline, its plugins show as offline.
- **Headers.** The static host's `_headers` and the hub give
  `/plugin-frame.html` the policy in §2. `verify-web.mjs` accepts that one
  path's policy only when it has `sandbox` without `allow-same-origin`, and
  `connect-src 'none'`.

## 7. What it does not stop

- **Phishing inside the frame.** A plugin can draw a fake sign-in form or a
  fake device approval in its own frame. The marking outside the frame (§5) is
  the only defence.
- **Leaking by navigation.** CSP has no `navigate-to`, and a sandboxed frame
  may still navigate itself to a URL that carries data. What it can carry is
  its own documents on that machine, which any page of that machine can
  already read: no new reach.
- **Plugins that break.** A plugin that uses `localStorage`, `fetch`es host
  URLs or builds asset URLs from `import.meta.url` does not run in the frame.
  The SDK already forbids the first two.
- **A busy frame** can use the tab's CPU, as any page can.

## Order of work

1. The switch (§1).
2. A prototype of §2–§3 (an opaque-origin frame, `blob:` modules, an import
   map written at run time, `files.open`), checked in Chrome, Firefox and
   Safari on macOS and iOS.
3. The server (§6): the bridge, the handlers, the feed and the package route.
4. The web: the frame page and its runtime, the page's relay (§4), nav, pages
   and panes (§5), and the switch in Settings › Plugins.
5. Headers and `verify-web.mjs`. Then [runtime.md](runtime.md) §5 and §8 point
   here.
