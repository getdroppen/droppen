import { useEffect, useMemo, useState } from "react";
import i18n from "i18next";
import { useTranslation } from "react-i18next";
import { Bell, EyeOff, Info, KeyRound, ListChecks, MessageSquareReply, Plug, SquarePlus, SquareTerminal, Users } from "lucide-react";
import { RegMark } from "@/components/reg-mark";
import { CapsLabel } from "@/components/ui/caps-label";
import { secondary } from "@/features/cloud/ui";
import { cn } from "@/lib/cn";
import { landingBundles } from "./copy";
import type { SignupMode } from "@/lib/hub/api";
import type { LatestRelease } from "@/lib/hub/releases";
import { browserCpu, browserHints, detectKind, type EnvironmentHints, type HeroOffer, heroOffer } from "./detect";
import { DirectPath } from "./direct-path";
import { type Availability, AVAILABILITY } from "./get-droppen";
import { HeroDiagram } from "./hero-diagram";
import { HeroGet } from "./hero-get";
import { OfflineCopy } from "./offline-copy";
import { FooterLine, Points, PublicHeader, Section, SectionHead } from "./page-kit";
import { useLatestRelease } from "./release-files";
import { APP_PATH, PRICING_PATH, publicLink, SECURITY_PATH, SIGN_IN_PATH, SIGN_UP_PATH, WAITLIST_HREF } from "./route";

/*
 * The landing page: what a signed-out visitor sees at `/` in cloud mode
 * (droppen.app). Loaded on demand, so the app's own boot does not carry it.
 *
 * It sells the product before the plumbing, as other AI-agent sites do: the
 * hero shows the work (your agents' sessions on your phone, one asking, one
 * done), then how to get there in three steps, what you can do, why it is
 * fast and works with the lid closed, and only then, in one section, that it
 * is end-to-end encrypted — the full account of that is /security's.
 *
 * Drafting-sheet language throughout: a 1152px framed
 * column of hairline sections with registration marks at their seams, one ink
 * primary action (the hero's, for the device reading it: `hero-get.tsx` — the
 * Mac App's download on a Mac, the install command on Linux, else "get
 * started"), display type only for the headlines, and
 * colour only where a diagram shows state — a session working or asking you,
 * a live direct path, a dropped one, a computer that is off, a round trip by
 * how fast it is.
 *
 * Headlines wrap between phrases, never inside a word: `auto-phrase` does it
 * for Japanese, and Chinese keeps its words whole (`keep-all`), breaking at
 * its punctuation and spaces — every Chinese headline here has a comma.
 */

// Added, never overwriting: a key the app already translates keeps its wording.
for (const [lng, bundle] of Object.entries(landingBundles)) i18n.addResourceBundle(lng, "translation", bundle, true, false);

