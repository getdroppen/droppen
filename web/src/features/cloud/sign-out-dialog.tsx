import { type ReactNode, useState } from "react";
import { useTranslation } from "react-i18next";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import type { CloudAccount, SelfRemoval } from "@/lib/hub/account";
import { hubShell } from "./hub-shell";
import { type HeldSession, signOut, type StillListed } from "./sign-out";
import { danger, Note, quiet, secondary } from "./ui";

type T = (key: string, vars?: Record<string, unknown>) => string;

/** What "sign out and remove this browser" can do here: what the device may do ({@link SelfRemoval}), or `unopened` (the page could not open the account). */
export type RemoveCase = SelfRemoval | "unopened";

/**
 * What "sign out and remove this browser" does, in one line, by what this
 * device may do about the account's devices (`CloudAccount.selfRemoval`):
 * its keys go from this browser whatever it is; it takes itself off the
 * account's devices only where that takes nothing else with it.
 * `recoveryWords`: whether the account has any (the chain's current key).
 */
export function removeBrowserText(why: RemoveCase, t: T, recoveryWords = true): string {
  switch (why) {
    case "removable":
      return t("On a shared or borrowed device: this browser leaves your account's devices and its keys are deleted here. To use it again, add it as a new device.");
    case "notAdmin":
      return t("On a shared or borrowed device: its keys are deleted here. It cannot take itself off your account's devices (only a full device can): remove it in my devices on a full device too.");
    case "lastAdmin":
      return recoveryWords
        ? t("On a shared or borrowed device: its keys are deleted here, but it stays on your account's devices, as the account's last full device. Without its keys, only your recovery words bring back a full device.")
        : t("On a shared or borrowed device: its keys are deleted here, but it stays on your account's devices, as the account's last full device. The account has no recovery words: without its keys, no device can add or remove devices, or become a full device, ever again.");
    case "keepsSignIn":
      return t("On a shared or borrowed device: its keys are deleted here, but it stays on your account's devices: taking it off would also void the recovery words or passkeys it set up. Remove it in my devices on another full device.");
    case "heldBack":
      return t("On a shared or borrowed device: its keys are deleted here, but it stays on your account's devices while a device's activation is held back. Remove it in my devices on another full device.");
    case "notActive":
      return t("On a shared or borrowed device: its keys are deleted here.");
    case "unopened":
      return t("On a shared or borrowed device: its keys are deleted here. The account could not be opened, so it stays on your account's devices: remove it in my devices on another device.");
  }
}

/**
 * The sign-out question: keep this browser as a device of the account (a
 * device of one's own), or remove it (a shared or borrowed one). Both are
 * always offered; the second says what it can and cannot do here, and for
 * the account's last full device asks a second time. `held`: a screen that
 * could not open the account — removing the browser there forgets its keys
 * as this browser noted them (`CloudAccount.wipeLocal`).
 */
export function SignOutDialog({ account, held, onOpenChange }: { account?: CloudAccount; held?: HeldSession; onOpenChange: (open: boolean) => void }) {
  const { t } = useTranslation();
  const [busy, setBusy] = useState<"keep" | "remove" | null>(null);
  const [confirming, setConfirming] = useState(false);
  const who = account ?? hubShell()?.account;
  const why: RemoveCase = typeof who?.selfRemoval === "function" ? who.selfRemoval() : held ? "unopened" : "notActive";
  const recoveryWords = !!who?.chain?.currentRecoveryKey;
  const go = (removeBrowser: boolean) => {
    setBusy(removeBrowser ? "remove" : "keep");
    void signOut(account, held, { removeBrowser });
  };
  return (
    <Dialog open onOpenChange={(open) => !busy && onOpenChange(open)}>
      <DialogContent title={t("sign out")} closeLabel={t("close")}>
        <div className="flex flex-col gap-3" data-testid="sign-out-choice">
          <p className="text-xs leading-5 text-muted">{t("Either way, this browser's offline copies are cleared and its notifications stop.")}</p>
          <div className="flex flex-col gap-1.5">
            <button type="button" data-testid="sign-out-keep" className={secondary} disabled={busy !== null} onClick={() => go(false)}>
              {busy === "keep" ? t("signing out…") : t("sign out")}
            </button>
            <p className="text-2xs leading-4 text-muted">
              {t("On a device of your own: this browser stays one of your account's devices, and signing in again opens it as before.")}
            </p>
          </div>
          <div className="flex flex-col gap-1.5">
            <button
              type="button"
              data-testid="sign-out-remove"
              className={danger}
              disabled={busy !== null || confirming}
              aria-expanded={why === "lastAdmin" ? confirming : undefined}
              onClick={() => (why === "lastAdmin" ? setConfirming(true) : go(true))}
            >
              {busy === "remove" ? t("removing this browser…") : t("sign out and remove this browser")}
            </button>
            <p data-testid="sign-out-remove-why" data-why={why} className="text-2xs leading-4 text-muted">
              {removeBrowserText(why, t, recoveryWords)}
            </p>
          </div>
          {confirming && (
            <div role="alertdialog" aria-label={t("sign out and remove this browser")} data-testid="sign-out-remove-confirm">
              <Note tone="danger">
                <p className="text-xs leading-5 text-danger">
                  {recoveryWords
                    ? t("This is your account's last full device. Once its keys are deleted, only your recovery words bring back a full device.")
                    : t("This is your account's last full device, and the account has no recovery words. Once its keys are deleted, no device can add or remove devices, or become a full device, ever again.")}
                </p>
                <div className="flex flex-wrap gap-2">
                  <button type="button" data-testid="sign-out-remove-anyway" className={danger} disabled={busy !== null} onClick={() => go(true)}>
                    {busy === "remove" ? t("removing this browser…") : t("delete its keys anyway")}
                  </button>
                  <button type="button" className={quiet} disabled={busy !== null} onClick={() => setConfirming(false)}>
                    {t("cancel")}
                  </button>
                </div>
              </Note>
            </div>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}

/**
 * A sign-out that asks which one first: `children` draws the button, given
 * what opens the question. `account`: the page's account where no hub shell
 * holds one (a screen before the shell); `held`: a screen that could not open
 * one.
 */
export function SignOutChoice({ account, held, children }: { account?: CloudAccount; held?: HeldSession; children: (ask: () => void) => ReactNode }) {
  const [asking, setAsking] = useState(false);
  return (
    <>
      {children(() => setAsking(true))}
      {asking && <SignOutDialog account={account} held={held} onOpenChange={setAsking} />}
    </>
  );
}

/** On the sign-in screen, after "sign out and remove this browser" left it on the account's devices: what to do about it. */
export function StillListedNote({ why }: { why: StillListed | null }) {
  const { t } = useTranslation();
  if (!why) return null;
  return (
    <Note tone="warn">
      <p data-testid="sign-out-still-listed" className="text-xs leading-5 text-fg">
        {t("This browser's keys were deleted, but it is still one of your account's devices. Remove it in my devices on another device.")}
      </p>
      {why.startsWith("failed: ") && <span className="font-mono text-2xs break-words text-muted">{why.slice("failed: ".length)}</span>}
    </Note>
  );
}
