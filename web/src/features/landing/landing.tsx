import { useEffect, useMemo, useState } from "react";
import i18n from "i18next";
import { useTranslation } from "react-i18next";
import { Bell, CalendarClock, Code, Info, KeyRound, ListChecks, Lock, MessageSquareReply, SquarePlus, Users } from "lucide-react";
import { secondary } from "@/features/cloud/ui";
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
import { APP_PATH, PRICING_PATH, publicLink, SECURITY_PATH, SIGN_IN_PATH, SIGN_UP_PATH, WAITLIST_HREF } from "./route";

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

const headline = "font-semibold text-balance text-fg [word-break:auto-phrase] [&:lang(zh)]:[word-break:keep-all]";

function Header({ signedIn }: { signedIn: boolean }) {
  const { t } = useTranslation();
  const nav = [
    { href: "#tour", label: t("Product") },
    { href: "#how", label: t("How it works") },
    { href: "#features", label: t("Features") },
    { href: "#security", label: t("Security") },
  ];
  return <PublicHeader signedIn={signedIn} nav={nav} />;
}

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
      <h1 className={cn(headline, "mb-6 max-w-[14em] text-[64px] leading-[1.02] tracking-[-0.04em] max-lg:text-[52px] max-md:text-[36px] max-md:leading-[1.08]")}>
        {t("Your computer's AI, in your pocket.")}
      </h1>
      <p className="max-w-[36em] text-[17px] leading-7 text-pretty text-muted [overflow-wrap:anywhere] [word-break:auto-phrase] [&:lang(zh)]:[word-break:keep-all] max-md:text-[15px] max-md:leading-6">
        {t("Claude Code and Codex keep running on your own computers. Answer them, steer them and open a terminal from your phone or any browser.")}
      </p>
      {/* The way onto this device, and the account's way in (signed in: back into the app). */}
      <HeroGet offer={offer} signedIn={signedIn} availability={availability} release={release} cpu={cpu} signup={signup} />
      {/* Below 640px the header has no room for its pricing link: this line is the way there. */}
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
        ) : (
          <p className="mt-3 font-mono text-xs text-muted">
            {t("Public beta: free for {{hosts}} computers.", { hosts: PLAN_NUMBERS.free.hosts })}{" "}
            <a {...publicLink(PRICING_PATH)} data-testid="hero-pricing" className="whitespace-nowrap text-fg underline underline-offset-4">
              {t("See the plans")}
            </a>
          </p>
        ))}
    </div>
  );
}

/** Three steps from nothing to answering your agents from your phone, each true of the build (`first-host.tsx`, `add-computer.tsx`, `pairing-screen.tsx`). */
function HowItWorks() {
  const { t } = useTranslation();
  const steps = [
    { title: t("Put Droppen where your AI runs"), body: t("The Mac app, or one command on Linux. Your sessions show up as they are.") },
    { title: t("Sign in on that computer"), body: t("One click in the Mac app, or a one-time code on a server. Add your other computers to the same account.") },
    { title: t("Pick up on your phone"), body: t("Open droppen.app on your phone, and let it in with a QR code from a device you already have.") },
  ];
  return (
    <Section id="how">
      <SectionHead title={t("Three steps, then it goes where you go.")} />
      <ol className="grid border-t border-border md:grid-cols-3">
        {steps.map(({ title, body }, i) => (
          <li key={title} className={cn("flex flex-col gap-2.5 px-6 py-7 max-md:px-4", i > 0 && "border-border max-md:border-t md:border-l")}>
            <span className="flex size-6 items-center justify-center bg-fg font-mono text-xs font-medium text-bg tabular-nums">{i + 1}</span>
            <span className="text-[15px] leading-6 font-semibold text-fg">{title}</span>
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
    { icon: ListChecks, title: t("Every session, one list"), body: t("Home and work computers on one account, and one list of what needs you across all of them.") },
    { icon: MessageSquareReply, title: t("Answer it, steer it"), body: t("Answer its questions, add a follow-up while it works, or stop a turn going the wrong way.") },
    { icon: Bell, title: t("Told when it needs you"), body: t("A notification when an agent asks you something, finishes a turn you haven't read, or fails.") },
    { icon: SquarePlus, title: t("Start one from your phone"), body: t("Pick the agent, the folder and the computer, attach a screenshot, and send.") },
    { icon: Users, title: t("Hit a limit? Carry on"), body: t("Add a few Claude accounts: when one hits its limit, a session can switch to another by itself.") },
    { icon: CalendarClock, title: t("Prompts on a schedule"), body: t("Run a prompt every morning, or every hour on weekdays, on the computer you choose.") },
  ];
  return (
    <Section id="features">
      <SectionHead title={t("Everything you would walk back to your desk for.")} />
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
            <Icon className="size-5 text-fg" />
            <span className="text-[15px] leading-6 font-semibold text-fg">{title}</span>
            <span className="text-sm leading-6 text-pretty text-muted">{body}</span>
          </li>
        ))}
      </ul>
    </Section>
  );
}

