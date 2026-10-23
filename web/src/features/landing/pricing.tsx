import { type ReactNode, useEffect, useMemo, useState } from "react";
import i18n from "i18next";
import { useTranslation } from "react-i18next";
import { Check, Minus } from "lucide-react";
import { CapsLabel } from "@/components/ui/caps-label";
import { primary, secondary } from "@/features/cloud/ui";
import { HubApi, type SignupMode, type WaitlistPlan } from "@/lib/hub/api";
import { cn } from "@/lib/cn";
import { landingBundles } from "./copy";
import { DOWNLOAD_PATH } from "./get-droppen";
import { FooterLine, Points, PublicHeader, Section, SectionHead } from "./page-kit";
import { gb, PLAN_NUMBERS, PRO_PRICE, THROTTLED_KB_PER_SEC, yearlySaving } from "./pricing-plans";
import { APP_PATH, publicLink, SIGN_UP_PATH } from "./route";
import { type WaitlistState, WaitlistForm } from "./waitlist";

/*
 * droppen.app/pricing: the three ways to use Droppen side by side — local
 * only (no account), Free and Pro — what each includes, the waitlist, and
 * the questions people ask before choosing. Signed in or not; its own lazy
 * chunk, in the landing page's drafting-sheet language.
 *
 * Product words, not implementation: devices that run AI, all your devices,
 * how far back your phone can read while a device is off, a month's
 * transfer through our servers. Every number is a plan's own
 * (`pricing-plans.ts`, held to the hub's plans by a test), and every plan
 * is end-to-end encrypted alike — the page says so on each.
 *
 * There is nothing to pay with yet (Stripe comes with M4): Pro's button is
 * "Join the waitlist", and brings the visitor to the form (`#waitlist`).
 * Free is a plain sign-up where the hub's sign-up is open (droppen.app since
 * 2026-10-23), and the form is Pro's alone. Where sign-up is by invite or
 * closed (`signup-mode.ts`: a hub that says so), Free's button joins the
 * waitlist too, the form has both plans, each card's button choosing its
 * own, and `?plan=pro` in the address chooses Pro. One primary action in view at a
 * time (docs/ui-spec.md V10): among the plans, Free's sign-up where sign-up
 * is open and the visitor has no account (the lead's call, 2026-10-24), else
 * Pro's; and the form's own further down.
 */

// Added, never overwriting: a key the app already translates keeps its wording.
for (const [lng, bundle] of Object.entries(landingBundles)) i18n.addResourceBundle(lng, "translation", bundle, true, false);

/** The plan an address names (`?plan=pro`), Free otherwise (where Free has a waitlist: sign-up by invite, or closed). */
export function planOfSearch(search: string): WaitlistPlan {
  return new URLSearchParams(search).get("plan") === "pro" ? "pro" : "free";
}

function PlanCard({
  testId,
  name,
  chip,
  price,
  unit,
  note,
  action,
  aside,
  lead,
  points,
  className,
}: {
  testId: string;
  name: string;
  chip?: string;
  price: string;
  unit: string;
  note: string;
  action: ReactNode;
  aside?: ReactNode;
  lead: string;
  points: string[];
  className?: string;
}) {
  return (
    // From 1024px the three cards share their rows (a subgrid), so prices, buttons and the lists below line up across them.
    <div data-testid={testId} className={cn("grid min-w-0 content-start gap-6 border-border px-6 py-8 max-md:px-4 max-md:py-7 lg:row-span-4 lg:grid-rows-subgrid", className)}>
      <div className="flex min-h-5 items-center gap-2">
        <span className="text-[13px] leading-5 font-semibold text-fg">{name}</span>
        {chip && <span className="inline-flex items-center border border-fg px-1.5 font-mono text-[10px] leading-4 font-semibold tracking-wider text-fg uppercase">{chip}</span>}
      </div>
      <div className="flex flex-col gap-2">
        <p className="flex items-baseline gap-2">
          <span className="text-[44px] leading-none font-semibold tracking-[-0.035em] text-fg tabular-nums max-md:text-[38px]">{price}</span>
          <span className="font-mono text-2xs text-muted">{unit}</span>
        </p>
        <p className="font-mono text-2xs leading-4 text-faint">{note}</p>
      </div>
      <div className="flex flex-col gap-2">
        {action}
        {aside}
      </div>
      <div className="flex flex-col gap-3 border-t border-border pt-5">
        <p className="text-xs leading-5 font-semibold text-fg">{lead}</p>
        <Points items={points} />
      </div>
    </div>
  );
}

