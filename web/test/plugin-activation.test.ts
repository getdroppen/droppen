// A plugin is activated once per machine, plugin and build
// (`plugins/activation.ts`): its host's storage is the machine's
// (`httpStorage(id, machine)`), and the entry URL names one exact build —
// other bytes are other code. An activation kept by plugin alone went on
// reading and writing the first machine's documents after a switch.
import assert from "node:assert/strict";
import test from "node:test";
import type { PluginModule, PluginSource } from "../src/plugins/activation.js";

/**
 * The typed client builds a `Request` from a root-relative URL, which Node's
 * own `Request` refuses; it reads `Request` when first imported, so the stub
 * goes in before the dynamic imports below (as in plugin-host.test.ts).
 */
class StubRequest {
  readonly url: string;
  readonly method: string;
  readonly headers: Headers;
  constructor(url: string, init: RequestInit = {}) {
    this.url = url;
    this.method = init.method ?? "GET";
    this.headers = new Headers(init.headers ?? {});
  }
  clone() {
    return this;
  }
}
globalThis.Request = StubRequest as unknown as typeof Request;

const { openMachine } = await import("../src/lib/machines/registry.js");
const { activatePlugin, pluginEntryUrl } = await import("../src/plugins/activation.js");

function recording(name: string, log: string[]) {
  return {
    fetch: async (request: Request) => {
      log.push(`${name} ${request.method} ${request.url}`);
      return new Response(JSON.stringify({ error: "doc_not_found" }), { status: 404, headers: { "content-type": "application/json" } });
    },
    socketUrl: async (path: string) => path,
    openSocket: () => {
      throw new Error("no sockets here");
    },
  };
}

/** A module that activates into nothing, remembering which entry URL was fetched. */
function moduleOf(id: string, log: string[]) {
  return async (entry: string): Promise<PluginModule> => {
    log.push(`load ${entry}`);
    return {
      default: {
        manifest: { id },
        activate: () => ({}),
      } as never,
    };
  };
}

const source = (build: string): PluginSource => ({ id: "demo", build, entry: `/plugins/key/demo/${build}/dist/index.js` });

test("a plugin is activated once per machine, and each machine's host reads and writes that machine's documents", async () => {
  const log: string[] = [];
  openMachine("m-a", recording("a", log));
  openMachine("m-b", recording("b", log));
  const hosts: unknown[] = [];
  const load = async (entry: string): Promise<PluginModule> => {
    log.push(`load ${entry}`);
    return {
      default: {
        manifest: { id: "demo" },
        activate: (host: unknown) => {
          hosts.push(host);
          return {};
        },
      } as never,
    };
  };
  const navigate = () => undefined;

  const a = await activatePlugin(source("1.0.0-aaaaaaaaaaaa"), navigate, "m-a", load);
  assert.equal(await activatePlugin(source("1.0.0-aaaaaaaaaaaa"), navigate, "m-a", load), a, "the same machine and build: the same activation, whatever remounts");
  const b = await activatePlugin(source("1.0.0-aaaaaaaaaaaa"), navigate, "m-b", load);
  assert.ok(a.ok && b.ok);
  assert.equal(hosts.length, 2, "another machine: a host of its own");
  assert.notEqual(a.host, b.host);
  assert.equal(log.filter((line) => line.startsWith("load ")).length, 2);

  assert.equal(await a.host.storage.get("state"), null);
  assert.equal(await b.host.storage.get("state"), null);
  assert.deepEqual(log.filter((line) => !line.startsWith("load ")), ["a GET /api/plugins/demo/docs/state", "b GET /api/plugins/demo/docs/state"]);
});

test("another build of the same plugin is other code: it is loaded and activated again", async () => {
  const log: string[] = [];
  openMachine("m-build", recording("build", log));
  const first = await activatePlugin(source("1.0.0-aaaaaaaaaaaa"), () => undefined, "m-build", moduleOf("demo", log));
  const second = await activatePlugin(source("1.0.0-bbbbbbbbbbbb"), () => undefined, "m-build", moduleOf("demo", log));
  assert.ok(first.ok && second.ok);
  assert.notEqual(first, second);
  assert.deepEqual(log.filter((line) => line.startsWith("load ")), [
    "load /plugins/key/demo/1.0.0-aaaaaaaaaaaa/dist/index.js",
    "load /plugins/key/demo/1.0.0-bbbbbbbbbbbb/dist/index.js",
  ]);
});

test("a package whose manifest names another id is refused", async () => {
  openMachine("m-id", recording("id", []));
  const result = await activatePlugin(source("1.0.0-aaaaaaaaaaaa"), () => undefined, "m-id", moduleOf("another", []));
  assert.equal(result.ok, false);
  assert.match(result.ok ? "" : result.error, /says it is "another".*installed it as "demo"/);
});

test("a machine closed on the page takes its plugin activations with it", async () => {
  const { closeMachine } = await import("../src/lib/machines/registry.js");
  const log: string[] = [];
  openMachine("m-c", recording("c", log));
  const first = await activatePlugin(source("1.0.0-aaaaaaaaaaaa"), () => undefined, "m-c", moduleOf("demo", log));
  closeMachine("m-c");
  openMachine("m-c", recording("c", log));
  const again = await activatePlugin(source("1.0.0-aaaaaaaaaaaa"), () => undefined, "m-c", moduleOf("demo", log));
  assert.notEqual(again, first, "a fresh activation for the machine opened again");
  assert.equal(log.filter((line) => line.startsWith("load ")).length, 2);
});

test("an entry module the browser imports is activated", async () => {
  openMachine("m-live", recording("live", []));
  const body = 'export default { manifest: { id: "demo" }, activate: () => ({}) };';
  const entry = `data:text/javascript,${encodeURIComponent(body)}`;
  const load = (url: string) => import(/* @vite-ignore */ url);
  const result = await activatePlugin({ id: "demo", build: "1.0.0-aaaaaaaaaaaa", entry }, () => undefined, "m-live", load);
  assert.equal(result.ok, true, result.ok ? "" : result.error);
});

test("plugin code is imported only from this server's /plugins/", async () => {
  const origin = "http://127.0.0.1:7000";
  assert.equal(pluginEntryUrl("/plugins/k/demo/1.0.0-aaaaaaaaaaaa/dist/index.js", origin), `${origin}/plugins/k/demo/1.0.0-aaaaaaaaaaaa/dist/index.js`);
  for (const entry of [
    "data:text/javascript,export%20default%20{}",
    "blob:http://127.0.0.1:7000/0b5e",
    "https://evil.example/plugins/k/demo/b/dist/index.js",
    "//evil.example/plugins/k/demo/b/dist/index.js",
    "/assets/index.js",
    "/plugins/../api/settings",
  ]) {
    assert.throws(() => pluginEntryUrl(entry, origin), /refusing to load plugin code/, entry);
  }
  assert.throws(() => pluginEntryUrl("/plugins/k/demo/b/dist/index.js", undefined), /refusing/, "no page origin, nothing loaded");

  openMachine("m-refuse", recording("refuse", []));
  const result = await activatePlugin({ id: "demo", build: "1.0.0-bbbbbbbbbbbb", entry: "data:text/javascript,export%20default%20{}" }, () => undefined, "m-refuse");
  assert.equal(result.ok, false, "the browser's own loader refuses an entry that is not this server's");
});
