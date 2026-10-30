import { type ReactNode, useEffect, useMemo, useState } from "react";
import i18n from "i18next";
import { useTranslation } from "react-i18next";
import { Check } from "lucide-react";
import { HubApi, type SignupMode, type WaitlistPlan } from "@/lib/hub/api";
import { cn } from "@/lib/cn";
import { landingBundles } from "./copy";
import { DOWNLOAD_PATH } from "./get-droppen";
import { cjkTracking, FooterLine, landingPrimary, landingSecondary, Points, PublicHeader, Section, SectionHead } from "./page-kit";
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
 * how many days your phone can read while a device is offline (the cloud's
 * copy expires once a device has been offline that long: docs/relay-and-plans.md
 * §3.4), a month's
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
  recommended,
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
  recommended?: boolean;
  price: string;
  unit?: string;
  note: string;
  action: ReactNode;
  aside?: ReactNode;
  lead: string;
  points: string[];
  className?: string;
}) {
  const { t } = useTranslation();
  return (
    // From 1024px the three cards share their rows (a subgrid), so prices, buttons and the lists below line up across them.
    <div
      data-testid={testId}
      role="group"
      aria-labelledby={`${testId}-name`}
      className={cn("grid min-w-0 content-start gap-6 border-border px-6 py-8 max-md:px-4 max-md:py-7 lg:row-span-4 lg:grid-rows-subgrid", className)}
    >
      <div className="flex min-h-6 items-center gap-2">
        <h3 id={`${testId}-name`} className="text-base leading-6 font-semibold text-fg">{name}</h3>
        {recommended && (
          <span className={cn("inline-flex items-center bg-fg px-1.5 py-0.5 text-xs font-semibold tracking-wider text-bg uppercase", cjkTracking)}>
            {t("Recommended")}
          </span>
        )}
      </div>
      <div className="flex flex-col gap-2">
        <p className="flex items-baseline gap-2">
          <span className="text-5xl leading-none font-semibold tracking-tight text-fg tabular-nums max-md:text-4xl">{price}</span>
          {unit && <span className="text-sm font-semibold text-muted">{unit}</span>}
        </p>
        <p className="text-sm leading-5 text-faint">{note}</p>
      </div>
      <div className="flex flex-col gap-2">
        {action}
        {aside}
      </div>
      <div className="flex flex-col gap-3 border-t border-border pt-5">
        <p className="text-base leading-6 font-semibold text-fg">{lead}</p>
        <Points items={points} large className="lg:hidden" />
      </div>
    </div>
  );
}

function Plans({ signedIn, signup, onChoose }: { signedIn: boolean; signup: SignupMode; onChoose: (plan: WaitlistPlan) => void }) {
  const { t } = useTranslation();
  const { free, pro } = PLAN_NUMBERS;
  const encrypted = t("End-to-end encrypted: we can't read your sessions, code or terminals");
  // To the form, with the plan chosen (the anchor scrolls; the form takes the focus).
  // The plans' one primary action: signing up where anyone can, else Pro's waitlist.
  const freeLeads = signup === "open" && !signedIn;
  const choose = (plan: WaitlistPlan) => () => {
    onChoose(plan);
    requestAnimationFrame(() => document.getElementById("waitlist-email")?.focus({ preventScroll: true }));
  };
  return (
    <Section id="plans">
      <h2 className="sr-only">{t("Pricing")}</h2>
      <div data-testid="plans" className="grid lg:grid-cols-3 lg:grid-rows-[auto_auto_auto_1fr]">
        <PlanCard
          testId="plan-local"
          name={t("Local only")}
          price="$0"
          unit={t("forever")}
          note={t("No account, nothing to sign up for")}
          action={
            <a {...publicLink(DOWNLOAD_PATH)} data-testid="plan-local-download" className={cn(landingSecondary, "w-full max-md:h-10")}>
              {t("Download Droppen")}
            </a>
          }
          lead={t("Droppen on the device your AI runs on, and nothing in the cloud.")}
          points={[
            t("1 device that runs AI: the one Droppen is installed on"),
            t("Use it in a browser on that device, or on another device on the same network"),
            t("Nothing goes through our servers"),
            encrypted,
          ]}
        />
        <PlanCard
          testId="plan-free"
          className="max-lg:border-t lg:border-l"
          name={t("Free")}
          price="$0"
          note={signup === "invite" ? t("Sign-up is by invite for now") : signup === "closed" ? t("Sign-up is closed for now") : t("Sign up with an e-mail address")}
          action={
            signedIn ? (
              <a {...publicLink(APP_PATH)} data-testid="plan-free-open" className={cn(landingSecondary, "w-full max-md:h-10")}>
                {t("Open app")}
              </a>
            ) : signup !== "open" ? (
              <a href="#waitlist" onClick={choose("free")} data-testid="plan-free-waitlist" className={cn(landingSecondary, "w-full max-md:h-10")}>
                {t("Join the waitlist")}
              </a>
            ) : (
              <a {...publicLink(SIGN_UP_PATH)} data-variant="primary" data-testid="plan-free-signup" className={cn(landingPrimary, "w-full max-md:h-10")}>
                {t("Sign up")}
              </a>
            )
          }
          aside={
            signedIn ? (
              <span className="text-sm leading-5 text-faint">{t("You have an account already.")}</span>
            ) : signup === "invite" ? (
              <a {...publicLink(SIGN_UP_PATH)} data-testid="plan-free-signup" className="w-fit text-sm text-muted underline underline-offset-4 hover:text-fg">
                {t("Have an invite code? Sign up")}
              </a>
            ) : undefined
          }
          lead={t("Your AI on your phone, wherever you are.")}
          points={[
            t("{{n}} devices that run AI", { n: free.hosts }),
            t("{{n}} devices in all, including those that run AI, phones and browsers", { n: free.devices }),
            t("While a device that runs AI is offline, read its last {{n}} days of sessions on your phone", { n: free.retentionDays }),
            t("{{n}} GB a month through our servers, then {{kb}} KB/s until the next month", { n: gb(free.transferBytes), kb: THROTTLED_KB_PER_SEC }),
            encrypted,
          ]}
        />
        <PlanCard
          testId="plan-pro"
          className="max-lg:border-t lg:border-l"
          name="Pro"
          recommended
          price={`$${PRO_PRICE.monthly}`}
          unit={t("USD / month")}
          note={t("or ${{price}} a year: save {{saving}}%", { price: PRO_PRICE.yearly, saving: yearlySaving() })}
          action={
            <a
              href="#waitlist"
              onClick={choose("pro")}
              data-variant={freeLeads ? undefined : "primary"}
              data-testid="plan-pro-waitlist"
              className={cn(freeLeads ? landingSecondary : landingPrimary, "w-full max-md:h-10")}
            >
              {t("Join the waitlist")}
            </a>
          }
          aside={<span className="text-sm leading-5 text-faint">{t("Payments are not open yet: we'll e-mail you when they are.")}</span>}
          lead={t("Everything in Free, with higher limits.")}
          points={[
            t("{{n}} devices that run AI", { n: pro.hosts }),
            t("{{n}} devices in all, including those that run AI, phones and browsers", { n: pro.devices }),
            t("While a device that runs AI is offline, read its last {{n}} days of sessions on your phone", { n: pro.retentionDays }),
            t("{{n}} GB a month through our servers, then billed by use, at full speed", { n: gb(pro.transferBytes) }),
          ]}
        />
      </div>
    </Section>
  );
}

