import { useEffect, useMemo, useState } from "react";
import i18n from "i18next";
import { useTranslation } from "react-i18next";
import { EyeOff, FolderTree, Info, KeyRound, MonitorSmartphone, Network, Plug, SquareTerminal, Unplug } from "lucide-react";
import { RegMark } from "@/components/reg-mark";
import { CapsLabel } from "@/components/ui/caps-label";
import { secondary } from "@/features/cloud/ui";
import { cn } from "@/lib/cn";
import { landingBundles } from "./copy";
import type { LatestRelease } from "@/lib/hub/releases";
import { browserCpu, browserHints, detectKind, type EnvironmentHints, type HeroOffer, heroOffer } from "./detect";
import { DirectPath } from "./direct-path";
import { type Availability, AVAILABILITY } from "./get-droppen";
import { HeroDiagram } from "./hero-diagram";
import { HeroGet } from "./hero-get";
import { Journey } from "./journey";
import { OfflineCopy } from "./offline-copy";
import { FooterLine, Points, PublicHeader, Section, SectionHead } from "./page-kit";
import { useLatestRelease } from "./release-files";
import { APP_PATH, onPublicLink, SECURITY_PATH, SIGN_IN_PATH, SIGN_UP_PATH, WAITLIST_HREF } from "./route";

/*
 * The landing page: what a signed-out visitor sees at `/` in cloud mode
 * (droppen.app). Loaded on demand, so the app's own boot does not carry it.
 *
 * Drafting-sheet language throughout: a 1152px framed
 * column of hairline sections with registration marks at their seams, one ink
 * primary action (the hero's, for the device reading it: `hero-get.tsx` — the
 * Mac App's download on a Mac, the install command on Linux, else "get
 * started"), display type only for the headlines, and
 * colour only where a diagram shows state — a live direct path, a dropped one,
 * a computer that is off, a round trip by how fast it is.
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
    { href: "#journey", label: t("Privacy") },
    { href: "#direct", label: t("Speed") },
    { href: "#offline", label: t("Offline") },
    { href: "#security", label: t("Security") },
  ];
  return <PublicHeader signedIn={signedIn} nav={nav} />;
}

function Hero({
  signedIn,
  offer,
  availability,
  release,
  cpu,
}: {
  signedIn: boolean;
  offer: HeroOffer;
  availability: Availability;
  release: LatestRelease | null | undefined;
  cpu: string | null;
}) {
  const { t } = useTranslation();
  const stats = [
    { value: "1–2 ms", label: t("keystroke echo in a browser on the same computer, direct (measured p50)") },
    { value: "63–73 ms", label: t("round trip from your phone on 4G, direct; 215–227 ms when it cannot connect directly (measured p50)") },
    { value: "0", label: t("keys on our servers that can open your code or your conversations") },
    { value: "12", label: t("recovery words, held only by you: we cannot recover them for you") },
  ];
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
            {t("Claude Code and Codex keep running on your own computer. Walk away, and from your phone you still watch them work, answer their questions and open a terminal. Every device on one screen, end-to-end encrypted: we cannot see your code.")}
          </p>
          {/* The way onto this device, and the account's ways in (signed in: back into the app). */}
          <HeroGet offer={offer} signedIn={signedIn} availability={availability} release={release} cpu={cpu} />
          {!signedIn && (
            <p className="mt-3 font-mono text-2xs text-faint">
              {t("Invite-only for now: sign-up needs an invite code.")}{" "}
              <a href={WAITLIST_HREF} onClick={onPublicLink(WAITLIST_HREF)} data-testid="hero-waitlist" className="text-muted underline underline-offset-4 hover:text-fg">
                {t("No code yet? Join the waitlist")}
              </a>
            </p>
          )}
        </div>
        <div className="relative border-border max-xl:border-t">
          <RegMark className="-top-[5px] -left-[5px] xl:hidden" />
          <RegMark className="-top-[5px] -right-[5px] xl:hidden" />
          <HeroDiagram />
        </div>
      </div>
      <Section>
        <dl className="grid grid-cols-2 lg:grid-cols-4">
          {stats.map((s, i) => (
            <div
              key={s.value}
              className={cn(
                "flex flex-col gap-2 border-border px-6 py-6 max-md:px-4",
                i > 0 && "lg:border-l",
                i % 2 === 1 && "max-lg:border-l",
                i > 1 && "max-lg:border-t",
              )}
            >
              <dt className="order-2 font-mono text-2xs leading-4 text-muted">{s.label}</dt>
              <dd className="order-1 text-[28px] leading-none font-semibold tracking-[-0.03em] text-fg tabular-nums max-md:text-[24px]">{s.value}</dd>
            </div>
          ))}
        </dl>
      </Section>
    </>
  );
}