/**
 * End-to-end encryption and the open-source client, once and briefly: three
 * cards, the beta's caveat, and the way to the whole account (/security,
 * `security/security.tsx`).
 */
function Security() {
  const { t } = useTranslation();
  const cards = [
    { icon: KeyRound, title: t("Only devices you approve get in"), body: t("A new phone or browser gets in only when a device you already have lets it in. Remove a lost one, and it can't open anything written afterwards.") },
    { icon: Lock, title: t("End-to-end encrypted"), body: t("Encrypted on your computer, opened only on your devices. Droppen Hub only keeps encrypted copies and Droppen relays only pass ciphertext on: neither holds a key that can read them.") },
    { icon: Code, title: t("An open-source client"), body: t("The program on your computers, the Mac app and the web app are open source (AGPL-3.0). Droppen Hub is not open source.") },
  ];
  return (
    <Section id="security">
      <SectionHead title={t("Your code, for your eyes only.")} />
      <ul className="grid border-t border-border md:grid-cols-3">
        {cards.map(({ icon: Icon, title, body }, i) => (
          <li key={title} className={cn("flex flex-col gap-2.5 px-6 py-7 max-md:px-4", i > 0 && "border-border max-md:border-t md:border-l")}>
            <Icon className="size-5 text-fg" />
            <span className="text-[15px] leading-6 font-semibold text-fg">{title}</span>
            <span className="text-sm leading-6 text-pretty text-muted">
              {body}
              {i === 2 && (
                <>
                  {" "}
                  <a href={SOURCE_URL} data-testid="landing-source" className="whitespace-nowrap text-fg underline underline-offset-4 hover:no-underline">
                    {t("Read the source")}
                  </a>
                </>
              )}
            </span>
          </li>
        ))}
      </ul>
      <div className="flex items-start gap-3 border-t border-border px-6 py-5 max-md:px-4">
        <Info className="mt-1 size-4 shrink-0 text-muted" />
        <p className="max-w-[60em] text-sm leading-6 text-pretty text-muted">
          {t("Droppen is in beta. Its encryption has had internal security reviews, not an external audit. In a browser, the page code as loaded can use your keys, and could copy them out.")}{" "}
          <a {...publicLink(SECURITY_PATH)} data-testid="landing-security" className="text-fg underline underline-offset-4 hover:no-underline">
            {t("How the encryption works, in detail")}
          </a>
        </p>
      </div>
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
            <p className={cn(headline, "text-[36px] leading-[1.1] tracking-[-0.03em] max-md:text-[28px]")}>{t("Your AI keeps working, wherever you are.")}</p>
            <div className="flex flex-wrap gap-3">
              <a {...publicLink(signup === "closed" ? WAITLIST_HREF : SIGN_UP_PATH)} className={secondary}>
                {signup === "closed" ? t("Join the waitlist") : t("Start for free")}
              </a>
              <a {...publicLink(SIGN_IN_PATH)} className={secondary}>
                {t("Sign in")}
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
    meta?.setAttribute("content", t("Claude Code and Codex keep running on your own Macs and Linux machines; every session on all of them lands in one list on your phone or in any browser, to answer, steer or open a terminal. End-to-end encrypted, with an open-source client."));
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
  const latest = useLatestRelease(release);
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
    <div data-testid="landing" className="h-full w-full overflow-y-auto bg-bg [scrollbar-gutter:stable]">
      <Header signedIn={signedIn} />
      <div className="md:px-8">
        <main className="mx-auto max-w-[1360px] md:border-x md:border-border">
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
                  title={t("Typing from your phone stays snappy.")}
                  lead={t("Keystrokes go straight to your computer when your phone can reach it, and through a Droppen relay when it can't. If the direct path drops, the relay takes over within 1.5 s and no keystroke is lost.")}
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
                  title={t("Lid closed? Your sessions are still there.")}
                  lead={t("While your computer is on, it leaves an encrypted copy of your sessions with Droppen Hub. Close the lid or lose the network, and your phone still opens any session from the last 30 days; sending and terminals wait for the computer to come back.")}
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