/** The plans side by side: each row's title over its detail on the left, the three plans' values on the right. */
function Compare({ signup }: { signup: SignupMode }) {
  const { t } = useTranslation();
  const { free, pro } = PLAN_NUMBERS;
  const yes = (
    <span className="inline-flex items-center">
      <Check aria-hidden="true" className="size-4" />
      <span className="sr-only">{t("included")}</span>
    </span>
  );
  const rows: { title: string; detail: string; values: [ReactNode, ReactNode, ReactNode] }[] = [
    {
      title: t("Price"),
      detail: t("In US dollars"),
      values: [t("Free forever, no account"), signup === "invite" ? t("Free; sign-up by invite for now") : signup === "closed" ? t("Free; sign-up is closed for now") : t("Free, with an account"), t("${{monthly}} a month, or ${{yearly}} a year", { monthly: PRO_PRICE.monthly, yearly: PRO_PRICE.yearly })],
    },
    { title: t("Devices that run AI"), detail: t("The devices Droppen is installed on"), values: [t("1: the device Droppen is installed on"), String(free.hosts), String(pro.hosts)] },
    { title: t("All your devices"), detail: t("Including phones and browsers"), values: [t("That device, and browsers on the same network"), String(free.devices), String(pro.devices)] },
    {
      title: t("Offline days"),
      detail: t("Sessions your phone can read"),
      values: [t("None: nothing is kept in the cloud"), t("{{n}} days", { n: free.retentionDays }), t("{{n}} days", { n: pro.retentionDays })],
    },
    {
      title: t("A month's transfer"),
      detail: t("Through our servers"),
      values: [
        t("Nothing goes through our servers"),
        t("{{n}} GB, then {{kb}} KB/s", { n: gb(free.transferBytes), kb: THROTTLED_KB_PER_SEC }),
        t("{{n}} GB, then billed by use", { n: gb(pro.transferBytes) }),
      ],
    },
    { title: t("End-to-end encrypted"), detail: t("We can't read your sessions, code or terminals"), values: [yes, yes, yes] },
  ];
  const heads = [t("Local only"), t("Free"), "Pro"];
  return (
    <Section id="compare" className="max-lg:hidden">
      <SectionHead label={t("Compare")} title={t("What each plan includes")} />
      {/* One line per value where the table has room (1320px); narrower, the values wrap rather than the table scroll. */}
      <div className="@container/compare overflow-x-auto">
        <table aria-label={t("What each plan includes")} data-testid="compare" className="w-full border-collapse border-t border-border text-left">
          <colgroup>
            <col className="w-[344px]" />
            <col />
            <col />
            <col />
          </colgroup>
          <thead>
            <tr className="border-b border-border">
              <th scope="col" className="min-w-[344px] px-6 py-3" />
              {heads.map((h) => (
                // The plans' names as their cards write them: in capitals, 「PRO」 read as another word beside 「免费版」.
                <th key={h} scope="col" className="border-l border-border px-6 py-3 text-base leading-6 font-semibold whitespace-nowrap text-fg">
                  {h}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {rows.map((r) => (
              <tr key={r.title} className="border-b border-border">
                <th scope="row" className="min-w-[344px] px-6 py-3.5 text-left align-top font-normal">
                  <span className="block text-base leading-6 text-fg">{r.title}</span>
                  <span className="block text-xs leading-4 text-muted">{r.detail}</span>
                </th>
                {r.values.map((v, i) => (
                  <td
                    key={i}
                    className="border-l border-border px-6 py-3.5 align-top text-base leading-6 text-muted tabular-nums @min-[1320px]/compare:whitespace-nowrap"
                  >
                    {v}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
          <tfoot>
            <tr data-testid="compare-note">
              <th scope="row" className="min-w-[344px] px-6 py-3.5 text-left align-top text-base leading-6 font-normal text-fg">
                {t("Data storage")}
              </th>
              <td colSpan={3} className="border-l border-border px-6 py-3.5 align-top text-sm leading-6 text-muted">
                {t("Free keeps an encrypted copy of each device's last {{free}} days of sessions in the cloud, Pro its last {{pro}}. A device offline for longer than that has its cloud copy deleted: what is on the device is not affected, and those days are uploaded again once it is back online.", {
                  free: free.retentionDays,
                  pro: pro.retentionDays,
                })}
              </td>
            </tr>
          </tfoot>
        </table>
      </div>
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
              ? t("Leave your address, and we'll write when it's your turn")
              : signup === "closed"
                ? t("Leave your address, and we'll write when sign-up or Pro opens")
                : t("Leave your address, and we'll write when Pro opens")
          }
          lead={
            signup === "invite"
              ? t("Sign-up is invite-only while Droppen is in beta, and Pro is not on sale yet. Pick what you are waiting for: we'll e-mail you an invite, or tell you when Pro opens. Submitting the form is all it takes.")
              : signup === "closed"
                ? t("Sign-up is closed for now, and Pro is not on sale yet. Pick what you are waiting for: we'll e-mail you when sign-up opens, or when Pro does. Submitting the form is all it takes.")
                : t("Pro is not on sale yet: payments are on their way. Submitting the form is all it takes.")
          }
          className="lg:border-r lg:border-border [&>p]:text-base"
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
      q: t("What if a device stays offline for longer?"),
      a: t(
        "On Free your phone can read a device's last {{free}} days of sessions while it is offline, on Pro its last {{pro}}. A device offline for longer than that has its encrypted copy in the cloud deleted: what is on the device itself is not affected, and its recent sessions are uploaded again once it is back online.",
        { free: free.retentionDays, pro: pro.retentionDays },
      ),
    },
    {
      q: t("What does “local only” mean?"),
      a: t(
        "Droppen runs on your device with no account. You use it in a browser on that device, or on another device on the same network, and nothing goes through our servers. Sign in to Droppen when you want to reach your AI from your phone anywhere, and the sessions of all your devices in one list.",
      ),
    },
    {
      q: t("Can I host it myself?"),
      a: t(
        "Local only runs entirely on your own devices and needs nothing from us. The Droppen client is open source under the AGPL-3.0, at github.com/getdroppen/droppen. Droppen Hub, the service at droppen.app that connects your devices, is not open source yet, so it cannot be self-hosted for now.",
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
            <dt className="text-base leading-6 font-semibold text-fg">{item.q}</dt>
            <dd className="max-w-prose text-base leading-6 text-muted">{item.a}</dd>
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
    <div data-testid="pricing" className="h-full w-full overflow-y-auto bg-bg text-base [scrollbar-gutter:stable]">
      <PublicHeader signedIn={signedIn} />
      <div className="md:px-8">
        <main className="mx-auto max-w-[1360px] md:border-x md:border-border">
          <div className="flex flex-col gap-5 px-10 pt-16 pb-10 max-lg:pt-12 max-md:px-4 max-md:pt-10 max-md:pb-8">
            <h1 className="max-w-[18em] text-5xl leading-display font-normal tracking-tighter text-balance text-fg cjk-wrap md:cjk-keep max-md:text-4xl">
              {t("Start free. Go Pro when you need more.")}
            </h1>
            <p className="max-w-[40em] text-base leading-6 text-muted">
              {t(
                "Every plan is end-to-end encrypted: on none of them can we read your sessions, your code or your terminals. A plan decides only how many devices you can connect, how many days of sessions your phone can read while a device that runs AI is offline, and how much traffic goes through our servers each month.",
              )}
            </p>
            <p className="text-sm leading-5 text-faint">{t("Prices in US dollars. Pro is not on sale yet, and nothing is charged before you choose to pay.")}</p>
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
