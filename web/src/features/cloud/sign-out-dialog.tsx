import { type ReactNode, useState } from "react";
import { useTranslation } from "react-i18next";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import type { CloudAccount, SelfRemoval } from "@/lib/hub/account";
import { hubShell } from "./hub-shell";
import { signOut, type StillListed } from "./sign-out";
import { danger, Note, secondary } from "./ui";

type T = (key: string, vars?: Record<string, unknown>) => string;

/**
 * What "sign out and remove this browser" does, in one line, by what this
 * device may do about the account's devices (`CloudAccount.selfRemoval`):
 * its keys go from this browser whatever it is; it takes itself off the
 * account's devices only where that takes nothing else with it.
 */
export function removeBrowserText(why: SelfRemoval, t: T): string {
  switch (why) {
    case "removable":
      return t("On a shared or borrowed device: this browser leaves your account's devices and its keys are deleted here. To use it again, add it as a new device.");
    case "notAdmin":
      return t("On a shared or borrowed device: its keys are deleted here. It cannot take itself off your account's devices (only a full device can): remove it in my devices on a full device too.");
    case "lastAdmin":
      return t("On a shared or borrowed device: its keys are deleted here, but it stays on your account's devices, as the account's last full device. Without its keys, only your recovery words bring back a full device.");
    case "keepsSignIn":
      return t("On a shared or borrowed device: its keys are deleted here, but it stays on your account's devices: taking it off would also void the recovery words or passkeys it set up. Remove it in my devices on another full device.");
    case "heldBack":
      return t("On a shared or borrowed device: its keys are deleted here, but it stays on your account's devices while a device's activation is held back. Remove it in my devices on another full device.");
    case "notActive":
      return t("On a shared or borrowed device: its keys are deleted here.");
  }
}

/**
 * The sign-out question: keep this browser as a device of the account (a
 * device of one's own), or remove it (a shared or borrowed one). Both are
 * always offered; the second says what it can and cannot do here.
 */
export function SignOutDialog({ account, onOpenChange }: { account?: CloudAccount; onOpenChange: (open: boolean) => void }) {
  const { t } = useTranslation();
  const [busy, setBusy] = useState<"keep" | "remove" | null>(null);
  const who = account ?? hubShell()?.account;
  const why: SelfRemoval = typeof who?.selfRemoval === "function" ? who.selfRemoval() : "notActive";
  const go = (removeBrowser: boolean) => {
    setBusy(removeBrowser ? "remove" : "keep");
    void signOut(account, undefined, { removeBrowser });
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
            <button type="button" data-testid="sign-out-remove" className={danger} disabled={busy !== null} onClick={() => go(true)}>
              {busy === "remove" ? t("removing this browser…") : t("sign out and remove this browser")}
            </button>
            <p data-testid="sign-out-remove-why" data-why={why} className="text-2xs leading-4 text-muted">
              {removeBrowserText(why, t)}
            </p>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}

/**
 * A sign-out that asks which one first: `children` draws the button, given
 * what opens the question. `account`: the page's account where no hub shell
 * holds one (a screen before the shell).
 */
export function SignOutChoice({ account, children }: { account?: CloudAccount; children: (ask: () => void) => ReactNode }) {
  const [asking, setAsking] = useState(false);
  return (
    <>
      {children(() => setAsking(true))}
      {asking && <SignOutDialog account={account} onOpenChange={setAsking} />}
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
