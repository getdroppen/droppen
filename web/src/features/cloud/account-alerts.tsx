/*
 * The account-wide alert stack, drawn in one place for the three pages that
 * show it: the bar above the app (`cloud-bar.tsx`), 「我的设备」 and 「我的账户」
 * (`account-settings.tsx`). One component keeps every security alert on every
 * page it was on, and makes the two deliberate differences visible: the
 * account page has never shown the chain's or the registered devices' alerts
 * (it is where those are acted on, not a second place to be told), and the
 * bar has never shown the e-mail verification, the plan strips or the
 * account's notices (they belong to the pages, not over the app).
 */

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { AlertTriangle, Loader2, RefreshCw } from "lucide-react";
import { formatExactTime, formatRelative } from "@/lib/format";
import { type ChainAlert, type CloudAccount, deviceFingerprint, type HeldChange, type RotationReport, type VetoPlan, type VetoTarget } from "@/lib/hub/account";
import { chainV3Writable } from "@/lib/hub/api";
import type { HubLink } from "@/lib/hub/link";
import { cn } from "@/lib/cn";
import { V3Pending, v3Blocker, type V3Blocker } from "./add-computer";
import { shellCourier } from "./hub-shell";
import { PushRenewNotice, RenewalNotice, useRenewals } from "./renewal";
import { useMe } from "./plan";
import { danger, ErrorLine, errorText, primary, quiet, secondary } from "./ui";
import { Button } from "@/components/ui/button";
import { CapsLabel } from "@/components/ui/caps-label";
import { CHECKBOX_INPUT, CHECKBOX_LABEL, CheckboxMark } from "@/components/ui/checkbox";
import { blockedByHold } from "./add-device";
import { ReportLines } from "./rotation-report";
import { useAccountVersion } from "./context";
import { PendingPasskeyBanner } from "./passkey-join";
import { VerifyEmailBanner } from "./email-verify";
import { PlanBanner, RegisteredDeviceAlerts, SignInChangeAlerts } from "./plan";

/** Where the stack is drawn; each place keeps exactly the alerts it had. */
export type AlertsWhere = "bar" | "devices" | "account";

export function AccountAlerts({
  account,
  link,
  where,
  onRemovePasskey,
  onReview,
}: {
  account: CloudAccount;
  link: HubLink;
  where: AlertsWhere;
  /** What a refused passkey's removal does here (the page's own way to the removal). */
  onRemovePasskey: (credHash: string) => void;
  /** Where a sign-in change is reviewed (absent on the account page, which is where it is reviewed). */
  onReview?: () => void;
}) {
  return (
    <>
      <PendingPasskeyBanner account={account} link={link} onRemovePasskey={onRemovePasskey} />
      <PendingRecoveryBanner account={account} link={link} />
      {where !== "account" && <ChainAlerts account={account} link={link} />}
      {where !== "account" && <RegisteredDeviceAlerts account={account} link={link} />}
      <SignInChangeAlerts account={account} link={link} onReview={onReview} />
      {where !== "bar" && <VerifyEmailBanner account={account} />}
      {where === "devices" && <PlanBanner account={account} link={link} />}
      {where !== "bar" && <AccountNotices account={account} link={link} />}
    </>
  );
}

/**
 * What the account needs said above 「我的设备」's list (`my-devices.tsx` draws
 * it at the top of the page): this browser is a limited device; the account
 * is still on chain v2, or waits for the hub; keys are due to be rotated; the
 * newest user key has not reached this device. A device that runs AI and
 * should be enrolled again says so on its own row.
 */
export function AccountNotices({ account, link }: { account: CloudAccount; link: HubLink }) {
  useAccountVersion(account);
  const me = useMe(account, link);
  const viewerAdmin = account.isAdmin;
  const upgraded = account.chain?.isV3 === true;
  const writable = chainV3Writable(me?.view);
  const blocker = v3Blocker(me?.view);
  const renewals = useRenewals();
  return (
    <>
      {!viewerAdmin && account.standing === "active" && <LimitedDeviceNote missing={account.missingHostKeys} upgraded={upgraded} />}
      {viewerAdmin && !upgraded && <UpgradeAccount account={account} writable={writable} blocker={blocker} waiting={me?.view?.chainV3?.waiting ?? []} onDone={() => me?.soon()} />}
      {viewerAdmin && upgraded && !writable && <V3Note blocker={blocker} waiting={me?.view?.chainV3?.waiting ?? []} />}
      {viewerAdmin && account.rotationDue && <RotateKeys account={account} link={link} due />}
      {account.missingUserKey !== null && <MissingUserKey account={account} link={link} epoch={account.missingUserKey} />}
      {renewals && account.standing === "active" && <RenewalNotice runner={renewals} />}
      <PushRenewNotice />
    </>
  );
}

