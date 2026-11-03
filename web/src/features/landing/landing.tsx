import { type ReactNode, useEffect, useId, useState } from "react";
import i18n from "i18next";
import { useTranslation } from "react-i18next";
import { motion, useReducedMotion } from "motion/react";
import { ArrowRight, CalendarClock, FolderOpen, Laptop as LaptopIcon, ListChecks, MessageSquareReply, SquarePlus } from "lucide-react";
import { appHost } from "@/lib/hub/hosts";
import { cn } from "@/lib/cn";
import { landingBundles } from "./copy";
import type { SignupMode } from "@/lib/hub/api";
import type { LatestRelease } from "@/lib/hub/releases";
import { browserCpu, browserHints, detectKind, type EnvironmentHints, type HeroOffer, heroOffer } from "./detect";
import { type Availability, AVAILABILITY, DOWNLOAD_PATH } from "./get-droppen";
import { HeroGet } from "./hero-get";
import { ArrowLink, cjkTracking, FaqAccordion, PublicLayout, Section, SOURCE_URL, usePageMeta } from "./page-kit";
import { PLAN_NUMBERS, PRO_PRICE, gb, yearlySaving } from "./pricing-plans";
import { AppWindow, BrowserWindow, Phone, ProductShot } from "./product-tour";
import { useLatestRelease } from "./release-files";
import { publicLink, PRICING_PATH, PRIVACY_PATH, SECURITY_PATH, SIGN_UP_PATH, TERMS_PATH, WAITLIST_HREF } from "./route";

/*
 * The landing page at `/` in cloud mode. A centred promise with one
 * device-aware action, the product, then the engineering, pricing and FAQ.
 * Loaded on demand so the app's own boot does not carry it.
 */

// Added, never overwriting: a key the app already translates keeps its wording.
for (const [lng, bundle] of Object.entries(landingBundles)) i18n.addResourceBundle(lng, "translation", bundle, true, false);

const WIDE = "mx-auto w-full max-w-[72rem]";

const gridPaper = (grid = 22) => ({
  backgroundImage: "linear-gradient(var(--color-border) 1px, transparent 1px), linear-gradient(90deg, var(--color-border) 1px, transparent 1px)",
  backgroundSize: `${grid}px ${grid}px`,
  backgroundPosition: "-1px -1px",
});

const TRUST = ["Free to start", "No credit card", "End-to-end encrypted", "Open-source client"] as const;

function TrustLines({ className }: { className?: string }) {
  const { t } = useTranslation();
  return (
    <div aria-label={t("Product facts")} className={cn("flex flex-wrap items-center justify-center gap-x-5 gap-y-2 text-xs text-muted", className)}>
      {TRUST.map((line) => (
        <span key={line} className="inline-flex items-center gap-2">
          <i aria-hidden="true" className="size-1 bg-border-strong" />
          {t(line)}
        </span>
      ))}
    </div>
  );
}

function Eyebrow({ children }: { children: ReactNode }) {
  return (
    <div className={cn("mb-3 flex items-center gap-2 font-mono text-xs tracking-label uppercase text-muted", cjkTracking)}>
      <span aria-hidden="true" className="h-px w-4 bg-border-strong" />
      {children}
    </div>
  );
}

function Head({ eyebrow, title, lead, action }: { eyebrow?: ReactNode; title: ReactNode; lead?: ReactNode; action?: ReactNode }) {
  return (
    <div className="flex flex-col gap-3 px-6 pt-12 pb-8 max-md:px-4 max-md:pt-10 max-md:pb-6">
      {eyebrow && <Eyebrow>{eyebrow}</Eyebrow>}
      <div className="flex flex-wrap items-end justify-between gap-6">
        <div className="min-w-0">
          <h2 className="text-2xl leading-8 font-semibold tracking-tight text-balance text-fg cjk-wrap md:cjk-keep max-md:text-xl">{title}</h2>
          {lead && <p className="mt-3 max-w-[42em] text-sm leading-6 text-pretty text-muted">{lead}</p>}
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
    <div className="mx-auto flex w-full max-w-[46rem] flex-col items-center px-6 pt-16 pb-10 text-center max-lg:pt-12 max-md:px-4 max-md:pt-10 max-md:pb-8">
      <h1 className="mb-6 max-w-[14em] text-5xl leading-display font-normal tracking-tighter text-balance text-fg cjk-wrap md:cjk-keep max-md:text-4xl">
        {t("Your computer's AI, from your phone.")}
      </h1>
      <p className="max-w-[36em] leading-6 text-pretty text-muted [overflow-wrap:anywhere] cjk-wrap md:cjk-keep">
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
      {(offer === "all" || offer === "windows" || offer === "phone") && (
        <p className="mt-4 font-mono text-xs leading-5 text-muted">{t("macOS 13 or later · Linux · Windows in the browser · iOS and Android")}</p>
      )}
      <TrustLines className="mt-4" />
    </div>
  );
}

