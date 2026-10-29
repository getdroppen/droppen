import { useEffect, useMemo, useState } from "react";
import i18n from "i18next";
import { useTranslation } from "react-i18next";
import { ArrowRight, Bell, CalendarClock, Code, FolderOpen, Info, KeyRound, ListChecks, Lock, MessageSquareReply, SquarePlus } from "lucide-react";
import { appHost } from "@/lib/hub/hosts";
import { cn } from "@/lib/cn";
import { landingBundles } from "./copy";
import type { SignupMode } from "@/lib/hub/api";
import type { LatestRelease } from "@/lib/hub/releases";
import { browserCpu, browserHints, detectKind, type EnvironmentHints, type HeroOffer, heroOffer } from "./detect";
import { DirectPath } from "./direct-path";
import { type Availability, AVAILABILITY } from "./get-droppen";
import { HeroGet } from "./hero-get";
import { OfflineCopy } from "./offline-copy";
import { FooterLine, PublicHeader, Section, SectionHead, SOURCE_URL } from "./page-kit";
import { PLAN_NUMBERS } from "./pricing-plans";
import { ProductTour } from "./product-tour";
import { useLatestRelease } from "./release-files";
import { APP_PATH, publicLink, SECURITY_PATH, SIGN_UP_PATH, WAITLIST_HREF } from "./route";

/*
 * The landing page: what a signed-out visitor sees at `/` in cloud mode
 * (droppen.app). Loaded on demand, so the app's own boot does not carry it.
 *
 * It sells the product before the plumbing, as other AI-agent sites do: the
 * headline, then the product itself — real screenshots of the Mac app, the
 * split screen, the terminal, the cloud's every-device page, the phone and
 * letting a device in, behind tabs (`product-tour.tsx`) — then how to get
 * there in three steps, what you can do, why it is fast and still readable
 * with the lid closed, and only then, in one section, that it is end-to-end
 * encrypted and the client open source; the full account is /security's.
 *
 * Drafting-sheet language throughout: a 1360px framed column of hairline
 * sections with registration marks at their seams, one ink primary action
 * (the hero's, for the device reading it: `hero-get.tsx` — the Mac App's
 * download on a Mac, the install command on Linux, else "start for free"),
 * display type only for the headlines, and colour only where a diagram shows
 * state — a live direct path, a dropped one, a computer that is off, a round
 * trip by how fast it is.
 *
 * The layout does the explaining: no caps line over a headline, no caption
 * that says what the picture or the section already shows. A section is its
 * headline, and words under it only for what the layout cannot show (how to
 * get it, the numbers, what is encrypted, the beta's caveat).
 *
 * Headlines wrap between phrases, never inside a word: `auto-phrase` does it
 * for Japanese, and Chinese keeps its words whole (`keep-all`), breaking at
 * its punctuation and spaces — every Chinese headline here has a comma.
 * Paragraphs wrap `pretty`, so a line never ends on one character.
 */

// Added, never overwriting: a key the app already translates keeps its wording.
for (const [lng, bundle] of Object.entries(landingBundles)) i18n.addResourceBundle(lng, "translation", bundle, true, false);

const headline = "font-normal text-balance text-fg [word-break:auto-phrase] [&:lang(zh)]:[word-break:keep-all]";

/** The space before a link that follows a sentence: none after Chinese or Japanese punctuation. */
const gapBefore = (language: string) => (/^(zh|ja)/.test(language) ? "" : " ");

function Hero({
  signedIn,
  signup,
  offer,
  availability,
  release,
  cpu,
}: {
  signedIn: boolean;
  /** How sign-up works here (`signup-mode.ts`): by invite or closed, the line says so and offers the waitlist. */
  signup: SignupMode;
  offer: HeroOffer;
  availability: Availability;
  release: LatestRelease | null | undefined;
  cpu: string | null;
}) {
  const { t } = useTranslation();
  return (
    <div className="flex flex-col px-10 pt-16 pb-12 max-lg:pt-12 max-md:px-4 max-md:pt-10 max-md:pb-8">
      <h1 className="mb-6 max-w-[14em] text-5xl leading-display font-normal tracking-tighter text-balance text-fg [word-break:auto-phrase] [&:lang(zh)]:[word-break:keep-all] max-md:text-4xl">
        {t("Your computer's AI, from your phone.")}
      </h1>
      <p className="max-w-[36em] leading-6 text-pretty text-muted [overflow-wrap:anywhere] [word-break:auto-phrase] [&:lang(zh)]:[word-break:keep-all]">
        {t("Claude Code and Codex keep running on your own computers. From a browser on your phone or another device, answer their questions, steer them and open a terminal on the computer.")}
      </p>
      {/* The way onto this device, and the account's way in (signed in: back into the app). */}
      <HeroGet offer={offer} signedIn={signedIn} availability={availability} release={release} cpu={cpu} signup={signup} />
      {/* By invite or closed, the way to the waitlist. Open sign-up says nothing of plans here: the first screen sells the product, not its price (the header and the footer link the plans). */}
      {!signedIn &&
        (signup === "invite" ? (
          <p className="mt-3 font-mono text-xs text-muted">
            {t("Invite-only for now: sign-up needs an invite code.")}{" "}
            <a {...publicLink(WAITLIST_HREF)} data-testid="hero-waitlist" className="whitespace-nowrap text-fg underline underline-offset-4">
              {t("No code yet? Join the waitlist")}
            </a>
          </p>
        ) : signup === "closed" ? (
          <p className="mt-3 font-mono text-xs text-muted">
            {t("Sign-up is closed for now.")}{" "}
            <a {...publicLink(WAITLIST_HREF)} data-testid="hero-waitlist" className="whitespace-nowrap text-fg underline underline-offset-4">
              {t("Join the waitlist")}
            </a>
          </p>
        ) : null)}
    </div>
  );
}

