// What the page makes of the instance's plugin list (`plugins/statuses.ts`,
// `plugins/aliases.ts`) and the two Settings › Plugins calls
// (`plugins/install-api.ts`), without React: which statuses run, which are
// named though they cannot run, what needs a reload, which old path redirects
// where, and what is sent to the server.
import assert from "node:assert/strict";
import test from "node:test";
import type { ApiClient } from "../src/api/client.js";
import type { paths } from "../src/api/schema.js";
import type { PluginStatus } from "../src/plugins/types.js";

const { baselineOf, changedUnderPage, runningStatuses, strandedStatuses } = await import("../src/plugins/statuses.js");
const { pluginAlias } = await import("../src/plugins/aliases.js");
const { installPlugin, uninstallPlugin } = await import("../src/plugins/install-api.js");
const { default: createClient } = await import("openapi-fetch");

/** Stands in for the instance-wide secret in an entry URL (128 bits of hex in a real one). */
const KEY = "0".repeat(32);

/** The server's `PluginStatus` for an installed plugin (`docs/plugins/runtime.md` §4). */
function status(id: string, overrides: Partial<PluginStatus> = {}): PluginStatus {
  const build = overrides.build ?? "0.1.0-972944261ff8";
  return {
    id,
    version: "0.1.0",
    name: { en: id },
    enabled: true,
    switchedOn: true,
    compatible: true,
    forced: false,
    reason: null,
    notice: null,
    notices: [],
    build,
    entry: `/plugins/${KEY}/${id}/${build}/dist/index.js`,
    aliases: [],
    permissions: ["storage"],
    ...overrides,
  };
}

test("the page runs every status with an entry, and skips one without", () => {
  const statuses = [
    status("tasks"),
    status("notes", { enabled: false, switchedOn: false, entry: null }),
    status("broken", { enabled: false, compatible: false, entry: null }),
    status("pinned"),
  ];
  assert.deepEqual(runningStatuses(statuses, baselineOf(statuses)).map((s) => s.id), ["tasks", "pinned"]);
  assert.deepEqual(runningStatuses(null, null), [], "nothing is known yet: nothing runs");
});

test("a plugin switched on that this host cannot run is named, not dropped; one switched off is not", () => {
  const behind = status("tasks", { enabled: false, compatible: false, entry: null, reason: "needs Droppen plugin SDK ^0.2.0; this host implements 0.1.0", aliases: ["/tasks"] });
  const statuses = [
    behind,
    status("notes", { enabled: false, switchedOn: false, compatible: false, entry: null }),
    status("pinned-off", { enabled: false, switchedOn: false, forced: true, compatible: false, entry: null }),
    status("fine"),
    status("off", { enabled: false, switchedOn: false, entry: null }),
  ];
  assert.deepEqual(strandedStatuses(statuses).map((s) => s.id), ["tasks"], "the one the user still wants");
  assert.deepEqual(runningStatuses(statuses, baselineOf(statuses)).map((s) => s.id), ["fine"], "and it is not loaded");
  assert.equal(pluginAlias(statuses, "/tasks"), "tasks", "its old path still leads to its page, which says why");
  assert.deepEqual(strandedStatuses([{ ...behind, switchedOn: false }]), [], "switched off: put away");
  assert.deepEqual(strandedStatuses(null), [], "nothing is known yet");
});

test("a page a hub serves reads no list and runs nothing", () => {
  const statuses = [status("tasks"), status("old", { enabled: false, compatible: false, entry: null })];
  assert.deepEqual(runningStatuses(statuses, baselineOf(statuses), true), []);
  assert.deepEqual(strandedStatuses(statuses, true), []);
  assert.equal(pluginAlias(statuses, "/tasks", true), undefined);
});

test("an install, an upgrade or an uninstall elsewhere needs a reload; a switch does not", () => {
  const before = [status("tasks"), status("notes", { enabled: false, switchedOn: false, entry: null })];
  const baseline = baselineOf(before);
  assert.equal(changedUnderPage(baseline, before), false, "the same list");

  const switched = [status("tasks", { enabled: false, switchedOn: false, entry: null }), status("notes")];
  assert.equal(changedUnderPage(baseline, switched), false, "turning plugins on and off leaves the code alone");

  const upgraded = [status("tasks", { version: "0.2.0", build: "0.2.0-bbbbbbbbbbbb" }), before[1]!];
  assert.equal(changedUnderPage(baseline, upgraded), true, "an upgrade elsewhere");

  assert.equal(changedUnderPage(baseline, [before[0]!]), true, "an uninstall elsewhere");
  assert.equal(changedUnderPage(baseline, [...before, status("extra")]), true, "an install elsewhere");
});

