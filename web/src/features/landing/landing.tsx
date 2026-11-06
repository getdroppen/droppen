import { type ReactNode, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { ArrowRight, CalendarClock, FolderOpen, Laptop as LaptopIcon, ListChecks, MessageSquareReply, SquarePlus } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { appHost } from "@/lib/hub/hosts";
import { cn } from "@/lib/cn";
import "./copy";
import type { SignupMode } from "@/lib/hub/api";
import type { LatestRelease } from "@/lib/hub/releases";
import { browserCpu, browserHints, detectKind, type EnvironmentHints, type HeroOffer, heroOffer } from "./detect";
import { type Availability, AVAILABILITY, DOWNLOAD_PATH } from "./get-droppen";
import { HeroGet } from "./hero-get";
import { ArrowLink, FaqAccordion, PublicLayout, Section, SOURCE_URL, usePageMeta } from "./page-kit";
import { PLAN_NUMBERS, PRO_PRICE, gb, yearlySaving } from "./pricing-plans";
import { AgentBurst } from "./agent-burst";
import { ProductTour } from "./product-tour";
import { useLatestRelease } from "./release-files";
import { publicLink, PRICING_PATH, SECURITY_PATH, SIGN_UP_PATH, WAITLIST_HREF } from "./route";
import { gridPaper } from "./figures/paper";
import { NetworkFigure } from "./figures/network/figure";
import { KeyFigure } from "./figures/key/figure";

/*
 * The landing page at `/` in cloud mode. A centred promise with one
 * device-aware action, the product, then the engineering, pricing and FAQ.
 * Loaded on demand so the app's own boot does not carry it.
 */

const WIDE = "mx-auto w-full max-w-[72rem]";

const TRUST = ["Free plan", "End-to-end encrypted", "Open-source client"] as const;

function TrustLines({ className }: { className?: string }) {
  const { t } = useTranslation();
  return (
    <div role="group" aria-label={t("Product facts")} className={cn("flex flex-wrap items-center justify-center gap-x-5 gap-y-2 text-xs text-muted", className)}>
      {TRUST.map((line) => (
        <span key={line} className="inline-flex items-center gap-2">
          <i aria-hidden="true" className="size-1 bg-border-strong" />
          {t(line)}
        </span>
      ))}
    </div>
  );
}

function Head({ title, action }: { title: ReactNode; action?: ReactNode }) {
  return (
    <div className="px-6 pt-16 pb-10 max-md:px-4 max-md:pt-14 max-md:pb-8">
      <div className="flex flex-wrap items-end justify-between gap-6">
        <div className="min-w-0">
          <h2 className="text-2xl leading-8 font-semibold tracking-tight text-balance text-fg cjk-wrap md:cjk-keep max-md:text-xl">{title}</h2>
        </div>
        {action}
      </div>
    </div>
  );
}

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
    <div className="relative mx-auto w-full max-w-[1200px] px-6 pt-16 max-lg:pt-12 max-md:px-4 max-md:pt-10">
      <div className="mx-auto flex w-full max-w-[46rem] flex-col items-center text-center">
        <h1 className="mb-6 max-w-[14em] text-5xl leading-display font-normal tracking-tighter text-balance text-fg cjk-wrap md:cjk-keep max-md:text-4xl">
          {t("Your computer's AI, from your phone.")}
        </h1>
        <p className="max-w-[36em] leading-6 text-pretty text-muted [overflow-wrap:anywhere] cjk-wrap md:cjk-keep">
          {t("Claude Code and Codex keep running on your own computers. From your phone or another device, check progress, answer questions, send instructions and open a terminal.")}
        </p>
      </div>
      {/* Where the screens used to be: the hero's action, with the marks of the agents it lists
          thrown around it like confetti when the page opens. The paper under it is the same
          drafting sheet the page's figures are drawn on. */}
      <div className="relative mt-2 max-md:mt-0">
        <div
          aria-hidden="true"
          className="pointer-events-none absolute inset-0 opacity-70 max-md:opacity-40 dark:opacity-50"
          style={{
            ...gridPaper(),
            // The paper is a soft halo around the action: clear under its words, faded at the edges.
            maskImage: "radial-gradient(ellipse 50% 50% at 50% 50%, transparent 45%, #000 62%, transparent 100%)",
            WebkitMaskImage: "radial-gradient(ellipse 50% 50% at 50% 50%, transparent 45%, #000 62%, transparent 100%)",
          }}
        />
        <AgentBurst />
        <div className="relative z-10 flex min-h-[24rem] w-full items-center justify-center px-6 py-10 max-md:min-h-[21rem] max-md:px-0 max-md:py-8">
          <div className="flex w-full max-w-[46rem] flex-col items-center text-center">
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
            {(offer === "all" || offer === "windows" || offer === "phone") && (
              <p className="mt-4 font-mono text-xs leading-5 text-muted">{t("macOS 13 or later · Linux · Windows in the browser · iOS and Android")}</p>
            )}
            <TrustLines className="mt-4" />
          </div>
        </div>
      </div>
    </div>
  );
}

