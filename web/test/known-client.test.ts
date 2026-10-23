// The hub's known-client token — given on every sign-in,
// kept per e-mail, sent with the next login — so a hub without Turnstile does
// not hold this browser back for others' failed logins at its account.
import assert from "node:assert/strict";
import test from "node:test";
import { HubApi, knownClientKey, migrateKnownClients } from "../src/lib/hub/api.js";

function memory() {
  const items = new Map<string, string>();
  return { items, getItem: (k: string) => items.get(k) ?? null, setItem: (k: string, v: string) => void items.set(k, v) };
}

/** `localStorage` as the page has it: a listing and removals too. */
function pageMemory(entries: Record<string, string> = {}) {
  const items = new Map<string, string>(Object.entries(entries));
  return {
    items,
    getItem: (k: string) => items.get(k) ?? null,
    setItem: (k: string, v: string) => void items.set(k, v),
    removeItem: (k: string) => void items.delete(k),
    key: (i: number) => [...items.keys()][i] ?? null,
    get length() {
      return items.size;
    },
  };
}

const answer = (known: string) =>
  new Response(JSON.stringify({ account: "a-1", token: "t", expires_ms: 1, known_client: known }), { status: 200, headers: { "content-type": "application/json" } });

test("a sign-in's known-client token is kept per e-mail and sent with the next login", async () => {
  const sent: Record<string, unknown>[] = [];
  let n = 0;
  const fetchImpl = async (_url: RequestInfo | URL, init?: RequestInit) => {
    sent.push(JSON.parse(String(init?.body)));
    n += 1;
    return new Response(JSON.stringify({ account: "a-1", token: "t", expires_ms: 1, known_client: `kc1.a-1.9.tag${n}` }), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
  };
  const storage = memory();
  const api = new HubApi("https://hub.test", fetchImpl, storage);
  await api.signUp("Owner@Example.com ", "a long password");
  assert.equal(sent[0]!.known_client, undefined, "sign-up sends none");
  await api.login("owner@example.com", "a long password");
  assert.equal(sent[1]!.known_client, "kc1.a-1.9.tag1", "the token sign-up gave, for the same e-mail however written");
  await api.login("owner@example.com", "a long password");
  assert.equal(sent[2]!.known_client, "kc1.a-1.9.tag2", "each sign-in's fresh one");
  await api.login("someone@example.com", "another password");
  assert.equal(sent[3]!.known_client, undefined, "never another e-mail's");
  // Without storage it simply goes without.
  const bare = new HubApi("https://hub.test", fetchImpl, undefined);
  await bare.login("owner@example.com", "a long password");
  assert.equal(sent[4]!.known_client, undefined);
});

test("a device's look at the account keeps the fresh known-client token under its e-mail", async () => {
  const sent: Record<string, unknown>[] = [];
  const fetchImpl = async (url: RequestInfo | URL, init?: RequestInit) => {
    if (String(url).endsWith("/v1/me")) {
      return new Response(
        JSON.stringify({
          account: "a-1",
          email: { address: "owner@example.com", verified: true, verification: "off" },
          chain_seq: null,
          known_client: "kc1.a-1.9.fresh",
        }),
        { status: 200, headers: { "content-type": "application/json" } },
      );
    }
    sent.push(JSON.parse(String(init?.body)));
    return new Response(JSON.stringify({ account: "a-1", token: "t", expires_ms: 1 }), { status: 200, headers: { "content-type": "application/json" } });
  };
  const storage = memory();
  const api = new HubApi("https://hub.test", fetchImpl, storage);
  await api.me("device-session");
  await api.login("Owner@example.com", "a long password");
  assert.equal(sent[0]!.known_client, "kc1.a-1.9.fresh");
});


test("the e-mail is not in the storage: each token is kept under a labelled hash of it", async () => {
  const storage = pageMemory();
  const api = new HubApi("https://hub.test", async () => answer("kc1.a-1.9.tag"), storage);
  await api.login("Owner@Example.com", "a long password");
  const key = await knownClientKey(" owner@example.COM ");
  assert.match(key!, /^rh\.hub\.knownClientHash:[0-9a-f]{64}$/);
  assert.deepEqual([...storage.items.keys()], [key], "one key, and it is the hash's");
  assert.equal(storage.items.get(key!), "kc1.a-1.9.tag");
  for (const k of storage.items.keys()) assert.doesNotMatch(k, /owner|example/i);
  assert.notEqual(await knownClientKey("someone@example.com"), key, "another e-mail, another key");
});

test("tokens kept under their e-mail (before the hash) move to the hash as the page starts, a newer one there winning", async () => {
  const hashedNewer = (await knownClientKey("newer@example.com"))!;
  const storage = pageMemory({
    "rh.hub.knownClient:owner@example.com": "kc1.old",
    "rh.hub.knownClient:newer@example.com": "kc1.stale",
    [hashedNewer]: "kc1.fresh",
    "rh.hub.session": "{}",
  });
  assert.equal(await migrateKnownClients(storage), 2);
  assert.equal(storage.items.get((await knownClientKey("owner@example.com"))!), "kc1.old");
  assert.equal(storage.items.get(hashedNewer), "kc1.fresh", "the newer token stays");
  assert.ok(![...storage.items.keys()].some((k) => k.startsWith("rh.hub.knownClient:")), "no e-mail left in a key");
  assert.equal(storage.items.get("rh.hub.session"), "{}", "nothing else is touched");
  assert.equal(await migrateKnownClients(storage), 0, "done once");

  // The page's API does it as it is made: the next login sends the moved token.
  const page = pageMemory({ "rh.hub.knownClient:owner@example.com": "kc1.legacy" });
  const sent: Record<string, unknown>[] = [];
  const api = new HubApi("https://hub.test", async (_u, init) => (sent.push(JSON.parse(String(init?.body))), answer("kc1.next")), page);
  await api.login("owner@example.com", "a long password");
  assert.equal(sent[0]!.known_client, "kc1.legacy");
  assert.deepEqual([...page.items.keys()], [(await knownClientKey("owner@example.com"))!]);
  assert.equal([...page.items.values()][0], "kc1.next");
});

test("a storage that cannot be listed moves an e-mail's token when that e-mail is next used", async () => {
  const storage = { ...memory(), removeItem(k: string) { storage.items.delete(k); } };
  storage.items.set("rh.hub.knownClient:owner@example.com", "kc1.legacy");
  const sent: Record<string, unknown>[] = [];
  const api = new HubApi("https://hub.test", async (_u, init) => (sent.push(JSON.parse(String(init?.body))), answer("kc1.next")), storage);
  await api.login("someone@example.com", "x");
  assert.equal(sent[0]!.known_client, undefined);
  assert.ok(storage.items.has("rh.hub.knownClient:owner@example.com"), "another e-mail's stays until it is used");
  await api.login("owner@example.com", "x");
  assert.equal(sent[1]!.known_client, "kc1.legacy");
  assert.equal(storage.items.has("rh.hub.knownClient:owner@example.com"), false);
});
