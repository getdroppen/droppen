// A machine added from another page (droppen.app/link, the popup a
// computer's local page opens) has its chain entry appended before its first
// key is put in the keyring. A page that follows the chain reads the machine's
// keys as the entry arrives — too early — and its offline copy
// (`HubCopySource.following`) waits for the account to change again. Settling
// after the chain moves refreshes the keyring and tells listeners, so they read
// again and find the key: the new device's sessions show without a reload.
import assert from "node:assert/strict";
import test from "node:test";
import { createV2Genesis, FakeCourier, FakeKeysHub, pairBrowser } from "./fake-keys-hub.js";

test("a machine whose key is put after its chain entry: the page following the chain hears of it once it settles, and reads the key then", async () => {
  const hub = new FakeKeysHub();
  const a = await hub.device("tok");
  await createV2Genesis(a, true);
  const b = await pairBrowser(hub, a, { rawComplete: true });
  await b.sync();
  await b.settle();
  // Another page (a) adds the machine: the chain first…
  const m = await hub.enroll(a);
  await b.sync();
  assert.equal((await b.machineKeys(m.id)).size, 0, "read as the entry arrived: no key yet");
  // …then its first key, into the keyring.
  await a.handOverMachineKey(m.id, new FakeCourier());
  let heard = 0;
  const off = b.onChange(() => void heard++);
  try {
    await b.settle();
  } finally {
    off();
  }
  assert.ok(heard >= 1, "listeners are told once it has settled");
  assert.deepEqual([...(await b.machineKeys(m.id)).keys()], [0], "and the key is there to read");
});