/**
 * This browser is a limited device (not chain v3's admin: an older pairing,
 * or a passkey's join): what it can do, and that another of the account's
 * devices makes it a full device — once the account is upgraded to chain v3
 * (`upgraded`), which that device does first otherwise. The list below is
 * read-only here.
 */
export function LimitedDeviceNote({ missing, upgraded = true }: { missing: string[]; upgraded?: boolean }) {
  const { t } = useTranslation();
  return (
    <div data-testid="limited-device-note" className="flex flex-col gap-1 border-b border-border px-6 py-3 max-md:px-4">
      <span className="font-mono text-xs text-fg">{t("this browser is a limited device")}</span>
      <span className="text-xs leading-5 text-muted">
        {upgraded
          ? t("It can control the devices that let it and read their offline copies. To add or remove devices here, make it a full device from another of your devices.")
          : t("It can control the devices that let it and read their offline copies. To add or remove devices here, upgrade the account to chain v3 from another of your devices, then make this browser a full device there.")}
      </span>
      {missing.length > 0 && (
        <span data-signal="warn" className="text-2xs text-warn-fg">
          {t("waiting for the keys of {{hosts}} from a full device", { hosts: missing.join(", ") })}
        </span>
      )}
    </div>
  );
}

/** On a v3 account while the hub takes no v3 entry: adding a computer and running AI again wait (the hub gates new hosts) — for the hub, or for computers not updated yet (`blocker`). */
export function V3Note({ waiting, blocker = "computers" }: { waiting: string[]; blocker?: V3Blocker | null }) {
  const { t } = useTranslation();
  return (
    <div data-testid="v3-note" className="flex flex-col gap-1 border-b border-border px-6 py-3 max-md:px-4">
      <span className="text-xs leading-5 text-muted">
        {blocker === "hub"
          ? t("Adding a device that runs AI, and running AI again on one, wait until the hub takes chain v3 changes for this account.")
          : t("Adding a device that runs AI, and running AI again on one, wait until Droppen is updated on every device of the account that runs AI.")}
      </span>
      <V3Pending waiting={waiting} blocker={blocker} />
    </div>
  );
}

/**
 * The upgrade to chain v3 of an account made before it (review D4,
 * local-aggregator §3.5; a new account is v3 from its genesis and never
 * shows this): a step of its own, from a full device, while the chain
 * holds no v3 entry — offered only while the hub takes v3 entries, and
 * written only once the person has read what it means and what it writes.
 */
export function UpgradeAccount({
  account,
  writable,
  waiting,
  blocker = "computers",
  onDone,
}: {
  account: Pick<CloudAccount, "upgradePlan" | "upgradeToV3">;
  writable: boolean;
  waiting: string[];
  /** Why it cannot upgrade now (`v3Blocker`): the hub (`/v1/me`'s `chain_v3.mode` is not `write`), or computers not updated yet. */
  blocker?: V3Blocker | null;
  onDone?: () => void;
}) {
  const { t } = useTranslation();
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [report, setReport] = useState<RotationReport | null>(null);
  const plan = account.upgradePlan();
  if (report) {
    return (
      <div data-testid="upgraded" data-signal="ok" className="flex flex-col gap-1 border-b border-border px-6 py-3 shadow-[inset_2px_0_0_var(--color-ok)] max-md:px-4">
        <span className="font-mono text-xs text-ok">{t("the account is on chain v3")}</span>
        <ReportLines report={report} t={t} alarm={false} />
      </div>
    );
  }
  if (!plan) return null;
  return (
    <div data-testid="upgrade-account" className="flex flex-col gap-2 border-b border-border px-6 py-3 max-md:px-4">
      <span className="font-mono text-xs text-fg">{t("this account is still on chain v2")}</span>
      <span className="text-xs leading-5 text-muted">
        {t("Upgrading it lets you stop AI on a device, decide who may control each device that runs AI, and make a limited device a full device.")}
      </span>
      {!writable && <V3Pending waiting={waiting} blocker={blocker} />}
      {writable && !confirming && (
        <button type="button" data-testid="upgrade-start" className={cn(secondary, "self-start")} onClick={() => setConfirming(true)}>
          {t("upgrade this account to chain v3")}
        </button>
      )}
      {writable && confirming && (
        <UpgradeConfirm
          ukEpoch={plan.ukEpoch}
          busy={busy}
          error={error}
          onCancel={() => setConfirming(false)}
          onConfirm={async () => {
            setBusy(true);
            setError(null);
            try {
              setReport(await account.upgradeToV3({ writable }));
              onDone?.();
            } catch (err) {
              setError(errorText(err));
            } finally {
              setBusy(false);
            }
          }}
        />
      )}
    </div>
  );
}