function ProductShowcase() {
  const { t } = useTranslation();
  return (
    <Section id="tour" rule={false} seam={false}>
      <div className={cn(WIDE, "pb-16 max-md:pb-12")}>
        <Head title={t("The app, in three screens.")} />
        <div className="mx-6 max-md:mx-4">
          <ProductTour
            screens={[
              {
                value: "sessions",
                title: t("All computers, one list."),
                body: t("Your home Mac, the Linux machine at work and the server in the rack, on one account. Open it and see which one needs you, instead of signing in to each."),
                shot: "app-session",
                alt: t("Droppen on a Mac: a list of Claude Code and Codex sessions, and a session waiting for an answer"),
                device: "app",
              },
              {
                value: "split",
                title: t("Code on the left, terminal on the right."),
                body: t("When you need to watch two tasks, put a session and a terminal side by side. Changes, test output and errors in one window, with nothing copied and pasted."),
                shot: "app-split",
                alt: t("Droppen with three panes: two sessions and a terminal"),
                device: "app",
              },
              {
                value: "phone",
                title: t("Leave the computer, take over on your phone."),
                body: t("Open {{app}} in your phone's browser and the same sessions are there. The row marked “needs you” comes first; open it and answer.", { app: appHost() }),
                shot: "phone-needs-you",
                alt: t("Droppen on a phone: what needs you, and a session's question"),
                device: "phone",
              },
            ]}
          />
        </div>
      </div>
    </Section>
  );
}

function Features() {
  const { t } = useTranslation();
  const { free, pro } = PLAN_NUMBERS;
  const items = [
    { icon: ListChecks, title: t("Every session in one list"), body: t("All your computers are on one account. Every session that is running, waiting for an answer or has unread messages is in one list.") },
    { icon: MessageSquareReply, title: t("Answer questions"), body: t("Answer the AI's questions, add instructions while it works, or stop it if it goes off track.") },
    {
      icon: LaptopIcon,
      title: t("Read offline"),
      body: t("When your computer is online, an encrypted copy is saved to your phone. If it is shut down, asleep or offline, you can still read the last {{free}} days ({{pro}} on Pro).", { free: free.retentionDays, pro: pro.retentionDays }),
    },
    { icon: SquarePlus, title: t("Start a session from your phone"), body: t("Choose Claude Code or Codex, a computer and a folder, attach a screenshot if you want, and send.") },
    { icon: FolderOpen, title: t("Browse files"), body: t("Browse the folders on your computer and open files from your phone.") },
    { icon: CalendarClock, title: t("Scheduled tasks"), body: t("Run an instruction automatically on the computer you choose, for example every morning or every hour on weekdays.") },
  ];
  return (
    <Section id="features" rule={false} seam={false}>
      <div className={cn(WIDE, "pb-16 max-md:pb-12")}>
        <Head title={t("What you can do from your phone.")} />
        <ul className="mx-6 grid border border-border max-md:mx-4 sm:grid-cols-2 lg:grid-cols-3">
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
                <h3 className="text-base leading-6 font-semibold text-balance text-fg cjk-wrap cjk-keep">{title}</h3>
              </div>
              <span className="text-sm leading-6 text-pretty text-muted">{body}</span>
            </li>
          ))}
        </ul>
      </div>
    </Section>
  );
}

