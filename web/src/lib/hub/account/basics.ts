/*
 * What every collaborator of `CloudAccount` may read of it: who it is, its
 * verified chain and chain clock as they stand now, the notes this browser
 * keeps for it, and how its page is told. Nothing here signs, seals or opens:
 * keys, vaults, the session and the hub API stay with the account.
 */
import type { ChainClock, ChainState } from "@/lib/e2e";

export interface AccountBasics {
  readonly account: string;
  readonly deviceId: string;
  /** The verified chain, `null` before there is one: read through on every use, as the account replaces it. */
  readonly state: ChainState | null;
  /** The chain clock (when this device first saw each entry): read through on every use, as the account replaces it. */
  readonly clock: ChainClock;
  /** This browser's note `key` for the account (`localStorage`; never throws). */
  readonly read: (key: string) => string | null;
  readonly write: (key: string, value: string) => void;
  /** Tell the page the account changed. */
  readonly emit: () => void;
}

/**
 * The basics over the account's own readers. Each value is read through when
 * used, never copied: the account sets its id and device from its constructor
 * and replaces its chain and clock as it syncs.
 */
export function accountBasics(of: {
  account: () => string;
  deviceId: () => string;
  state: () => ChainState | null;
  clock: () => ChainClock;
  read: (key: string) => string | null;
  write: (key: string, value: string) => void;
  emit: () => void;
}): AccountBasics {
  return Object.freeze({
    get account() {
      return of.account();
    },
    get deviceId() {
      return of.deviceId();
    },
    get state() {
      return of.state();
    },
    get clock() {
      return of.clock();
    },
    read: of.read,
    write: of.write,
    emit: of.emit,
  });
}