function Features() {
  const { t } = useTranslation();
  const items = [
    { icon: SquareTerminal, title: t("Answer it from anywhere"), body: t("Watch sessions as they run and see at a glance which one is asking you. Answer it, send a follow-up, or open a terminal.") },
    { icon: Plug, title: t("Your AI, your accounts"), body: t("Claude Code and Codex run on your own computer, with your own accounts, quotas and code, never on our servers. Add a few Claude accounts, and when one hits its limit a session can carry on with another by itself.") },
    { icon: MonitorSmartphone, title: t("All your devices, one screen"), body: t("The computers at home and at work, side by side on one account, and one list of what needs you across all of them: which session is asking, which has news, which is still working.") },
    { icon: KeyRound, title: t("Only devices you approve get in"), body: t("A new phone or browser joins only when a device you already have approves it, by QR code or a one-time code. Twelve recovery words; no key escrow.") },
    { icon: Unplug, title: t("Lost a phone? Cut it off"), body: t("Removing a device closes its connections and direct paths and rotates the keys: it cannot open anything written afterwards.") },
    { icon: Network, title: t("Direct or not: your call"), body: t("One switch, “Allow direct connections”, decides: turn it off and your device and your computer never learn each other's IP address.") },
  ];
  return (
    <Section id="features">
      <SectionHead label={t("What you get")} title={t("Walk away from your desk. Everything stays within reach.")} />
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

function Security() {
  const { t } = useTranslation();
  const columns = [
    {
      icon: FolderTree,
      title: t("What we can see"),
      items: [
        t("your e-mail address and when you sign in"),
        t("which devices and computers are online, when they connect, and from which IP address"),
        t("how much data moves and when: stored data is padded to fixed sizes, live connections are not"),
        t("so, roughly when you are working, and in a terminal the rhythm of your typing"),
      ],
    },
    {
      icon: EyeOff,
      title: t("What we cannot see"),
      items: [
        t("what is said in a session, or typed and printed in a terminal"),
        t("session titles, folders, repositories and branches"),
        t("model names, which CLI, your accounts and quotas"),
        t("the names of your computers and devices, your files and their names"),
      ],
    },
    {
      icon: KeyRound,
      title: t("Who holds the keys"),
      items: [
        t("Device keys: made in each browser or phone, and kept only there"),
        t("A computer's identity key: on that computer only"),
        t("Recovery words: with you only; neither Droppen Hub nor we can recover them for you"),
        t("Us (Droppen Hub and relays): no key that can decrypt anything"),
      ],
    },
  ];
  return (
    <Section id="security">
      <SectionHead
        label={t("Security")}
        title={t("What we can see and what we cannot, line by line.")}
        lead={t("Our servers come in two kinds. Droppen Hub only finds your computers and keeps their encrypted copies; Droppen relays only pass ciphertext on. Neither can read what goes through it. Here is exactly where that line is.")}
      />
      <div className="grid border-t border-border md:grid-cols-3">
        {columns.map(({ icon: Icon, title, items }, i) => (
          <div key={title} className={cn("flex flex-col gap-4 px-6 py-6 max-md:px-4", i > 0 && "border-border max-md:border-t md:border-l")}>
            <span className="flex items-center gap-2">
              <Icon className="size-4 text-fg" />
              <span className="text-[13px] font-semibold text-fg">{title}</span>
            </span>
            <Points items={items} />
          </div>
        ))}
      </div>
      <div className="flex items-start gap-3 border-t border-border px-6 py-5 max-md:px-4">
        <Info className="mt-0.5 size-4 shrink-0 text-muted" />
        <p className="max-w-[60em] text-xs leading-5 text-muted">
          {t("Droppen is an invite-only beta. Its encryption design has had internal security reviews, not an external audit. In a browser you trust the page code as it was loaded that time: that code can use your private keys, and could copy them out.")}{" "}
          <a href={SECURITY_PATH} onClick={onPublicLink(SECURITY_PATH)} data-testid="landing-security" className="text-fg underline underline-offset-4 hover:no-underline">
            {t("How the encryption works, in detail")}
          </a>
        </p>
      </div>
    </Section>
  );
}

function Footer({ signedIn }: { signedIn: boolean }) {
  const { t } = useTranslation();
  return (
    <>
      {/* The invitation is for visitors; someone signed in has the way back in at the top. */}
      {!signedIn && (
        <Section>
          <div className="flex flex-col gap-6 px-6 py-14 max-md:px-4 max-md:py-10 md:flex-row md:items-end md:justify-between">
            <div className="flex flex-col gap-3">
              <CapsLabel>{t("Invite-only beta")}</CapsLabel>
              <p className="text-[32px] leading-[1.1] font-semibold tracking-[-0.03em] text-balance text-fg [word-break:auto-phrase] [&:lang(zh)]:[word-break:keep-all] max-md:text-[26px]">{t("Your AI keeps working. You can walk away.")}</p>
              <p className="text-sm leading-6 text-muted">{t("Sign up with your invite code, connect your computer, and pick up from your phone.")}</p>
            </div>
            <div className="flex flex-wrap gap-3">
              <a href={SIGN_UP_PATH} onClick={onPublicLink(SIGN_UP_PATH)} className={secondary}>
                {t("Get started")}
              </a>
              <a href={SIGN_IN_PATH} onClick={onPublicLink(SIGN_IN_PATH)} className={secondary}>
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
    meta?.setAttribute("content", t("Claude Code and Codex keep running on your own computer; from your phone you watch them, answer their questions and open a terminal. All your devices on one screen, end-to-end encrypted: we cannot see your code."));
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
 * (tests, previews).
 */
export default function LandingPage({
  signedIn = false,
  availability = AVAILABILITY,
  hints,
  cpu,
  release,
}: {
  signedIn?: boolean;
  availability?: Availability;
  hints?: EnvironmentHints;
  cpu?: string | null;
  release?: LatestRelease | null;
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
          <Hero signedIn={signedIn} offer={offer} availability={availability} release={latest} cpu={cpu === undefined ? detectedCpu : cpu} />
          <Section id="journey">
            <SectionHead
              label={t("End-to-end encrypted")}
              title={t("Your code, for your eyes only.")}
              lead={t("From the agent on your computer to the phone in your hand, a message can be read at the two ends and nowhere in between: not on the network, and not on our servers.")}
            />
            <div className="border-t border-border">
              <Journey />
            </div>
          </Section>
          <Section id="direct">
            <div className="grid lg:grid-cols-[minmax(0,1fr)_minmax(0,1.55fr)]">
              <div className="flex flex-col lg:border-r lg:border-border">
                <SectionHead
                  label={t("Direct path")}
                  title={t("Typing from your phone stays snappy.")}
                  lead={t("Whenever your phone and your computer can reach each other, keystrokes go straight across instead of taking the detour through a Droppen relay: on the same computer or network, with IPv6 on both ends, or through a NAT that can be punched through. When they cannot, you do nothing: the Droppen relay is always there and takes over by itself.")}
                />
                <Points
                  className="px-6 pb-8 max-md:px-4"
                  items={[
                    t("Just as private: the direct path runs its own end-to-end handshake, with the same encryption and the same admission rules as through a Droppen relay."),
                    t("No keystroke lost: if the direct path drops, traffic is back on the Droppen relay within 1.5 s, and unconfirmed keystrokes are sent again in order. None lost, doubled or reordered."),
                    t("So far about two in three testers got a direct path; the rest go through a Droppen relay automatically."),
                    t("Rather your other devices and the STUN servers did not learn your IP address? Turn off “Allow direct connections”."),
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
                  label={t("Offline copy")}
                  title={t("Lid closed? Your sessions are still there.")}
                  lead={t("While your computer is on, the agent encrypts what is new every few seconds and leaves it with Droppen Hub. Once the lid is closed, your phone fetches that encrypted copy from Droppen Hub and opens it on the spot.")}
                />
                <Points
                  className="px-6 pb-8 max-md:px-4"
                  items={[
                    t("Still only yours: Droppen Hub keeps session text only as encrypted pages, and its database holds metadata only."),
                    t("Read-only meanwhile: sending and terminals wait for your computer to come back."),
                    t("Opens at once next time: each device keeps its own copy, cleared when you sign out."),
                    t("What syncs: each computer's own sessions updated in the last 30 days."),
                  ]}
                />
              </div>
            </div>
          </Section>
          <Features />
          <Security />
          <Footer signedIn={signedIn} />
        </main>
      </div>
    </div>
  );
}