function Network() {
  const { t } = useTranslation();
  return (
    <Section id="direct" rule={false} seam={false}>
      <div className={cn(WIDE, "pb-16 max-md:pb-12")}>
        <Head title={t("Works when the connection is bad.")} />
        <NetworkFigure />
      </div>
    </Section>
  );
}

function Security() {
  const { t, i18n: active } = useTranslation();
  const linkClass = "whitespace-nowrap text-fg underline underline-offset-4 hover:no-underline";
  const facts = [
    { title: t("End-to-end encrypted"), body: t("Encrypted on your computer and decrypted only on your devices. Droppen Hub stores only encrypted copies and Droppen relays forward only encrypted data; neither holds a key that can decrypt them.") },
    { title: t("Only devices you approve can join"), body: t("A new phone or browser can join only when a device you already have approves it. If one is lost, remove it from another phone or browser of yours: it can't decrypt anything written afterwards, and what was already synced to it can't be recalled.") },
    {
      title: t("Open-source client"),
      body: t("The droppen command-line program, the Mac app and the web app are open source under AGPL-3.0. Droppen Hub is not open source."),
      link: (
        <a href={SOURCE_URL} data-testid="landing-source" className={linkClass}>
          {t("Read the source")}
        </a>
      ),
    },
  ];
  return (
    <Section id="security" rule={false} seam={false}>
      <div className={cn(WIDE, "pb-16 max-md:pb-12")}>
        <Head title={t("Security and encryption")} />
        <KeyFigure />
        <ul className="mx-6 mt-12 grid gap-x-12 gap-y-14 max-md:mx-4 lg:grid-cols-3">
          {facts.map(({ title, body, link }) => (
            <li key={title} className="flex flex-col gap-2.5">
              <h3 className="text-base leading-6 font-semibold text-fg cjk-wrap cjk-keep">{title}</h3>
              <span className="text-sm leading-6 text-pretty text-muted">
                {body}
                {link && (
                  <>
                    {gapBefore(active.language)}
                    {link}
                  </>
                )}
              </span>
            </li>
          ))}
        </ul>
        <div className="flex flex-wrap items-center gap-x-4 gap-y-2 px-6 pt-6 font-mono text-xs text-muted max-md:px-4">
          <span>{t("Beta · the encryption design has had internal review.")}</span>
          <a {...publicLink(SECURITY_PATH)} data-testid="landing-security" className="text-fg underline underline-offset-4 hover:no-underline">
            {t("How the encryption works")}
          </a>
        </div>
      </div>
    </Section>
  );
}

function Plan({
  name,
  badge,
  price,
  unit,
  points,
  cta,
  href,
}: {
  name: string;
  badge?: string;
  price: string;
  unit?: string;
  points: string[];
  cta: string;
  href: string;
}) {
  return (
    <div className="flex min-w-0 flex-col px-6 py-7 max-md:px-4">
      <div className="flex items-center gap-2 text-sm font-semibold text-fg">
        {name}
        {badge && <Badge className="h-[18px] bg-fg px-1.5 text-xs tracking-label text-bg uppercase shadow-none">{badge}</Badge>}
      </div>
      <div className="mt-4 font-mono text-3xl leading-none font-medium tracking-tight text-fg tabular-nums">
        {price}
        {unit && <small className="ml-1 text-xs font-normal tracking-normal text-muted">{unit}</small>}
      </div>
      <ul className="mt-5 flex flex-col gap-2">
        {points.map((point) => (
          <li key={point} className="flex gap-2 text-sm leading-6 text-muted">
            <span aria-hidden="true" className="font-semibold text-fg">·</span>
            <span>{point}</span>
          </li>
        ))}
      </ul>
      <a {...publicLink(href)} className="mt-5 inline-flex w-fit items-center gap-1.5 border-b border-border-strong pb-0.5 text-sm text-fg hover:border-fg">
        {cta}
        <ArrowRight className="size-3.5" />
      </a>
    </div>
  );
}