function Header({ signedIn }: { signedIn: boolean }) {
  const { t } = useTranslation();
  const nav = [
    { href: "#how", label: t("How it works") },
    { href: "#features", label: t("Features") },
    { href: "#direct", label: t("Speed") },
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
    <>
      {/* From 1280px: copy on the left, the figure on the grid to its right. Below: the figure in a band under the copy. */}
      <div className="flex flex-col xl:grid xl:grid-cols-[minmax(0,44fr)_minmax(0,56fr)]">
        <div className="relative flex flex-col justify-center px-10 pt-16 pb-12 max-lg:pt-12 max-md:px-4 max-md:pt-10 max-md:pb-8 xl:border-r xl:border-border xl:pb-16">
          <RegMark className="-top-[5px] -right-[5px] md:hidden xl:block" />
          <RegMark className="-right-[5px] -bottom-[5px] md:hidden xl:block" />
          <CapsLabel>{t("For Claude Code and Codex")}</CapsLabel>
          <h1 className="mt-4 mb-5 text-[52px] leading-[1.02] font-semibold tracking-[-0.035em] text-balance text-fg [word-break:auto-phrase] [&:lang(zh)]:[word-break:keep-all] max-md:text-[34px] xl:text-[44px]">
            {t("Your computer's AI, in your pocket.")}
          </h1>
          <p className="max-w-[34em] text-[15px] leading-[26px] text-muted max-md:text-sm max-md:leading-6">
            {t("Claude Code and Codex keep running on your own computers. When one asks you something or finishes, your phone knows: answer it, send the next prompt, or open a terminal. Every computer and every session in one list, end-to-end encrypted.")}
          </p>
          {/* The way onto this device, and the account's ways in (signed in: back into the app). */}
          <HeroGet offer={offer} signedIn={signedIn} availability={availability} release={release} cpu={cpu} signup={signup} />
          {/* Below 640px the header has no room for its pricing link: this line is the way there. */}
          {!signedIn &&
            (signup === "invite" ? (
              <p className="mt-3 font-mono text-2xs text-faint">
                {t("Invite-only for now: sign-up needs an invite code.")}{" "}
                <a {...publicLink(WAITLIST_HREF)} data-testid="hero-waitlist" className="text-muted underline underline-offset-4 hover:text-fg">
                  {t("No code yet? Join the waitlist")}
                </a>
              </p>
            ) : signup === "closed" ? (
              <p className="mt-3 font-mono text-2xs text-faint">
                {t("Sign-up is closed for now.")}{" "}
                <a {...publicLink(WAITLIST_HREF)} data-testid="hero-waitlist" className="text-muted underline underline-offset-4 hover:text-fg">
                  {t("Join the waitlist")}
                </a>
              </p>
            ) : (
              <p className="mt-3 font-mono text-2xs text-faint">
                {t("Public beta: sign-up is open.")}{" "}
                <a {...publicLink(PRICING_PATH)} data-testid="hero-pricing" className="text-muted underline underline-offset-4 hover:text-fg">
                  {t("See the plans")}
                </a>
              </p>
            ))}
        </div>
        <div className="relative border-border max-xl:border-t">
          <RegMark className="-top-[5px] -left-[5px] xl:hidden" />
          <RegMark className="-top-[5px] -right-[5px] xl:hidden" />
          <HeroDiagram />
        </div>
      </div>
    </>
  );
}

/** Three steps from nothing to answering your agents from your phone, each true of the build (`first-host.tsx`, `add-computer.tsx`, `pairing-screen.tsx`). */
function HowItWorks() {
  const { t } = useTranslation();
  const steps = [
    { title: t("Install it where your AI runs"), body: t("The Mac app, or one command on Linux, on the computer that has Claude Code or Codex. Your sessions show up as they are: nothing to move or set up.") },
    { title: t("Sign that computer in"), body: t("One click in the Mac app, or a one-time code on a server. Add the computers at home and at work to the same account.") },
    { title: t("Pick up on your phone"), body: t("Open droppen.app on your phone and let it in from a device you already have, with a QR code. Add it to your home screen, turn on notifications, and you hear when an agent needs you.") },
  ];
  return (
    <Section id="how">
      <SectionHead label={t("How it works")} title={t("Three steps, then walk away.")} />
      <ol className="grid border-t border-border md:grid-cols-3">
        {steps.map(({ title, body }, i) => (
          <li key={title} className={cn("flex flex-col gap-2 px-6 py-6 max-md:px-4", i > 0 && "border-border max-md:border-t md:border-l")}>
            <span className="flex size-[18px] items-center justify-center bg-fg font-mono text-[10px] font-medium text-bg tabular-nums">{i + 1}</span>
            <span className="text-[13px] leading-5 font-semibold text-fg">{title}</span>
            <span className="text-xs leading-5 text-muted">{body}</span>
          </li>
        ))}
      </ol>
    </Section>
  );
}