function HeroStage() {
  const { t } = useTranslation();
  const session = t("Droppen on a Mac: a list of Claude Code and Codex sessions, and a session waiting for an answer");
  return (
    <div className="relative mt-10 pb-2 max-lg:mt-8">
      <div
        aria-hidden="true"
        className="pointer-events-none absolute inset-0"
        style={{
          ...gridPaper(),
          maskImage: "radial-gradient(75% 80% at 50% 45%, #000 35%, transparent 100%)",
          WebkitMaskImage: "radial-gradient(75% 80% at 50% 45%, #000 35%, transparent 100%)",
          opacity: 0.75,
        }}
      />
      <div className="relative mx-auto hidden w-full max-w-[1120px] px-8 lg:block">
        <div className="relative aspect-[1200/620] w-full">
          <div className="absolute top-[24%] left-[-1%] z-10 w-[27%]">
            <Laptop>
              <BrowserWindow>
                <ProductShot shot="app-session" alt="" width={2560} height={1600} />
              </BrowserWindow>
            </Laptop>
          </div>
          <div className="absolute top-[4%] left-[15%] z-20 w-[40%]">
            <Laptop>
              <AppWindow>
                <ProductShot shot="app-session" alt={session} eager width={2560} height={1600} />
              </AppWindow>
            </Laptop>
          </div>
          <div className="absolute top-[15%] left-[52%] z-30 w-[21%]">
            <Tablet>
              <ProductShot shot="app-session" alt="" width={2560} height={1600} />
            </Tablet>
          </div>
          <div className="absolute top-[25%] left-[82%] z-40 w-[13%]">
            <Android>
              <ProductShot shot="phone-needs-you" alt="" width={640} height={1386} />
            </Android>
          </div>
          <div className="absolute top-[34%] left-[70%] z-50 w-[13%]">
            <Phone>
              <ProductShot shot="phone-session" alt={t("Droppen on a phone: what needs you, and a session's question")} width={640} height={1386} />
            </Phone>
          </div>
        </div>
      </div>
      <div className="relative flex justify-center px-6 lg:hidden">
        <div className="w-[264px] max-w-full">
          <Phone>
            <ProductShot shot="phone-session" alt={t("Droppen on a phone: what needs you, and a session's question")} eager width={640} height={1386} />
          </Phone>
        </div>
      </div>
      <p className="mx-auto mt-7 max-w-[64rem] px-6 text-center font-mono text-xs leading-5 text-muted max-md:px-4">
        {t("macOS and Linux run Droppen natively · Windows, iPad, iPhone and Android in the browser · one account, and your sessions follow")}
      </p>
    </div>
  );
}

function Laptop({ children }: { children: ReactNode }) {
  return (
    <div className="relative">
      <div className="overflow-hidden rounded-[10px] border-[6px] border-raised bg-raised shadow-[0_30px_80px_-44px_rgb(15_18_22/0.38)]">{children}</div>
      <div className="mx-[-6%] h-2.5 rounded-b-[8px] bg-raised" />
      <div className="mx-auto h-[3px] w-[13%] rounded-b-[3px] bg-border-strong" />
    </div>
  );
}

function Tablet({ children }: { children: ReactNode }) {
  return (
    <div className="overflow-hidden rounded-[18px] border-[8px] border-raised bg-raised shadow-[0_30px_80px_-44px_rgb(15_18_22/0.38)]">
      <div className="relative overflow-hidden rounded-[10px] bg-bg">
        <span aria-hidden="true" className="absolute top-1 left-1/2 z-1 size-1 -translate-x-1/2 rounded-full bg-border-strong" />
        {children}
      </div>
    </div>
  );
}

function Android({ children }: { children: ReactNode }) {
  return (
    <div className="overflow-hidden rounded-[22px] border-[5px] border-raised bg-raised shadow-[0_30px_80px_-44px_rgb(15_18_22/0.38)]">
      <div className="relative overflow-hidden rounded-[17px] bg-bg">
        <div aria-hidden="true" className="relative flex h-5 items-center justify-between px-2 font-mono text-xs text-fg">
          <span>9:41</span>
          <span className="absolute top-1.5 left-1/2 size-1.5 -translate-x-1/2 rounded-full bg-fg" />
          <span className="flex items-end gap-px">
            {[3, 5, 7].map((height) => (
              <span key={height} className="w-px bg-fg" style={{ height }} />
            ))}
          </span>
        </div>
        {children}
      </div>
    </div>
  );
}

function Focus({ left, top, width, height, label }: { left: string; top: string; width: string; height: string; label: string }) {
  return (
    <span data-signal="waiting" className="pointer-events-none absolute border border-warn" style={{ left, top, width, height }}>
      <b className="absolute -top-px -left-px bg-warn px-1.5 py-1 font-mono text-xs font-medium whitespace-nowrap text-on-signal">{label}</b>
    </span>
  );
}

function ProductRow({
  n,
  title,
  body,
  points,
  caption,
  reverse,
  figure,
}: {
  n: string;
  title: string;
  body: string;
  points: string[];
  caption: string;
  reverse?: boolean;
  figure: ReactNode;
}) {
  return (
    <article
      className={cn(
        "grid items-center gap-8 border-b border-border py-12 max-lg:gap-6 max-lg:py-10 lg:gap-16",
        reverse ? "lg:grid-cols-[minmax(0,1.2fr)_minmax(0,0.8fr)]" : "lg:grid-cols-[minmax(0,0.8fr)_minmax(0,1.2fr)]",
      )}
    >
      <div className={cn("min-w-0", reverse && "lg:order-2")}>
        <div className="font-mono text-xs tracking-label uppercase text-muted">{n}</div>
        <h3 className="mt-3 text-xl leading-7 font-semibold tracking-tight text-balance text-fg cjk-wrap md:cjk-keep max-md:text-lg">{title}</h3>
        <p className="mt-3 max-w-[44ch] text-sm leading-6 text-pretty text-muted">{body}</p>
        <ul className="mt-5 flex flex-col gap-2">
          {points.map((point) => (
            <li key={point} className="relative pl-4 text-sm leading-6 text-muted">
              <span aria-hidden="true" className="absolute top-[0.7em] left-0 size-1 bg-border-strong" />
              {point}
            </li>
          ))}
        </ul>
      </div>
      <figure className={cn("min-w-0", reverse && "lg:order-1")}>
        {figure}
        <figcaption className="mt-3 text-xs leading-5 text-muted">{caption}</figcaption>
      </figure>
    </article>
  );
}

