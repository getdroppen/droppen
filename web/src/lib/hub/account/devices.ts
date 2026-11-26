/*
 * A device's place in the account: the entry that adds it, whether it is a
 * limited device or ever ran AI, where this device stands and why the hub
 * would refuse it a session, and its keys as people compare them.
 */
import { type Action, base32, type Bytes, type ChainState, concat, type DeviceRecord, fingerprint } from "@/lib/e2e";
import { HubApiError, hubErrorKind } from "../api";
import { hostAdminRefused, notUpgraded } from "./errors";

/**
 * Whether device `record` has ever run AI (D1 extended), as the chain's
 * verifier judges `setAdmin true`: it was a host at some point (`ranAi`), or
 * the chain committed a machine key for it. Such a device never manages the
 * account — the one notion `setAdmin` and the devices page both use.
 */
export function hasRunAi(chain: Pick<ChainState, "machineKeyEpochs">, record: DeviceRecord): boolean {
  return record.ranAi || chain.machineKeyEpochs(record.info.id) > 0;
}

/** A device's keys as an entry names them. */
export interface JoinKeys {
  id: string;
  signKey: Bytes;
  dhKey: Bytes;
}

/**
 * The entry that adds a device to `chain` (docs/local-aggregator.md §3.5,
 * review D4): a v3 `addDevice3` with its two flags once the chain holds a v3
 * entry, else what chain v2 says the same with — `addMachine` for a host,
 * `addDevice{full}` for a device that manages the account — and nothing v2
 * cannot say: never a v2 device that is not full (an old agent would let it
 * reach the admin routes). Never a host that manages the account, in either
 * version (review D1).
 */
export function joinAction(device: JoinKeys, opts: { host: boolean; admin: boolean }, chain: Pick<ChainState, "isV3">): Action {
  const keys = { id: device.id, signKey: device.signKey, dhKey: device.dhKey };
  if (opts.host && opts.admin) throw hostAdminRefused();
  if (chain.isV3) return { type: "addDevice3", device: { ...keys, flags: { host: opts.host, admin: opts.admin } } };
  if (opts.host) return { type: "addMachine", machine: keys };
  if (!opts.admin) throw notUpgraded();
  return { type: "addDevice", device: { ...keys, full: true } };
}

/** Why "make it a full device" cannot be pressed on a limited device's row (`null`: it can). */
export type PromoteBlock =
  /** This device's own row: another full device makes it one. */
  | "self"
  /** The device looking is a limited device itself: a read-only list. */
  | "notAdmin"
  /** The chain holds no v3 entry yet: upgrade the account first (review D4). */
  | "notUpgraded";

/**
 * Whether `device` is a limited device as `viewer` sees it in 「我的设备」:
 * a current browser or phone of the account that is not a full device
 * (chain v3's admin) — an older pairing, or a passkey's join, which the chain
 * adds as one (`RULES.passkeyPlain`). Every browser or phone is meant to be a
 * full device; a full device makes a limited one so (`setAdmin true`), never
 * its own row, and only on an account upgraded to chain v3 (`upgraded`: the
 * chain holds a v3 entry — review D4). A device that runs AI, or ever did
 * (D1 extended: {@link hasRunAi}), is no limited device: it never manages the
 * account (review D1). `null`: not a limited device.
 */
export function limitedDevice(input: {
  viewer: string;
  viewerAdmin: boolean;
  device: DeviceRecord;
  upgraded: boolean;
  /** It ran AI before ({@link hasRunAi}: the chain's own notion; `device.ranAi` when not given). */
  ranAi?: boolean;
}): { block: PromoteBlock | null } | null {
  const { viewer, viewerAdmin, device, upgraded } = input;
  if (device.status.type !== "active" || device.admin || device.host || (input.ranAi ?? device.ranAi)) return null;
  if (device.info.id === viewer) return { block: "self" };
  if (!viewerAdmin) return { block: "notAdmin" };
  if (!upgraded) return { block: "notUpgraded" };
  return { block: null };
}

/** Where this device stands in the account's chain. */
export type Standing = "active" | "pending" | "removed" | "absent";

/** Whether this device may take itself out of the account as it signs out for good, and if not, why (`CloudAccount.selfRemoval`). */
export type SelfRemoval = "removable" | "notActive" | "notAdmin" | "lastAdmin" | "keepsSignIn" | "heldBack";

/**
 * Why the hub will not give this device a session: it says the device was
 * `removed`, or it `denied` it — or the account itself is gone: `deleted`
 * (`410 account_deleted` to a session renewal, or `error account_deleted` on
 * the device socket), whose keys open nothing any more and are wiped.
 */
export type Refusal = "removed" | "denied" | "deleted";

/** A device's keys as a person compares them on two screens: base32 of their fingerprint, in groups of four. */
export async function deviceFingerprint(signKey: Uint8Array, dhKey: Uint8Array): Promise<string> {
  const fp = await fingerprint(concat(signKey, dhKey));
  return (base32(fp.slice(0, 10)).match(/.{1,4}/g) ?? []).join("-");
}

export function refusalOf(error: unknown): Refusal | null {
  if (!(error instanceof HubApiError)) return null;
  if (hubErrorKind(error) === "accountDeleted") return "deleted";
  if (error.status === 403 && error.code === "device_removed") return "removed";
  if (error.status === 401) return "denied";
  return null;
}
