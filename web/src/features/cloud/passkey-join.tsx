import { useState } from "react";
import { useTranslation } from "react-i18next";
import { CircleNotchIcon, FingerprintIcon, WarningIcon } from "@phosphor-icons/react";
import { formatMoment } from "@/lib/format";
import { hex } from "@/lib/e2e";
import type { CloudAccount } from "@/lib/hub/account";
import type { HubLink } from "@/lib/hub/link";
import { pageCredentials, passkeyCancelled } from "@/lib/hub/passkey";
import { joinAccountWithPasskey, passkeySignIn } from "@/lib/hub/passkey-join";
import { cn } from "@/lib/cn";
import { AddDevice, useAccountVersion } from "./devices";
import { shellCourier } from "./hub-shell";
import { danger, ErrorLine, errorText, Note, primary, quiet } from "./ui";

/*
 * Joining the account with a passkey (docs/passkeys.md §4.6), as a person
 * meets it:
 *
 * - on a new browser, signed in but not in the chain, the pairing screen's
 *   first tab (`PasskeyJoin`) when the account lets new devices join with a
 *   passkey: one more Face ID / Touch ID, and this browser asks to join;
 * - it then waits (`PasskeyPendingScreen`, in `pairing-screen.tsx`), showing the ordinary pairing
 *   offer — an 8-digit code, or the QR code — made of its own keys; it reads
 *   nothing and controls nothing meanwhile;
 * - every full device of the account says so at the top of the page
 *   (`PendingPasskeyBanner`): "approve" opens the ordinary pairing there
 *   (type the code, scan the QR code), and the device is approved only if
 *   the keys that pairing checked are the ones it joined with
 *   (`CloudAccount.pairDevice`) — so the approval is bound to the screen the
 *   person holds, not to an id or a number someone could match; "not me"
 *   removes it (the veto), and says what that means for the passkey (§4.6
 *   5c): whoever holds its sync account can still sign in with it — the way
 *   to remove it is offered at once. Nothing activates by itself: the slots
 *   this page makes let none (`selfActivate: false`).
 */

/** Whether this browser can join `account` with a passkey: WebAuthn here, and a slot on the chain. */
export function canJoinWithPasskey(account: CloudAccount): boolean {
  return pageCredentials() !== null && account.passkeySlots().length > 0;
}

/** The pairing screen's passkey tab: ask to join with the passkey this person signed in with (or picks). */
export function PasskeyJoin({ account, onPending }: { account: CloudAccount; onPending: () => void }) {
  const { t } = useTranslation();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const join = async () => {
    const credentials = pageCredentials();
    if (!credentials || busy) return;
    setBusy(true);
    setError(null);
    setNote(null);
    try {
      await joinAccountWithPasskey(account, { credentials }, { signIn: passkeySignIn(account.account) });
      onPending();
    } catch (err) {
      if (passkeyCancelled(err)) setNote(t("no passkey was used"));
      else setError(errorText(err));
      setBusy(false);
    }
  };
  return (
    <div data-testid="passkey-join" className="flex flex-col gap-3">
      <p className="text-xs leading-5 text-muted">
        {t("Use your passkey once more (Face ID, Touch ID or your device's PIN) and this browser asks to join the account.")}
      </p>
      <p className="text-xs leading-5 text-muted">
        {t("It then waits: it reads nothing and controls none of your devices until you approve it from another device of the account, with the code or QR code it shows. Any full device of the account can refuse it.")}
      </p>
      <button type="button" data-variant="primary" className={primary} disabled={busy} onClick={() => void join()}>
        {busy ? <CircleNotchIcon className="size-3.5 animate-spin" /> : <FingerprintIcon className="size-3.5" />}
        {t("join with a passkey")}
      </button>
      {note && <p className="text-2xs text-faint">{note}</p>}
      <ErrorLine message={error} />
    </div>
  );
}

/**
 * On every full device while a device that joined with a passkey waits: who,
 * when, "approve" — the ordinary pairing, opened here — and "not me" (remove
 * it, the veto, after the warning §4.6 5c asks for). A refused join leaves a
 * line here with the way to remove that passkey (`onRemovePasskey`, its
 * credential hash).
 */