function Plans({ signedIn, signup, onChoose }: { signedIn: boolean; signup: SignupMode; onChoose: (plan: WaitlistPlan) => void }) {
  const { t } = useTranslation();
  const { free, pro } = PLAN_NUMBERS;
  const encrypted = t("End-to-end encrypted: we can't see your code");
  // To the form, with the plan chosen (the anchor scrolls; the form takes the focus).
  // The plans' one primary action: signing up where anyone can, else Pro's waitlist.
  const freeLeads = signup === "open" && !signedIn;
  const choose = (plan: WaitlistPlan) => () => {
    onChoose(plan);
    requestAnimationFrame(() => document.getElementById("waitlist-email")?.focus({ preventScroll: true }));
  };
  return (
    <Section id="plans">
      <div data-testid="plans" className="grid lg:grid-cols-3 lg:grid-rows-[auto_auto_auto_1fr]">
        <PlanCard
          testId="plan-local"
          name={t("Local only")}
          price="$0"
          unit={t("forever")}
          note={t("No account, nothing to sign up for")}
          action={
            <a {...publicLink(DOWNLOAD_PATH)} data-testid="plan-local-download" className={cn(secondary, "w-full")}>
              {t("Download Droppen")}
            </a>
          }
          lead={t("Droppen on the device your AI runs on, and nothing in the cloud.")}
          points={[
            t("1 device that runs AI: this one"),
            t("Use it from any browser on the same network"),
            t("Nothing goes through our servers"),
            encrypted,
          ]}
        />
        <PlanCard
          testId="plan-free"
          className="max-lg:border-t lg:border-l"
          name={t("Free")}
          price="$0"
          unit={t("USD / month")}
          note={signup === "invite" ? t("Sign-up is by invite for now") : signup === "closed" ? t("Sign-up is closed for now") : t("Sign up with an e-mail address")}
          action={
            signedIn ? (
              <a {...publicLink(APP_PATH)} data-testid="plan-free-open" className={cn(secondary, "w-full")}>
                {t("Open app")}
              </a>
            ) : signup !== "open" ? (
              <a href="#waitlist" onClick={choose("free")} data-testid="plan-free-waitlist" className={cn(secondary, "w-full")}>
                {t("Join the waitlist")}
              </a>
            ) : (
              <a {...publicLink(SIGN_UP_PATH)} data-variant="primary" data-testid="plan-free-signup" className={cn(primary, "w-full")}>
                {t("Sign up")}
              </a>
            )
          }
          aside={
            signedIn ? (
              <span className="font-mono text-2xs leading-4 text-faint">{t("You have an account already.")}</span>
            ) : signup === "invite" ? (
              <a {...publicLink(SIGN_UP_PATH)} data-testid="plan-free-signup" className="w-fit text-xs text-muted underline underline-offset-4 hover:text-fg">
                {t("Have an invite code? Sign up")}
              </a>
            ) : undefined
          }
          lead={t("Your AI on your phone, wherever you are.")}
          points={[
            t("{{n}} devices that run AI", { n: free.hosts }),
            t("{{n}} devices in all, phones and browsers included", { n: free.devices }),
            t("Read the last {{n}} days on your phone while a device is off", { n: free.retentionDays }),
            t("{{n}} GB a month through our servers, then slower", { n: gb(free.transferBytes) }),
            encrypted,
          ]}
        />
        <PlanCard
          testId="plan-pro"
          className="max-lg:border-t lg:border-l"
          name="Pro"
          chip={t("Waitlist")}
          price={`$${PRO_PRICE.monthly}`}
          unit={t("USD / month")}
          note={t("or ${{price}} a year: save {{saving}}%", { price: PRO_PRICE.yearly, saving: yearlySaving() })}
          action={
            <a
              href="#waitlist"
              onClick={choose("pro")}
              data-variant={freeLeads ? undefined : "primary"}
              data-testid="plan-pro-waitlist"
              className={cn(freeLeads ? secondary : primary, "w-full")}
            >
              {t("Join the waitlist")}
            </a>
          }
          aside={<span className="font-mono text-2xs leading-4 text-faint">{t("Payments are not open yet: we'll e-mail you when they are.")}</span>}
          lead={t("Everything in Free, plus:")}
          points={[
            t("{{n}} devices that run AI", { n: pro.hosts }),
            t("{{n}} devices in all, phones and browsers included", { n: pro.devices }),
            t("Read the last {{n}} days on your phone while a device is off", { n: pro.retentionDays }),
            t("{{n}} GB a month through our servers, then billed by use, at full speed", { n: gb(pro.transferBytes) }),
          ]}
        />
      </div>
    </Section>
  );
}

