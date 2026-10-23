import { Fragment, type ReactNode, useEffect } from "react";
import i18n from "i18next";
import { useTranslation } from "react-i18next";
import { CapsLabel } from "@/components/ui/caps-label";
import { landingBundles } from "./copy";
import { FooterLine, Points, PublicHeader, Section, SectionHead } from "./page-kit";
import { publicLink, SECURITY_PATH } from "./route";
import { PRIVACY_EMAIL } from "./waitlist";

/*
 * droppen.app/privacy: a short, honest notice — what the waitlist keeps, why,
 * for how long and how to be taken off it, what Droppen's servers can and
 * cannot see of an account (docs/cross-machine-architecture.md §2, the
 * landing page's security section; the security design at `/security` says
 * how), and what deleting an account erases and keeps (docs/hub-api/http.md
 * `DELETE /v1/account`). Signed in or not; drawn like the other
 * public pages. The contact address is a placeholder for the user to confirm.
 */

// Added, never overwriting: a key the app already translates keeps its wording.
for (const [lng, bundle] of Object.entries(landingBundles)) i18n.addResourceBundle(lng, "translation", bundle, true, false);

/** When the notice last changed, as the page says it. */
export const PRIVACY_UPDATED = "2026-10-23";

/** How long droppen.app keeps a deleted account's address hash (a setting of Droppen Hub; `GET /v1/signup/config` `deletion`). */
export const DELETED_EMAIL_HOLD_DAYS = 7;

/**
 * How long copies in droppen.app's backups and logs live before they expire,
 * as the page says it (e.g. "30 days"). Only the owner can say: until then it
 * is a placeholder in capitals and brackets, which the page shows marked
 * ({@link marked}), listed in `web/owner-placeholders.txt`: a release refuses
 * a web build that still holds one (`scripts/release-text-check.sh`), so the
 * notice cannot ship unfilled by accident.
 */
export const BACKUPS_EXPIRE_AFTER: string = "[BACKUP RETENTION]";

/** A value still to be supplied by the owner: `[IN CAPITALS]`. */
export const PLACEHOLDER = /\[[A-Z][A-Z ]*\]/;

/** `text` with every placeholder in it marked, so an unfilled notice shows it. */
export function marked(text: string): ReactNode {
  const parts = text.split(new RegExp(`(${PLACEHOLDER.source})`));
  if (parts.length === 1) return text;
  return parts.map((part, i) =>
    i % 2 === 1 ? (
      <mark key={i} data-placeholder="" className="bg-warn/25 px-0.5 font-mono text-warn-fg">
        {part}
      </mark>
    ) : (
      <Fragment key={i}>{part}</Fragment>
    ),
  );
}

function Block({ title, items }: { title: string; items: string[] }) {
  return (
    <div className="flex min-w-0 flex-col gap-3 px-6 py-6 max-md:px-4">
      <span className="text-[13px] leading-5 font-semibold text-fg">{title}</span>
      <Points items={items} render={marked} />
    </div>
  );
}

function useDocumentMeta() {
  const { t, i18n: active } = useTranslation();
  useEffect(() => {
    const before = document.title;
    document.title = t("Privacy notice — Droppen");
    document.documentElement.lang = active.language;
    return () => {
      document.title = before;
    };
  }, [t, active.language]);
}

