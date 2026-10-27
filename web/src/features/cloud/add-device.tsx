/*
 * Adding a browser or phone to the account: pairing by pasted text, QR code
 * or the code it shows, approving the candidate's fingerprint, and retiring
 * the device a new one says it replaces. The panel (`devices.tsx`) draws it.
 */

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { AlertTriangle, ClipboardPaste, Hash, Loader2 } from "lucide-react";
import { type ChainState, offerFromText } from "@/lib/e2e";
import { formatAsOf, formatRelative } from "@/lib/format";
import { type CloudAccount, deviceFingerprint, hasRunAi, type RotationReport } from "@/lib/hub/account";
import { type PairingSummary, type RegisteredDevice } from "@/lib/hub/api";
import type { HubLink } from "@/lib/hub/link";
import { abandonCandidate, type Candidate, candidateFromOffer, runCodeInitiator } from "@/lib/hub/pairing";
import { uaLabel, uaLabelOf } from "@/lib/hub/ua-label";
import { cn } from "@/lib/cn";
import { atLimit, DEVICES_KEY, type LimitHit, limitOf } from "@/lib/hub/plan";
import { shellCourier } from "./hub-shell";
import { freshLimit } from "@/lib/hub/me";
import { LimitNote, useMe } from "./plan";
import { QrScanner } from "./qr";
import { danger, ErrorLine, errorText, field, Fingerprint, Note, primary, quiet, Tabs } from "./ui";
import { ReportLines } from "./rotation-report";
import { useAccountVersion } from "./context";

export type AddMode = "paste" | "scan" | "code";

/** How recently a device signed in for it to count as in use: never offered for removal ({@link retireOffer}). */
export const IN_USE_MS = 10 * 60_000;

/** The device a new one says it replaces, as offered for removal: what a person needs to recognise it. */
export interface RetireOffer {
  id: string;
  /** Its browser by name, as the hub has it (`uaLabelOf`): the same as the new device's, or it is not offered. */
  label: string;
  /** When it registered, and when it last signed in (`null`: not known). */
  addedMs: number | null;
  lastSeenMs: number | null;
}

/**
 * The device `added` says it takes the place of — its browser lost that
 * one's keys and said so when it registered (`/v1/me` `devices[].replaces`,
 * the new device's claim, nothing more) — when it is worth offering to
 * remove:
 * - a browser or phone the chain still lists as active: never this device,
 *   the new one, or one that runs or ran AI;
 * - not one whose removal voids the account's recovery words (it set them,
 *   or came back with them): that one is left to the list's own removal,
 *   which says so;
 * - the same browser as the new one, by the hub's name for each
 *   (`ua_label`): a browser that lost its keys is still the same browser,
 *   so a different (or unknown) one is not offered;
 * - not signed in now (`online`), nor within {@link IN_USE_MS}.
 * Else `null`. Only ever an offer: the removal is the person's to confirm.
 */
export function retireOffer({
  chain,
  devices,
  added,
  self,
  nowMs,
}: {
  chain: ChainState | null;
  devices: readonly RegisteredDevice[] | null | undefined;
  added: string;
  self: string;
  nowMs: number;
}): RetireOffer | null {
  const fresh = devices?.find((d) => d.id === added);
  const old = fresh?.replaces ?? null;
  if (!old || old === self || old === added || !chain) return null;
  const record = chain.device(old);
  if (!record || record.host || hasRunAi(chain, record)) return null;
  if (chain.voidsRecoveryWords(old, "removal")) return null;
  const row = devices?.find((d) => d.id === old);
  const label = uaLabelOf(row?.uaLabel);
  if (!label || label !== uaLabelOf(fresh?.uaLabel)) return null;
  if (row?.online === true) return null;
  if (row?.lastSeenMs != null && nowMs - row.lastSeenMs < IN_USE_MS) return null;
  return { id: old, label, addedMs: row?.createdMs ?? null, lastSeenMs: row?.lastSeenMs ?? null };
}

