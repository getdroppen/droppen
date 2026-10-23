import { type ReactNode, useEffect } from "react";
import i18n from "i18next";
import { useTranslation } from "react-i18next";
import { CapsLabel } from "@/components/ui/caps-label";
import { landingBundles } from "../copy";
import { FooterLine, Points, PublicHeader, SECURITY_EMAIL, Section, SectionHead } from "../page-kit";
import { gb, PLAN_NUMBERS } from "../pricing-plans";
import { PRICING_PATH, PRIVACY_PATH, publicLink } from "../route";
import { PRIVACY_EMAIL } from "../waitlist";
import { termsBundles } from "./terms-copy";

/*
 * droppen.app/terms: the terms of service of the hosted service, in plain
 * words — what it is and what the terms cover (droppen.app, not the code,
 * which its licences govern), the account, the reader's own devices and AI
 * tools, fair use, the plans, availability and liability, leaving, and
 * changes. Every fact is the product's own: end-to-end encryption and what
 * our servers see (the privacy notice, `/security`), the plans' numbers
 * (`pricing-plans.ts`), the e-mail check (docs/hub-api/http.md, sign-up),
 * the identity renewal in testing (`/security` §06), abuse judged by volume
 * and behaviour (docs/cross-machine-architecture.md §6), security reports
 * (SECURITY.md), the licences (README, LICENSE). Signed in or not; drawn
 * like the privacy notice; its own lazy chunk with its own words
 * (`terms-copy.ts`), as `/security` has.
 *
 * The owner's facts (2026-10-23): Droppen operates droppen.app, the laws and
 * courts of Singapore, no postal address published, 16 and over. A fact still
 * to supply would be a placeholder `[IN CAPITALS]`, listed in
 * `web/owner-placeholders.txt`, which a release checks its build against.
 */

// Added, never overwriting: a key the app already translates keeps its wording.
for (const bundles of [landingBundles, termsBundles]) {
  for (const [lng, bundle] of Object.entries(bundles)) i18n.addResourceBundle(lng, "translation", bundle, true, false);
}

/** When the terms last changed, as the page says it. */
export const TERMS_UPDATED = "2026-10-23";

/** The minimum age to use droppen.app, in years (the owner's word). */
export const MINIMUM_AGE = 16;

function Block({ title, items }: { title: string; items: string[] }) {
  return (
    <div className="flex min-w-0 flex-col gap-3 px-6 py-6 max-md:px-4">
      <span className="text-[13px] leading-5 font-semibold text-fg">{title}</span>
      <Points items={items} />
    </div>
  );
}

/** Two blocks side by side (stacked below 768px), as on the privacy notice. */
function Pair({ left, right }: { left: ReactNode; right: ReactNode }) {
  return (
    <div className="grid border-t border-border md:grid-cols-2">
      {left}
      <div className="max-md:border-t max-md:border-border md:border-l md:border-border">{right}</div>
    </div>
  );
}

const linkClass = "text-fg underline underline-offset-4 hover:no-underline";

function useDocumentMeta() {
  const { t, i18n: active } = useTranslation();
  useEffect(() => {
    const before = document.title;
    document.title = t("Terms of Service — Droppen");
    document.documentElement.lang = active.language;
    return () => {
      document.title = before;
    };
  }, [t, active.language]);
}