export function PendingPasskeyBanner({ account, link, onRemovePasskey }: { account: CloudAccount; link: HubLink; onRemovePasskey: (credHash: string) => void }) {
  const { t } = useTranslation();
  useAccountVersion(account);
  // Refused here: the passkey each one joined with, which still signs in.
  const [refused, setRefused] = useState<{ id: string; credHash: string | null }[]>([]);
  const chain = account.chain;
  const pending = account.standing === "active" && account.isAdmin && chain ? chain.pendingPasskeyJoins() : [];
  if (!pending.length && !refused.length) return null;
  return (
    <div data-testid="pending-passkey-joins" data-signal="alarm" className="flex flex-col gap-2 border-b border-border bg-danger/4 px-4 py-2.5 shadow-[inset_2px_0_0_var(--color-danger)]">
      {pending.map((p) => (
        <PendingPasskeyJoin
          key={p.id}
          account={account}
          link={link}
          id={p.id}
          joinSeq={p.joinSeq}
          onRefused={(credHash) => setRefused((r) => (r.some((x) => x.id === p.id) ? r : [...r, { id: p.id, credHash }]))}
        />
      ))}
      {refused.map((r) => (
        <div key={r.id} data-testid="join-refused" className="flex flex-col gap-1.5 text-xs">
          <p role="status" className="flex items-start gap-2 text-muted">
            <WarningIcon className="mt-0.5 size-3.5 shrink-0 text-danger" />
            <span className="min-w-0 flex-1">
              <span className="font-mono whitespace-nowrap text-fg">{t("removed {{id}}", { id: r.id })}</span>.{" "}
              {t("The passkey it joined with still signs in to your account. Secure the account it syncs with, then remove that passkey here and make a new one.")}
            </span>
          </p>
          <div className="flex flex-wrap items-center gap-2 pl-5.5">
            {r.credHash && (
              <button type="button" data-testid="remove-that-passkey" className={cn(danger, "h-7 px-3 text-xs")} onClick={() => onRemovePasskey(r.credHash!)}>
                {t("remove that passkey")}
              </button>
            )}
            <button type="button" className={cn(quiet, "h-7 px-2 text-2xs")} onClick={() => setRefused((list) => list.filter((x) => x.id !== r.id))}>
              {t("done")}
            </button>
          </div>
        </div>
      ))}
    </div>
  );
}

/** What one waiting join shows: the line, then — as asked — the pairing that approves it, or the warning before "not me". */
function PendingPasskeyJoin({ account, link, id, joinSeq, onRefused }: { account: CloudAccount; link: HubLink; id: string; joinSeq: number; onRefused: (credHash: string | null) => void }) {
  const { t, i18n } = useTranslation();
  const [open, setOpen] = useState<"approve" | "refuse" | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const atMs = account.chain?.entryAt(joinSeq)?.draft.createdMs ?? null;
  const remove = async () => {
    if (busy) return;
    // The passkey it joined with, by the slot that let it in (the removal voids the slot; its credential hash stays on the chain).
    const slot = account.chain?.deviceRecord(id)?.viaPasskey ?? null;
    const named = slot ? account.chain?.allSlots().find((s) => s.slot.slot === slot) : undefined;
    const credHash = named ? hex(named.slot.credHash) : null;
    setBusy(true);
    setError(null);
    try {
      await account.removeDevice(id, shellCourier(link, account));
      onRefused(credHash);
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div data-testid="pending-passkey-join" className="flex flex-col gap-1.5 text-xs">
      <p role="status" className="flex items-start gap-2 text-muted">
        <WarningIcon className="mt-0.5 size-3.5 shrink-0 text-danger" />
        <span className="min-w-0 flex-1">
          {t("A new device is asking to join with a passkey:")} <span className="font-mono whitespace-nowrap text-fg">{id}</span>
          {atMs !== null && <span className="font-mono whitespace-nowrap text-faint"> · {formatMoment(atMs, i18n.language, true)}</span>}
          <br />
          {t("If it is yours, approve it with the code (or QR code) it shows. If not, remove it.")}
        </span>
      </p>
      {open === null && (
        <div className="flex flex-wrap items-center gap-2 pl-5.5">
          <button type="button" data-testid="approve-join" className={cn(primary, "h-7 px-3 text-xs")} onClick={() => setOpen("approve")}>
            {t("approve")}
          </button>
          <button type="button" data-testid="refuse-join" className={cn(danger, "h-7 px-3 text-xs")} onClick={() => setOpen("refuse")}>
            {t("not me: remove it")}
          </button>
        </div>
      )}
      {open === "approve" && (
        <div className="max-w-xl pl-5.5">
          <AddDevice account={account} link={link} initialMode="code" onDone={() => setOpen(null)} />
        </div>
      )}
      {open === "refuse" && (
        <div className="max-w-xl pl-5.5">
          <Note tone="danger">
            <p data-testid="refuse-warning" className="text-xs leading-5 text-muted">
              {t(
                "If this was not you, someone else can use that passkey — most likely through the account it syncs with (iCloud Keychain, Google Password Manager, 1Password…). Removing this device stops it and turns off joining with that passkey, but the passkey still signs in. Secure that account first, then remove the passkey and make a new one.",
              )}
            </p>
            <div className="flex flex-wrap gap-2">
              <button type="button" data-testid="refuse-confirm" className={cn(danger, "h-7 px-3 text-xs pointer-coarse:h-9")} disabled={busy} onClick={() => void remove()}>
                {busy && <CircleNotchIcon className="size-3 animate-spin" />}
                {t("remove {{id}}", { id })}
              </button>
              <button type="button" className={cn(quiet, "h-7 pointer-coarse:h-9")} disabled={busy} onClick={() => setOpen(null)}>
                {t("cancel")}
              </button>
            </div>
          </Note>
        </div>
      )}
      <div className="pl-5.5">
        <ErrorLine message={error} />
      </div>
    </div>
  );
}