/** Three steps from nothing to answering your agents from your phone, each true of the build (`first-host.tsx`, `add-computer.tsx`, `pairing-screen.tsx`). */
function HowItWorks() {
  const { t } = useTranslation();
  const steps = [
    { title: t("Install Droppen where Claude Code or Codex runs"), body: t("The Mac app, or one install command on Linux. Sessions already on that computer appear in Droppen, with nothing to migrate.") },
    { title: t("Sign in on that computer"), body: t("Sign in from the Mac app. On a Linux server, run the command with a one-time code from a device that is already signed in. Add your other computers the same way.") },
    { title: t("Continue on your phone"), body: t("Open {{app}} in your phone's browser, then scan the QR code it shows with a device that is already signed in.", { app: appHost() }) },
  ];
  return (
    <Section id="how">
      <SectionHead title={t("Three steps to set up, then carry on away from your desk.")} />
      <ol className="grid border-t border-border md:grid-cols-3">
        {steps.map(({ title, body }, i) => (
          <li key={title} className={cn("flex flex-col gap-2.5 px-6 py-7 max-md:px-4", i > 0 && "border-border max-md:border-t md:border-l")}>
            <div className="flex items-center gap-2.5">
              <span className="flex size-6 shrink-0 items-center justify-center bg-fg font-mono text-xs font-medium text-bg tabular-nums">{i + 1}</span>
              <h3 className="text-base leading-6 font-semibold text-fg [&:lang(en)]:text-balance [word-break:auto-phrase] [&:lang(zh)]:[word-break:keep-all]">{title}</h3>
            </div>
            <span className="text-sm leading-6 text-pretty text-muted">{body}</span>
          </li>
        ))}
      </ol>
    </Section>
  );
}

function Features() {
  const { t } = useTranslation();
  const items = [
    { icon: ListChecks, title: t("Every computer, one list"), body: t("Home and work computers on one account, with every session that is waiting for an answer, has unread messages or is running in one list.") },
    { icon: MessageSquareReply, title: t("Answer and steer"), body: t("Answer the AI's questions, add a message while it works, or stop the current run if it goes the wrong way.") },
    { icon: Bell, title: t("Notified when a session needs you"), body: t("A notification when the AI asks a question, finishes a reply you haven't read, or fails.") },
    { icon: SquarePlus, title: t("Start a session from your phone"), body: t("Choose Claude Code or Codex, a computer and a folder, optionally attach a screenshot, and send.") },
    { icon: FolderOpen, title: t("Browse your files"), body: t("Browse the folders on your computer and preview files from your phone.") },
    { icon: CalendarClock, title: t("Run prompts on a schedule"), body: t("Have a prompt run automatically on the computer you choose, for example every morning or every hour on weekdays.") },
  ];
  return (
    <Section id="features">
      <SectionHead title={t("Away from your computer, handle all of this from your phone.")} />
      <ul className="grid border-t border-border sm:grid-cols-2 lg:grid-cols-3">
        {items.map(({ icon: Icon, title, body }) => (
          <li
            key={title}
            className={cn(
              "flex flex-col gap-2.5 border-border px-6 py-7 max-md:px-4",
              "max-sm:border-b max-sm:last:border-b-0",
              "sm:max-lg:border-b sm:max-lg:[&:nth-child(2n)]:border-l sm:max-lg:[&:nth-last-child(-n+2)]:border-b-0",
              "lg:[&:nth-child(-n+3)]:border-b lg:[&:not(:nth-child(3n+1))]:border-l",
            )}
          >
            <div className="flex items-center gap-2.5">
              <Icon className="size-5 shrink-0 text-fg" />
              <h3 className="text-base leading-6 font-semibold text-fg">{title}</h3>
            </div>
            <span className="text-sm leading-6 text-pretty text-muted">{body}</span>
          </li>
        ))}
      </ul>
    </Section>
  );
}

