/*
 * The re-enroll warnings (review D3): per host that cannot renew its identity
 * key, the devices that could control it and no longer can, with what the
 * host's own record says of each and what a person dismissed in this browser.
 * `CloudAccount` documents the methods it hands to these.
 */
import { type ChainState, utf8 } from "@/lib/e2e";
import { reEnrollConnectedKey, reEnrollDismissedKey } from "../account-storage";
import { compareBytes, type ReEnrollLine, reEnrollCulprits, reEnrollEvidence, seqMaps } from "../chain-policy";
import type { ControllersRecord } from "../streams";

/** What the warnings read of the account they belong to, and how they tell its page. */
export interface ReEnrollDeps {
  /** The account, whose id names the notes kept in this browser. */
  account(): string;
  /** The verified chain, `null` before there is one. */
  state(): ChainState | null;
  /** Whether `host` renews its identity key instead of being re-enrolled. */
  renews(host: string): boolean;
  read(key: string): string | null;
  write(key: string, value: string): void;
  emit(): void;
}

export class ReEnrollWarnings {
  constructor(private readonly deps: ReEnrollDeps) {}

  /** The account's id, as the notes below are kept under it. */
  private get account(): string {
    return this.deps.account();
  }

  /** The account's verified chain, read each time: it moves on as the account syncs. */
  private get state(): ChainState | null {
    return this.deps.state();
  }

  reEnrollDue(controllersOf?: (host: string) => ControllersRecord | undefined): Set<string> {
    const due = new Set<string>();
    for (const [host, lines] of this.reEnrollWarnings(controllersOf)) if (lines.some((l) => !l.dismissed)) due.add(host);
    return due;
  }

  reEnrollWarnings(controllersOf?: (host: string) => ControllersRecord | undefined): Map<string, ReEnrollLine[]> {
    const chain = this.state;
    const out = new Map<string, ReEnrollLine[]>();
    if (!chain) return out;
    const culprits = reEnrollCulprits(chain);
    for (const host of [...culprits.keys()]) if (this.deps.renews(host)) culprits.delete(host);
    const records = new Map<string, ControllersRecord>();
    for (const h of chain.hosts()) {
      const record = h.status.type === "active" ? controllersOf?.(h.info.id) : undefined;
      if (record) records.set(h.info.id, record);
    }
    const connected = this.keepConnected(chain, records);
    const dismissed = this.reEnrollDismissed();
    for (const [host, devices] of culprits) {
      const lines: ReEnrollLine[] = [];
      for (const [device, lostSeq] of devices) {
        const evidence = reEnrollEvidence(chain, host, device, lostSeq, records.get(host), connected[host]?.[device]);
        const at = dismissed[host]?.[device];
        lines.push({ device, lostSeq, evidence, dismissed: at !== undefined && at >= lostSeq && evidence.kind !== "connected" });
      }
      out.set(host, lines.sort((a, b) => compareBytes(utf8(a.device), utf8(b.device))));
    }
    return out;
  }

  dismissReEnroll(host: string, device: string, lostSeq: number): void {
    const now = this.state ? reEnrollCulprits(this.state).get(host)?.get(device) : undefined;
    if (now === undefined) return;
    const dismissed = this.reEnrollDismissed();
    // Never past the loss the chain has now: a dismissal covers only what was shown.
    (dismissed[host] ??= {})[device] = Math.min(lostSeq, now);
    this.deps.write(reEnrollDismissedKey(this.account), JSON.stringify(dismissed));
    this.deps.emit();
  }

  warnAgainReEnroll(host: string, device: string): void {
    const dismissed = this.reEnrollDismissed();
    if (dismissed[host]?.[device] === undefined) return;
    delete dismissed[host]![device];
    if (Object.keys(dismissed[host]!).length === 0) delete dismissed[host];
    this.deps.write(reEnrollDismissedKey(this.account), JSON.stringify(dismissed));
    this.deps.emit();
  }

  private reEnrollDismissed(): Record<string, Record<string, number>> {
    return seqMaps(this.deps.read(reEnrollDismissedKey(this.account)));
  }

  /** Every device a current host's record ever named here, with when it first connected: `records` merged into what is kept. */
  private keepConnected(chain: ChainState, records: ReadonlyMap<string, ControllersRecord>): Record<string, Record<string, number>> {
    const kept = seqMaps(this.deps.read(reEnrollConnectedKey(this.account)));
    // A device this chain does not name is not added — and what is kept is never deleted for it: another
    // tab's newer chain may name it. A host removed from the account takes its list with it (one that
    // merely stopped running AI keeps it: it may run AI again, and an old loss must not become dismissable).
    const named = (id: string) => chain.deviceRecord(id) !== undefined;
    let changed = false;
    // A list is made only for a device it gains: a record that names nobody new writes nothing (this runs on every render).
    for (const [host, record] of records) {
      for (const d of record.devices) {
        const at = kept[host]?.[d.id];
        if (!named(d.id) || (at !== undefined && at <= d.firstMs)) continue;
        (kept[host] ??= {})[d.id] = d.firstMs;
        changed = true;
      }
    }
    for (const host of Object.keys(kept)) {
      if (Object.keys(kept[host]!).length > 0 && chain.deviceRecord(host)?.status.type !== "removed") continue;
      delete kept[host];
      changed = true;
    }
    if (changed) this.deps.write(reEnrollConnectedKey(this.account), JSON.stringify(kept));
    // What this chain cannot place is left out of what is read, not out of what is kept.
    const view: Record<string, Record<string, number>> = {};
    for (const [host, devices] of Object.entries(kept)) view[host] = Object.fromEntries(Object.entries(devices).filter(([id]) => named(id)));
    return view;
  }
}
