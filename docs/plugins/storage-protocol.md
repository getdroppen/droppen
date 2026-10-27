# Plugin document store — HTTP protocol (SDK 0.1)

The one server-side extension point of SDK 0.1. Every plugin gets a private
document store per instance; the host implements it, no plugin ships server
code. Licensed with the SDK (MIT OR Apache-2.0). Wire types live in
`server-rs/crates/droppen-plugin-sdk` and `packages/plugin-sdk`; this page is
what they describe.

A plugin is installed into an instance at runtime (`docs/plugins/runtime.md`):
the web build ships no plugin code, and the server serves each instance's
installed packages to the pages it serves itself. All routes below are behind
the instance's normal authentication. JSON is camelCase. Errors are
`{ "error": <code>, "message"?: string, ... }`.

## Plugins

| Method and path | Body | Answer |
|---|---|---|
| `GET /api/plugins` | — | `200 { plugins: PluginStatus[] }` — every plugin **installed** in this instance |
| `POST /api/plugins` | the package's bytes: a gzip'd tar (`application/gzip` or `application/octet-stream`), ≤ 32 MiB | `200 PluginStatus`; `400 invalid_package`; `413 too_large` |
| `DELETE /api/plugins/{id}` | — | `204`; `404 plugin_not_found` |
| `PUT /api/plugins/{id}/enabled` | `{ enabled: boolean }` | `200 PluginStatus`; `404 plugin_not_found`; `409 forced_by_config` |

```ts
interface PluginStatus {
  id: string;            // ^[a-z][a-z0-9-]{1,31}$
  version: string;       // the installed package's version
  name: LocalizedText;   // string | { en, "zh-CN"?, "zh-TW"?, ja? }
  enabled: boolean;      // on for this instance AND compatible
  switchedOn: boolean;   // on by the configuration or the instance, compatible or not
  compatible: boolean;   // the host satisfies engines.droppen
  forced: boolean;       // the configuration pins it; the switch is read-only
  reason: string | null; // why it is off / incompatible / pinned
  notice: string | null; // every notice's text, joined
  notices: PluginNotice[]; // one by one, oldest first
  build: string;         // "<version>-<first 12 hex of the tarball's sha256>"
  entry: string | null;  // "/plugins/<key>/<id>/<build>/<entry>" when enabled and
                         // compatible; null otherwise
  aliases: string[];     // old top-level paths the host redirects to /p/<id>/
  permissions: string[]; // storage | sessions:read | notify — declared, not enforced
}

interface PluginNotice {
  id: string;            // stable while the notice exists
  kind: string;          // storeReset
  text: string;
  at: string;
  dismissible: boolean;  // a store reset is not: it goes when its files are cleared
}
```

`POST /api/plugins` unpacks and checks the package (every refusal is in
`docs/plugins/runtime.md` §1) into
`<data>/instances/<instance>/plugin-packages/<id>/`, replacing an installed
plugin of the same id atomically, and switches it on. `DELETE` removes the
package directory only: its documents and its on/off decision stay for a later
install. The `entry` URL is a capability — the `<key>` is an instance secret —
so it is only ever handed to a signed-in caller of the instance's own pages,
never to a request bridged from droppen.app (`404 plugin_not_found` there).

Notices are announced to open clients with a `plugins` frame. The shell lists
them above the plugin's page and flags them in its panes. Today the only one
is `storeReset`, computed from the `store.corrupt-<time>` files beside a store
that had to be set aside; it does not stop the plugin working, cannot be
dismissed, and goes away when those files are cleared.

`GET /api/settings` and the global feed's `settings` frame carry the same list
as `plugins`. After an install, an uninstall or a switch, the feed sends
`{ "type": "plugins", "plugins": PluginStatus[] }`. None of this reaches a
bridged caller (`cloud:<device>`): its `settings` (route or frame) carries
`plugins: []`, it is sent no `plugins` or `plugin-doc` frame, and the hub's
offline copy — made from the agent's own bridged `/api/settings` — has no
plugin status either (`runtime.md` §5).

Who may install, uninstall or switch: any signed-in user of the instance
(there are no roles). The configuration wins over the switch: `[plugins]
enable = "a,b"` / `disable = "c"`, or `APP_PLUGINS__ENABLE` /
`APP_PLUGINS__DISABLE`; `disable` beats `enable`. Without either, the
instance's own decision (kept in its `state.db`; an install writes `true`),
else off.

## Serving the code

```
GET /plugins/<key>/<id>/<build>/<path inside the package>
```

`<key>` is the instance-wide capability from `PluginStatus.entry`; the build id
is in the URL so a reinstall of the same version with other bytes is a new URL.
Only an installed plugin that is enabled and compatible answers, only at its
current build, only for a regular file inside its package, and only `GET` and
`HEAD`; everything else is `404`. The media type comes from the extension
(`text/javascript` for `.js`/`.mjs`), and the answer carries
`x-content-type-options: nosniff`,
`cache-control: private, max-age=31536000, immutable` and
`cross-origin-resource-policy: same-origin`. `docs/plugins/runtime.md` §2 is
the contract.

## Documents

