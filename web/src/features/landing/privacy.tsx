import { useTranslation } from "react-i18next";
import { SIGNUP_CONFIG } from "@/lib/hub/api";
import { PLAN_TABLE } from "@/lib/hub/plan";
import "./copy";
import { LegalContact, LegalFootLink, LegalSection } from "./legal-kit";
import { DocumentHead, MailLink, PRIVACY_EMAIL, PublicLayout, usePageMeta } from "./page-kit";
import { SECURITY_PATH } from "./route";

/** When the notice last changed, as the page says it. */
export const PRIVACY_UPDATED = "2026-10-28";

/** How long droppen.app keeps a deleted account's address hash (droppen.app's `SIGNUP_CONFIG.deletion`). */
const DELETED_EMAIL_HOLD_DAYS = SIGNUP_CONFIG.deletion?.emailHoldDays ?? 0;

/** Within how many days copies in droppen.app's backups and logs expire (the owner's word, 2026-10-23). */
export const BACKUPS_EXPIRE_DAYS = 30;

/** How long an account that never verified its e-mail, and never added a device, lasts (the hub's sweep of unverified accounts). */
export const UNVERIFIED_ACCOUNT_DAYS = 7;

export default function PrivacyPage({ signedIn = false }: { signedIn?: boolean }) {
  const { t } = useTranslation();
  usePageMeta(t("Privacy notice — Droppen"));
  return (
    <PublicLayout testId="privacy" signedIn={signedIn}>
      <DocumentHead
        label={t("Privacy notice")}
        title={t("What we keep, and what we can't see.")}
        lead={t("Droppen is end-to-end encrypted, so there is little to tell: here is all of it.")}
        note={t("Last updated {{date}}", { date: PRIVACY_UPDATED })}
      />
      <LegalSection
        id="waitlist"
        label={t("Waitlist")}
        title={t("If you joined the waitlist")}
        left={{
          title: t("What we keep, and why"),
          items: [
            t("Your e-mail address, the plan you are waiting for (Free or Pro), the language of the page and when you joined."),
            t("We use them only to e-mail you an invite, or to tell you when sign-up or Pro opens. Nothing else is sent, and your address is never sold or used for marketing."),
            t("Not your IP address, nor anything else about your browser; our hosting provider's edge logs do record your IP address and User-Agent (see below). To stop abuse, our servers count requests per address and per network in memory, for a day at most."),
          ],
        }}
        right={{
          title: t("How long, and how to be taken off"),
          items: [
            t("Until you ask us to remove it, and never more than 12 months after you last joined: older entries are deleted automatically."),
            t("To be taken off, write to {{email}} from that address and we will delete it.", { email: PRIVACY_EMAIL }),
          ],
        }}
      />
      <LegalSection
        id="downloads"
        label={t("Downloads")}
        title={t("If you download Droppen")}
        left={{
          title: t("What we count"),
          items: [
            t("When you download Droppen from droppen.app, or run its install command, our servers add one to that day's count for that download: the macOS app, a Linux build or the install script."),
            t("That number is all we keep: one count per download per day, which says nothing about who downloaded."),
          ],
        }}
        right={{
          title: t("What we don't keep"),
          items: [
            t("Not your IP address, nor anything else about your browser, and no cookie. To keep one source from inflating the counts, our servers hold the address and the network a download came from in memory, for an hour at most, and write them nowhere."),
            t("Our hosting provider's edge logs do record the IP address and User-Agent of every request, downloads included (see below)."),
          ],
        }}
      />
      <LegalSection
        id="account"
        label={t("Your account")}
        title={t("If you use Droppen")}
        lead={t("Your sessions and terminals are encrypted on your devices, and only your devices hold the keys. Our servers pass and keep what they cannot read.")}
        left={{
          title: t("What our servers can see"),
          items: [
            t("Your e-mail address, and when you sign in"),
            t("Which of your devices are online, when they connect and from which IP address"),
            t("What kind of device each one is and which browser, as a short name such as “iPhone · Chrome” made from its User-Agent; our servers do not keep the User-Agent itself, though our hosting provider's edge logs do (see below)"),
            t("How much data moves and when. What we store is padded to fixed sizes; live connections are not, so while our servers carry one they see its timing, its volume and the rhythm of typing in a terminal: roughly when you are working"),
            t("Your plan, and how much of it you use"),
          ],
        }}
        right={{
          title: t("What they cannot see"),
          items: [
            t("What is said in a session, or typed and shown in a terminal"),
            t("Session titles, folders, repositories and branches"),
            t("Model names, which AI tool you use, your AI accounts and their quotas"),
            t("The names of your devices, your files and their names"),
          ],
        }}
      >
        <LegalFootLink path={SECURITY_PATH} testId="privacy-security">
          {t("How the encryption works, in detail")}
        </LegalFootLink>
      </LegalSection>
      <LegalSection
        id="deletion"
        label={t("Deleting your account")}
        title={t("If you delete your account")}
        left={{
          title: t("What is deleted"),
          items: [
            t("You can delete your account yourself, in its settings. It is gone at once and for good: nobody, us included, can bring it back."),
            t("Deleted with it: your devices, the devices that run AI you added, the encrypted copies of your sessions and terminals, your keys, your plan, your password and passkeys, and the records of verification mail sent to your address."),
            t("Your address also leaves the waitlist, if it was on it."),
          ],
        }}
        right={{
          title: t("What we keep, and for how long"),
          items: [
            t("A keyed hash of your e-mail address, never the address itself, for {{days}} days, so the same inbox cannot sign up again straight away. How long is a setting of Droppen Hub: {{days}} days on droppen.app.", { days: DELETED_EMAIL_HOLD_DAYS }),
            t("For the same {{days}} days, your account's and your devices' random ids and your devices' public keys, which open nothing: our servers use them to turn the account's connections away, and to tell a device that comes back that the account is gone, so it forgets its keys.", { days: DELETED_EMAIL_HOLD_DAYS }),
            t("Deleting removes all this from our live systems at once. Copies in our backups and logs expire within {{days}} days.", { days: BACKUPS_EXPIRE_DAYS }),
          ],
        }}
      />
      <LegalSection
        id="processing"
        label={t("Providers and retention")}
        title={t("Who else handles it, and for how long")}
        lead={t("These services run parts of Droppen for us. Like our servers, none of them can read what is end-to-end encrypted.")}
        left={{
          title: t("Our providers"),
          items: [
            t("Railway, in Singapore, runs our servers. Its edge logs record the IP address and User-Agent of every request to droppen.app, and keep them for as long as Railway keeps its logs."),
            t("PlanetScale runs our database (Postgres), in Singapore."),
            t("Tigris stores the encrypted pages of your sessions."),
            t("Cloudflare runs our DNS and routes mail to our addresses. It also runs one of the STUN servers for direct connections (Google runs the other): a STUN server sees your device's public IP address."),
            t("An e-mail relay service we operate sends our e-mail, such as the verification mail, to your address."),
          ],
        }}
        right={{
          title: t("How long we keep it"),
          items: [
            t("The encrypted copies of your sessions: the last {{free}} days on Free, {{pro}} on Pro, and {{internal}} on the internal plan, which is not for sale. A device offline for longer than that has its copy deleted.", {
              free: PLAN_TABLE.free!.retentionDays,
              pro: PLAN_TABLE.pro!.retentionDays,
              internal: PLAN_TABLE.internal!.retentionDays,
            }),
            t("An account whose e-mail address was never verified, and that never added a device, is deleted after {{days}} days.", { days: UNVERIFIED_ACCOUNT_DAYS }),
          ],
        }}
      />
      <LegalContact label={t("Contact")}>
        <p className="text-sm leading-6 text-muted">
          {t("Questions, or a request about your data:")} <MailLink email={PRIVACY_EMAIL} testId="privacy-email" />
        </p>
      </LegalContact>
    </PublicLayout>
  );
}