function Features() {
  const { t } = useTranslation();
  const items = [
    { icon: ListChecks, title: t("Every session, one list"), body: t("The computers at home and at work, side by side on one account, and one list of what needs you across all of them: which session is asking, which has news, which is still working.") },
    { icon: MessageSquareReply, title: t("Answer it, steer it"), body: t("Answer an agent's question, send a follow-up (it waits its turn while the agent works), or stop a turn that is going the wrong way.") },
    { icon: Bell, title: t("Told when it needs you"), body: t("A notification when an agent stops to ask, when a turn ends and you have not read it, or when a run fails. Not one for every line it prints.") },
    { icon: SquarePlus, title: t("Start one from your phone"), body: t("Pick the agent, the folder and the computer, attach a screenshot, and send: the session starts on that computer.") },
    { icon: SquareTerminal, title: t("Terminals and files"), body: t("Open a terminal on any of your computers, browse and preview its files, and lay sessions and terminals out side by side.") },
    { icon: Users, title: t("Hit a limit? Carry on"), body: t("Add a few Claude accounts, and when one hits its limit a session can carry on with another by itself. Prompts can run on a schedule, too.") },
  ];
  return (
    <Section id="features">
      <SectionHead label={t("What you get")} title={t("Everything you would walk back to your desk for.")} />
      <ul className="grid border-t border-border sm:grid-cols-2 lg:grid-cols-3">
        {items.map(({ icon: Icon, title, body }) => (
          <li
            key={title}
            className={cn(
              "flex flex-col gap-2 border-border px-6 py-6 max-md:px-4",
              "max-sm:border-b max-sm:last:border-b-0",
              "sm:max-lg:border-b sm:max-lg:[&:nth-child(2n)]:border-l sm:max-lg:[&:nth-last-child(-n+2)]:border-b-0",
              "lg:[&:nth-child(-n+3)]:border-b lg:[&:not(:nth-child(3n+1))]:border-l",
            )}
          >
            <Icon className="size-4 text-fg" />
            <span className="text-[13px] leading-5 font-semibold text-fg">{title}</span>
            <span className="text-xs leading-5 text-muted">{body}</span>
          </li>
        ))}
      </ul>
    </Section>
  );
}

/**
 * End-to-end encryption, once and briefly: what it means for you in three
 * cards, the beta's caveat, and the way to the whole account of it
 * (/security, `security/security.tsx`).
 */