/**
 * After adding device `added`, which says it took the place of `offer` (its
 * browser lost that one's keys): the claim, put as the new device's, with
 * what tells the old device apart — its browser, when it was added and last
 * seen, its fingerprint — and one question: is that one no longer usable,
 * and may it go? Removing is the ordinary removal (`onRemove`), only on the
 * person's tap, and says what it rotated as the list's removal does, alarms
 * included; "not now" leaves it listed, to remove from the list later.
 * Exported for the render tests.
 */
export function RetireReplacedPrompt({
  added,
  offer,
  fingerprint,
  onRemove,
}: {
  added: string;
  offer: RetireOffer;
  fingerprint: string | null;
  onRemove: () => Promise<RotationReport>;
}) {
  const { t } = useTranslation();
  const [state, setState] = useState<"ask" | "busy" | "kept">("ask");
  const [report, setReport] = useState<RotationReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const device = offer.id;
  if (state === "kept") return null;
  if (report) {
    return (
      <div data-testid="retire-replaced-done" className="flex flex-col gap-1">
        <span className="font-mono text-xs text-ok">{t("removed {{id}}", { id: device })}</span>
        <ReportLines report={report} t={t} />
      </div>
    );
  }
  const facts = [
    offer.label,
    offer.addedMs !== null ? t("added on {{when}}", { when: formatAsOf(offer.addedMs) }) : null,
    offer.lastSeenMs !== null ? t("seen {{when}}", { when: formatRelative(new Date(offer.lastSeenMs).toISOString()) }) : null,
  ].filter((x): x is string => !!x);
  return (
    <div role="alertdialog" data-testid="retire-replaced" className="flex flex-col gap-2 border border-border bg-warn/7 p-3 shadow-[inset_2px_0_0_var(--color-warn)]">
      <span className="text-xs leading-5 text-fg">
        {t("The new device {{new}} says it replaces {{old}}. Confirm that {{old}} can no longer be used, and remove it?", { new: added, old: device })}
      </span>
      <span data-testid="retire-replaced-facts" className="font-mono text-2xs text-muted">
        {facts.join(" · ")}
      </span>
      {fingerprint && <Fingerprint value={fingerprint} />}
      <ErrorLine message={error} />
      <div className="flex gap-2">
        <button
          type="button"
          data-testid="retire-replaced-remove"
          className={danger}
          disabled={state === "busy"}
          onClick={async () => {
            setState("busy");
            setError(null);
            try {
              setReport(await onRemove());
            } catch (err) {
              setError(errorText(err));
              setState("ask");
            }
          }}
        >
          {state === "busy" && <Loader2 className="size-3.5 animate-spin" />}
          {state === "busy" ? t("removing and rotating keys…") : t("remove {{id}}", { id: device })}
        </button>
        <button type="button" className={quiet} disabled={state === "busy"} onClick={() => setState("kept")}>
          {t("not now")}
        </button>
      </div>
    </div>
  );
}

/**
 * Pairing a browser or phone from this device — and approving one that
 * joined with a passkey and waits, the same way (`initialMode`: the tab it
 * opens on). Exported for the 「添加可运行 AI 的设备」 guide, whose sign-in
 * window may need it, and for the passkey join's notice.
 */