/** The upgrade's confirmation: what it means, that it cannot be undone, and exactly what it writes. */
export function UpgradeConfirm({ ukEpoch, busy, error, onConfirm, onCancel }: { ukEpoch: number; busy: boolean; error: string | null; onConfirm: () => void; onCancel: () => void }) {
  const { t } = useTranslation();
  return (
    <div role="alertdialog" data-testid="upgrade-confirm" data-signal="warn" className="flex max-w-xl flex-col gap-2 border border-border bg-warn/7 p-3 shadow-[inset_2px_0_0_var(--color-warn)]">
      <span className="font-mono text-xs text-fg">{t("upgrade this account to chain v3?")}</span>
      <ul className="flex list-disc flex-col gap-1 pl-4 text-xs leading-5 text-muted">
        <li>{t("Older Droppen apps, and older Droppen on your devices that run AI, cannot follow the account afterwards: update them first.")}</li>
        <li className="text-fg">{t("This cannot be undone.")}</li>
      </ul>
      <p data-testid="upgrade-writes" className="text-2xs leading-4 text-muted">
        {t("What is written: one entry, a new user key (epoch {{n}}), as chain v3. Every full device of the account gets the new key; the recovery words keep working.", { n: ukEpoch })}
      </p>
      <ErrorLine message={error} />
      <div className="flex gap-2">
        <button type="button" data-testid="upgrade-confirm-button" className={danger} disabled={busy} onClick={onConfirm}>
          {busy && <Loader2 className="size-3.5 animate-spin" />}
          {t("upgrade the account")}
        </button>
        <button type="button" className={quiet} disabled={busy} onClick={onCancel}>
          {t("cancel")}
        </button>
      </div>
    </div>
  );
}

/**
 * "Missing key": the chain commits a newer user key than this device holds
 * (it missed the rotation's mail, or the rotation died after its commit). It
 * wraps no machine key until it has it (M2 re-review) — it has asked the
 * account's other devices to send it; if none has it, rotating again mends it.
 */
export function MissingUserKey({ account, link, epoch }: { account: CloudAccount; link: HubLink; epoch: number }) {
  const { t } = useTranslation();
  const [busy, setBusy] = useState<"ask" | "rotate" | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const run = async (what: "ask" | "rotate") => {
    setBusy(what);
    setError(null);
    setNote(null);
    try {
      if (what === "ask") {
        const asked = await account.requestUserKeys(epoch, true);
        setNote(asked.length ? t("asked {{ids}}", { ids: asked.join(", ") }) : t("no other device to ask"));
      } else {
        await account.rotate(shellCourier(link, account), true);
        await account.checkUserKey();
      }
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(null);
    }
  };
  return (
    <div
      role="alert"
      data-testid="missing-user-key"
      data-signal="warn"
      className="flex flex-wrap items-center gap-x-3 gap-y-2 border-b border-border bg-warn/7 px-6 py-3 shadow-[inset_2px_0_0_var(--color-warn)] max-md:px-4"
    >
      <span className="flex min-w-[min(100%,16rem)] flex-1 items-start gap-3">
        <span className="mt-0.5 inline-flex h-4 shrink-0 items-center bg-warn px-1 font-mono text-[10px] leading-none font-semibold text-on-signal">{t("missing key")}</span>
        <span className="min-w-0 flex-1 text-xs leading-5 text-muted">
          <span className="text-fg">{t("This device does not hold the account's newest user key (epoch {{n}}).", { n: epoch })}</span>{" "}
          {t("It wraps no new content key until it does, and has asked the account's other devices to send it. If none of them has it, rotate the keys again.")}
          {note && <span className="font-mono text-2xs text-faint"> · {note}</span>}
        </span>
      </span>
      <Button disabled={busy !== null} onClick={() => void run("ask")}>
        {busy === "ask" && <Loader2 className="animate-spin" />}
        {t("ask again")}
      </Button>
      <Button variant="primary" disabled={busy !== null} onClick={() => void run("rotate")}>
        {busy === "rotate" ? <Loader2 className="animate-spin" /> : <RefreshCw />}
        {busy === "rotate" ? t("rotating keys…") : t("rotate keys again")}
      </Button>
      <ErrorLine message={error} />
    </div>
  );
}

/**
 * "Rotate keys again": a new user key for every device and a new key for
 * every machine (`CloudAccount.rotate`), committed in the chain first. Always
 * here; drawn as the thing to do (`due`) when the chain's last removal was
 * not followed by new keys — a rotation that never ran, or died half-way.
 */
