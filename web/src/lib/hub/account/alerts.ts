/*
 * The device-change alerts: every change another device made to who is in
 * the account, told to the person until they say it was them (an alert that
 * went missing would hide a hostile device). `CloudAccount` documents the
 * methods it hands to these, and keeps the acknowledgement itself, as
 * pinning the account writes where alerts start.
 */
import { ASSURANCE_ON_DEVICE, PAUSE_COMPLETED, PAUSE_START } from "@/lib/e2e";
import { admits } from "../chain-policy";
import type { AckRecord } from "../seen-store";
import type { AccountBasics } from "./basics";
import type { Standing } from "./devices";
import type { ChainAlert } from "./views";

/** What the alerts read of the account beyond its basics. */
export interface ChainAlertsDeps {
  /** Whether this browser trusts the account's genesis (`CloudAccount.pinned`). */
  pinned(): boolean;
  /** Where this device stands in the chain. */
  standing(): Standing;
  /** What this device acknowledged, as kept, or as this page keeps it when the store would not (`null`: nothing kept). */
  readAck(): AckRecord | null;
  writeAck(ack: AckRecord): void;
}

export class ChainAlerts {
  constructor(
    private readonly basics: AccountBasics,
    private readonly deps: ChainAlertsDeps,
  ) {}

  alerts(): ChainAlert[] {
    if (!this.deps.pinned() || this.deps.standing() !== "active") return [];
    const ack = this.ack();
    const extra = new Set(ack.extra);
    return this.alertCandidates(ack.through)
      .filter((c) => !c.pending && !extra.has(c.alert.seq))
      .map((c) => c.alert);
  }

  acknowledge(seq: number): void {
    const ack = this.ack();
    const extra = new Set([...ack.extra, seq]);
    const open = this.alertCandidates(ack.through).filter((c) => c.pending || !extra.has(c.alert.seq));
    const through = open.length ? open[0]!.alert.seq - 1 : (this.basics.state?.seq ?? ack.through);
    this.deps.writeAck({ through: Math.max(ack.through, through), extra: [...extra].filter((s) => s > Math.max(ack.through, through)).sort((a, b) => a - b) });
    this.basics.emit();
  }

  /**
   * What this device acknowledged; with nothing kept (the store would not
   * keep where alerts start), from where it came into the account: the entry
   * that let it in, or the genesis it made. Never from the head, which would
   * take every alert away, the ones still to come too.
   */
  private ack(): AckRecord {
    const kept = this.deps.readAck();
    if (kept) return kept;
    const chain = this.basics.state;
    if (!chain) return { through: 0, extra: [] };
    for (let seq = chain.seq; seq >= 1; seq--) {
      const action = chain.entryAt(seq)?.draft.action;
      const joins = action?.type === "addDevice" || action?.type === "addDevice3" || action?.type === "recoveryJoin" || action?.type === "recoveryJoin3";
      if (joins && action.device.id === this.basics.deviceId) return { through: seq - 1, extra: [] };
    }
    return { through: 0, extra: [] };
  }