/** The plans side by side, row by row: a table from 768px, each row's name over its three values below. */
function Compare({ signup }: { signup: SignupMode }) {
  const { t } = useTranslation();
  const { free, pro } = PLAN_NUMBERS;
  const yes = (
    <>
      <Check aria-hidden="true" className="size-4" />
      <span className="sr-only">{t("included")}</span>
    </>
  );
  const none = (
    <>
      <Minus aria-hidden="true" className="size-4 text-faint" />
      <span className="sr-only">{t("not included")}</span>
    </>
  );
  const rows: { name: string; values: [ReactNode, ReactNode, ReactNode] }[] = [
    {
      name: t("Price"),
      values: [t("Free forever, no account"), signup === "invite" ? t("Free; sign-up by invite for now") : signup === "closed" ? t("Free; sign-up is closed for now") : t("Free, with an account"), t("${{monthly}} a month, or ${{yearly}} a year", { monthly: PRO_PRICE.monthly, yearly: PRO_PRICE.yearly })],
    },
    { name: t("Devices that run AI"), values: [t("1: this device"), String(free.hosts), String(pro.hosts)] },
    { name: t("All your devices, phones and browsers included"), values: [t("This device, and browsers on the same network"), String(free.devices), String(pro.devices)] },
    {
      name: t("How far back your phone can read while a device is off"),
      values: [none, t("{{n}} days", { n: free.retentionDays }), t("{{n}} days", { n: pro.retentionDays })],
    },
    {
      name: t("A month's transfer through our servers"),
      values: [none, t("{{n}} GB, then slower", { n: gb(free.transferBytes) }), t("{{n}} GB, then billed by use", { n: gb(pro.transferBytes) })],
    },
    { name: t("End-to-end encrypted: we can't see your code"), values: [yes, yes, yes] },
  ];
  const heads = [t("Local only"), t("Free"), "Pro"];
  return (
    <Section id="compare">
      <SectionHead label={t("Compare")} title={t("What each plan includes")} />
      <div role="table" aria-label={t("What each plan includes")} data-testid="compare" className="border-t border-border">
        <div role="row" className="grid grid-cols-3 border-b border-border md:grid-cols-[minmax(0,1.4fr)_repeat(3,minmax(0,1fr))]">
          <span role="columnheader" className="max-md:sr-only" />
          {heads.map((h, i) => (
            <span key={h} role="columnheader" className={cn("px-6 py-3 max-md:px-4", i > 0 && "border-l border-border", "md:border-l md:border-border")}>
              <CapsLabel className="text-fg">{h}</CapsLabel>
            </span>
          ))}
        </div>
        {rows.map((r) => (
          <div key={r.name} role="row" className="grid grid-cols-3 border-b border-border last:border-b-0 md:grid-cols-[minmax(0,1.4fr)_repeat(3,minmax(0,1fr))]">
            <span role="rowheader" className="col-span-3 px-6 pt-3.5 text-[13px] leading-5 font-semibold text-fg max-md:px-4 md:col-span-1 md:pb-3.5">
              {r.name}
            </span>
            {r.values.map((v, i) => (
              <span
                key={i}
                role="cell"
                className={cn(
                  "flex min-w-0 items-start px-6 py-3.5 text-xs leading-5 text-muted tabular-nums max-md:px-4 max-md:pt-1.5",
                  i > 0 && "border-l border-border",
                  "md:border-l md:border-border",
                )}
              >
                {v}
              </span>
            ))}
          </div>
        ))}
      </div>
      <p data-testid="compare-note" className="border-t border-border px-6 py-3.5 text-xs leading-5 text-muted max-md:px-4">
        {t("A device off for longer than that has its copy in the cloud deleted: what is on the device stays, and its last {{free}} days ({{pro}} on Pro) come back once it is online.", {
          free: free.retentionDays,
          pro: pro.retentionDays,
        })}
      </p>
    </Section>
  );
}

