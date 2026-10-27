/**
 * droppen.app's 「我的设备」 (docs/local-aggregator.md §3.7): the one page for
 * every device of the account, at `/devices` (`devices-route.ts`), reached
 * from the bar's 「设备」 and shown whenever no device is in view.
 *
 * One page, top to bottom (`devices.tsx`'s `DevicesPanel`): what the plan
 * uses (the transfer this period, the cloud storage, the devices and those
 * that run AI), this device's own settings (direct connections, the relay),
 * then every device the chain names — browsers, phones and the devices that
 * run AI — as one list, each with badges for what it is, whether it is up,
 * and its actions in a 「⋯」 menu; a device that runs AI also says how many of
 * its sessions need you, with the way into it. Removed devices are folded at
 * the bottom.
 *
 * Nothing here dials a device: everything is read off what the page already
 * follows of each (its info, its index and status copies). The page around
 * it — the account (by its e-mail), its alerts and notices
 * (`AccountNotices`: a key rotation due, the upgrade, …), the title's unread
 * count, the way back to the device the app last showed — is drawn here.
 */
import { type ReactNode, useEffect } from "react";
import { useTranslation } from "react-i18next";
import { ArrowLeft, LayoutGrid, UserRound } from "lucide-react";
import { goPublic } from "@/features/landing/route";
import { cn } from "@/lib/cn";
import type { CloudAccount } from "@/lib/hub/account";
import type { HubLink } from "@/lib/hub/link";
import { useStore } from "@/lib/machines/store";
import { useDeviceSessions } from "@/features/machines/device-sessions";
import { unreadAcross } from "@/features/machines/needs-you";
import { UnreadSignals } from "@/features/machines/unread-signals";
import { ACCOUNT_PATH, askedPlace, askRemoval, DEVICES_PATH, isAccountPath } from "./devices-route";
import { AccountAlerts } from "./account-alerts";
import { DevicesPanel, useAccountVersion } from "./devices";
import { hubShell, useShellMachines } from "@/features/machines/shells";
import { useMe } from "./plan";
import { SignOutChoice } from "./sign-out-dialog";
import { CloudPage, quiet } from "./ui";

/**
 * Go to 「我的设备」 — scrolled to the element `anchor` names (the relay
 * picker: `relays`) — from wherever the page is. The device in view goes out
 * of view (`HubShell.select(null)`).
 */
export function openMyDevices(anchor?: string): void {
  if (anchor) askedPlace.set({ anchor });
  hubShell()?.select(null);
  // From the account's settings (no device in view there either): over to the list.
  if (typeof location !== "undefined" && isAccountPath(location.pathname)) goPublic(DEVICES_PATH);
}

/**
 * Go to the account's settings (`/account`) from wherever the page is: the
 * device in view goes out of view, as for 「我的设备」. From the app the
 * address is replaced (as 「设备」 replaces it), from 「我的设备」 a new entry
 * (back returns to the list).
 */
export function openAccount(): void {
  const shell = hubShell();
  const inApp = (shell?.focus.get() ?? null) !== null;
  shell?.select(null);
  goPublic(ACCOUNT_PATH, { replace: inApp });
}

/** Over to the account's settings, with the removal of the passkey whose credential hash is `credHash` open (a join with it was refused). */
export function removePasskeyThere(credHash: string): void {
  askRemoval(credHash);
  openAccount();
}

/** The bar's way to the account's settings: 「账户」. */
export function AccountButton() {
  const { t } = useTranslation();
  return (
    <button
      type="button"
      data-testid="account-settings"
      onClick={() => openAccount()}
      className="flex h-5 items-center gap-1 px-1.5 text-muted hover:bg-hover hover:text-fg"
      title={t("account settings")}
    >
      <UserRound className="size-3" />
      <span className="max-sm:hidden">{t("my account")}</span>
    </button>
  );
}