/** droppen.app/terms, signed in or not. */
export default function TermsPage({ signedIn = false }: { signedIn?: boolean }) {
  const { t } = useTranslation();
  useDocumentMeta();
  const free = PLAN_NUMBERS.free;
  const mail = (email: string, testId: string) => (
    <a href={`mailto:${email}`} data-testid={testId} className={linkClass}>
      {email}
    </a>
  );
  return (
    <div data-testid="terms" className="h-full w-full overflow-y-auto bg-bg [scrollbar-gutter:stable]">
      <PublicHeader signedIn={signedIn} />
      <div className="md:px-8">
        <main className="mx-auto max-w-[1152px] md:border-x md:border-border">
          <div className="flex flex-col gap-5 px-10 pt-16 pb-10 max-lg:pt-12 max-md:px-4 max-md:pt-10 max-md:pb-8">
            <CapsLabel>{t("Terms of Service")}</CapsLabel>
            <h1 className="max-w-[18em] text-[44px] leading-[1.05] font-semibold tracking-[-0.035em] text-balance text-fg [word-break:auto-phrase] [&:lang(zh)]:[word-break:keep-all] max-md:text-[30px]">
              {t("What you can expect from us, and what we ask of you.")}
            </h1>
            <p className="max-w-[40em] text-[15px] leading-[26px] text-muted max-md:text-sm max-md:leading-6">
              {t("These terms are the agreement between you and Droppen (“we”), which operates droppen.app. Creating an account, or using the service, means you accept them.")}
            </p>
            <p className="font-mono text-2xs text-faint">{t("Last updated {{date}}", { date: TERMS_UPDATED })}</p>
          </div>
          <Section id="service">
            <SectionHead label={t("The service")} title={t("What these terms cover")} />
            <Pair
              left={
                <Block
                  title={t("What Droppen is")}
                  items={[
                    t("Droppen lets you work with the AI coding tools on your own devices from anywhere: from your phone, or from a browser on another device."),
                    t("Your sessions and terminals are end-to-end encrypted on your devices, and only your devices hold the keys. We cannot read them, so we cannot check, restore or hand over what is in them."),
                  ]}
                />
              }
              right={
                <Block
                  title={t("What they cover, and what they don't")}
                  items={[
                    t("These terms cover only the hosted service at droppen.app: your account, and our servers that connect your devices and keep their encrypted copies."),
                    t("The Droppen app and server are free software, licensed under the AGPL-3.0 (some of their libraries under MIT or Apache-2.0), with their source code at github.com/getdroppen/droppen. Those licences, not these terms, govern the code."),
                    t("Local only, on your own devices with no account, does not use our service at all."),
                  ]}
                />
              }
            />
          </Section>
          <Section id="account">
            <SectionHead label={t("Your account")} title={t("One person, one account")} />
            <Pair
              left={
                <Block
                  title={t("Signing up")}
                  items={[
                    t("You must be at least {{age}} years old to use droppen.app.", { age: MINIMUM_AGE }),
                    t("An account is for one person: don't share it with others."),
                    t("One account per person and per e-mail inbox: an alias or a +tag of the same inbox counts as the same one. Don't make more to get around a plan's limits."),
                    t("Use an e-mail address you read, and verify it when we ask. An account still unverified after 14 days cannot add devices until it is verified."),
                  ]}
                />
              }
              right={
                <Block
                  title={t("Keeping it safe")}
                  items={[
                    t("Keep your password, passkeys, recovery words and devices safe: anyone who can use one of your devices can act as you through it, on your devices that run AI too."),
                    t("Only your devices and your recovery words can decrypt your data. If you lose them all, nobody can decrypt it again, not even us."),
                  ]}
                />
              }
            />
          </Section>
          <Section id="devices">
            <SectionHead label={t("Your devices")} title={t("Your devices, your AI tools")} />
            <Pair
              left={
                <Block
                  title={t("You are responsible for")}
                  items={[
                    t("The devices you connect: connect only devices you own or are allowed to use, and keep them secure."),
                    t("The AI tools you run through Droppen, the accounts you use them with, and keeping to their providers' terms."),
                    t("What those tools do on your devices: the code they write and the commands they run. Check their work as you would anyone's."),
                  ]}
                />
              }
              right={
                <Block
                  title={t("What we don't do")}
                  items={[
                    t("We don't provide AI tools, models, or accounts with their providers: Droppen connects you to the ones on your devices."),
                    t("We don't back up your work. The encrypted copy in the cloud lets you read recent sessions while a device is off; a device off for longer than your plan allows has that copy deleted. Keep your own copies of what matters."),
                  ]}
                />
              }
            />
          </Section>
          <Section id="use">
            <SectionHead label={t("Acceptable use")} title={t("Use it fairly")} />
            <Pair
              left={
                <Block
                  title={t("Don't")}
                  items={[
                    t("Use Droppen for anything illegal, or to harm other people or their systems."),
                    t("Attack the service: get into accounts that are not yours, read or change other people's data, or overload our servers. No load or denial-of-service testing."),
                    t("Use our servers as a general-purpose tunnel, proxy or VPN, or as file storage: they are there for your Droppen sessions."),
                    t("Get around a plan's limits, for example with extra accounts or automated sign-ups."),
                  ]}
                />
              }
              right={
                <Block
                  title={t("Security research")}
                  items={[
                    t("Found a vulnerability? Report it privately: to {{email}}, or through GitHub's private vulnerability reporting at github.com/getdroppen/droppen.", { email: SECURITY_EMAIL }),
                    t("When you test against droppen.app, use only accounts you own, don't access or change anyone else's data, and don't slow the service down for others."),
                  ]}
                />
              }
            />
          </Section>
          <Section id="plans">
            <SectionHead label={t("Plans")} title={t("Plans, and how they may change")} />
            <Pair
              left={
                <Block
                  title={t("Today")}
                  items={[
                    t("The Free plan includes {{hosts}} devices that run AI, {{devices}} devices in all, the last {{days}} days readable while a device is off, and {{gb}} GB a month through our servers, then slower.", {
                      hosts: free.hosts,
                      devices: free.devices,
                      days: free.retentionDays,
                      gb: gb(free.transferBytes),
                    }),
                    t("Pro is not on sale yet. Nothing is charged unless you choose to pay, and you see the price and how billing works before you do."),
                  ]}
                />
              }
              right={
                <Block
                  title={t("Changes")}
                  items={[
                    t("We may change plans, their limits and their prices. The pricing page always shows the current ones."),
                    t("If a change takes away something your plan includes, we tell you by e-mail before it applies to you."),
                    t("A change of plan never deletes your account or its keys. On a smaller plan the cloud keeps fewer days of copies, and what goes beyond its limits is refused."),
                  ]}
                />
              }
            />
            <p className="border-t border-border px-6 py-4 text-xs leading-5 max-md:px-4">
              <a {...publicLink(PRICING_PATH)} data-testid="terms-pricing" className={linkClass}>
                {t("See the plans")}
              </a>
            </p>
          </Section>
          <Section id="availability">
            <SectionHead label={t("Availability")} title={t("In beta, provided as is")} />
            <Pair
              left={
                <Block
                  title={t("What to expect")}
                  items={[
                    t("Droppen is in beta. We work to keep it running well, but it is provided “as is” and “as available”: there is no service-level agreement, and no promise that it is always up, fast or free of bugs."),
                    t("Features marked as in testing, such as renewing the identity key of a device that runs AI, may change or be withdrawn."),
                    t("We may change or stop parts of the service. If we ever shut it down, we tell you by e-mail first, and Droppen on your own devices keeps working locally without us."),
                  ]}
                />
              }
              right={
                <Block
                  title={t("Liability")}
                  items={[
                    t("As far as the law allows, we are not liable for indirect or consequential losses, lost profits or lost data, or for what your devices and AI tools do."),
                    t("As far as the law allows, our total liability to you is limited to what you paid us for the service in the 12 months before the claim."),
                    t("Nothing in these terms limits a liability the law does not let us limit, or takes away your rights as a consumer."),
                  ]}
                />
              }
            />
          </Section>
          <Section id="ending">
            <SectionHead label={t("Ending")} title={t("Leaving, and deleting your account")} />
            <Pair
              left={
                <Block
                  title={t("If you leave")}
                  items={[
                    t("You can stop using droppen.app at any time, and delete your account yourself, in its settings: it is gone at once, with its devices and what our servers keep for it (the privacy notice says what stays for a short while)."),
                    t("Deleting the account does not touch your own devices: what is on them stays until you remove it."),
                  ]}
                />
              }
              right={
                <Block
                  title={t("If we suspend it")}
                  items={[
                    t("We may suspend or close an account that breaks these terms or puts the service or other people at risk. Where we can, we tell you first, say why, and give you a chance to put it right."),
                    t("We can't read your sessions, so we judge by what our servers can see: how much an account sends, when, and how."),
                  ]}
                />
              }
            />
          </Section>
          <Section id="legal">
            <SectionHead label={t("Legal")} title={t("Changes, the law, and who we are")} />
            <Pair
              left={
                <Block
                  title={t("Changes to these terms")}
                  items={[
                    t("We may update these terms. When a change matters, we tell you by e-mail before it takes effect, and the date at the top changes."),
                    t("If you keep using droppen.app after that, the new terms apply. If you don't accept them, you can delete your account."),
                  ]}
                />
              }
              right={
                <Block
                  title={t("Who we are")}
                  items={[
                    t("Droppen operates droppen.app and the service on it."),
                    t("These terms are governed by the laws of Singapore, and the courts of Singapore have jurisdiction over any dispute about them. If you are a consumer, you keep the protection of the law where you live."),
                  ]}
                />
              }
            />
          </Section>
          <Section id="contact">
            <div className="flex flex-col gap-3 px-6 py-8 max-md:px-4">
              <CapsLabel>{t("Contact")}</CapsLabel>
              <p className="text-sm leading-6 text-muted">
                {t("Questions about these terms, or a request about your account:")} {mail(PRIVACY_EMAIL, "terms-email")}
              </p>
              <p className="text-sm leading-6 text-muted">
                {t("Security issues:")} {mail(SECURITY_EMAIL, "terms-security-email")}
              </p>
              <p className="text-xs leading-5">
                <a {...publicLink(PRIVACY_PATH)} data-testid="terms-privacy" className={linkClass}>
                  {t("What we keep, and what we can't see: the privacy notice")}
                </a>
              </p>
            </div>
          </Section>
          <FooterLine />
        </main>
      </div>
    </div>
  );
}