function Security() {
  const { t } = useTranslation();
  const cards = [
    { icon: KeyRound, title: t("Only devices you approve get in"), body: t("A new phone or browser gets in only when a device you already have lets it in, with a QR code or a one-time code. Lost one? Remove it, and it cannot open anything written afterwards.") },
    { icon: EyeOff, title: t("Nothing readable on our servers"), body: t("What is said in a session, typed in a terminal or kept in your files, and the names of your sessions, folders and models: none of it can be read on our servers. They do see your e-mail address, which devices are online and from which IP address, and how much data moves, when.") },
    { icon: Plug, title: t("Your computer, your accounts"), body: t("Claude Code and Codex run on your own computer, with your own accounts and quotas, never on our servers. On one computer, Droppen needs no account at all.") },
  ];
  return (
    <Section id="security">
      <SectionHead
        label={t("End-to-end encrypted")}
        title={t("Your code, for your eyes only.")}
        lead={t("Encrypted on your computer, opened only on the devices you approved. In between, Droppen Hub only finds your computers and keeps their encrypted copies; Droppen relays only pass ciphertext on. Neither holds a key that can read them.")}
      />
      <ul className="grid border-t border-border md:grid-cols-3">
        {cards.map(({ icon: Icon, title, body }, i) => (
          <li key={title} className={cn("flex flex-col gap-2 px-6 py-6 max-md:px-4", i > 0 && "border-border max-md:border-t md:border-l")}>
            <Icon className="size-4 text-fg" />
            <span className="text-[13px] leading-5 font-semibold text-fg">{title}</span>
            <span className="text-xs leading-5 text-muted">{body}</span>
          </li>
        ))}
      </ul>
      <div className="flex items-start gap-3 border-t border-border px-6 py-5 max-md:px-4">
        <Info className="mt-0.5 size-4 shrink-0 text-muted" />
        <p className="max-w-[60em] text-xs leading-5 text-muted">
          {t("Droppen is in beta. Its encryption design has had internal security reviews, not an external audit. In a browser you trust the page code as it was loaded that time: that code can use your private keys, and could copy them out.")}{" "}
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
          <div className="flex flex-col gap-6 px-6 py-14 max-md:px-4 max-md:py-10 md:flex-row md:items-end md:justify-between">
            <div className="flex flex-col gap-3">
              <CapsLabel>{signup === "invite" ? t("Invite-only beta") : signup === "closed" ? t("Beta") : t("Public beta")}</CapsLabel>
              <p className="text-[32px] leading-[1.1] font-semibold tracking-[-0.03em] text-balance text-fg [word-break:auto-phrase] [&:lang(zh)]:[word-break:keep-all] max-md:text-[26px]">{t("Your AI keeps working. You can walk away.")}</p>
              {/* Closed: no sign-up to describe; the way in below is the waitlist. */}
              {signup !== "closed" && (
                <p className="text-sm leading-6 text-muted">
                  {signup === "invite"
                    ? t("Sign up with your invite code, connect your computer, and pick up from your phone.")
                    : t("Sign up with your e-mail, connect your computer, and pick up from your phone.")}
                </p>
              )}
            </div>
            <div className="flex flex-wrap gap-3">
              <a {...publicLink(signup === "closed" ? WAITLIST_HREF : SIGN_UP_PATH)} className={secondary}>
                {signup === "closed" ? t("Join the waitlist") : t("Get started")}
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
    meta?.setAttribute("content", t("Claude Code and Codex keep running on your own computers; from your phone you answer their questions, send the next prompt and open a terminal. Every computer and every session in one list, end-to-end encrypted."));
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
        <main className="mx-auto max-w-[1152px] md:border-x md:border-border">
          <Hero signedIn={signedIn} signup={signup} offer={offer} availability={availability} release={latest} cpu={cpu === undefined ? detectedCpu : cpu} />
          <HowItWorks />
          <Features />
          <Section id="direct">
            <div className="grid lg:grid-cols-[minmax(0,1fr)_minmax(0,1.55fr)]">
              <div className="flex flex-col lg:border-r lg:border-border">
                <SectionHead
                  label={t("Speed")}
                  title={t("Typing from your phone stays snappy.")}
                  lead={t("When your phone can reach your computer directly, keystrokes go straight across. When it cannot, a Droppen relay takes over by itself: you never pick a path.")}
                />
                <Points
                  className="px-6 pb-8 max-md:px-4"
                  items={[
                    t("Measured p50: a keystroke echoes in 1–2 ms in a browser on the same computer, and in 63–73 ms from a phone on 4G, direct; 215–227 ms through a Droppen relay."),
                    t("No keystroke lost: if the direct path drops, traffic is back on the Droppen relay within 1.5 s, and unconfirmed keystrokes are sent again in order. None lost, doubled or reordered."),
                    t("So far about two in three testers got a direct path; the rest go through a Droppen relay automatically."),
                  ]}
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
                  label={t("Offline")}
                  title={t("Lid closed? Your sessions are still there.")}
                  lead={t("Close the lid or lose the network, and your phone still opens every session as it last was: while your computer is on, the agent leaves an encrypted copy of what is new with Droppen Hub every few seconds.")}
                />
                <Points
                  className="px-6 pb-8 max-md:px-4"
                  items={[
                    t("Read-only meanwhile: sending and terminals wait for your computer to come back."),
                    t("Opens at once next time: each device keeps its own copy, cleared when you sign out."),
                    t("What syncs: each computer's own sessions updated in the last 30 days."),
                  ]}
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
