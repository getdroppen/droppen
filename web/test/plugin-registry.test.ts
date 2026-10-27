// What the page makes of the instance's plugin list (`plugins/statuses.ts`,
// `plugins/aliases.ts`) and the two Settings › Plugins calls
// (`plugins/install-api.ts`), without React: which statuses run, what needs a
// reload, which old path redirects where, and what is sent to the server.
import assert from "node:assert/strict";
import test from "node:test";
import type { ApiClient } from "../src/api/client.js";
import type { PluginStatus } from "../src/plugins/types.js";

const { baselineOf, changedUnderPage, runningStatuses } = await import("../src/plugins/statuses.js");
const { pluginAlias } = await import("../src/plugins/aliases.js");
const { installPlugin, uninstallPlugin } = await import("../src/plugins/install-api.js");

function status(id: string, overrides: Partial<PluginStatus> = {}): PluginStatus {
  const build = overrides.build ?? "1.0.0-0123456789ab";
  return {
    id,
    version: "1.0.0",
    name: { en: id },
    enabled: true,
    compatible: true,
    forced: false,
    reason: null,
    notice: null,
    build,
    entry: `/plugins/key/${id}/${build}/dist/index.js`,
    aliases: [],
    permissions: ["storage"],
    ...overrides,
  };
}

test("the page runs every status with an entry, and skips one without", () => {
  const statuses = [
    status("tasks"),
    status("notes", { enabled: false, entry: null }),
    status("broken", { compatible: false, entry: null }),
    status("pinned"),
  ];
  assert.deepEqual(runningStatuses(statuses, baselineOf(statuses)).map((s) => s.id), ["tasks", "pinned"]);
  assert.deepEqual(runningStatuses(null, null), [], "nothing is known yet: nothing runs");
});

test("a page a hub serves reads no list and runs nothing", () => {
  const statuses = [status("tasks")];
  assert.deepEqual(runningStatuses(statuses, baselineOf(statuses), true), []);
  assert.equal(pluginAlias(statuses, "/tasks", true), undefined);
});

test("an install, an upgrade or an uninstall elsewhere needs a reload; a switch does not", () => {
  const before = [status("tasks"), status("notes", { enabled: false, entry: null })];
  const baseline = baselineOf(before);
  assert.equal(changedUnderPage(baseline, before), false, "the same list");

  const switched = [status("tasks", { enabled: false, entry: null }), status("notes")];
  assert.equal(changedUnderPage(baseline, switched), false, "turning plugins on and off leaves the code alone");

  const upgraded = [status("tasks", { build: "2.0.0-bbbbbbbbbbbb" }), before[1]!];
  assert.equal(changedUnderPage(baseline, upgraded), true, "an upgrade elsewhere");

  assert.equal(changedUnderPage(baseline, [before[0]!]), true, "an uninstall elsewhere");
  assert.equal(changedUnderPage(baseline, [...before, status("extra")]), true, "an install elsewhere");
});

test("the page keeps running the build it started with, and does not start a new one", () => {
  const first = status("tasks", { build: "1.0.0-aaaaaaaaaaaa", entry: "/plugins/key/tasks/1.0.0-aaaaaaaaaaaa/dist/index.js" });
  const baseline = baselineOf([first]);
  const upgraded = status("tasks", { build: "2.0.0-bbbbbbbbbbbb" });
  const running = runningStatuses([upgraded], baseline);
  assert.deepEqual(running.map((s) => [s.build, s.entry]), [["1.0.0-aaaaaaaaaaaa", "/plugins/key/tasks/1.0.0-aaaaaaaaaaaa/dist/index.js"]], "the old module, not the new one");

  const installedElsewhere = runningStatuses([first, status("new")], baseline);
  assert.deepEqual(installedElsewhere.map((s) => s.id), ["tasks"], "a plugin installed elsewhere waits for the reload");

  const turnedOff = runningStatuses([status("tasks", { build: "2.0.0-bbbbbbbbbbbb", enabled: false, entry: null })], baseline);
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
  const init = calls[0]!.init as { body: Blob; bodySerializer: (body: unknown) => BodyInit; params: { header: Record<string, string> } };
  assert.equal(init.body, file, "the bytes, not a JSON encoding of them");
  assert.equal(init.bodySerializer(file), file, "openapi-fetch's JSON default is bypassed");
  assert.deepEqual(init.params.header, { "content-type": "application/gzip" });
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