/**
 * The waitlist: Pro's alone where sign-up is open (no plan to choose), or
 * Free's and Pro's where sign-up is by invite or closed (`onPlan` given: the
 * form offers both).
 */
function Waitlist({
  api,
  plan,
  onPlan,
  state,
  signup,
}: {
  api: Pick<HubApi, "joinWaitlist">;
  plan: WaitlistPlan;
  onPlan?: (plan: WaitlistPlan) => void;
  state?: WaitlistState;
  signup: SignupMode;
}) {
  const { t } = useTranslation();
  return (
    <Section id="waitlist">
      <div className="grid lg:grid-cols-[minmax(0,1fr)_minmax(0,1.2fr)]">
        <SectionHead
          label={t("Waitlist")}
          title={
            signup === "invite"
              ? t("Leave your address, and we'll write when it's your turn.")
              : signup === "closed"
                ? t("Leave your address, and we'll write when sign-up or Pro opens.")
                : t("Leave your address, and we'll write when Pro opens.")
          }
          lead={
            signup === "invite"
              ? t("Sign-up is invite-only while Droppen is in beta, and Pro is not on sale yet. Pick what you are waiting for: we'll e-mail you an invite, or tell you when Pro opens. Submitting the form is all it takes.")
              : signup === "closed"
                ? t("Sign-up is closed for now, and Pro is not on sale yet. Pick what you are waiting for: we'll e-mail you when sign-up opens, or when Pro does. Submitting the form is all it takes.")
                : t("Pro is not on sale yet: payments are on their way. Submitting the form is all it takes.")
          }
          className="lg:border-r lg:border-border"
        />
        <div className="px-6 pt-10 pb-10 max-lg:border-t max-lg:border-border max-md:px-4 max-md:py-8">
          <WaitlistForm api={api} plan={plan} onPlan={onPlan} state={state} signup={signup} />
        </div>
      </div>
    </Section>
  );
}

function Faq() {
  const { t } = useTranslation();
  const { free, pro } = PLAN_NUMBERS;
  const items = [
    {
      q: t("What happens when I use up a month's transfer?"),
      a: t(
        "Only what goes through our servers counts: what goes straight between your devices does not. On Free, past {{free}} GB that traffic slows to {{kb}} KB/s until the next month starts; terminals and reading still work, just more slowly. On Pro nothing slows down: what you use past {{pro}} GB is billed by use, at a price we will publish before payments open.",
        { free: gb(free.transferBytes), pro: gb(pro.transferBytes), kb: THROTTLED_KB_PER_SEC },
      ),
    },
    {
      q: t("What if a device stays off for longer?"),
      a: t(
        "On Free your phone can read a device's last {{free}} days while it is off, on Pro its last {{pro}}. A device off for longer than that has its encrypted copy in the cloud deleted: what is on the device itself is not affected, and its recent days are uploaded again once it is back online.",
        { free: free.retentionDays, pro: pro.retentionDays },
      ),
    },
    {
      q: t("What does “local only” mean?"),
      a: t(
        "Droppen runs on your device with no account. You use it in a browser on that device, or on another device on the same network, and nothing goes through our servers. Sign in to Droppen when you want your AI on your phone from anywhere, and all your devices on one screen.",
      ),
    },
    {
      q: t("Can I host it myself?"),
      a: t(
        "The Droppen app and server are licensed under the AGPL-3.0, and local only needs nothing from us. Their source code is at github.com/getdroppen/droppen. Droppen Hub, the service at droppen.app that connects your devices, is not open source yet, so it cannot be self-hosted for now.",
      ),
    },
    {
      q: t("When can I pay for Pro?"),
      a: t(
        "Not yet: payments are on their way. Until then Pro is by waitlist: join it and we'll e-mail you when Pro opens. Nothing is charged before you choose to pay, and prices are in US dollars.",
      ),
    },
    {
      q: t("Is my code safer on Pro?"),
      a: t(
        "It is just as safe on every plan: all of them are end-to-end encrypted the same way, and we can't read your sessions, your code or your terminals on any. A plan only changes how much we keep and carry for you, never who can read it.",
      ),
    },
  ];
  return (
    <Section id="faq">
      <SectionHead label={t("Questions")} title={t("Before you choose")} />
      <dl data-testid="faq" className="grid border-t border-border lg:grid-cols-2">
        {items.map((item, i) => (
          <div key={item.q} className={cn("flex flex-col gap-2 border-border px-6 py-6 max-md:px-4", i > 0 && "max-lg:border-t", i > 1 && "lg:border-t", i % 2 === 1 && "lg:border-l")}>
            <dt className="text-[13px] leading-5 font-semibold text-fg">{item.q}</dt>
            <dd className="text-xs leading-5 text-muted">{item.a}</dd>
          </div>
        ))}
      </dl>
    </Section>
  );
}