export function AddDevice({ account, link, onDone, initialMode = "paste" }: { account: CloudAccount; link: HubLink; onDone: () => void; initialMode?: AddMode }) {
  const { t } = useTranslation();
  const [mode, setMode] = useState<AddMode>(initialMode);
  const [candidate, setCandidate] = useState<Candidate | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [added, setAdded] = useState<string | null>(null);
  const me = useMe(account, link);
  // The plan's device limit (§4.3), as last read: said before anything is
  // scanned or typed. A device pairing again is not added, so it may go on.
  const full = atLimit(me?.plan, DEVICES_KEY);
  // Once a device is added: the one it says it replaces, if that is worth offering (never one an activation hold keeps), and its fingerprint.
  const retire = added ? retireOffer({ chain: account.chain, devices: me?.view?.devices, added, self: account.deviceId, nowMs: Date.now() }) : null;
  const retiring = retire && !blockedByHold(account, retire.id) ? retire : null;
  const [retiredPrint, setRetiredPrint] = useState<{ id: string; print: string } | null>(null);
  useEffect(() => {
    const record = retiring ? account.chain?.device(retiring.id) : undefined;
    if (!record) return;
    let live = true;
    void deviceFingerprint(record.info.signKey, record.info.dhKey).then((print) => {
      if (live) setRetiredPrint({ id: record.info.id, print });
    });
    return () => {
      live = false;
    };
  }, [account, retiring?.id]);

  const fromText = async (text: string) => {
    setError(null);
    try {
      setCandidate(await candidateFromOffer(text, account.chain!));
    } catch (err) {
      setError(errorText(err));
    }
  };

  if (added) {
    // The device it took the place of, if any (never one in use, nor while an activation is held back).
    const old = retiring;
    return (
      <div data-signal="ok" className="flex flex-col gap-2 border border-border p-3 shadow-[inset_2px_0_0_var(--color-ok)]">
        <span className="font-mono text-xs text-ok">{t("added {{id}}", { id: added })}</span>
        <p className="text-xs leading-5 text-muted">{t("Its keys are on their way: the new device says “paired” once it has checked them.")}</p>
        {old && (
          <RetireReplacedPrompt
            key={old.id}
            added={added}
            offer={old}
            fingerprint={retiredPrint?.id === old.id ? retiredPrint.print : null}
            onRemove={() => account.removeDevice(old.id, shellCourier(link, account))}
          />
        )}
        <button type="button" className={quiet} onClick={onDone}>
          {t("done")}
        </button>
      </div>
    );
  }
  if (candidate) {
    return (
      <ConfirmCandidate
        account={account}
        me={me}
        candidate={candidate}
        onAdded={() => setAdded(candidate.device)}
        onCancel={() => {
          abandonCandidate(candidate);
          setCandidate(null);
        }}
      />
    );
  }
  return (
    <div className="flex flex-col gap-2 border border-border p-3">
      {full && <LimitNote hit={full} />}
      <Tabs
        value={mode}
        onChange={(m) => {
          setMode(m);
          setError(null);
        }}
        options={[
          { id: "paste", label: t("paste its text") },
          { id: "scan", label: t("scan its QR code") },
          { id: "code", label: t("enter a code") },
        ]}
      />
      {mode === "paste" && <PasteOffer onText={(text) => void fromText(text)} />}
      {mode === "scan" && (
        <QrScanner
          accept={(text) => {
            try {
              offerFromText(text);
              return true;
            } catch {
              return false;
            }
          }}
          onText={(text) => void fromText(text)}
        />
      )}
      {mode === "code" && <EnterCode account={account} onCandidate={setCandidate} />}
      <ErrorLine message={error} />
      <button type="button" className={quiet} onClick={onDone}>
        {t("cancel")}
      </button>
    </div>
  );
}

function PasteOffer({ onText }: { onText: (text: string) => void }) {
  const { t } = useTranslation();
  const [text, setText] = useState("");
  return (
    <form
      className="flex flex-col gap-2"
      onSubmit={(e) => {
        e.preventDefault();
        if (text.trim()) onText(text);
      }}
    >
      <p className="text-xs leading-5 text-muted">{t("On the new browser, sign in to this account and copy the text under its QR code.")}</p>
      <textarea
        value={text}
        onChange={(e) => setText(e.target.value)}
        rows={3}
        spellCheck={false}
        autoComplete="off"
        placeholder={t("the text under its QR code")}
        className={cn(field, "h-auto py-1.5 break-all")}
      />
      <div className="flex gap-2">
        <button type="submit" data-variant="primary" className={primary} disabled={!text.trim()}>
          <ClipboardPaste className="size-3.5" />
          {t("read it")}
        </button>
        {navigator.clipboard?.readText && (
          <button
            type="button"
            className={quiet}
            onClick={() => {
              void navigator.clipboard.readText().then((v) => {
                setText(v);
                if (v.trim()) onText(v);
              });
            }}
          >
            {t("paste from clipboard")}
          </button>
        )}
      </div>
    </form>
  );
}