function Pricing({ signedIn, signup }: { signedIn: boolean; signup: SignupMode }) {
  const { t } = useTranslation();
  const { free, pro } = PLAN_NUMBERS;
  const closed = signup === "closed";
  const freeHref = signedIn || closed ? PRICING_PATH : SIGN_UP_PATH;
  return (
    <Section id="pricing" rule={false} seam={false}>
      <div className={cn(WIDE, "pb-16 max-md:pb-12")}>
        <Head
          title={t("Pricing")}
          action={
            <a {...publicLink(PRICING_PATH)} className="inline-flex flex-none items-center gap-2 border-b border-border-strong pb-0.5 text-sm text-fg hover:border-fg">
              {t("Compare all plans")}
              <ArrowRight className="size-4" />
            </a>
          }
        />
        <div className="mx-6 grid divide-y divide-border border border-border max-md:mx-4 lg:grid-cols-2 lg:divide-x lg:divide-y-0">
          <Plan
            name={t("Free")}
            price="$0"
            points={[t("{{n}} devices that run AI", { n: free.hosts }), t("{{devices}} devices, phones and browsers included", { devices: free.devices }), t("{{free}} days of offline access · {{gb}} GB relay", { free: free.retentionDays, gb: gb(free.transferBytes) })]}
            cta={signedIn || closed ? t("Compare all plans") : t("Create a free account")}
            href={freeHref}
          />
          <Plan
            name="Pro"
            badge={t("Recommended")}
            price={`$${PRO_PRICE.monthly}`}
            unit={t("USD / month")}
            points={[
              t("{{pro}} devices that run AI · {{devices}} devices", { pro: pro.hosts, devices: pro.devices }),
              t("{{pro}} days of offline access · {{gb}} GB relay", { pro: pro.retentionDays, gb: gb(pro.transferBytes) }),
              t("Extra usage is billed"),
            ]}
            cta={t("Choose Pro")}
            href={PRICING_PATH}
          />
        </div>
        <p className="px-6 pt-4 font-mono text-xs text-muted max-md:px-4">
          {t("${{yearly}} a year, about {{saving}}% off · prices in US dollars", { yearly: PRO_PRICE.yearly, saving: yearlySaving() })}
        </p>
      </div>
    </Section>
  );
}

