import { useTranslation } from "react-i18next";
import "../copy";
import { LegalContact, LegalFootLink, LegalSection } from "../legal-kit";
import { DocumentHead, MailLink, PRIVACY_EMAIL, PublicLayout, SECURITY_EMAIL, TextLink, usePageMeta } from "../page-kit";
import { gb, PLAN_NUMBERS } from "../pricing-plans";
import { PRICING_PATH, PRIVACY_PATH, publicLink } from "../route";
import "./terms-copy";

// Owner-supplied legal facts: Singapore law, minimum age 16, no published postal address.
// Unsupplied facts remain capitalized placeholders checked against web/owner-placeholders.txt.

/** When the terms last changed, as the page says it. */
export const TERMS_UPDATED = "2026-10-28";

/** The minimum age to use droppen.app, in years (the owner's word). */
export const MINIMUM_AGE = 16;

export default function TermsPage({ signedIn = false }: { signedIn?: boolean }) {
  const { t } = useTranslation();
  usePageMeta(t("Terms of Service — Droppen"));
  const free = PLAN_NUMBERS.free;
  return (
    <PublicLayout testId="terms" signedIn={signedIn}>
      <DocumentHead
        label={t("Terms of Service")}
        title={t("What you can expect from us, and what we ask of you.")}
        lead={t("These terms are the agreement between you and Droppen (“we”), which operates droppen.app. Creating an account, or using the service, means you accept them.")}
        note={t("Last updated {{date}}", { date: TERMS_UPDATED })}
      />
      <LegalSection
        id="service"
        label={t("The service")}
        title={t("What these terms cover")}
        left={{
          title: t("What Droppen is"),
          items: [
            t("Droppen lets you work with the AI coding tools on your own devices from anywhere: from your phone, or from a browser on another device."),
            t("Your sessions and terminals are end-to-end encrypted on your devices, and only your devices hold the keys. We cannot read them, so we cannot check, restore or hand over what is in them."),
          ],
        }}
        right={{
          title: t("What they cover, and what they don't"),
          items: [
            t("These terms cover only the hosted service at droppen.app: your account, and our servers that connect your devices and keep their encrypted copies."),
            t("The Droppen app and server are free software, licensed under the AGPL-3.0 (some of their libraries under MIT or Apache-2.0), with their source code at github.com/getdroppen/droppen. Those licences, not these terms, govern the code."),
            t("Local only, on your own devices with no account, does not use our service at all."),
          ],
        }}
      />
      <LegalSection
        id="account"
        label={t("Your account")}
        title={t("One person, one account")}
        left={{
          title: t("Signing up"),
          items: [
            t("You must be at least {{age}} years old to use droppen.app.", { age: MINIMUM_AGE }),
            t("An account is for one person: don't share it with others."),
            t("One account per person and per e-mail inbox: an alias or a +tag of the same inbox counts as the same one. Don't make more to get around a plan's limits."),
            t("Use an e-mail address you read, and verify it when we ask. An account still unverified after 14 days cannot add devices until it is verified."),
          ],
        }}
        right={{
          title: t("Keeping it safe"),
          items: [
            t("Keep your password, passkeys, recovery words and devices safe: anyone who can use one of your devices can act as you through it, on your devices that run AI too."),
            t("Only your devices and your recovery words can decrypt your data. If you lose them all, nobody can decrypt it again, not even us."),
          ],
        }}
      />
      <LegalSection
        id="devices"
        label={t("Your devices")}
        title={t("Your devices, your AI tools")}
        left={{
          title: t("You are responsible for"),
          items: [
            t("The devices you connect: connect only devices you own or are allowed to use, and keep them secure."),
            t("The AI tools you run through Droppen, the accounts you use them with, and keeping to their providers' terms."),
            t("What those tools do on your devices: the code they write and the commands they run. Check their work as you would anyone's."),
          ],
        }}
        right={{
          title: t("What we don't do"),
          items: [
            t("We don't provide AI tools, models, or accounts with their providers: Droppen connects you to the ones on your devices."),
            t("We don't back up your work. The encrypted copy in the cloud lets you read recent sessions while a device is offline; a device offline for longer than your plan allows has that copy deleted. Keep your own copies of what matters."),
          ],
        }}
      />
      <LegalSection
        id="use"
        label={t("Acceptable use")}
        title={t("Use it fairly")}
        left={{
          title: t("Don't"),
          items: [
            t("Use Droppen for anything illegal, or to harm other people or their systems."),
            t("Attack the service: get into accounts that are not yours, read or change other people's data, or overload our servers. No load or denial-of-service testing."),
            t("Use our servers as a general-purpose tunnel, proxy or VPN, or as file storage: they are there for your Droppen sessions."),
            t("Get around a plan's limits, for example with extra accounts or automated sign-ups."),
          ],
        }}
        right={{
          title: t("Security research"),
          items: [
            t("Found a vulnerability? Report it privately: to {{email}}, or through GitHub's private vulnerability reporting at github.com/getdroppen/droppen.", { email: SECURITY_EMAIL }),
            t("When you test against droppen.app, use only accounts you own, don't access or change anyone else's data, and don't slow the service down for others."),
          ],
        }}
      />
      <LegalSection
        id="plans"
        label={t("Plans")}
        title={t("Plans, and how they may change")}
        left={{
          title: t("Today"),
          items: [
            t("The Free plan includes {{hosts}} devices that run AI, {{devices}} devices in all, the last {{days}} days readable while a device is offline, and {{gb}} GB a month through our servers, then slower.", {
              hosts: free.hosts,
              devices: free.devices,
              days: free.retentionDays,
              gb: gb(free.transferBytes),
            }),
            t("Pro is not on sale yet. Nothing is charged unless you choose to pay, and you see the price and how billing works before you do."),
          ],
        }}
        right={{
          title: t("Changes"),
          items: [
            t("We may change plans, their limits and their prices. The pricing page always shows the current ones."),
            t("If a change takes away something your plan includes, we tell you by e-mail before it applies to you."),
            t("A change of plan never deletes your account or its keys. On a smaller plan the cloud keeps fewer days of copies, and what goes beyond its limits is refused."),
          ],
        }}
      >
        <LegalFootLink path={PRICING_PATH} testId="terms-pricing">
          {t("See the plans")}
        </LegalFootLink>
      </LegalSection>
      <LegalSection
        id="availability"
        label={t("Availability")}
        title={t("In beta, provided as is")}
        left={{
          title: t("What to expect"),
          items: [
            t("Droppen is in beta. We work to keep it running well, but it is provided “as is” and “as available”: there is no service-level agreement, and no promise that it is always up, fast or free of bugs."),
            t("Features marked as in testing, such as renewing the identity key of a device that runs AI, may change or be withdrawn."),
            t("We may change or stop parts of the service. If we ever shut it down, we tell you by e-mail first, and Droppen on your own devices keeps working locally without us."),
          ],
        }}
        right={{
          title: t("Liability"),
          items: [
            t("As far as the law allows, we are not liable for indirect or consequential losses, lost profits or lost data, or for what your devices and AI tools do."),
            t("As far as the law allows, our total liability to you is limited to what you paid us for the service in the 12 months before the claim."),
            t("Nothing in these terms limits a liability the law does not let us limit, or takes away your rights as a consumer."),
          ],
        }}
      />
      <LegalSection
        id="ending"
        label={t("Ending")}
        title={t("Leaving, and deleting your account")}
        left={{
          title: t("If you leave"),
          items: [
            t("You can stop using droppen.app at any time, and delete your account yourself, in its settings: it is gone at once, with its devices and what our servers keep for it (the privacy notice says what stays for a short while)."),
            t("Deleting the account does not touch your own devices: what is on them stays until you remove it."),
          ],
        }}
        right={{
          title: t("If we suspend it"),
          items: [
            t("We may suspend or close an account that breaks these terms or puts the service or other people at risk. Where we can, we tell you first, say why, and give you a chance to put it right."),
            t("We can't read your sessions, so we judge by what our servers can see: how much an account sends, when, and how."),
          ],
        }}
      />
      <LegalSection
        id="legal"
        label={t("Legal")}
        title={t("Changes, the law, and who we are")}
        left={{
          title: t("Changes to these terms"),
          items: [
            t("We may update these terms. When a change matters, we tell you by e-mail before it takes effect, and the date at the top changes."),
            t("If you keep using droppen.app after that, the new terms apply. If you don't accept them, you can delete your account."),
          ],
        }}
        right={{
          title: t("Who we are"),
          items: [
            t("Droppen operates droppen.app and the service on it."),
            t("These terms are governed by the laws of Singapore, and the courts of Singapore have jurisdiction over any dispute about them. If you are a consumer, you keep the protection of the law where you live."),
          ],
        }}
      />
      <LegalContact label={t("Contact")}>
        <p className="text-sm leading-6 text-muted">
          {t("Questions about these terms, or a request about your account:")} <MailLink email={PRIVACY_EMAIL} testId="terms-email" />
        </p>
        <p className="text-sm leading-6 text-muted">
          {t("Security issues:")} <MailLink email={SECURITY_EMAIL} testId="terms-security-email" />
        </p>
        <p className="text-xs leading-5">
          <TextLink {...publicLink(PRIVACY_PATH)} data-testid="terms-privacy">
            {t("What we keep, and what we can't see: the privacy notice")}
          </TextLink>
        </p>
      </LegalContact>
    </PublicLayout>
  );
}