function EnterCode({ account, onCandidate }: { account: CloudAccount; onCandidate: (c: Candidate) => void }) {
  const { t } = useTranslation();
  const [pairings, setPairings] = useState<PairingSummary[] | null>(null);
  const [picked, setPicked] = useState<string | null>(null);
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [wrong, setWrong] = useState(false);
  useEffect(() => {
    let live = true;
    const load = async () => {
      try {
        const list = await account.api.pairings(account.session.token);
        if (!live) return;
        setPairings(list);
        setPicked((p) => (p && list.some((x) => x.id === p) ? p : (list.at(-1)?.id ?? null)));
      } catch (err) {
        if (live) setError(errorText(err));
      }
    };
    void load();
    const timer = setInterval(() => void load(), 3000);
    return () => {
      live = false;
      clearInterval(timer);
    };
  }, [account]);
  const digits = code.replace(/\D/g, "");
  const pairing = pairings?.find((p) => p.id === picked) ?? null;
  return (
    <form
      className="flex flex-col gap-2"
      onSubmit={async (e) => {
        e.preventDefault();
        if (busy || !pairing || digits.length !== 8) return;
        setBusy(true);
        setError(null);
        setWrong(false);
        try {
          onCandidate(await runCodeInitiator({ api: account.api, token: account.session.token, account: account.account }, pairing, digits));
        } catch (err) {
          // One guess per code: whatever went wrong, this code is spent.
          setWrong(true);
          setError(errorText(err));
          setCode("");
          setBusy(false);
        }
      }}
    >
      <p className="text-xs leading-5 text-muted">{t("On the new browser, choose “use a code instead”, then type the 8 digits it shows.")}</p>
      {pairings === null ? (
        <Loader2 className="size-4 animate-spin text-faint" />
      ) : pairings.length === 0 ? (
        <p className="flex items-center gap-2 text-2xs text-faint">
          <Loader2 className="size-3 animate-spin" />
          {t("no device is showing a code yet…")}
        </p>
      ) : (
        <ul className="flex flex-col gap-1">
          {pairings.map((p) => (
            <li key={p.id}>
              <label className="flex items-center gap-2 font-mono text-2xs text-muted">
                <input type="radio" name="pairing" checked={picked === p.id} onChange={() => setPicked(p.id)} />
                <span className="text-fg">{p.device}</span>
                <span className="text-faint">{formatRelative(new Date(p.createdMs).toISOString())}</span>
                {/* Any full device may close one it does not recognise (M2 re-review). */}
                <button
                  type="button"
                  className={cn(quiet, "ml-auto h-5 px-1.5 text-2xs")}
                  title={t("close this pairing (not a device of yours?)")}
                  onClick={() => {
                    void account.api
                      .closePairing(account.session.token, p.id)
                      .then(() => setPairings((list) => (list ?? []).filter((x) => x.id !== p.id)))
                      .catch((err: unknown) => setError(errorText(err)));
                  }}
                >
                  {t("close")}
                </button>
              </label>
            </li>
          ))}
        </ul>
      )}
      <input
        value={code}
        onChange={(e) => setCode(e.target.value)}
        inputMode="numeric"
        autoComplete="one-time-code"
        placeholder="0000 0000"
        maxLength={9}
        className={cn(field, "text-center text-base tracking-[0.3em]")}
      />
      {wrong && (
        <p className="flex items-start gap-1.5 text-2xs text-danger">
          <AlertTriangle className="mt-0.5 size-3 shrink-0" />
          {t("That did not work, and the code is spent. On the new device, start again for a new code.")}
        </p>
      )}
      <ErrorLine message={wrong ? null : error} />
      <button type="submit" data-variant="primary" className={primary} disabled={busy || !pairing || digits.length !== 8}>
        {busy ? <Loader2 className="size-3.5 animate-spin" /> : <Hash className="size-3.5" />}
        {t("check the code")}
      </button>
    </form>
  );
}

/**
 * The candidate's fingerprint, compared, and the pairing: a browser or phone
 * is added as a full device, always — there is no choice to make (chain v2's
 * full device, v3's admin). One that pairs again keeps what it is.
 * Exported for the render tests.
 */