/** The bar's way to 「我的设备」: one entry, 「设备」. */
export function DevicesButton() {
  const { t } = useTranslation();
  return (
    <button
      type="button"
      data-testid="my-devices"
      onClick={() => openMyDevices()}
      className="flex h-5 items-center gap-1 px-1.5 text-muted hover:bg-hover hover:text-fg"
      title={t("my devices")}
    >
      <LayoutGrid className="size-3" />
      <span className="max-sm:hidden">{t("devices")}</span>
    </button>
  );
}

export interface MyDevicesProps {
  account: CloudAccount;
  link: HubLink;
}

export function MyDevicesScreen({ account, link }: MyDevicesProps) {
  useAccountVersion(account);
  useAskedPlace();
  return (
    <MyDevicesFrame account={account} link={link}>
      <DevicesPanel account={account} link={link} />
    </MyDevicesFrame>
  );
}

/** A part of the page a way in asked for, brought into view. */
function useAskedPlace(): void {
  const asked = useStore(askedPlace);
  useEffect(() => {
    if (!asked) return;
    askedPlace.set(null);
    scrollTo(asked.anchor);
  }, [asked]);
}

/** Bring the element `id` into view once it is drawn (the relay picker draws once it has the relays). */
function scrollTo(id: string): void {
  const until = Date.now() + 2_000;
  const look = () => {
    const el = document.getElementById(id);
    if (el) el.scrollIntoView({ block: "center" });
    else if (Date.now() < until) requestAnimationFrame(look);
  };
  requestAnimationFrame(look);
}

/** The account as a person knows it: its e-mail address once `/v1/me` says it, its id until then (and as the tooltip). */
export function AccountName({ account, link }: { account: CloudAccount; link: HubLink }) {
  const me = useMe(account, link);
  const email = me?.view?.email.address || null;
  return (
    <span data-testid="account-name" title={account.account} className="truncate font-mono text-xs text-faint max-md:hidden">
      {email ?? account.account}
    </span>
  );
}

/** The page around the list: the bar's account and way back, the account's alerts and notices. */
function MyDevicesFrame({ account, link, children }: { account: CloudAccount; link: HubLink; children: ReactNode }) {
  const { t } = useTranslation();
  // The tab and the badge: only devices that are up (`unreadAcross`), read off their copies.
  const lists = useDeviceSessions();
  const machines = useShellMachines();
  const shell = hubShell();
  const back = shell?.lastShown ?? null;
  const backName = back ? (machines.find((m) => m.key === back)?.name ?? back) : null;
  return (
    <CloudPage
      aside={
        <>
          <AccountName account={account} link={link} />
          {back && backName && (
            <button
              type="button"
              data-testid="my-devices-back"
              className={cn(quiet, "h-7 min-w-0 px-2")}
              title={t("back to {{name}}", { name: backName })}
              aria-label={t("back to {{name}}", { name: backName })}
              onClick={() => shell?.select(back)}
            >
              <ArrowLeft className="size-3.5 shrink-0" />
              <span className="max-w-40 truncate max-sm:hidden">{backName}</span>
            </button>
          )}
          <button
            type="button"
            data-testid="my-devices-account"
            className={cn(quiet, "h-7 px-2")}
            title={t("account settings")}
            aria-label={t("account settings")}
            onClick={() => openAccount()}
          >
            <UserRound className="size-3.5 shrink-0" />
            <span className="max-sm:hidden">{t("my account")}</span>
          </button>
          <SignOutChoice account={account}>
            {(ask) => (
              <button type="button" data-testid="my-devices-sign-out" className={cn(quiet, "h-7 px-2")} onClick={ask}>
                {t("sign out")}
              </button>
            )}
          </SignOutChoice>
        </>
      }
    >
      <UnreadSignals unread={unreadAcross(lists)} />
      <AccountAlerts account={account} link={link} where="devices" onRemovePasskey={removePasskeyThere} onReview={() => openAccount()} />
      {children}
    </CloudPage>
  );
}
