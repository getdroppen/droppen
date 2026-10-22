import { useEffect } from "react";
import i18n from "i18next";
import { useTranslation } from "react-i18next";
import { CapsLabel } from "@/components/ui/caps-label";
import { landingBundles } from "./copy";
import { FooterLine, Points, PublicHeader, Section, SectionHead } from "./page-kit";
import { onPublicLink, SECURITY_PATH } from "./route";
import { PRIVACY_EMAIL } from "./waitlist";

/*
 * droppen.app/privacy: a short, honest notice — what the waitlist keeps, why,
 * for how long and how to be taken off it, and what Droppen's servers can and
 * cannot see of an account (docs/cross-machine-architecture.md §2, the
 * landing page's security section; the security design at `/security` says
 * how). Signed in or not; drawn like the other
 * public pages. The contact address is a placeholder for the user to confirm.
 */

// Added, never overwriting: a key the app already translates keeps its wording.
for (const [lng, bundle] of Object.entries(landingBundles)) i18n.addResourceBundle(lng, "translation", bundle, true, false);

/** When the notice last changed, as the page says it. */
export const PRIVACY_UPDATED = "2026-10-22";

function Block({ title, items }: { title: string; items: string[] }) {
  return (
    <div className="flex min-w-0 flex-col gap-3 px-6 py-6 max-md:px-4">
      <span className="text-[13px] leading-5 font-semibold text-fg">{title}</span>
      <Points items={items} />
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
              <a href={SECURITY_PATH} onClick={onPublicLink(SECURITY_PATH)} data-testid="privacy-security" className="text-fg underline underline-offset-4 hover:no-underline">
                {t("How the encryption works, in detail")}
              </a>
            </p>
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
