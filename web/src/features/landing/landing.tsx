import { useTranslation } from "react-i18next";
import { ArrowRightIcon, CalendarDotsIcon, ChatCircleDotsIcon, FolderOpenIcon, LaptopIcon, ListChecksIcon, PlusSquareIcon } from "@phosphor-icons/react";
import { appHost } from "@/lib/hub/hosts";
import { cn } from "@/lib/cn";
import "./copy";
import type { SignupMode } from "@/lib/hub/api";
import type { LatestRelease } from "@/lib/hub/releases";
import { browserHints, detectKind, type EnvironmentHints, type HeroOffer, heroOffer, useBrowserCpu } from "./detect";
import { type Availability, AVAILABILITY } from "./get-droppen";
import { HeroGet } from "./hero-get";
import { ArrowLink, FaqAccordion, LINK, NOWRAP_LINK, PAGE_WIDE, PublicLayout, RecommendedBadge, Section, SectionHead, SOURCE_URL, usePageMeta, SUBHEAD } from "./page-kit";
import { PLAN_NUMBERS, PRO_PRICE, gb, yearlySaving } from "./pricing-plans";
import { AgentBurst } from "./agent-burst";
import { ProductTour } from "./product-tour";
import { useLatestRelease } from "./release-files";
import { DOWNLOAD_PATH, publicLink, PRICING_PATH, SECURITY_PATH, SIGN_UP_PATH, WAITLIST_HREF } from "./route";
import { NetworkFigure } from "./figures/network/figure";
import { SIGNUP_COPY } from "./signup-copy";
import { KeyFigure } from "./figures/key/figure";

const SECTION_BODY = cn(PAGE_WIDE, "pb-24 max-md:pb-16");

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

function Hero({
  signedIn,
  signup,
  offer,
  availability,
  release,
  cpu,
}: {
  signedIn: boolean;
  signup: SignupMode;
  offer: HeroOffer;
  availability: Availability;
  release: LatestRelease | null | undefined;
  cpu: string | null;
}) {
  const { t } = useTranslation();
  const copy = SIGNUP_COPY[signup];
  return (
    <div className="relative isolate mx-auto flex min-h-[40rem] w-full items-center overflow-hidden px-6 pt-10 pb-16 max-md:min-h-0 max-md:px-4 max-md:pt-8 max-md:pb-14">
      <AgentBurst />
      <div
        aria-hidden="true"
        data-shape=""
        className="pointer-events-none absolute top-1/2 left-1/2 hidden h-[40rem] w-[72rem] -translate-x-1/2 -translate-y-1/2 [background:radial-gradient(closest-side,color-mix(in_oklab,var(--color-fg)_7%,transparent),color-mix(in_oklab,var(--color-fg)_4%,transparent)_45%,transparent_85%)] dark:block"
      />
      <div
        aria-hidden="true"
        data-shape=""
        className="pointer-events-none absolute top-1/2 left-1/2 h-[44rem] w-[76rem] -translate-x-1/2 -translate-y-1/2 [mask-image:radial-gradient(closest-side,black_0%,black_38%,rgb(0_0_0/0.94)_54%,rgb(0_0_0/0.55)_72%,rgb(0_0_0/0.16)_87%,transparent_100%)] bg-bg/88 max-md:h-[32rem] max-md:w-[88vw]"
        style={{
          backgroundImage: `url("data:image/svg+xml,%3Csvg width='180' height='180' viewBox='0 0 180 180' xmlns='http://www.w3.org/2000/svg'%3E%3Cfilter id='n'%3E%3CfeTurbulence type='fractalNoise' baseFrequency='.8' numOctaves='4' stitchTiles='stitch'/%3E%3CfeColorMatrix type='saturate' values='0'/%3E%3C/filter%3E%3Crect width='100%25' height='100%25' opacity='.22' filter='url(%23n)'/%3E%3C/svg%3E")`,
        }}
      />
      <div
        aria-hidden="true"
        data-shape=""
        className="pointer-events-none absolute top-1/2 left-1/2 h-[46rem] w-[86rem] -translate-x-1/2 -translate-y-1/2 backdrop-blur-xl [mask-image:radial-gradient(closest-side,black_0%,rgb(0_0_0/0.85)_30%,rgb(0_0_0/0.4)_58%,rgb(0_0_0/0.1)_80%,transparent_100%)] max-md:h-[34rem] max-md:w-[104vw]"
      />
      <div className="relative z-10 mx-auto flex w-full max-w-[46rem] flex-col items-center text-center">
        <h1 className="relative mb-6 max-w-[14em] text-6xl leading-display font-semibold tracking-tighter text-balance text-fg cjk-wrap md:cjk-keep max-md:mb-4 max-md:text-3xl max-md:leading-display">
          {t("Your computer's Agent, on your phone.")}
        </h1>
        <p className="relative max-w-[36em] leading-6 text-pretty text-muted [overflow-wrap:anywhere] cjk-wrap md:cjk-keep max-md:text-sm max-md:leading-6">
          {t("Claude Code and Codex keep running on your Mac and Linux machines. Monitor progress, approve tool permissions, and send instructions from any phone browser.")}
        </p>
        <div className="flex w-full items-center justify-center">
          <div className="flex w-full flex-col items-center text-center">
            <HeroGet offer={offer} signedIn={signedIn} availability={availability} release={release} cpu={cpu} signup={signup} />
            {!signedIn && copy.heroNote && copy.waitlistLabel && (
              <p className="mt-3 font-mono text-xs text-muted">
                {t(copy.heroNote)}{" "}
                <a {...publicLink(WAITLIST_HREF)} data-testid="hero-waitlist" className={NOWRAP_LINK}>
                  {t(copy.waitlistLabel)}
                </a>
              </p>
            )}
            {(offer === "all" || offer === "windows" || offer === "phone") && (
              <p className="mt-4 font-mono text-xs leading-5 text-muted max-md:mt-3">{t("macOS 13 or later · Linux · Windows in the browser · iOS and Android")}</p>
            )}
            <TrustLines className="relative mt-5 max-md:mt-4" />
          </div>
        </div>
      </div>
    </div>
  );
}

