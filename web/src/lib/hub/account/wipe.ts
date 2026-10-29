/*
 * Forgetting everything this browser keeps for an account, every step tried
 * whatever the others did (`CloudAccount.wipe`, `CloudAccount.wipeLocal`).
 */
import { forgetDeviceKeys, type KeyStore } from "@/lib/e2e";
import {
  type AccountStorage,
  ackKey,
  afreshKey,
  deviceKey,
  deviceSignKey,
  forgetIn,
  hostKeyRequestKey,
  joinedKey,
  keptDisputesKey,
  keyRequestKey,
  localAssuredKey,
  lostDeviceKey,
  lostWhyKey,
  reEnrollConnectedKey,
  reEnrollDismissedKey,
} from "../account-storage";
import { forgetPendingReencrypts } from "../deliver";
import { forgetLocalWrapKey, type HostKeyVault, LEGACY_MACHINE_KEY_PREFIX, legacyMachineKeyName, type RenewalStash, type UserKeyVault } from "../keyring";

/** What a wipe could not forget, each step on its own (`CloudAccount.wipe`, `CloudAccount.wipeLocal`). */
export class WipeError extends Error {
  constructor(readonly failures: string[]) {
    super(failures.join("; "));
    this.name = "WipeError";
  }
}

/** The failures a wipe's error names: each of a {@link WipeError}'s, or the error itself. */
export function wipeFailures(error: unknown): string[] {
  if (error instanceof WipeError) return error.failures;
  return [error instanceof Error ? error.message : String(error)];
}

/**
 * Everything a browser keeps for `account` (as device `deviceId`, when it
 * noted one), forgotten one step at a time: every step is tried whatever an
 * earlier one did, and what failed is thrown at the end ({@link WipeError}).
 */
export async function forgetBrowser(input: {
  store: KeyStore;
  storage: AccountStorage;
  account: string;
  deviceId: string | null;
  vaults: [UserKeyVault, HostKeyVault, RenewalStash, { wipe(): Promise<void> }];
  legacyHosts: string[];
  forgetSeen: () => Promise<void>;
}): Promise<void> {
  const { store, storage, account, deviceId } = input;
  const [vault, hostKeys, renewalStash, keyring] = input.vaults;
  const failures: string[] = [];
  const step = async (what: string, work: () => Promise<unknown> | unknown) => {
    try {
      await work();
    } catch (error) {
      failures.push(`${what}: ${error instanceof Error ? error.message : String(error)}`.slice(0, 200));
    }
  };
  // The keys that sign as this device and open what it was sealed, first.
  await step("device keys", () => forgetDeviceKeys(store));
  await step("local wrap key", () => forgetLocalWrapKey(store));
  await step("user keys", () => vault.wipe());
  await step("host keys", () => hostKeys.wipe());
  await step("renewals", () => renewalStash.wipe());
  await step("keyring copy", () => keyring.wipe());
  await step("machine keys", async () => {
    // M1/M2a's raw machine keys: every one the store lists, whether or not a
    // chain names its host (a wipe without one too) — they are not kept per
    // account, but nor are the device keys and the local wrap key that go
    // above — and, in a store that cannot list, those of the hosts named.
    const names = new Set((await store.list?.(LEGACY_MACHINE_KEY_PREFIX)) ?? []);
    for (const host of input.legacyHosts) {
      for (let epoch = 0; epoch < 64; epoch++) {
        const name = legacyMachineKeyName(host, epoch);
        if ((await store.get(name)) !== undefined) names.add(name);
      }
    }
    for (const name of names) await store.delete(name);
  });
  await step("pending re-encryptions", () => forgetPendingReencrypts(account));
  // The browser's notes (forgetting one never throws).
  const notes = [deviceKey(account), deviceSignKey(account), lostDeviceKey(account), lostWhyKey(account), afreshKey(account)];
  if (deviceId) notes.push(hostKeyRequestKey(account, deviceId), keyRequestKey(account, deviceId), joinedKey(account, deviceId), ackKey(account, deviceId));
  notes.push(reEnrollDismissedKey(account), reEnrollConnectedKey(account), keptDisputesKey(account), localAssuredKey(account));
  for (const key of notes) forgetIn(storage, key);
  await step("seen record", () => input.forgetSeen());
  if (failures.length) throw new WipeError(failures);
}
