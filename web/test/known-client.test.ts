// The hub's known-client token — given on every sign-in,
// kept per e-mail, sent with the next login — so a hub without Turnstile does
// not hold this browser back for others' failed logins at its account.
import assert from "node:assert/strict";
import test from "node:test";
import { HubApi } from "../src/lib/hub/api.js";

function memory() {
  const items = new Map<string, string>();
  return { items, getItem: (k: string) => items.get(k) ?? null, setItem: (k: string, v: string) => void items.set(k, v) };
}

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