function Faq() {
  const { t } = useTranslation();
  const { free, pro } = PLAN_NUMBERS;
  const items = [
    {
      q: t("Do I have to keep my computer on?"),
      a: t("No. The AI runs on your computer, but you can leave it. While it is offline, your phone can still read the last {{free}} days ({{pro}} on Pro) of synced sessions; sending messages and using the terminal wait until it is back online.", {
        free: free.retentionDays,
        pro: pro.retentionDays,
      }),
    },
    {
      q: t("Is there a Windows client?"),
      a: t("Droppen's web app opens in a Windows browser, but the AI has to run on a Mac or Linux computer. A Windows client has not been released."),
    },
    {
      q: t("Will my code be uploaded?"),
      a: t("Session content is encrypted on your computer before it leaves. Droppen Hub keeps encrypted copies, and relays forward only encrypted data; neither has the key. In a browser, you trust the page code as it was loaded that time."),
    },
    {
      q: t("Is the free plan enough?"),
      a: t("If you have up to {{hosts}} computers that run AI and need {{free}} days of offline access, the free plan is enough. Choose Pro for more devices, {{pro}} days offline or more relay traffic.", {
        hosts: free.hosts,
        free: free.retentionDays,
        pro: pro.retentionDays,
      }),
    },
  ];
  return (
    <Section id="faq" rule={false} seam={false}>
      <div className={cn(WIDE, "pb-16 max-md:pb-12")}>
        <div className="mx-6 grid gap-8 pt-16 max-md:mx-4 max-md:pt-14 lg:grid-cols-[minmax(0,0.45fr)_minmax(0,1fr)] lg:items-center lg:gap-16">
          <h2 className="text-2xl leading-8 font-semibold tracking-tight text-balance text-fg cjk-wrap md:cjk-keep max-md:text-xl">{t("FAQ")}</h2>
          <FaqAccordion
            items={items}
            testId="landing-faq"
            className="min-w-0 border-t border-border"
            triggerClassName="px-6 py-6 max-md:px-4"
            panelClassName="mt-2.5 px-6 pb-6 max-md:px-4"
            answerClassName="max-w-[52ch] text-sm text-pretty"
          />
        </div>
      </div>
    </Section>
  );
}

function FinalCta({ signup }: { signup: SignupMode }) {
  const { t } = useTranslation();
  const closed = signup === "closed";
  return (
    <Section id="start" rule={false} seam={false} className="relative overflow-hidden">
      <div
        aria-hidden="true"
        className="pointer-events-none absolute inset-0"
        style={{
          ...gridPaper(),
          maskImage: "radial-gradient(60% 70% at 50% 50%, #000 20%, transparent 100%)",
          WebkitMaskImage: "radial-gradient(60% 70% at 50% 50%, #000 20%, transparent 100%)",
          opacity: 0.7,
        }}
      />
      <div className="relative mx-auto w-full max-w-[46rem] px-6 py-20 text-center max-md:px-4 max-md:py-14">
        <h2 className="mx-auto max-w-[30em] text-3xl leading-tight font-semibold tracking-tight text-balance text-fg cjk-wrap md:cjk-keep max-md:text-2xl">
          {t("Your AI keeps working on your computer. When it needs you, answer from your phone.")}
        </h2>
        <div className="mt-8 flex flex-wrap justify-center gap-3">
          <ArrowLink {...publicLink(closed ? WAITLIST_HREF : SIGN_UP_PATH)}>{closed ? t("Join the waitlist") : t("Create a free account")}</ArrowLink>
          <ArrowLink {...publicLink(DOWNLOAD_PATH)}>{t("Download for Mac")}</ArrowLink>
        </div>
      </div>
    </Section>
  );
}

/**
 * At `/` whether or not the browser is signed in. `hints`, `cpu`, `release`
 * and `signup` stand in for the browser and the hub in tests and previews.
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
  usePageMeta(
    t("Droppen — your computer's AI, on your phone"),
    t("Claude Code and Codex keep running on your own Macs and Linux machines. From your phone or a browser, see every session across them, answer their questions, send instructions and open a terminal. End-to-end encrypted, with an open-source client."),
  );
  const offer = heroOffer(detectKind(hints ?? browserHints() ?? { userAgent: "" }), availability);
  const needsRelease = offer === "mac" || offer === "linux" || offer === "all";
  const latest = useLatestRelease(release === undefined && !needsRelease ? null : release);
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
    <PublicLayout testId="landing" signedIn={signedIn} home>
      <Hero signedIn={signedIn} signup={signup} offer={offer} availability={availability} release={latest} cpu={cpu === undefined ? detectedCpu : cpu} />
      <ProductShowcase />
      <Features />
      <Network />
      <Security />
      <Pricing signedIn={signedIn} signup={signup} />
      <Faq />
      {/* Signed in, there is nothing to start: the hero already opens the app. */}
      {!signedIn && <FinalCta signup={signup} />}
    </PublicLayout>
  );
}