test("the page keeps running the build it started with, and does not start a new one", () => {
  const first = status("tasks", { build: "0.1.0-aaaaaaaaaaaa", entry: `/plugins/${KEY}/tasks/0.1.0-aaaaaaaaaaaa/dist/index.js` });
  const baseline = baselineOf([first]);
  const upgraded = status("tasks", { version: "0.2.0", build: "0.2.0-bbbbbbbbbbbb" });
  const running = runningStatuses([upgraded], baseline);
  assert.deepEqual(running.map((s) => [s.build, s.entry]), [["0.1.0-aaaaaaaaaaaa", `/plugins/${KEY}/tasks/0.1.0-aaaaaaaaaaaa/dist/index.js`]], "the old module, not the new one");

  const installedElsewhere = runningStatuses([first, status("new")], baseline);
  assert.deepEqual(installedElsewhere.map((s) => s.id), ["tasks"], "a plugin installed elsewhere waits for the reload");

  const turnedOff = runningStatuses([status("tasks", { version: "0.2.0", build: "0.2.0-bbbbbbbbbbbb", enabled: false, switchedOn: false, entry: null })], baseline);
  assert.deepEqual(turnedOff, [], "the current switch still wins");
});

test("aliases come from the statuses, and only whole top-level paths", () => {
  const statuses = [status("tasks", { aliases: ["/tasks"] }), status("notes", { aliases: ["/notes"] })];
  assert.equal(pluginAlias(statuses, "/tasks"), "tasks");
  assert.equal(pluginAlias(statuses, "/tasks/"), "tasks", "a pasted trailing slash");
  assert.equal(pluginAlias(statuses, "/tasks/extra"), undefined, "an alias is one segment");
  assert.equal(pluginAlias(statuses, "/unknown"), undefined);
  assert.equal(pluginAlias(null, "/tasks"), undefined, "nothing is known yet");
});

test("install posts the tarball's bytes to /api/plugins and answers the installed status", async () => {
  const calls: { method: string; path: string; init?: unknown }[] = [];
  const installed = status("tasks");
  const api = {
    POST: async (path: string, init: unknown) => {
      calls.push({ method: "POST", path, init });
      return { data: installed, response: new Response(null, { status: 200 }) };
    },
  } as unknown as ApiClient;
  const file = new Blob([new Uint8Array([0x1f, 0x8b, 0x08])]);
  assert.equal(await installPlugin(api, file), installed);
  assert.equal(calls.length, 1);
  assert.equal(calls[0]!.path, "/api/plugins");
  const init = calls[0]!.init as { body: Blob; bodySerializer: (body: unknown) => BodyInit; headers: Record<string, string> };
  assert.equal(init.body, file, "the bytes, not a JSON encoding of them");
  assert.equal(init.bodySerializer(file), file, "openapi-fetch's JSON default is bypassed");
  assert.deepEqual(init.headers, { "content-type": "application/gzip" });
});

test("openapi-fetch sends the tarball as bytes, with the gzip content type", async () => {
  const installed = status("tasks");
  const seen: Request[] = [];
  const client = createClient<paths>({
    baseUrl: "http://device.test",
    fetch: async (request) => {
      seen.push(request);
      return new Response(JSON.stringify(installed), { status: 200, headers: { "content-type": "application/json" } });
    },
  });
  const bytes = [0x1f, 0x8b, 0x08, 0x00];
  assert.deepEqual(await installPlugin(client, new Blob([new Uint8Array(bytes)])), installed);
  assert.equal(seen[0]!.method, "POST");
  assert.equal(seen[0]!.url, "http://device.test/api/plugins");
  assert.equal(seen[0]!.headers.get("content-type"), "application/gzip", "not the JSON default");
  assert.deepEqual([...new Uint8Array(await seen[0]!.clone().arrayBuffer())], bytes, "the tarball's bytes, untouched");
});

test("uninstall deletes /api/plugins/{id}", async () => {
  const calls: { method: string; path: string; init?: unknown }[] = [];
  const api = {
    DELETE: async (path: string, init: unknown) => {
      calls.push({ method: "DELETE", path, init });
      return { response: new Response(null, { status: 204 }) };
    },
  } as unknown as ApiClient;
  await uninstallPlugin(api, "tasks");
  assert.deepEqual(calls, [{ method: "DELETE", path: "/api/plugins/{id}", init: { params: { path: { id: "tasks" } } } }]);
});

test("a refused install says why", async () => {
  const api = {
    POST: async () => ({
      error: { error: "invalid_package", message: "package/package.json is missing" },
      response: new Response(null, { status: 400 }),
    }),
  } as unknown as ApiClient;
  await assert.rejects(installPlugin(api, new Blob(["x"])), /package\/package\.json is missing/);
});

test("a refused uninstall says why", async () => {
  const api = {
    DELETE: async () => ({ error: { error: "plugin_not_found" }, response: new Response(null, { status: 404 }) }),
  } as unknown as ApiClient;
  await assert.rejects(uninstallPlugin(api, "gone"), /plugin_not_found/);
});