Every route below answers `404 { error: "plugin_disabled" }` when the plugin is
installed but not enabled (or incompatible), and `404 { error:
"plugin_not_found" }` when it is not installed at all. Nothing is created on
disk for a plugin that is not enabled.

Document names: 1-200 characters of `A-Z a-z 0-9 . _ -` in `/`-separated
segments, none empty, `.` or `..` (`state`, `events/2026-10`). In a URL the
name is one path segment, so `/` is sent as `%2F`.

| Method and path | Body | Answer |
|---|---|---|
| `GET /api/plugins/{id}/docs?prefix=` | — | `200 { docs: DocMeta[] }`, by name |
| `GET /api/plugins/{id}/docs/{name}` | — | `200 Doc`; `404 doc_not_found` |
| `PUT /api/plugins/{id}/docs/{name}` | `{ expectedRevision: number \| null, value }` | `200 Doc`; `409 Conflict`; `413 too_large` |
| `POST /api/plugins/{id}/batch` | `{ writes: [{ name, expectedRevision, value }] }` | `200 { docs: Doc[] }`; `409 Conflict`; `413 too_large` |
| `GET /api/plugins/{id}/docs/{name}/history?offset=&limit=` | — | `200 { versions: DocVersion[], more: boolean }`, newest first, one page |

```ts
interface Doc<T> { name: string; revision: number; updatedAt: string; value: T }
interface DocMeta { name: string; revision: number; updatedAt: string }
interface Conflict<T> { error: "conflict"; message: string; doc: string; current: Doc<T> | null }
interface DocVersion<T> { revision: number; updatedAt: string; updatedBy: string | null; value: T }
```

- **Compare-and-set.** `expectedRevision: null` means "must not exist yet";
  a number means "must be exactly this revision". A successful write sets the
  revision to `expectedRevision + 1` (1 for a new document). Anything else is
  `409` with the document as it is now (`null` when it does not exist).
- **Batch.** One SQLite transaction: every write is checked first; if any one
  conflicts, nothing is written and the `409` names the first. A name may
  appear once per batch.
- **Limits.** One document's value ≤ 4 MiB of JSON; one request ≤ 8 MiB; one
  plugin's whole store ≤ 256 MiB — live documents **and** their history,
  names included. History gives way first: when a write would go over, the
  oldest past versions (of any document) are forgotten; only live documents
  that alone exceed the limit refuse a write. Over a limit: `413 { error:
  "too_large", message, limit }`.
- **History.** Every write keeps the value it replaced; per document the newest
  50 versions are kept, and at most 16 MiB of them. It is a safety net for a
  plugin's own bugs, not an API to build features on. A page is `limit`
  versions (10 by default, at most 50) after skipping `offset`, cut short once
  their values pass 8 MiB (the first is always there); `more` says whether
  older ones remain.
- **Feed.** After every committed write, the global feed (`/api/events`) sends
  one frame per written document: `{ "type": "plugin-doc", "plugin": <id>,
  "name": <doc>, "revision": <n> }`. That is what `host.storage.watch` listens
  to. After a reconnect the client re-reads what it watches.
- **No deletes** in 0.1. A plugin that needs one writes a tombstone value.

## On disk

```
<data>/instances/<instance>/plugin-packages/<id>/   the unpacked package, package.json at the top
<data>/instances/<instance>/plugins/<id>/store.db   its documents
```

An install unpacks into a temporary directory beside `<id>/`, checks it, and
swaps it in; `.droppen-install.json` in the package directory records its id,
version, build, tarball sha256 and entry, and is what `GET /api/plugins` and
the asset route read beside `package.json`.

The store is one SQLite file per plugin per instance (WAL, `synchronous =
FULL`, `0600`), created the first time an enabled plugin's store is used:

```sql
docs(name TEXT PRIMARY KEY, revision INTEGER, value TEXT, updated_at TEXT, updated_by TEXT)
doc_history(name TEXT, revision INTEGER, value TEXT, updated_at TEXT, updated_by TEXT)
meta(key TEXT PRIMARY KEY, value TEXT)
```

The host never interprets a value. Uninstalling a plugin keeps its directory
and its switch; disabling it keeps its directory.
`remote-hand-server plugin purge <id> --yes` moves it aside to
`plugins/<id>.purged-<time>/` rather than deleting it.

## CLI

```
remote-hand-server plugin list                     [--instance <name>]
remote-hand-server plugin install <file.tgz>       [--instance <name>]
remote-hand-server plugin uninstall <id>           [--instance <name>]
remote-hand-server plugin enable  <id>             [--instance <name>]
remote-hand-server plugin disable <id>             [--instance <name>]
remote-hand-server plugin export  <id> [--out <file>]
remote-hand-server plugin import  <id> <file> [--replace]
remote-hand-server plugin verify  <id>
remote-hand-server plugin purge   <id> --yes
```

Safe while the server runs (SQLite arbitrates between processes; a package
swap is a rename). `list` shows each plugin's build. `export` writes
`{ format: "droppen.plugin-store.v1", plugin, exportedAt, docs: Doc[] }`;
`import` reads the same.