/**
 * The one caveat the page owes a visitor before anything else: it is a beta,
 * the encryption has not been audited, and a browser trusts the code it loaded.
 * First line of the framed column, above the hero; it links the whole account
 * of it (/security).
 */
function BetaNote() {
  const { t, i18n: active } = useTranslation();
  return (
    <div className="flex items-start gap-3 border-b border-border px-6 py-4 max-md:px-4">
      <Info className="mt-0.5 size-4 shrink-0 text-muted" />
      <p className="max-w-[60em] text-xs leading-5 text-pretty text-muted">
        {t("Droppen is in beta: its encryption design has had internal security reviews, not an external audit. Due to browser limitations, we recommend the more secure Mac app.")}
        {gapBefore(active.language)}
        <a {...publicLink(SECURITY_PATH)} data-testid="landing-security" className="whitespace-nowrap text-fg underline underline-offset-4 hover:no-underline">
          {t("How the encryption works")}
        </a>
      </p>
    </div>
  );
}

/**
 * End-to-end encryption and the open-source client, once and briefly: three
 * cards and the way to the whole account (/security, `security/security.tsx`).
 */
function Security() {
  const { t, i18n: active } = useTranslation();
  const cards = [
    { icon: KeyRound, title: t("Only devices you approve can join"), body: t("A new phone or browser can join only when a device you already have approves it. If one is lost, remove it from another phone or browser of yours: it can't decrypt anything written afterwards, and what was already synced to it can't be recalled.") },
    { icon: Lock, title: t("End-to-end encrypted"), body: t("Encrypted on your computer and decrypted only on your devices. Droppen Hub stores only encrypted copies and Droppen relays forward only ciphertext; neither holds a key that can decrypt them.") },
    { icon: Code, title: t("Open-source client"), body: t("The droppen command-line program, the Mac app and the web app are open source under AGPL-3.0. Droppen Hub is not open source.") },
  ];
  return (
    <Section id="security">
      <SectionHead title={t("Droppen can't read your code.")} />
      <ul className="grid border-t border-border md:grid-cols-3">
        {cards.map(({ icon: Icon, title, body }, i) => (
          <li key={title} className={cn("flex flex-col gap-2.5 px-6 py-7 max-md:px-4", i > 0 && "border-border max-md:border-t md:border-l")}>
            <div className="flex items-center gap-2.5">
              <Icon className="size-5 shrink-0 text-fg" />
              <h3 className="text-base leading-6 font-semibold text-fg">{title}</h3>
            </div>
            <span className="text-sm leading-6 text-pretty text-muted">
              {body}
              {i === 2 && (
                <>
                  {gapBefore(active.language)}
                  <a href={SOURCE_URL} data-testid="landing-source" className="whitespace-nowrap text-fg underline underline-offset-4 hover:no-underline">
                    {t("Read the source")}
                  </a>
                </>
              )}
            </span>
          </li>
        ))}
      </ul>
    </Section>
  );
}

function Footer({ signedIn, signup }: { signedIn: boolean; signup: SignupMode }) {
  const { t } = useTranslation();
  return (
    <>
      {/* The invitation is for visitors; someone signed in has the way back in at the top. */}
      {!signedIn && (
        <Section>
          <div className="flex flex-col gap-6 px-6 py-16 max-md:px-4 max-md:py-10 md:flex-row md:items-end md:justify-between">
            <p className={cn(headline, "text-4xl leading-display tracking-tight max-md:text-2xl max-md:max-[360px]:text-xl")}>{t("Your AI keeps working on your computer. When it needs you, answer from your phone.")}</p>
            <div className="flex flex-wrap gap-3">
              <a
                {...publicLink(signup === "closed" ? WAITLIST_HREF : SIGN_UP_PATH)}
                className="group flex h-10 shrink-0 items-center justify-between gap-3 whitespace-nowrap border border-fg bg-fg px-4 text-sm font-medium text-bg transition-colors hover:bg-fg/85"
              >
                <span>{signup === "closed" ? t("Join the waitlist") : t("Sign up on the web")}</span>
                <ArrowRight className="size-4 shrink-0 transition-transform duration-150 group-hover:translate-x-0.5" />
              </a>
            </div>
          </div>
        </Section>
      )}
      <FooterLine />
    </>
  );
}

