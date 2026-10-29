/*
 * What every collaborator of `CloudAccount` may read of it: who it is, its
 * verified chain and chain clock as they stand now, and how its page is told.
 * Nothing here signs, seals or opens, nor reaches this browser's storage: keys,
 * vaults, the session, the hub API and the notes kept in `localStorage` (the
 * hub session among them) stay with the account. A collaborator that keeps a
 * note gets it bound to that note's key, in its own dependencies.
 */
import type { ChainClock, ChainState } from "@/lib/e2e";

export interface AccountBasics {
  readonly account: string;
  readonly deviceId: string;
  /** The verified chain, `null` before there is one: read through on every use, as the account replaces it. */
  readonly state: ChainState | null;
  /** The chain clock (when this device first saw each entry): read through on every use, as the account replaces it. */
  readonly clock: ChainClock;
  /** Tell the page the account changed. */
  readonly emit: () => void;
}

/**
 * One note this browser keeps for the account in `localStorage`, bound to its
 * key: a collaborator handed it reaches that note and no other.
 */
export interface AccountNote {
  read(): string | null;
  write(value: string): void;
  forget(): void;
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
    emit: of.emit,
  });
}