/** The page's title and description in the reader's language (`invite`: sign-up is by invite here). */
function useDocumentMeta(invite: boolean) {
  const { t, i18n: active } = useTranslation();
  useEffect(() => {
    const before = document.title;
    document.title = t("Droppen pricing — start free, Pro for ${{price}} a month", { price: PRO_PRICE.monthly });
    document.documentElement.lang = active.language;
    const meta = document.querySelector('meta[name="description"]');
    const description = meta?.getAttribute("content") ?? null;
    meta?.setAttribute(
      "content",
      invite
        ? t("Use Droppen on one device with no account, free with an invite, or Pro for more devices and longer offline reading. Every plan is end-to-end encrypted.")
        : t("Use Droppen on one device with no account, free with an account, or Pro for more devices and longer offline reading. Every plan is end-to-end encrypted."),
    );
    return () => {
      document.title = before;
      if (meta && description !== null) meta.setAttribute("content", description);
    };
  }, [t, active.language, invite]);
}

/**
 * droppen.app/pricing, signed in or not. `api` is the hub's (the page's own
 * origin by default); `waitlist` stands in for the form's state (tests,
 * previews); `signup` is the hub's sign-up mode (`signup-mode.ts`).
 */
export default function PricingPage({
  signedIn = false,
  api,
  waitlist,
  signup = "open",
}: {
  signedIn?: boolean;
  api?: Pick<HubApi, "joinWaitlist">;
  waitlist?: WaitlistState;
  signup?: SignupMode;
}) {
  const { t } = useTranslation();
  const open = signup === "open";
  useDocumentMeta(signup === "invite");
  const hub = useMemo(() => api ?? new HubApi(), [api]);
  // Arrived at a part of the page (`/pricing#waitlist` from the landing page): the page was not there to scroll to yet.
  useEffect(() => {
    const id = location.hash.slice(1);
    if (id) requestAnimationFrame(() => document.getElementById(id)?.scrollIntoView());
  }, []);
  const [chosen, setPlan] = useState<WaitlistPlan>(() => planOfSearch(typeof location === "undefined" ? "" : location.search));
  // Open sign-up: Free needs no waitlist, so the form is Pro's.
  const plan = open ? "pro" : chosen;
  return (
    <div data-testid="pricing" className="h-full w-full overflow-y-auto bg-bg [scrollbar-gutter:stable]">
      <PublicHeader signedIn={signedIn} />
      <div className="md:px-8">
        <main className="mx-auto max-w-[1152px] md:border-x md:border-border">
          <div className="flex flex-col gap-5 px-10 pt-16 pb-10 max-lg:pt-12 max-md:px-4 max-md:pt-10 max-md:pb-8">
            <CapsLabel>{t("Pricing")}</CapsLabel>
            <h1 className="max-w-[18em] text-[52px] leading-[1.02] font-semibold tracking-[-0.035em] text-balance text-fg [word-break:auto-phrase] [&:lang(zh)]:[word-break:keep-all] max-md:text-[34px] xl:text-[48px]">
              {t("Start free. Go Pro when you need more.")}
            </h1>
            <p className="max-w-[40em] text-[15px] leading-[26px] text-muted max-md:text-sm max-md:leading-6">
              {t("Every plan is end-to-end encrypted: we can't see your code on any of them. A plan only decides how many devices you connect, how far back your phone can read while a device is off, and how much our servers carry for you each month.")}
            </p>
            <p className="font-mono text-2xs text-faint">{t("Prices in US dollars.")}</p>
          </div>
          <Plans signedIn={signedIn} signup={signup} onChoose={setPlan} />
          <Compare signup={signup} />
          <Waitlist api={hub} plan={plan} onPlan={open ? undefined : setPlan} state={waitlist} signup={signup} />
          <Faq />
          <FooterLine />
        </main>
      </div>
    </div>
  );
}