export function ConfirmCandidate({
  account,
  me,
  candidate,
  onAdded,
  onCancel,
}: {
  account: Pick<CloudAccount, "chain" | "pairDevice"> & Partial<Pick<CloudAccount, "onChange" | "heldBack">>;
  me: ReturnType<typeof useMe>;
  candidate: Candidate;
  onAdded: () => void;
  onCancel: () => void;
}) {
  const { t } = useTranslation();
  // The chain as it moves on while this is open (a join replaced meanwhile is said at once).
  useAccountVersion(account.onChange ? (account as CloudAccount) : null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [limit, setLimit] = useState<LimitHit | null>(null);
  const record = account.chain?.deviceRecord(candidate.device);
  // While this device holds an activation back it pairs no device, not even one already listed (`CloudAccount.pairDevice`): said before the click.
  const held = account.heldBack ?? null;
  // Waiting after a passkey join when this was read: approving it is what this does.
  const [joining] = useState(() => record?.viaPasskey != null && record.status.type === "pending");
  // …and a newer join (or a removal) took its place meanwhile: nothing to approve any more.
  const replaced = joining && !busy && record?.status.type !== "pending";
  if (replaced) {
    return (
      <div data-testid="join-replaced" className="flex flex-col gap-2 border border-fg p-3">
        <Note tone="danger">
          <p className="text-xs leading-5 text-muted">
            {t("{{id}} no longer waits to join: a newer join with the same passkey replaced it, or a device removed it. Nothing was approved.", { id: candidate.device })}
          </p>
        </Note>
        <button type="button" className={quiet} onClick={onCancel}>
          {t("close")}
        </button>
      </div>
    );
  }
  return (
    <div className="flex flex-col gap-2 border border-fg p-3">
      <span className="font-mono text-xs text-fg">
        {joining
          ? t("approve {{id}}, which asked to join with a passkey?", { id: candidate.device })
          : record
            ? t("{{id}} is in this account already: send it its keys again?", { id: candidate.device })
            : t("add {{id}} to this account?", { id: candidate.device })}
      </span>
      <p className="text-xs leading-5 text-muted">{t("Check that the new device shows the same fingerprint:")}</p>
      <div className="border border-border bg-raised p-2.5 text-center">
        <Fingerprint value={candidate.fingerprint} />
      </div>
      {limit && <LimitNote hit={limit} action={{ label: t("devices"), onClick: onCancel }} />}
      {held && (
        <Note tone="danger">
          <p data-testid="pairing-held" className="text-xs leading-5 text-muted">
            {t("approve or remove the device whose activation is held back first")}
          </p>
        </Note>
      )}
      <ErrorLine message={error} />
      <div className="flex gap-2">
        <button
          type="button"
          data-variant="primary" className={primary}
          disabled={busy || !!held}
          onClick={async () => {
            setBusy(true);
            setError(null);
            setLimit(null);
            try {
              // An addition (not a device pairing again): the plan's device
              // limit, freshly read, before anything is signed (§4.3).
              if (me && !account.chain?.deviceRecord(candidate.device)) {
                const hit = await freshLimit(me, DEVICES_KEY, (err) => console.warn("cloud: /v1/me before adding a device:", err));
                if (hit) {
                  setLimit(hit);
                  setBusy(false);
                  return;
                }
              }
              // Every browser or phone is a full device (a decision of 2026-10-20).
              await account.pairDevice(candidate, { admin: true });
              onAdded();
              me?.soon();
            } catch (err) {
              const hit = limitOf(err);
              if (hit) setLimit(hit);
              else setError(errorText(err));
              setBusy(false);
            }
          }}
        >
          {busy && <Loader2 className="size-3.5 animate-spin" />}
          {joining ? t("the fingerprints match: approve it") : t("the fingerprints match: add it")}
        </button>
        <button type="button" className={quiet} disabled={busy} onClick={onCancel}>
          {t("cancel")}
        </button>
      </div>
    </div>
  );
}

/**
 * While an activation is held back, removing anyone but the device it
 * activates would go on the hub's chain past it (M2 re-review N1): only the
 * veto of that device is offered, and it takes out what the device brought in.
 */
export function blockedByHold(account: Pick<CloudAccount, "heldBack">, device: string): boolean {
  const held = account.heldBack;
  return !!held && held.device !== device;
}
