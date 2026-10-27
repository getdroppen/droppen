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
import type { CloudAccount } from "@/lib/hub/account";
import type { HubLink } from "@/lib/hub/link";
import { AccountNotices, ChainAlerts, PendingRecoveryBanner } from "./devices";
import { VerifyEmailBanner } from "./email-verify";
import { PendingPasskeyBanner } from "./passkey-join";
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