function ProductShowcase() {
  const { t } = useTranslation();
  return (
    <Section id="tour">
      <div className={cn(SECTION_BODY, "pt-16 max-md:pt-12")}>
        <div className="mx-6 max-md:mx-4 [--tour-picture:max(22rem,calc(100vh-19.5rem))] [--tour-picture:max(22rem,calc(100svh-19.5rem))]">
          <ProductTour
            screens={[
              {
                value: "sessions",
                title: t("All computers, one list."),
                body: t("Connect multiple Mac and Linux computers under one account. View all Claude Code and Codex sessions in real time and see immediately which task requires your input."),
                shot: "app-session",
                alt: t("Droppen on a Mac: a list of Claude Code and Codex sessions, and a session waiting for an answer"),
                device: "app",
              },
              {
                value: "split",
                title: t("Code on the left, terminal on the right."),
                body: t("Run an AI session and native terminal side by side. Inspect code changes, test logs, and command outputs in a single window without switching applications."),
                shot: "app-split",
                alt: t("Droppen with three panes: two sessions and a terminal"),
                device: "app",
              },
              {
                value: "phone",
                title: t("Leave the computer, take over on your phone."),
                body: t("Open {{app}} in any mobile browser to sync sessions instantly. Tasks awaiting confirmation are pinned to the top for quick approval and replies.", { app: appHost() }),
                shot: "phone-needs-you",
                alt: t("Droppen on a phone: what needs you, and a session's question"),
                device: "phones",
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
    { icon: ListChecksIcon, title: t("Every session in one list"), body: t("Aggregate Claude Code, Codex, and ACP agent sessions across all computers. Active, waiting, and unread tasks are prioritized in a single view.") },
    { icon: ChatCircleDotsIcon, title: t("Answer questions and approve actions"), body: t("Approve Bash commands and file edits, answer agent questions, add instructions during execution, or abort tasks immediately.") },
    {
      icon: LaptopIcon,
      title: t("Read offline"),
      body: t("When your computer is online, an encrypted copy syncs to your phone. If it shuts down or disconnects, you can still read the last {{free}} days ({{pro}} on Pro).", { free: free.retentionDays, pro: pro.retentionDays }),
    },
    { icon: PlusSquareIcon, title: t("Start a session from your phone"), body: t("Select Claude Code or Codex, choose a target computer and folder, attach screenshots, and dispatch new tasks directly from your phone.") },
    { icon: FolderOpenIcon, title: t("Browse files and terminals"), body: t("Browse project directories on your remote computers, inspect code diffs, and open native host terminals directly from your mobile browser.") },
    { icon: CalendarDotsIcon, title: t("Scheduled tasks"), body: t("Configure recurring schedules on target computers to run automated agent workflows, tests, or daily maintenance automatically.") },
  ];
  return (
    <Section id="features">
      <div className={SECTION_BODY}>
        <SectionHead compact title={t("What you can do from your phone.")} />
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
    <Section id="direct">
      <div className={SECTION_BODY}>
        <SectionHead compact title={t("Direct connection with automatic relay fallback.")} />
        <NetworkFigure />
      </div>
    </Section>
  );
}

function Security() {
  const { t } = useTranslation();
  const facts = [
    { title: t("End-to-end encrypted"), body: t("Encrypted on your computer and decrypted only on your devices. Droppen Hub stores only encrypted copies and Droppen relays forward only encrypted data; neither holds a key that can decrypt them.") },
    { title: t("Only devices you approve can join"), body: t("A new phone or browser can join only when an existing authorized device approves it. If a device is lost, remove it remotely: removed devices cannot decrypt subsequent data.") },
    {
      title: t("Open-source client"),
      body: t("The droppen command-line program, the Mac app and the web app are open source under AGPL-3.0. Droppen Hub is not open source."),
      link: (
        <a href={SOURCE_URL} data-testid="landing-source" className={NOWRAP_LINK}>
          {t("Read the source")}
        </a>
      ),
    },
  ];
  return (
    <Section id="security">
      <div className={SECTION_BODY}>
        <SectionHead compact title={t("Security and encryption")} />
        <KeyFigure />
        <ul className="mx-6 mt-12 grid gap-x-12 gap-y-14 max-md:mx-4 lg:grid-cols-3">
          {facts.map(({ title, body, link }) => (
            <li key={title} className="flex flex-col gap-2.5">
              <h3 className={cn(SUBHEAD, "cjk-wrap cjk-keep")}>{title}</h3>
              <span className="text-sm leading-6 text-pretty text-muted">
                {body}
                {link && <> {link}</>}
              </span>
            </li>
          ))}
        </ul>
        <div className="flex flex-wrap items-center gap-x-4 gap-y-2 px-6 pt-6 font-mono text-xs text-muted max-md:px-4">
          <span>{t("Beta · the encryption design has had internal review.")}</span>
          <a {...publicLink(SECURITY_PATH)} data-testid="landing-security" className={LINK}>
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
        {badge && <RecommendedBadge>{badge}</RecommendedBadge>}
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
        <ArrowRightIcon className="size-3.5" />
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
    <Section id="pricing">
      <div className={SECTION_BODY}>
        <SectionHead
          compact
          title={t("Pricing")}
          action={
            <a {...publicLink(PRICING_PATH)} className="inline-flex flex-none items-center gap-2 border-b border-border-strong pb-0.5 text-sm text-fg hover:border-fg">
              {t("Compare all plans")}
              <ArrowRightIcon className="size-4" />
            </a>
          }
        />
        <div className="mx-6 grid divide-y divide-border border border-border max-md:mx-4 lg:grid-cols-2 lg:divide-x lg:divide-y-0">
          <Plan
            name={t("Free")}
            price="$0"
            points={[t("{{n}} devices that run AI", { n: free.hosts }), t("{{devices}} devices, phones and browsers included", { devices: free.devices }), t("{{free}} days of offline access · {{gb}} GB relay", { free: free.retentionDays, gb: gb(free.transferBytes) })]}
            cta={signedIn || closed ? t("Compare all plans") : t(SIGNUP_COPY[signup].freeCta)}
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
      a: t("No. The AI runs on your computer, but you do not need to stay at your desk. While it is offline, your phone can still read the last {{free}} days ({{pro}} on Pro) of synced sessions. Sending prompts and using the terminal resume once it reconnects.", {
        free: free.retentionDays,
        pro: pro.retentionDays,
      }),
    },
    {
      q: t("Is there a Windows client?"),
      a: t("Droppen's web app runs in any Windows browser, but the AI must run on a Mac, Linux, or WSL 2 machine. A native Windows host client is planned."),
    },
    {
      q: t("Will my code be uploaded?"),
      a: t("Session content is encrypted on your computer before transmission. Droppen Hub stores only encrypted copies, and relays forward only encrypted data; neither holds decryption keys. In a browser, you trust the page code loaded during that session."),
    },
    {
      q: t("Is the free plan enough?"),
      a: t("If you have up to {{hosts}} computers that run AI and need {{free}} days of offline access, the free plan is sufficient. Upgrade to Pro for more devices, {{pro}} days of offline access, or additional relay bandwidth.", {
        hosts: free.hosts,
        free: free.retentionDays,
        pro: pro.retentionDays,
      }),
    },
  ];
  return (
    <Section id="faq">
      <div className={SECTION_BODY}>
        <div className="mx-6 grid gap-8 pt-16 max-md:mx-4 max-md:gap-6 max-md:pt-14 lg:grid-cols-[minmax(0,0.45fr)_minmax(0,1fr)] lg:items-center lg:gap-16">
          <h2 className="text-2xl leading-8 font-semibold tracking-tight text-balance text-fg cjk-wrap md:cjk-keep max-md:text-xl">{t("FAQ")}</h2>
          <FaqAccordion
            items={items}
            testId="landing-faq"
            className="min-w-0"
            triggerClassName="px-6 py-5 max-md:px-4 max-md:py-4"
            panelClassName="mt-1 px-6 pb-5 max-md:px-4"
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
    <Section id="start" className="relative overflow-hidden">
      <div className="relative mx-auto w-full max-w-[46rem] px-6 py-20 text-center max-md:px-4 max-md:py-14">
        <h2 className="mx-auto max-w-[30em] text-3xl leading-tight font-semibold tracking-tight text-balance text-fg cjk-wrap md:cjk-keep max-md:text-2xl">
          {t("Your AI keeps working on your computer. When it needs you, take control from your phone.")}
        </h2>
        <div className="mt-8 flex flex-wrap justify-center gap-3">
          <ArrowLink {...publicLink(closed ? WAITLIST_HREF : SIGN_UP_PATH)}>{t(SIGNUP_COPY[signup].freeCta)}</ArrowLink>
          <ArrowLink {...publicLink(DOWNLOAD_PATH)}>{t("Download for Mac")}</ArrowLink>
        </div>
      </div>
    </Section>
  );
}

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
    t("Droppen — your computer's Agent, on your phone"),
    t("Claude Code and Codex keep running on your Mac and Linux machines. Monitor sessions, approve tool permissions, answer prompts, and open terminals from any mobile browser. End-to-end encrypted, with an open-source client."),
  );
  const kind = detectKind(hints ?? browserHints() ?? { userAgent: "" });
  const offer = heroOffer(kind, availability);
  const needsRelease = offer === "mac" || offer === "linux" || offer === "all";
  const latest = useLatestRelease(release, needsRelease);
  const detectedCpu = useBrowserCpu({ ask: offer === "linux", cpu, nav: hints ? null : undefined });
  return (
    <PublicLayout testId="landing" signedIn={signedIn} home>
      <Hero signedIn={signedIn} signup={signup} offer={offer} availability={availability} release={latest} cpu={detectedCpu} />
      <ProductShowcase />
      <Features />
      <Network />
      <Security />
      <Pricing signedIn={signedIn} signup={signup} />
      <Faq />
      {!signedIn && <FinalCta signup={signup} />}
    </PublicLayout>
  );
}