/** droppen.app/privacy, signed in or not. */
export default function PrivacyPage({ signedIn = false }: { signedIn?: boolean }) {
  const { t } = useTranslation();
  useDocumentMeta();
  const mail = (
    <a href={`mailto:${PRIVACY_EMAIL}`} data-testid="privacy-email" className="text-fg underline underline-offset-4 hover:no-underline">
      {PRIVACY_EMAIL}
    </a>
  );
  return (
    <div data-testid="privacy" className="h-full w-full overflow-y-auto bg-bg [scrollbar-gutter:stable]">
      <PublicHeader signedIn={signedIn} />
      <div className="md:px-8">
        <main className="mx-auto max-w-[1152px] md:border-x md:border-border">
          <div className="flex flex-col gap-5 px-10 pt-16 pb-10 max-lg:pt-12 max-md:px-4 max-md:pt-10 max-md:pb-8">
            <CapsLabel>{t("Privacy notice")}</CapsLabel>
            <h1 className="max-w-[18em] text-[44px] leading-[1.05] font-semibold tracking-[-0.035em] text-balance text-fg [word-break:auto-phrase] [&:lang(zh)]:[word-break:keep-all] max-md:text-[30px]">
              {t("What we keep, and what we can't see.")}
            </h1>
            <p className="max-w-[40em] text-[15px] leading-[26px] text-muted max-md:text-sm max-md:leading-6">
              {t("Droppen is end-to-end encrypted, so there is little to tell: here is all of it.")}
            </p>
            <p className="font-mono text-2xs text-faint">{t("Last updated {{date}}", { date: PRIVACY_UPDATED })}</p>
          </div>
          <Section id="waitlist">
            <SectionHead label={t("Waitlist")} title={t("If you joined the waitlist")} />
            <div className="grid border-t border-border md:grid-cols-2">
              <Block
                title={t("What we keep, and why")}
                items={[
                  t("Your e-mail address, the plan you chose (Free or Pro), the language of the page and when you joined."),
                  t("We use them only to e-mail you an invite, or to tell you when Pro opens. Nothing else is sent, and your address is never sold or used for marketing."),
                  t("Not your IP address, nor anything else about your browser. To stop abuse, our servers count requests per network in memory, for a day at most."),
                ]}
              />
              <div className="max-md:border-t max-md:border-border md:border-l md:border-border">
                <Block
                  title={t("How long, and how to be taken off")}
                  items={[
                    t("Until you ask us to remove it, and never more than 12 months after you last joined: older entries are deleted automatically."),
                    t("To be taken off, write to {{email}} from that address and we will delete it.", { email: PRIVACY_EMAIL }),
                  ]}
                />
              </div>
            </div>
          </Section>
          <Section id="account">
            <SectionHead
              label={t("Your account")}
              title={t("If you use Droppen")}
              lead={t("Your sessions and terminals are encrypted on your devices, and only your devices hold the keys. Our servers pass and keep what they cannot read.")}
            />
            <div className="grid border-t border-border md:grid-cols-2">
              <Block
                title={t("What our servers can see")}
                items={[
                  t("Your e-mail address, and when you sign in"),
                  t("Which of your devices are online, when they connect and from which IP address"),
                  t("What kind of device each one is and which browser, as a short name such as “iPhone · Chrome” made from its User-Agent; it does not keep the User-Agent itself"),
                  t("How much data moves and when. What we store is padded to fixed sizes; live connections are not, so while our servers carry one they see its timing, its volume and the rhythm of typing in a terminal: roughly when you are working"),
                  t("Your plan, and how much of it you use"),
                ]}
              />
              <div className="max-md:border-t max-md:border-border md:border-l md:border-border">
                <Block
                  title={t("What they cannot see")}
                  items={[
                    t("What is said in a session, or typed and shown in a terminal"),
                    t("Session titles, folders, repositories and branches"),
                    t("Model names, which AI tool you use, your AI accounts and their quotas"),
                    t("The names of your devices, your files and their names"),
                  ]}
                />
              </div>
            </div>
            <p className="border-t border-border px-6 py-4 text-xs leading-5 max-md:px-4">
              <a {...publicLink(SECURITY_PATH)} data-testid="privacy-security" className="text-fg underline underline-offset-4 hover:no-underline">
                {t("How the encryption works, in detail")}
              </a>
            </p>
          </Section>
          <Section id="deletion">
            <SectionHead label={t("Deleting your account")} title={t("If you delete your account")} />
            <div className="grid border-t border-border md:grid-cols-2">
              <Block
                title={t("What is deleted")}
                items={[
                  t("You can delete your account yourself, in its settings. It is gone at once and for good: nobody, us included, can bring it back."),
                  t("Deleted with it: your devices, the devices that run AI you added, the encrypted copies of your sessions and terminals, your keys, your plan, your password and passkeys, and the records of verification mail sent to your address."),
                  t("Your address also leaves the waitlist, if it was on it."),
                ]}
              />
              <div className="max-md:border-t max-md:border-border md:border-l md:border-border">
                <Block
                  title={t("What we keep, and for how long")}
                  items={[
                    t("A keyed hash of your e-mail address, never the address itself, for {{days}} days, so the same inbox cannot sign up again straight away. How long is a setting of Droppen Hub: {{days}} days on droppen.app.", { days: DELETED_EMAIL_HOLD_DAYS }),
                    t("For the same {{days}} days, your account's and your devices' random ids and your devices' public keys, which open nothing: our servers use them to turn the account's connections away, and to tell a device that comes back that the account is gone, so it forgets its keys.", { days: DELETED_EMAIL_HOLD_DAYS }),
                    t("Deleting removes all this from our live systems at once. Copies in our backups and logs expire after {{period}}.", { period: BACKUPS_EXPIRE_AFTER }),
                  ]}
                />
              </div>
            </div>
          </Section>
          <Section id="contact">
            <div className="flex flex-col gap-3 px-6 py-8 max-md:px-4">
              <CapsLabel>{t("Contact")}</CapsLabel>
              <p className="text-sm leading-6 text-muted">
                {t("Questions, or a request about your data:")} {mail}
              </p>
            </div>
          </Section>
          <FooterLine />
        </main>
      </div>
    </div>
  );
}