function ProductShowcase() {
  const { t } = useTranslation();
  const { free, pro } = PLAN_NUMBERS;
  return (
    <Section id="tour">
      <div className={cn(WIDE, "pb-12 max-md:pb-10")}>
        <Head
          eyebrow={t("Product interface")}
          title={t("What it actually looks like.")}
          lead={t("No new workflow to learn. The sessions, terminal and projects you know, carried on from another screen.")}
        />
        <div className="border-t border-border px-6 max-md:px-4">
          <ProductRow
            n={`01 / ${t("Queue")}`}
            title={t("Every computer, one queue.")}
            body={t("Your home Mac, the Linux machine at work and the server in the rack, on one account. Open it and see which one needs you, instead of signing in to each.")}
            points={[t("Amber is “waiting for you”; green is “working”."), t("Unread, running and waiting for an answer, sorted by state.")]}
            caption={t("Every device's state in one column; an amber row left alone stops the AI there.")}
            figure={
              <AppWindow>
                <div className="relative">
                  <ProductShot shot="app-session" alt={t("Droppen on a Mac: a list of Claude Code and Codex sessions, and a session waiting for an answer")} width={2560} height={1600} />
                  <Focus left="3.6%" top="8.2%" width="17.5%" height="9.6%" label={t("Waiting for you / working")} />
                </div>
              </AppWindow>
            }
          />
          <ProductRow
            reverse
            n={`02 / ${t("Split")}`}
            title={t("Code on the left, terminal on the right.")}
            body={t("When you need to watch two tasks, put a session and a terminal side by side. Changes, test output and errors in one window, with nothing copied and pasted.")}
            points={[t("Two independent sessions that do not interrupt each other."), t("The terminal is a full session, not a read-only log.")]}
            caption={t("The terminal, diffs and test results stay in one view; the mouse does not fly between windows.")}
            figure={
              <AppWindow>
                <div className="relative">
                  <ProductShot shot="app-split" alt={t("Droppen with three panes: two sessions and a terminal")} width={2560} height={1600} />
                  <Focus left="55.2%" top="60.4%" width="43.6%" height="38%" label={t("The terminal is right there")} />
                </div>
              </AppWindow>
            }
          />
          <ProductRow
            n={`03 / ${t("Phone")}`}
            title={t("Leave the computer, take over on your phone.")}
            body={t("Open {{app}} in your phone's browser and the same sessions are there. The amber “needs you” row comes first; open it and answer.", { app: appHost() })}
            points={[
              t("Answer questions, add instructions, stop a run."),
              t("With the computer off, the last {{free}} days stay readable ({{pro}} on Pro).", { free: free.retentionDays, pro: pro.retentionDays }),
            ]}
            caption={t("The phone is not a shrunken desktop: the conclusion first, the detail after.")}
            figure={
              <div className="flex justify-center py-2">
                <div className="w-[300px] max-w-full">
                  <Phone>
                    <div className="relative">
                      <ProductShot shot="phone-needs-you" alt={t("Droppen on a phone: what needs you, and a session's question")} width={640} height={1386} />
                      <Focus left="3%" top="58.6%" width="94%" height="23.5%" label={t("Only what needs you")} />
                    </div>
                  </Phone>
                </div>
              </div>
            }
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
    { icon: ListChecks, title: t("Every computer, one list"), body: t("Home and work computers on one account, with every session that is waiting for an answer, has unread messages or is running in one list.") },
    { icon: MessageSquareReply, title: t("Answer and steer"), body: t("Answer the AI's questions, add a message while it works, or stop the current run if it goes the wrong way.") },
    {
      icon: LaptopIcon,
      title: t("Readable with the lid closed"),
      body: t("While the computer is online, an encrypted copy is synced. Shut down, asleep or offline, your phone can still read the last {{free}} days ({{pro}} on Pro).", { free: free.retentionDays, pro: pro.retentionDays }),
    },
    { icon: SquarePlus, title: t("Start a session from your phone"), body: t("Choose Claude Code or Codex, a computer and a folder, optionally attach a screenshot, and send.") },
    { icon: FolderOpen, title: t("Browse your files"), body: t("Browse the folders on your computer and preview files from your phone.") },
    { icon: CalendarClock, title: t("Run prompts on a schedule"), body: t("Have a prompt run automatically on the computer you choose, for example every morning or every hour on weekdays.") },
  ];
  return (
    <Section id="features">
      <div className={cn(WIDE, "pb-12 max-md:pb-10")}>
        <Head
          eyebrow={t("Why Droppen")}
          title={t("Leaving your computer is not leaving the work.")}
          lead={t("The AI keeps running on your computer; you only step in for the few seconds that need you.")}
        />
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

const NETWORK = { direct: [63, 73] as const, relay: [215, 227] as const };

const PATH = {
  direct: "M176 300C330 386 646 386 800 300",
  relay: "M176 286C270 214 350 168 466 148M536 148C650 170 730 226 800 286",
  phoneDirect: "M92 140C165 190 150 300 196 330",
  phoneRelay: "M92 80C150 60 195 80 230 96M268 132C300 200 300 280 294 322",
};

function Packet({ path, slow = false }: { path: string; slow?: boolean }) {
  const reduced = useReducedMotion();
  if (reduced) return null;
  return (
    <motion.circle
      r={3.4}
      className={slow ? "fill-warn" : "fill-ok"}
      style={{ offsetPath: `path("${path}")` }}
      initial={{ offsetDistance: "0%" }}
      animate={{ offsetDistance: "100%" }}
      transition={{ duration: slow ? 6 : 4.2, repeat: Infinity, ease: "linear", delay: slow ? -2 : 0 }}
    />
  );
}

function NetworkDiagram() {
  const { t } = useTranslation();
  const id = useId();
  const title = t("The direct and relay paths between your phone and your computer");
  const desc = t("Your phone reaches your computer directly when it can; when it cannot, it goes through a Droppen relay. The relay takes over within 1.5 s and unconfirmed keys are resent in order.");
  const direct = `${NETWORK.direct[0]}–${NETWORK.direct[1]} ms`;
  const relay = `${NETWORK.relay[0]}–${NETWORK.relay[1]} ms`;
  const label = "font-mono text-xs fill-muted";
  const strong = "font-mono text-xs fill-fg";
  const soft = "fill-none stroke-muted";
  const line = "fill-none stroke-fg";
  return (
    <figure className="relative mx-6 border border-border-strong bg-bg max-md:mx-4" style={gridPaper()}>
      <span className={cn("absolute top-0 left-0 z-1 border-r border-b border-border bg-bg px-2.5 py-1.5 font-mono text-xs tracking-label text-muted uppercase", cjkTracking)}>
        {t("Connection path · direct first")}
      </span>
      <svg className="hidden w-full text-fg md:block" viewBox="0 0 1000 440" role="img" aria-labelledby={`${id}-title ${id}-desc`}>
        <title id={`${id}-title`}>{title}</title>
        <desc id={`${id}-desc`}>{desc}</desc>

        <g className={soft} strokeWidth={1.2}>
          <path d="M40 60h140" />
          <path d="M40 60v14" />
          <path d="M180 60v14" />
        </g>
        <text x="40" y="48" className={label}>{t("P2P / direct first")}</text>

        <g>
          <path d="M760 62h54" className={line} strokeWidth={1.5} />
          <text x="824" y="66" className={label}>{t("direct")}</text>
          <path d="M900 62h30" className={soft} strokeWidth={1.3} strokeDasharray="5 5" />
          <text x="974" y="66" className={label} textAnchor="end">{t("relay")}</text>
        </g>

        <path d={PATH.direct} className={line} strokeWidth={1.5} />
        <path d="M176 286C270 214 350 168 466 148" className={soft} strokeWidth={1.3} strokeDasharray="5 5" />
        <path d="M536 148C650 170 730 226 800 286" className={soft} strokeWidth={1.3} strokeDasharray="5 5" />
        <Packet path={PATH.direct} />
        <Packet path={PATH.relay} slow />
        <circle cx="176" cy="300" r="3" className="fill-ok" />
        <circle cx="800" cy="300" r="3" className="fill-ok" />
        <circle cx="500" cy="365" r="3.4" className="fill-ok" />
        <circle cx="658" cy="189" r="3.4" className="fill-warn" />

        <g className="fill-none stroke-fg" strokeWidth={1.4} strokeLinejoin="round">
          <rect x="100" y="248" width="76" height="128" rx="12" />
          <rect x="108" y="260" width="60" height="96" rx="2" className={soft} strokeWidth={1.2} />
          <path d="M126 255h24" className={soft} strokeWidth={1.2} />
          <circle cx="138" cy="366" r="2.6" className={soft} strokeWidth={1.2} />
        </g>
        <text x="138" y="410" className={strong} textAnchor="middle">{t("Your phone")}</text>
        <text x="138" y="428" className={label} textAnchor="middle">{t("Any network")}</text>

        <g className="fill-none stroke-fg" strokeWidth={1.4} strokeLinejoin="round">
          <path d="M500 82l46 23-46 23-46-23Z" />
          <path d="M454 105v40l46 23v-40Z" />
          <path d="M546 105v40l-46 23v-40Z" />
          <path d="M500 128v40" className={soft} strokeWidth={1.2} />
          <path d="M500 64v18" className={soft} strokeWidth={1.2} />
          <circle cx="500" cy="60" r="3" />
        </g>
        <text x="500" y="196" className={strong} textAnchor="middle">{t("Droppen relay")}</text>
        <text x="500" y="214" className={label} textAnchor="middle">{t("Fallback · ciphertext only")}</text>

        <g className="fill-none stroke-fg" strokeWidth={1.4} strokeLinejoin="round">
          <rect x="792" y="238" width="120" height="78" rx="3" />
          <rect x="802" y="248" width="100" height="58" className={soft} strokeWidth={1.2} />
          <path d="M772 316h160l14 18H758Z" />
          <path d="M834 324h36" className={soft} strokeWidth={1.2} />
        </g>
        <text x="852" y="368" className={strong} textAnchor="middle">{t("Your computer")}</text>
        <text x="852" y="386" className={label} textAnchor="middle">{t("The session runs here")}</text>

        <rect x="406" y="350" width="188" height="30" className="fill-bg stroke-border-strong" strokeWidth={1} />
        <text x="500" y="370" className={strong} textAnchor="middle">{t("Direct · {{ms}}", { ms: direct })}</text>

        <rect x="586" y="98" width="212" height="30" className="fill-bg stroke-border-strong" strokeWidth={1} />
        <text x="692" y="118" className={label} textAnchor="middle">{t("Relay · {{ms}}", { ms: relay })}</text>

        <text x="500" y="246" className={label} textAnchor="middle">{t("Switches within 1.5 s")}</text>
        <path d="M500 222v14" className={soft} strokeWidth={1.2} />
      </svg>

      <svg className="block w-full text-fg md:hidden" viewBox="0 0 330 500" role="img" aria-labelledby={`${id}-m-title ${id}-m-desc`}>
        <title id={`${id}-m-title`}>{title}</title>
        <desc id={`${id}-m-desc`}>{desc}</desc>

        <text x="20" y="44" className={label}>{t("direct first")}</text>
        <path d="M20 56h60" className={soft} strokeWidth={1.2} />

        <g className="fill-none stroke-fg" strokeWidth={1.4} strokeLinejoin="round">
          <rect x="28" y="66" width="64" height="112" rx="10" />
          <rect x="35" y="77" width="50" height="84" rx="2" className={soft} strokeWidth={1.2} />
          <circle cx="60" cy="169" r="2.4" className={soft} strokeWidth={1.2} />
        </g>
        <text x="60" y="198" className={strong} textAnchor="middle">{t("Your phone")}</text>

        <g className="fill-none stroke-fg" strokeWidth={1.4} strokeLinejoin="round">
          <path d="M268 76l34 17-34 17-34-17Z" />
          <path d="M234 93v30l34 17v-30Z" />
          <path d="M302 93v30l-34 17v-30Z" />
          <path d="M268 110v30" className={soft} strokeWidth={1.2} />
          <path d="M268 62v14" className={soft} strokeWidth={1.2} />
          <circle cx="268" cy="58" r="2.6" />
        </g>
        <text x="268" y="158" className={strong} textAnchor="middle">{t("Droppen relay")}</text>
        <text x="268" y="174" className={label} textAnchor="middle">{t("Only ciphertext")}</text>

        <g className="fill-none stroke-fg" strokeWidth={1.4} strokeLinejoin="round">
          <rect x="196" y="322" width="96" height="62" rx="3" />
          <rect x="204" y="330" width="80" height="46" className={soft} strokeWidth={1.2} />
          <path d="M180 384h128l12 15H168Z" />
        </g>
        <text x="244" y="420" className={strong} textAnchor="middle">{t("Your computer")}</text>

        <path d={PATH.phoneDirect} className={line} strokeWidth={1.5} />
        <Packet path={PATH.phoneDirect} />
        <circle cx="92" cy="140" r="2.8" className="fill-ok" />
        <circle cx="196" cy="330" r="2.8" className="fill-ok" />
        <circle cx="148" cy="250" r="3" className="fill-ok" />
        <rect x="10" y="250" width="132" height="26" className="fill-bg stroke-border-strong" strokeWidth={1} />
        <text x="76" y="267" className={strong} textAnchor="middle">{t("Direct · {{ms}}", { ms: direct })}</text>

        <path d="M92 80C150 60 195 80 230 96" className={soft} strokeWidth={1.3} strokeDasharray="5 5" />
        <path d="M268 132C300 200 300 280 294 322" className={soft} strokeWidth={1.3} strokeDasharray="5 5" />
        <Packet path={PATH.phoneRelay} slow />
        <circle cx="268" cy="210" r="3" className="fill-warn" />
        <rect x="150" y="196" width="126" height="26" className="fill-bg stroke-border-strong" strokeWidth={1} />
        <text x="213" y="213" className={label} textAnchor="middle">{t("Relay · {{ms}}", { ms: relay })}</text>

        <text x="180" y="462" className={label} textAnchor="middle">{t("Switches within 1.5 s")}</text>
        <text x="180" y="480" className={label} textAnchor="middle">{t("Unconfirmed keys resend in order")}</text>
      </svg>
    </figure>
  );
}

function NetworkStats() {
  const { t } = useTranslation();
  const stats = [
    { value: `${NETWORK.direct[0]}–${NETWORK.direct[1]}`, unit: "ms", label: t("Direct median round trip") },
    { value: `${NETWORK.relay[0]}–${NETWORK.relay[1]}`, unit: "ms", label: t("Through a Droppen relay") },
    { value: "0", unit: t("Keys lost"), label: t("Resent in order after a drop") },
  ];
  return (
    <div className="mx-6 grid border-x border-b border-border max-md:mx-4 sm:grid-cols-3">
      {stats.map(({ value, unit, label }, i) => (
        <div key={label} className={cn("px-5 py-5", i > 0 && "border-border max-sm:border-t sm:border-l")}>
          <div className="font-mono text-2xl leading-8 font-medium tracking-tight text-fg tabular-nums">
            {value}
            <small className="ml-1 text-xs font-normal tracking-normal text-muted">{unit}</small>
          </div>
          <div className="mt-1 text-xs leading-5 text-muted">{label}</div>
        </div>
      ))}
    </div>
  );
}

function Network() {
  const { t } = useTranslation();
  return (
    <Section id="direct">
      <div className={cn(WIDE, "pb-12 max-md:pb-10")}>
        <Head
          eyebrow={t("Built for real networks")}
          title={t("A weak network does not stop the work.")}
          lead={t("Keystrokes go straight to your computer when your phone can reach it, and through a Droppen relay when it can't; if the direct path drops, the relay takes over within 1.5 s and resends unconfirmed keystrokes in order, so none are lost.")}
        />
        <NetworkDiagram />
        <div className="mx-6 max-md:mx-4">
          <NetworkStats />
        </div>
      </div>
    </Section>
  );
}

function KeyBoundary() {
  const { t } = useTranslation();
  const id = useId();
  const title = t("The key boundary of end-to-end encryption");
  const desc = t("Your computer and your phone each hold a key. Droppen Hub stores only encrypted copies, and relays forward only ciphertext; neither has the key.");
  const label = "font-mono text-xs";
  return (
    <figure className="mx-6 border border-border max-md:mx-4">
      <div className={cn("border-b border-border px-3 py-2 font-mono text-xs tracking-label text-muted uppercase", cjkTracking)}>{t("Key boundary · end-to-end encryption")}</div>
      <svg className="hidden w-full text-fg md:block" viewBox="0 0 1000 360" role="img" aria-labelledby={`${id}-title ${id}-desc`}>
        <title id={`${id}-title`}>{title}</title>
        <desc id={`${id}-desc`}>{desc}</desc>
        <g fill="none" stroke="currentColor" strokeWidth={1.4} strokeLinejoin="round">
          <rect x="86" y="88" width="132" height="88" rx="3" />
          <rect x="96" y="98" width="112" height="68" strokeOpacity={0.45} />
          <path d="M66 176h172l16 20H50Z" />
          <path d="M132 184h40" strokeOpacity={0.45} />
          <path d="M128 118h48M128 132h34" strokeOpacity={0.45} />
        </g>
        <g fill="none" stroke="currentColor">
          <rect x="806" y="76" width="82" height="136" rx="12" strokeWidth={1.4} />
          <rect x="814" y="88" width="66" height="104" rx="2" strokeOpacity={0.45} />
          <path d="M832 82h30" strokeOpacity={0.45} />
          <circle cx="847" cy="202" r="2.6" strokeOpacity={0.45} />
          <path d="M824 112h46M824 126h34" strokeOpacity={0.45} />
        </g>
        <text x="152" y="224" textAnchor="middle" className={cn(label, "fill-current")}>{t("Your computer")}</text>
        <text x="152" y="242" textAnchor="middle" className={cn(label, "fill-current")} opacity={0.55}>{t("Encrypted here")}</text>
        <text x="847" y="238" textAnchor="middle" className={cn(label, "fill-current")}>{t("Your phone")}</text>
        <text x="847" y="256" textAnchor="middle" className={cn(label, "fill-current")} opacity={0.55}>{t("Decrypted here")}</text>

        <path d="M234 150h556" stroke="currentColor" strokeWidth={1.4} />
        <g fill="none" stroke="currentColor" strokeWidth={1.2}>
          <rect x="394" y="137" width="22" height="17" rx="1" />
          <path d="M394 137l11 8 11-8" strokeOpacity={0.55} />
          <rect x="584" y="137" width="22" height="17" rx="1" />
          <path d="M584 137l11 8 11-8" strokeOpacity={0.55} />
          <rect x="401" y="146" width="8" height="8" strokeOpacity={0.55} />
          <rect x="591" y="146" width="8" height="8" strokeOpacity={0.55} />
          <path d="M405 92v58M595 92v58" strokeDasharray="5 5" strokeOpacity={0.5} />
          <rect x="320" y="38" width="170" height="54" />
          <rect x="510" y="38" width="170" height="54" />
        </g>
        <rect x="380" y="196" width="240" height="28" fill="var(--color-fg)" stroke="currentColor" strokeWidth={1} />
        <text x="500" y="215" textAnchor="middle" className={cn(label, "fill-bg")}>{t("End-to-end encrypted · ciphertext")}</text>
        <text x="405" y="60" textAnchor="middle" className={cn(label, "fill-current")}>DROPPEN HUB</text>
        <text x="405" y="78" textAnchor="middle" className={cn(label, "fill-current")} opacity={0.55}>{t("Only encrypted copies")}</text>
        <text x="595" y="60" textAnchor="middle" className={cn(label, "fill-current")}>{t("Droppen relay")}</text>
        <text x="595" y="78" textAnchor="middle" className={cn(label, "fill-current")} opacity={0.55}>{t("Only ciphertext")}</text>

        <KeyGlyph x={112} y={278} ok />
        <text x="136" y="282" className={cn(label, "fill-current")} opacity={0.8}>{t("The key stays on this device")}</text>
        <KeyGlyph x={874} y={278} ok />
        <text x="850" y="282" textAnchor="end" className={cn(label, "fill-current")} opacity={0.8}>{t("The key stays on this device")}</text>
        <KeyGlyph x={340} y={114} slashed />
        <text x="364" y="118" className={cn(label, "fill-current")} opacity={0.55}>{t("No key")}</text>
        <KeyGlyph x={640} y={114} slashed />
        <text x="664" y="118" className={cn(label, "fill-current")} opacity={0.55}>{t("No key")}</text>
        <text x="500" y="310" textAnchor="middle" className={cn(label, "fill-current")} opacity={0.55}>{t("Keys are negotiated between your devices and never pass through Droppen Hub or a relay.")}</text>
      </svg>

      <svg className="block w-full text-fg md:hidden" viewBox={`0 0 ${330} ${512}`} role="img" aria-labelledby={`${id}-m-title ${id}-m-desc`}>
        <title id={`${id}-m-title`}>{title}</title>
        <desc id={`${id}-m-desc`}>{desc}</desc>
        <g fill="none" stroke="currentColor" strokeWidth={1.4}>
          <rect x="108" y="10" width="114" height="72" rx="3" />
          <rect x="116" y="18" width="98" height="56" strokeOpacity={0.45} />
          <path d="M92 82h146l14 18H78Z" />
          <path d="M165 140v34" />
          <rect x="20" y="174" width="290" height="48" />
          <rect x="20" y="236" width="290" height="48" />
          <path d="M165 222v14" />
          <path d="M165 284v8" />
          <path d="M165 316v10" />
          <rect x="131" y="326" width="68" height="112" rx="10" />
          <rect x="138" y="336" width="54" height="84" rx="2" strokeOpacity={0.45} />
          <circle cx="165" cy="423" r="2.4" strokeOpacity={0.45} />
        </g>
        <text x="165" y="118" textAnchor="middle" className={cn(label, "fill-current")}>{t("Your computer")}</text>
        <text x="165" y="134" textAnchor="middle" className={cn(label, "fill-current")} opacity={0.55}>{t("Encrypted here")}</text>
        <text x="165" y="192" textAnchor="middle" className={cn(label, "fill-current")}>DROPPEN HUB</text>
        <text x="165" y="210" textAnchor="middle" className={cn(label, "fill-current")} opacity={0.55}>{t("Only encrypted copies")}</text>
        <text x="165" y="254" textAnchor="middle" className={cn(label, "fill-current")}>{t("Droppen relay")}</text>
        <text x="165" y="272" textAnchor="middle" className={cn(label, "fill-current")} opacity={0.55}>{t("Only ciphertext")}</text>
        <KeyGlyph x={32} y={188} slashed />
        <text x="56" y="192" className={cn(label, "fill-current")} opacity={0.55}>{t("No key")}</text>
        <KeyGlyph x={32} y={250} slashed />
        <text x="56" y="254" className={cn(label, "fill-current")} opacity={0.55}>{t("No key")}</text>
        <rect x="60" y="292" width="210" height="24" fill="var(--color-fg)" stroke="currentColor" strokeWidth={1} />
        <text x="165" y="308" textAnchor="middle" className={cn(label, "fill-bg")}>{t("End-to-end encrypted")}</text>
        <text x="165" y="456" textAnchor="middle" className={cn(label, "fill-current")}>{t("Your phone")}</text>
        <text x="165" y="472" textAnchor="middle" className={cn(label, "fill-current")} opacity={0.55}>{t("Decrypted here")}</text>
        <KeyGlyph x={40} y={490} ok />
        <text x="64" y="494" className={cn(label, "fill-current")} opacity={0.8}>{t("The key stays on this device")}</text>
      </svg>
    </figure>
  );
}

function KeyGlyph({ x, y, ok, slashed }: { x: number; y: number; ok?: boolean; slashed?: boolean }) {
  return (
    <g transform={`translate(${x} ${y})`} fill="none" stroke={ok ? "var(--color-ok)" : "currentColor"} strokeWidth={1.2} opacity={ok ? 1 : 0.55}>
      <circle cx="0" cy="0" r="5" />
      <path d="M5 0h16M14 0v5M18 0v3" />
      {slashed && <path d="M-9 -9L20 9" />}
    </g>
  );
}

function Security() {
  const { t, i18n: active } = useTranslation();
  const linkClass = "whitespace-nowrap text-fg underline underline-offset-4 hover:no-underline";
  const facts = [
    { title: t("End-to-end encrypted"), body: t("Encrypted on your computer and decrypted only on your devices. Droppen Hub stores only encrypted copies and Droppen relays forward only ciphertext; neither holds a key that can decrypt them.") },
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
    <Section id="security">
      <div className={cn(WIDE, "pb-12 max-md:pb-10")}>
        <Head
          eyebrow={t("Security")}
          title={t("Droppen can't read your code.")}
          lead={t("The key is made on your computer and never leaves your devices. Everything Droppen keeps or passes on is already sealed.")}
        />
        <KeyBoundary />
        <ul className="mx-6 grid border-x border-b border-border max-md:mx-4 lg:grid-cols-3">
          {facts.map(({ title, body, link }, i) => (
            <li key={title} className={cn("flex flex-col gap-2.5 px-6 py-7 max-md:px-4", i > 0 && "border-border max-lg:border-t lg:border-l")}>
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
  description,
  points,
  cta,
  href,
  pro,
}: {
  name: string;
  badge?: string;
  price: string;
  unit?: string;
  description: string;
  points: string[];
  cta: string;
  href: string;
  pro?: boolean;
}) {
  return (
    <div className={cn("flex min-w-0 flex-col px-6 py-7 max-md:px-4", pro && "border-t-2 border-t-fg lg:-mt-px")}>
      <div className="flex items-center gap-2 text-sm font-semibold text-fg">
        {name}
        {badge && <span className="inline-flex h-[18px] items-center bg-fg px-1.5 font-mono text-xs tracking-label text-bg uppercase">{badge}</span>}
      </div>
      <div className="mt-4 font-mono text-3xl leading-none font-medium tracking-tight text-fg tabular-nums">
        {price}
        {unit && <small className="ml-1 text-xs font-normal tracking-normal text-muted">{unit}</small>}
      </div>
      <p className="mt-3 min-h-12 text-sm leading-6 text-muted max-md:min-h-0">{description}</p>
      <ul className="mt-4 flex flex-col gap-2">
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
    <Section id="pricing">
      <div className={cn(WIDE, "pb-12 max-md:pb-10")}>
        <Head
          eyebrow={t("Pricing")}
          title={t("Start free.")}
          lead={t("Using it locally needs no account. To go across devices, free is enough to start.")}
          action={
            <a {...publicLink(PRICING_PATH)} className="inline-flex flex-none items-center gap-2 border-b border-border-strong pb-0.5 text-sm text-fg hover:border-fg">
              {t("Compare all plans")}
              <ArrowRight className="size-4" />
            </a>
          }
        />
        <div className="mx-6 grid border-t border-border max-md:mx-4 lg:grid-cols-3 lg:divide-x lg:divide-border">
          <Plan
            name={t("Local only")}
            price="$0"
            description={t("On your own computer, with no account.")}
            points={[t("1 device that runs AI"), t("No cross-device access"), t("No cloud copy")]}
            cta={t("Full install instructions")}
            href={DOWNLOAD_PATH}
          />
          <Plan
            name={t("Free")}
            price="$0"
            description={t("Where cross-device starts.")}
            points={[t("{{n}} devices that run AI", { n: free.hosts }), t("{{devices}} devices, phones and browsers included", { devices: free.devices }), t("{{free}} days of offline copy · {{gb}} GB relay", { free: free.retentionDays, gb: gb(free.transferBytes) })]}
            cta={signedIn || closed ? t("Compare all plans") : t("Create a free account")}
            href={freeHref}
          />
          <Plan
            pro
            name="Pro"
            badge={t("Recommended")}
            price={`$${PRO_PRICE.monthly}`}
            unit={t("USD / month")}
            description={t("For people who move between devices every day.")}
            points={[
              t("{{pro}} devices that run AI · {{devices}} devices", { pro: pro.hosts, devices: pro.devices }),
              t("{{pro}} days of offline copy · {{gb}} GB relay", { pro: pro.retentionDays, gb: gb(pro.transferBytes) }),
              t("Billed by use beyond that"),
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
      a: t("Session content is encrypted on your computer before it leaves. Droppen Hub keeps encrypted copies, and relays forward only ciphertext; neither has the key. In a browser, you trust the page code as it was loaded that time."),
    },
    {
      q: t("Is the free plan enough?"),
      a: t("If you have up to {{hosts}} computers that run AI and need {{free}} days of offline reading, free is enough. Choose Pro for more devices, {{pro}} days offline or more relay transfer.", {
        hosts: free.hosts,
        free: free.retentionDays,
        pro: pro.retentionDays,
      }),
    },
  ];
  return (
    <Section id="faq">
      <div className={cn(WIDE, "pb-12 max-md:pb-10")}>
        <Head eyebrow={t("Questions and answers")} title={t("Before you start.")} />
        <FaqAccordion
          multiple
          items={items}
          className="mx-6 grid border-t border-border max-md:mx-4 md:grid-cols-2 md:gap-x-16"
          triggerClassName="py-4"
          questionClassName="font-medium"
          panelClassName="mt-2.5"
          answerClassName="max-w-[52ch] text-sm text-pretty"
        />
      </div>
    </Section>
  );
}

function FinalCta({ signup }: { signup: SignupMode }) {
  const { t } = useTranslation();
  const closed = signup === "closed";
  return (
    <Section id="start" className="relative overflow-hidden">
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
        <p className="mx-auto mt-4 max-w-[34em] text-base leading-6 text-pretty text-muted">{t("Free to start. Install once, then carry on from any of your devices.")}</p>
        <div className="mt-8 flex flex-wrap justify-center gap-3">
          <ArrowLink {...publicLink(closed ? WAITLIST_HREF : SIGN_UP_PATH)}>{closed ? t("Join the waitlist") : t("Create a free account")}</ArrowLink>
          <ArrowLink {...publicLink(DOWNLOAD_PATH)}>{t("Download for Mac")}</ArrowLink>
        </div>
        <TrustLines className="mt-5 font-mono" />
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
    t("Claude Code and Codex keep running on your own Macs and Linux machines. From your phone or a browser, see every session across them, answer their questions, steer them and open a terminal. End-to-end encrypted, with an open-source client."),
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
      <HeroStage />
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