/** The page's title and description in the reader's language (the shell carries the zh-CN ones for crawlers that do not run scripts). */
function useDocumentMeta() {
  const { t, i18n: active } = useTranslation();
  useEffect(() => {
    const before = document.title;
    document.title = t("Droppen — your computer's AI, on your phone");
    document.documentElement.lang = active.language;
    const meta = document.querySelector('meta[name="description"]');
    const description = meta?.getAttribute("content") ?? null;
    meta?.setAttribute("content", t("Claude Code and Codex keep running on your own Macs and Linux machines. From your phone or a browser, see every session across them, answer their questions, steer them and open a terminal. End-to-end encrypted, with an open-source client."));
    return () => {
      document.title = before;
      if (meta && description !== null) meta.setAttribute("content", description);
    };
  }, [t, active.language]);
}

/**
 * The landing page, at `/` whether or not this browser is signed in. Signed
 * in (a device session is kept here), its ways in become a way back into the
 * app at {@link APP_PATH}, and the invitations to sign up go. The hero offers
 * the way onto the device reading it (`detect.ts`), its files as the hub
 * names them (`release-files.tsx`); `availability`, `hints`, `cpu` and
 * `release` stand in for the build's switches, the browser and the hub
 * (tests, previews). `signup` is the hub's sign-up mode (`signup-mode.ts`):
 * open unless it says invite (the page speaks of invite codes and the
 * waitlist) or closed (sign-up is closed for now, and the waitlist).
 */
export default function LandingPage({
  signedIn = false,
  availability = AVAILABILITY,
  hints,
  cpu,
  release,
  signup = "open",
}: {
  signedIn?: boolean;
  availability?: Availability;
  hints?: EnvironmentHints;
  cpu?: string | null;
  release?: LatestRelease | null;
  signup?: SignupMode;
}) {
  const { t } = useTranslation();
  useDocumentMeta();
  const offer = useMemo(() => heroOffer(detectKind(hints ?? browserHints() ?? { userAgent: "" }), availability), [hints, availability]);
  // A phone or Windows hero names no release file: do not ask the hub for one.
  const needsRelease = offer === "mac" || offer === "linux" || offer === "all";
  const latest = useLatestRelease(release === undefined && !needsRelease ? null : release);
  // Linux's processor, for its tarball (the high-entropy hints, else the user agent).
  const [detectedCpu, setDetectedCpu] = useState<string | null>(null);
  useEffect(() => {
    if (offer !== "linux" || cpu !== undefined) return;
    let live = true;
    void browserCpu().then((value) => live && setDetectedCpu(value));
    return () => {
      live = false;
    };
  }, [offer, cpu]);
  return (
    <div data-testid="landing" className="h-full w-full overflow-y-auto bg-bg text-base [scrollbar-gutter:stable]">
      <PublicHeader signedIn={signedIn} />
      <div className="md:px-8">
        <main className="mx-auto max-w-[1360px] md:border-x md:border-border">
          <BetaNote />
          <Hero signedIn={signedIn} signup={signup} offer={offer} availability={availability} release={latest} cpu={cpu === undefined ? detectedCpu : cpu} />
          <Section id="tour">
            <ProductTour />
          </Section>
          <HowItWorks />
          <Features />
          <Section id="direct">
            <div className="grid lg:grid-cols-[minmax(0,1fr)_minmax(0,1.55fr)]">
              <div className="flex flex-col lg:border-r lg:border-border">
                <SectionHead
                  title={t("Typing from your phone stays responsive.")}
                  lead={t("When your phone can reach your computer directly, keystrokes go straight there; when it can't, a Droppen relay forwards them automatically. If the direct path drops, the relay takes over within 1.5 s and resends unconfirmed keystrokes in order, so none are lost.")}
                />
              </div>
              <div className="max-lg:border-t max-lg:border-border">
                <DirectPath />
              </div>
            </div>
          </Section>
          <Section id="offline">
            <div className="grid lg:grid-cols-[minmax(0,1.55fr)_minmax(0,1fr)]">
              <div className="max-lg:order-2 max-lg:border-t max-lg:border-border lg:border-r lg:border-border">
                <OfflineCopy />
              </div>
              <div className="flex flex-col">
                <SectionHead
                  title={t("Your sessions stay readable while your computer is offline.")}
                  lead={t("While your computer is online, an encrypted copy of its sessions is kept on Droppen Hub in the cloud, and only your devices can decrypt it. When the computer is shut down, asleep or offline, your phone can still open the sessions updated in the last {{free}} days ({{pro}} on Pro); sending messages and using the terminal wait until that computer is back online.", {
                    free: PLAN_NUMBERS.free.retentionDays,
                    pro: PLAN_NUMBERS.pro.retentionDays,
                  })}
                />
              </div>
            </div>
          </Section>
          <Security />
          <Footer signedIn={signedIn} signup={signup} />
        </main>
      </div>
    </div>
  );
}
