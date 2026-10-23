// Signing out stops this browser's notifications (`lib/push.ts`
// `disablePushOnSignOut`, called by `features/cloud/sign-out.ts` either way):
// the device it subscribed with drops its row, and the browser's own
// subscription goes, so the push service forgets the endpoint. A device that
// does not answer does not hold the sign-out up, and nothing here throws.
import assert from "node:assert/strict";
import test from "node:test";
import { disablePushOnSignOut, type PushEnv, readPushRecord } from "../src/lib/push.js";

function env(opts: { record?: boolean; subscribed?: boolean; deleteHangs?: boolean; deleteFails?: boolean; noWorker?: boolean } = {}) {
  const log: string[] = [];
  const values = new Map<string, string>();
  if (opts.record !== false) values.set("rh.hub.push.subscribed", JSON.stringify({ machine: "m-1", vapid: "AAAA", seq: null }));
  values.set("rh.hub.push.renew", "m-2");
  let current: { endpoint: string; unsubscribe(): Promise<boolean> } | null =
    opts.subscribed === false ? null : { endpoint: "https://push.example/e1", unsubscribe: async () => (log.push("unsubscribe"), (current = null), true) };
  const pushEnv: PushEnv = {
    ready: async () => (opts.noWorker ? null : ({ pushManager: { getSubscription: async () => current } } as never)),
    permission: () => "granted",
    storage: { getItem: (k) => values.get(k) ?? null, setItem: (k, v) => void values.set(k, v), removeItem: (k) => void values.delete(k) },
    api: (machine) => ({
      GET: async () => ({ data: { publicKey: "AAAA" } }),
      POST: async () => ({}),
      DELETE: async (_path, init) => {
        log.push(`delete ${machine} ${init.body.endpoint}`);
        if (opts.deleteFails) throw new Error("the device is offline");
        if (opts.deleteHangs) await new Promise(() => undefined);
        return {};
      },
    }),
  };
  return { env: pushEnv, log, values, current: () => current };
}

test("sign-out drops the device's row, then the browser's subscription, and forgets what it kept", async () => {
  const e = env();
  await disablePushOnSignOut(e.env);
  assert.deepEqual(e.log, ["delete m-1 https://push.example/e1", "unsubscribe"]);
  assert.equal(e.current(), null, "the browser holds no subscription");
  assert.equal(readPushRecord(e.env), null);
  assert.equal(e.values.has("rh.hub.push.renew"), false);
});

test("a device that does not answer, or fails, does not keep the browser subscribed or the sign-out waiting", async () => {
  const hanging = env({ deleteHangs: true });
  const started = Date.now();
  await disablePushOnSignOut(hanging.env, 50);
  assert.ok(Date.now() - started < 2_000, "on after the wait, not forever");
  assert.deepEqual(hanging.log, ["delete m-1 https://push.example/e1", "unsubscribe"]);
  const failing = env({ deleteFails: true });
  await disablePushOnSignOut(failing.env);
  assert.deepEqual(failing.log, ["delete m-1 https://push.example/e1", "unsubscribe"]);
});

test("with no device on record the browser's subscription goes all the same; with none, or no worker, nothing fails", async () => {
  const orphan = env({ record: false });
  await disablePushOnSignOut(orphan.env);
  assert.deepEqual(orphan.log, ["unsubscribe"], "no device to ask: the subscription itself goes");
  const none = env({ subscribed: false });
  await disablePushOnSignOut(none.env);
  assert.deepEqual(none.log, []);
  assert.equal(readPushRecord(none.env), null, "the record goes even so");
  const noWorker = env({ noWorker: true });
  await disablePushOnSignOut(noWorker.env);
  assert.equal(readPushRecord(noWorker.env), null);
  // A worker that throws: said, never thrown.
  await disablePushOnSignOut({ ...orphan.env, ready: async () => Promise.reject(new Error("no worker")) });
});