  /** Device-changing entries since `through` this device did not sign or act on: `{entry, pending}` (a pending recovery join waits in its own banner). */
  private alertCandidates(through: number): { alert: ChainAlert; pending: boolean }[] {
    const chain = this.basics.state;
    if (!chain) return [];
    const me = this.basics.deviceId;
    const out: { alert: ChainAlert; pending: boolean }[] = [];
    const actedOnByMe = (id: string, after: number) => {
      for (let s = after + 1; s <= chain.seq; s++) {
        const e = chain.entryAt(s);
        const a = e?.draft.action;
        if (e?.draft.signer.type === "device" && e.draft.signer.id === me && (a?.type === "approveDevice" || a?.type === "removeDevice" || a?.type === "removeDevice3") && a.id === id) return true;
      }
      return false;
    };
    for (let seq = Math.max(through + 1, 1); seq <= chain.seq; seq++) {
      const entry = chain.entryAt(seq);
      if (!entry) continue;
      const { action, signer } = entry.draft;
      if (signer.type === "device" && signer.id === me) continue;
      // A passkey slot (chain v3) is named by its slot id.
      const by = signer.type === "device" ? signer.id : signer.type === "passkey" ? signer.slot : null;
      const atMs = this.basics.clock.firstSeen.get(seq) ?? entry.draft.createdMs;
      const removable = (id: string | null) => (id && id !== me && chain.deviceRecord(id) && chain.deviceRecord(id)!.status.type !== "removed" ? id : null);
      switch (action.type) {
        case "addDevice":
        case "recoveryJoin":
        case "addDevice3":
        case "recoveryJoin3": {
          const subject = action.device.id;
          if (subject === me || actedOnByMe(subject, seq)) continue;
          const recovery = action.type === "recoveryJoin" || action.type === "recoveryJoin3";
          const pending = recovery && chain.deviceRecord(subject)?.status.type === "pending";
          // Chain v3 says what the device may do (§3.2: 「可运行 AI」「可管理账号」 in words).
          const flags = action.type === "addDevice3" || action.type === "recoveryJoin3" ? { flags: { ...action.device.flags } } : {};
          if (action.type === "addDevice3" && signer.type === "passkey") {
            // A passkey slot's join waits to be approved (or vetoed: any admin may remove it).
            out.push({ alert: { seq, kind: "passkey", passkey: "join", by, subject, atMs, remove: removable(subject), ...flags }, pending: false });
            break;
          }
          out.push({ alert: { seq, kind: recovery ? "recoveryJoin" : "addDevice", by, subject, atMs, remove: removable(subject), ...flags }, pending });
          break;
        }
        case "setAdmin":
          out.push({ alert: { seq, kind: "setAdmin", by, subject: action.id, on: action.admin, atMs, remove: action.admin ? removable(action.id) : null }, pending: false });
          break;
        case "setHost":
          out.push({ alert: { seq, kind: "setHost", by, subject: action.id, on: action.host, atMs, remove: null }, pending: false });
          break;
        case "setAccept":
          out.push({ alert: { seq, kind: "setAccept", by, subject: action.host, accept: action.from, atMs, remove: null }, pending: false });
          break;
        case "setRenewalPolicy":
          // The account's renewal policy (host identity rotation §3.4 rule 9): a note.
          out.push({ alert: { seq, kind: "renewalPolicy", by, subject: null, on: action.alwaysAssured, atMs, remove: null }, pending: false });
          break;
        case "renewalPause": {
          // Control paused over hosts while identity keys are renewed (§3.4 rule 10): a note. An ending marker names no
          // removals; its start did. "Resumes" names only the control that comes back: never an edge the base has since
          // dropped (a hand setAccept, a removal, a host that stopped hosting) or another open pause still holds.
          const phase = action.phase === PAUSE_START ? "start" : action.phase === PAUSE_COMPLETED ? "completed" : "cancelled";
          const removals = (action.phase === PAUSE_START ? action.removals : (chain.pause(action.op)?.removals ?? []))
            .map((r) => ({ host: r.host, ids: action.phase === PAUSE_COMPLETED ? r.ids.filter((id) => admits(chain, r.host, id)) : [...r.ids] }))
            .filter((r) => r.ids.length > 0);
          out.push({ alert: { seq, kind: "renewalPause", by, subject: null, pause: { phase, removals }, atMs, remove: null }, pending: false });
          break;
        }
        case "rotateHostKey": {
          // A note with the new fingerprint (host identity rotation §3.4): the renewal took the host's old key's
          // signature too, so no admin made it alone; the host itself is the one that can tell a renewal it did not stage.
          const r = action.rotation;
          const renewed = { signKey: r.signKey, dhKey: r.dhKey, onDevice: r.assurance === ASSURANCE_ON_DEVICE };
          out.push({ alert: { seq, kind: "rotateHostKey", by, subject: r.host, renewed, atMs, remove: null }, pending: false });
          break;
        }
        case "passkeySlotSet":
          out.push({ alert: { seq, kind: "passkey", passkey: "slotSet", by, subject: action.slot.slot, atMs, remove: removable(by) }, pending: false });
          break;
        case "passkeySlotRemove":
          out.push({ alert: { seq, kind: "passkey", passkey: "slotRemove", by, subject: action.slot, atMs, remove: null }, pending: false });
          break;
        case "passkeyActivate":
          if (action.id === me) continue;
          out.push({ alert: { seq, kind: "passkey", passkey: "activate", by, subject: action.id, atMs, remove: removable(action.id) }, pending: false });
          break;
        case "removeDevice3":
        case "removeDevice":
          if (action.id === me) continue;
          out.push({ alert: { seq, kind: "removeDevice", by, subject: action.id, atMs, remove: null }, pending: false });
          break;
        case "setRecoveryKey":
          out.push({ alert: { seq, kind: "setRecoveryKey", by, subject: null, atMs, remove: removable(by) }, pending: false });
          break;
        case "keyEpochs": {
          // New key epochs on their own — not a removal's rotation, nor an
          // enrollment (those are said by the entry the batch is for): left
          // unsaid, a device could commit epochs at will without anybody
          // hearing of it.
          // The account's upgrade to chain v3 (its first v3 entry) says so.
          if (seq === chain.v3From) {
            out.push({ alert: { seq, kind: "upgrade", by, subject: null, atMs, remove: removable(by) }, pending: false });
            break;
          }
          const prev = chain.entryAt(seq - 1)?.draft;
          const sameSigner = prev?.signer.type === "device" && signer.type === "device" && prev.signer.id === signer.id;
          // (Chain v3, as the agent's alerts: a host added, removed or hosting again carries its keys too.)
          const p = prev?.action;
          // A renewed identity key carries the host's next machine key epoch (host identity rotation §3.4, rule 6).
          const batch =
            sameSigner &&
            (p?.type === "removeDevice" ||
              p?.type === "addMachine" ||
              p?.type === "keyEpochs" ||
              p?.type === "addDevice3" ||
              p?.type === "removeDevice3" ||
              p?.type === "rotateHostKey" ||
              (p?.type === "setHost" && p.host));
          if (!batch) out.push({ alert: { seq, kind: "keyEpochs", by, subject: null, atMs, remove: removable(by) }, pending: false });
          break;
        }
        default:
          break;
      }
    }
    return out;
  }
}