export function RotateKeys({ account, link, due }: { account: CloudAccount; link: HubLink; due: boolean }) {
  const { t } = useTranslation();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [report, setReport] = useState<RotationReport | null>(null);
  const run = async () => {
    setBusy(true);
    setError(null);
    try {
      setReport(await account.rotate(shellCourier(link, account), true));
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };
  if (report) {
    return (
      <div data-testid="rotated" data-signal="ok" className="mx-6 my-4 flex flex-col gap-1 border border-border p-3 shadow-[inset_2px_0_0_var(--color-ok)] max-md:mx-4">
        <span className="font-mono text-xs text-ok">{t("keys rotated")}</span>
        <ReportLines report={report} t={t} />
        <Button variant="ghost" className="self-start" onClick={() => setReport(null)}>
          {t("done")}
        </Button>
      </div>
    );
  }
  if (due) {
    return (
      <div
        role="alert"
        data-testid="rotation-due"
        data-signal="warn"
        className="flex flex-wrap items-center gap-x-3 gap-y-2 border-b border-border bg-warn/7 px-6 py-3 shadow-[inset_2px_0_0_var(--color-warn)] max-md:px-4"
      >
        <span className="flex min-w-[min(100%,16rem)] flex-1 items-start gap-3">
          <span className="mt-0.5 inline-flex h-4 shrink-0 items-center bg-warn px-1 font-mono text-[10px] leading-none font-semibold text-on-signal">{t("needs you")}</span>
          <span className="min-w-0 flex-1 text-xs leading-5 text-muted">
            <span className="text-fg">{t("A device that held the user key was removed or made a limited device, but its keys were not retired.")}</span>{" "}
            {t("Until they are, it can still read what is new. Rotate them now: every device gets a new user key and every device that runs AI a new content key.")}
          </span>
        </span>
        <Button variant="primary" disabled={busy} onClick={() => void run()}>
          {busy ? <Loader2 className="animate-spin" /> : <RefreshCw />}
          {busy ? t("rotating keys…") : t("rotate keys again")}
        </Button>
        <ErrorLine message={error} />
      </div>
    );
  }
  return (
    <div data-testid="rotate-keys" className="flex flex-col gap-2">
      <CapsLabel>{t("keys")}</CapsLabel>
      <p className="text-xs leading-5 text-muted">
        {t("Rotate the keys if a removal did not finish, or you think a key got out: every device gets a new user key and every device that runs AI a new content key. Old history stays readable here.")}
      </p>
      <Button className="self-start" disabled={busy} onClick={() => void run()}>
        {busy ? <Loader2 className="animate-spin" /> : <RefreshCw />}
        {busy ? t("rotating keys…") : t("rotate keys again")}
      </Button>
      <ErrorLine message={error} />
    </div>
  );
}

/** How loudly an alert speaks (local-aggregator §3.2): "is this you?", louder for a device made a full device (it gets the account's keys), a note for the rest. */
export function alertLevel(alert: ChainAlert): "confirm" | "high" | "info" {
  if (alert.kind === "setAdmin") return alert.on ? "high" : "info";
  if (alert.kind === "setHost" || alert.kind === "setAccept" || alert.kind === "removeDevice" || alert.kind === "keyEpochs" || alert.kind === "upgrade" || alert.kind === "rotateHostKey" || alert.kind === "renewalPause") return "info";
  // Renewals confirmed on the device turned off (or never on): "is this you?" on every device. Turned on: a note.
  if (alert.kind === "renewalPolicy") return alert.on ? "info" : "confirm";
  if (alert.kind === "passkey") return alert.passkey === "join" || alert.passkey === "slotSet" ? "confirm" : "info";
  return "confirm";
}

/** What an alert says happened, in words (`fingerprint`: a renewed identity key's new fingerprint, once computed). */
export function alertText(alert: ChainAlert, t: (key: string, opts?: Record<string, unknown>) => string, fingerprint?: string): string {
  const by = alert.by ?? "?";
  const device = alert.subject ?? "?";
  switch (alert.kind) {
    case "addDevice": {
      const f = alert.flags;
      // A full device (chain v2's every addition, v3's admin) is what a browser or phone is: no more to say. A host is never one (review D1).
      if (!f || (f.admin && !f.host)) return t("{{by}} added {{device}} to this account", { by, device });
      if (f.host) return t("{{by}} added {{device}} to this account: it can run AI", { by, device });
      return t("{{by}} added {{device}} to this account as a limited device", { by, device });
    }
    case "removeDevice":
      return t("{{by}} removed {{device}} from this account", { by, device });
    case "recoveryJoin":
      return t("{{device}} came back into this account with the recovery words", { device });
    case "setRecoveryKey":
      return t("{{by}} made new recovery words for this account", { by });
    case "keyEpochs":
      return t("{{by}} made new keys for this account, on its own", { by });
    case "upgrade":
      return t("{{by}} upgraded this account to chain v3 (with a new user key)", { by });
    case "setAdmin":
      return alert.on ? t("{{by}} made {{device}} a full device", { by, device }) : t("{{by}} made {{device}} a limited device", { by, device });
    case "setHost":
      return alert.on ? t("{{device}} runs AI again ({{by}} restarted it)", { by, device }) : t("{{device}} no longer runs AI ({{by}} stopped it)", { by, device });
    case "setAccept": {
      const a = alert.accept;
      if (a?.type === "all") return t("{{by}} let every device of this account control {{host}}", { by, host: device });
      if (a?.type === "devices") return t("{{by}} let {{ids}} control {{host}}", { by, host: device, ids: a.ids.join(", ") });
      return t("{{by}} set {{host}} to take control only from your full devices", { by, host: device });
    }
    case "passkey":
      switch (alert.passkey) {
        case "slotSet":
          return t("{{by}} let new devices join with a passkey", { by });
        case "slotRemove":
          return t("{{by}} stopped new devices joining with a passkey", { by });
        case "join":
          return t("a passkey added {{device}} to this account (it waits to be approved)", { device });
        default:
          return t("{{device}} activated itself with a passkey", { device });
      }
    case "renewalPause": {
      // Host identity rotation §3.4 rule 10: control paused over hosts while identity keys are renewed, a note.
      const edges = (alert.pause?.removals ?? []).map((r) => t("{{ids}} over {{host}}", { ids: r.ids.join(", "), host: r.host })).join("; ") || "?";
      if (alert.pause?.phase === "completed") {
        if (alert.pause.removals.length === 0) return t("a renewal pause was completed; none of the control it paused comes back (by {{by}})", { by });
        return t("control by {{edges}} resumes (by {{by}})", { edges, by });
      }
      if (alert.pause?.phase === "cancelled") return t("the pause of control by {{edges}} was ended (by {{by}})", { edges, by });
      return t("control by {{edges}} is paused while identity keys are renewed (by {{by}})", { edges, by });
    }
    case "renewalPolicy":
      // Host identity rotation §3.4 rule 9: the account's renewal policy, a note.
      return alert.on
        ? t("identity key renewals must now be confirmed on the device (set by {{by}})", { by })
        : t("identity key renewals may now be automatic (set by {{by}})", { by });
    case "rotateHostKey":
      // Host identity rotation §3.4 "告警": a note, with the new fingerprint as the device list shows it — and, as the
      // agent's notice says, whether the renewal was confirmed on the device itself (assurance 1).
      if (alert.renewed?.onDevice) {
        return fingerprint
          ? t("{{device}} renewed its identity key, confirmed on the device · new fingerprint {{fingerprint}}", { device, fingerprint })
          : t("{{device}} renewed its identity key, confirmed on the device", { device });
      }
      return fingerprint ? t("{{device}} renewed its identity key · new fingerprint {{fingerprint}}", { device, fingerprint }) : t("{{device}} renewed its identity key", { device });
  }
}

/**
 * The account's alerts on this device: every change to who is in the account
 * — a device added or removed, a recovery join, new recovery words — that
 * another device made and this one has not acknowledged. "That was me"
 * dismisses one here (and only here); where it applies, the device it is
 * about can be removed at once (which also retires the keys it held).
 */
export function ChainAlerts({ account, link }: { account: CloudAccount; link: HubLink }) {
  const { t } = useTranslation();
  useAccountVersion(account);
  const [busy, setBusy] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const alerts = account.alerts();
  // A renewed identity key is said with its new fingerprint, which takes a hash: worked out here, by the entry's seq.
  const renewals = alerts.filter((a) => a.kind === "rotateHostKey" && a.renewed);
  const renewalSeqs = renewals.map((a) => a.seq).join(",");
  const [prints, setPrints] = useState<ReadonlyMap<number, string>>(new Map());
  useEffect(() => {
    let live = true;
    void Promise.all(renewals.map(async (a) => [a.seq, await deviceFingerprint(a.renewed!.signKey, a.renewed!.dhKey)] as const)).then((list) => {
      if (live) setPrints(new Map(list));
    });
    return () => {
      live = false;
    };
    // The renewals are the entries at these seqs: nothing else changes what they say.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [renewalSeqs]);
  if (!alerts.length) return null;
  return (
    <div role="alert" data-testid="chain-alerts" data-signal="warn" className="flex flex-col border-b border-border bg-warn/7 shadow-[inset_2px_0_0_var(--color-warn)]">
      {alerts.map((a) => {
        const iso = new Date(a.atMs).toISOString();
        return (
          <div key={a.seq} data-testid={`alert-${a.seq}`} className="flex flex-wrap items-center gap-x-3 gap-y-2 border-b border-border px-4 py-2.5 last:border-b-0">
            {/* The words keep a readable width; on a narrow page the actions go under them. */}
            <span className="flex min-w-[min(100%,16rem)] flex-1 items-start gap-3">
              {alertLevel(a) === "high" ? (
                <span data-signal="alarm" data-level="high" className="mt-0.5 inline-flex h-4 shrink-0 items-center bg-danger px-1 font-mono text-[10px] leading-none font-semibold text-on-signal">
                  {t("important")}
                </span>
              ) : alertLevel(a) === "info" ? (
                <span data-level="info" className="mt-0.5 inline-flex h-4 shrink-0 items-center px-1 font-mono text-[10px] leading-none font-semibold text-fg shadow-[inset_0_0_0_1px_var(--color-border-strong)]">
                  {t("note")}
                </span>
              ) : (
                <span data-level="confirm" className="mt-0.5 inline-flex h-4 shrink-0 items-center bg-warn px-1 font-mono text-[10px] leading-none font-semibold text-on-signal">{t("check")}</span>
              )}
              <span className="min-w-0 flex-1 text-xs leading-5 text-muted">
                <span className="text-fg">{alertText(a, t, prints.get(a.seq))}</span>
                <span className="text-faint"> · </span>
                <span className="font-mono text-2xs text-faint" title={formatExactTime(iso)}>
                  {formatRelative(iso)}
                </span>
                <span className="text-faint"> · </span>
                {a.remove ? t("not you? remove it, then make new recovery words") : t("not you? check this account's devices")}
              </span>
            </span>
            <span className="flex shrink-0 flex-wrap items-center gap-2">
              <Button variant="secondary" disabled={busy !== null} onClick={() => account.acknowledge(a.seq)}>
                {t("that was me")}
              </Button>
              {a.remove && (
                <Button
                  variant="danger"
                  disabled={busy !== null || blockedByHold(account, a.remove)}
                  title={blockedByHold(account, a.remove) ? t("approve or remove the device whose activation is held back first") : undefined}
                  onClick={async () => {
                    setBusy(a.seq);
                    setError(null);
                    try {
                      await account.removeDevice(a.remove!, shellCourier(link, account), true);
                      account.acknowledge(a.seq);
                    } catch (err) {
                      setError(errorText(err));
                    } finally {
                      setBusy(null);
                    }
                  }}
                >
                  {busy === a.seq && <Loader2 className="animate-spin" />}
                  {t("remove {{id}}", { id: a.remove })}
                </Button>
              )}
            </span>
          </div>
        );
      })}
      {error && (
        <div className="px-4 pb-2.5">
          <ErrorLine message={error} />
        </div>
      )}
    </div>
  );
}

/** What approving a held-back activation takes in: the hub's entries from it on. */
function HeldChanges({ account }: { account: CloudAccount }) {
  const { t } = useTranslation();
  const changes = account.heldChanges().filter((c) => !((c.kind === "activateRecovered" || c.kind === "passkeyActivate") && c.subject === account.heldBack?.device));
  if (!changes.length) return null;
  const say = (c: HeldChange): string => {
    const by = c.by ?? t("the recovery words");
    const subject = c.subject ?? "";
    switch (c.kind) {
      case "addDevice":
        return t("{{by}} added {{subject}}", { by, subject });
      case "removeDevice":
        return t("{{by}} removed {{subject}}", { by, subject });
      case "approveDevice":
        return t("{{by}} approved {{subject}}", { by, subject });
      case "activateRecovered":
      case "passkeyActivate":
        return t("{{subject}} activated itself", { subject });
      case "recoveryJoin":
        return t("{{subject}} joined with the recovery words", { subject });
      case "addMachine":
        return t("{{by}} added {{subject}} (can run AI)", { by, subject });
      case "removeMachine":
        return t("{{by}} removed {{subject}} (can run AI)", { by, subject });
      case "setRecoveryKey":
        return t("{{by}} changed the recovery words", { by });
      case "keyEpochs":
        return t("{{by}} made new keys", { by });
      default:
        return c.kind;
    }
  };
  return (
    <span data-testid="held-changes" className="mt-1 block text-2xs text-muted">
      {t("After it, the hub's chain has:")}{" "}
      {changes.map((c, i) => (
        <span key={c.seq} className="font-mono text-fg">
          {i ? "; " : ""}
          {say(c)}
        </span>
      ))}
      {". "}
      {t("Approving takes all of it in; removing takes out the device and what it brought in.")}
    </span>
  );
}

/** One device of a veto's list: its id, who brought it in, and whether it runs AI. */
function VetoTargetLine({ target, held }: { target: VetoTarget; held: string }) {
  const { t } = useTranslation();
  const how = target.id === held ? null : target.by ? t("brought in by {{by}}", { by: target.by }) : t("came in with the recovery words");
  return (
    <li data-testid={`veto-target-${target.id}`}>
      <span className="font-mono text-fg">{target.id}</span>
      {how && <span className="text-muted"> · {how}</span>}
      {target.host && <span className="text-muted"> · {t("can run AI")}</span>}
    </li>
  );
}

/**
 * The veto's confirmation, drawn from its plan (`CloudAccount.vetoPlan`): what
 * it takes out, what that does to the recovery words, and — when the devices
 * this browser trusted added any while it held the activation back — the
 * person's choice to take those out too (issue #17; off by default).
 */
export function VetoChoice({
  plan,
  alsoTrusted,
  onAlsoTrusted,
  busy,
  onConfirm,
  onCancel,
}: {
  plan: VetoPlan;
  alsoTrusted: boolean;
  onAlsoTrusted: (on: boolean) => void;
  busy: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const { t } = useTranslation();
  const trustedHosts = plan.trusted.some((d) => d.host);
  return (
    <div data-testid="veto-confirm" className="flex w-full flex-col gap-1.5 border border-border bg-bg p-2.5 text-xs">
      <span className="font-mono text-danger">{t("remove {{id}}?", { id: plan.device })}</span>
      <span className="text-muted">{t("This removes:")}</span>
      <ul data-testid="veto-scope" className="flex flex-col gap-0.5 pl-3">
        {plan.scope.map((d) => (
          <VetoTargetLine key={d.id} target={d} held={plan.device} />
        ))}
      </ul>
      {plan.voidsWords && (
        <p data-testid="veto-voids-words" className="text-fg">
          {t("The account's recovery words stop working with this: a device it removes set them, or came back with them. Make new recovery words afterwards.")}
        </p>
      )}
      {plan.voidsPasskeys && (
        <p data-testid="veto-voids-passkeys" className="text-fg">
          {t("Joining with the passkeys they set up, or came in with, is turned off too.")}
        </p>
      )}
      {plan.trusted.length > 0 && (
        <div className="flex flex-col gap-1 border-t border-border pt-1.5">
          <label className={cn(CHECKBOX_LABEL, "text-xs text-fg", busy && "cursor-not-allowed opacity-60")}>
            <input
              type="checkbox"
              className={CHECKBOX_INPUT}
              data-testid="veto-also-trusted"
              checked={alsoTrusted}
              disabled={busy}
              onChange={(e) => onAlsoTrusted(e.target.checked)}
            />
            <CheckboxMark />
            <span className="min-w-0">
              {t("Also remove what your other devices added while this browser held it back, and what those added:")}
            </span>
          </label>
          <ul data-testid="veto-trusted" className="flex flex-col gap-0.5 pl-8">
            {plan.trusted.map((d) => (
              <VetoTargetLine key={d.id} target={d} held={plan.device} />
            ))}
          </ul>
          <p className="pl-6 text-2xs leading-4 text-muted">
            {trustedHosts
              ? t("Leave this off if you added them yourself. Turn it on if you think one of your other devices was misused too: they lose access at once, and one that runs AI has to be added again.")
              : t("Leave this off if you added them yourself. Turn it on if you think one of your other devices was misused too: they lose access at once.")}
          </p>
          {plan.trustedVoidsWords && (
            <p data-testid="veto-trusted-voids-words" className="pl-6 text-2xs leading-4 text-fg">
              {t("Removing them also voids the account's recovery words: one of them set the current ones, or came back with them. Make new recovery words afterwards.")}
            </p>
          )}
          {plan.trustedVoidsPasskeys && (
            <p data-testid="veto-trusted-voids-passkeys" className="pl-6 text-2xs leading-4 text-fg">
              {t("Removing them also turns off joining with the passkeys they set up, or came in with.")}
            </p>
          )}
        </div>
      )}
      <div className="flex flex-wrap gap-2">
        <button type="button" data-testid="veto-go" className={cn(danger, "h-7 px-3 text-xs")} disabled={busy} onClick={onConfirm}>
          {busy && <Loader2 className="size-3 animate-spin" />}
          {alsoTrusted && plan.trusted.length > 0 ? t("remove all of them") : t("remove it and what it brought in")}
        </button>
        <button type="button" className={cn(quiet, "h-7 px-2 text-xs")} disabled={busy} onClick={onCancel}>
          {t("cancel")}
        </button>
      </div>
    </div>
  );
}

/** The list a veto's wider choice is made on: the ids shown, in order. */
export function trustedShown(plan: VetoPlan | null): string {
  return plan?.trusted.length ? JSON.stringify(plan.trusted.map((d) => d.id)) : "";
}

/** Whether the wider choice stands for `plan`: ticked on this very list. One that changed since (the plan read again) is unticked. */
export function alsoTrustedFor(ticked: string | null, plan: VetoPlan | null): boolean {
  const shown = trustedShown(plan);
  return ticked !== null && shown !== "" && ticked === shown;
}

/** The veto of a held-back activation, once its plan is read off the hub's chain; then what the person chose is what is signed. */
function VetoConfirm({ account, link, onCancel }: { account: CloudAccount; link: HubLink; onCancel: () => void }) {
  const { t } = useTranslation();
  const version = useAccountVersion(account);
  const [plan, setPlan] = useState<VetoPlan | null>(null);
  // The choice is bound to the very list it was made on: a list that grows (the plan read again) is unticked.
  const [ticked, setTicked] = useState<string | null>(null);
  const shown = trustedShown(plan);
  const alsoTrusted = alsoTrustedFor(ticked, plan);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Read again as the hub's chain moves: the list shown is the list signed. (None once the hold is over: the banner goes then.)
  useEffect(() => {
    let live = true;
    account.vetoPlan().then(
      (p) => {
        if (live && p) setPlan(p);
      },
      (err: unknown) => {
        if (live) setError(errorText(err));
      },
    );
    return () => {
      live = false;
    };
  }, [account, version]);
  if (!plan) {
    return (
      <div data-testid="veto-confirm" className="flex w-full flex-col gap-1 text-xs text-muted">
        {!error && (
          <span className="flex items-center gap-1.5">
            <Loader2 className="size-3 animate-spin" />
            {t("reading what it takes out…")}
          </span>
        )}
        <ErrorLine message={error} />
      </div>
    );
  }
  const confirm = async () => {
    setBusy(true);
    setError(null);
    try {
      // Always as the veto confirmed: even if the hold ended just before the click, it takes out what the device brought in.
      await account.removeDevice(plan.device, shellCourier(link, account), false, { veto: plan, ...(alsoTrusted ? { alsoTrusted: plan.trusted.map((d) => d.id) } : {}) });
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="flex w-full flex-col gap-1">
      <VetoChoice plan={plan} alsoTrusted={alsoTrusted} onAlsoTrusted={(on) => setTicked(on ? shown : null)} busy={busy} onConfirm={() => void confirm()} onCancel={onCancel} />
      <ErrorLine message={error} />
    </div>
  );
}

/**
 * While someone is coming back with the recovery words, every full device
 * says so — with the means to approve it, or veto it (remove it, which also
 * retires the keys it opened). A veto of an activation held back here lists
 * what it takes out before it is signed (`VetoConfirm`).
 */
export function PendingRecoveryBanner({ account, link }: { account: CloudAccount; link: HubLink }) {
  const { t } = useTranslation();
  useAccountVersion(account);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [vetoing, setVetoing] = useState<string | null>(null);
  const pending = account.standing === "active" ? (account.chain?.pendingRecoveries() ?? []) : [];
  if (!pending.length) return null;
  return (
    <div role="alert" data-testid="pending-recovery" data-signal="alarm" className="flex flex-col gap-1.5 border-b border-border bg-danger/4 px-4 py-2.5 shadow-[inset_2px_0_0_var(--color-danger)]">
      {pending.map((p) => (
        <div key={p.id} className="flex flex-wrap items-center gap-2 text-xs">
          <AlertTriangle className="size-3.5 shrink-0 text-danger" />
          <span className="min-w-0 flex-1 text-muted">
            {t("A device is coming back into this account with the recovery words:")} <span className="font-mono text-fg">{p.id}</span>.{" "}
            {account.heldBack?.device === p.id
              ? t("The hub says it already activated itself, but this browser only just saw it come back and holds it back until {{time}}. If that was not you, remove it now; otherwise approve it.", {
                  time: formatExactTime(new Date(account.heldBack.readyMs).toISOString()),
                })
              : t("If that was not you, remove it now; otherwise approve it (or it activates itself after 24 hours).")}
            {account.heldBack?.device === p.id && <HeldChanges account={account} />}
          </span>
          {vetoing === p.id && account.heldBack?.device === p.id ? (
            <VetoConfirm account={account} link={link} onCancel={() => setVetoing(null)} />
          ) : (
            <>
          <button
            type="button"
            className={cn(primary, "h-7 px-3 text-xs")}
            disabled={busy !== null}
            onClick={async () => {
              setBusy(p.id);
              setError(null);
              try {
                await account.approveDevice(p.id);
              } catch (err) {
                setError(errorText(err));
              } finally {
                setBusy(null);
              }
            }}
          >
            {t("approve")}
          </button>
          <button
            type="button"
            className={cn(danger, "h-7 px-3 text-xs")}
            disabled={busy !== null || blockedByHold(account, p.id)}
            title={blockedByHold(account, p.id) ? t("approve or remove the device whose activation is held back first") : undefined}
            onClick={async () => {
              // The veto of a held activation lists what it takes out first (issue #17).
              if (account.heldBack?.device === p.id) {
                setError(null);
                setVetoing(p.id);
                return;
              }
              setBusy(p.id);
              setError(null);
              try {
                await account.removeDevice(p.id, shellCourier(link, account));
              } catch (err) {
                setError(errorText(err));
              } finally {
                setBusy(null);
              }
            }}
          >
            {busy === p.id && <Loader2 className="size-3 animate-spin" />}
            {account.heldBack?.device === p.id ? t("remove it and what it brought in") : t("remove")}
          </button>
            </>
          )}
        </div>
      ))}
      <ErrorLine message={error} />
    </div>
  );
}
