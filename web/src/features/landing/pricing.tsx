import { type ReactNode, useEffect, useMemo, useState } from "react";
import i18n from "i18next";
import { useTranslation } from "react-i18next";
import { Accordion } from "@base-ui-components/react/accordion";
import { Check, Plus } from "lucide-react";
import { HubApi, type SignupMode, type WaitlistPlan } from "@/lib/hub/api";
import { cn } from "@/lib/cn";
import { landingBundles } from "./copy";
import { cjkTracking, FooterLine, PublicHeader, Section, SectionHead } from "./page-kit";
import { gb, PLAN_NUMBERS, PRO_PRICE, THROTTLED_KB_PER_SEC, yearlySaving } from "./pricing-plans";
import { type WaitlistState, WaitlistForm } from "./waitlist";

// Added, never overwriting: a key the app already translates keeps its wording.
for (const [lng, bundle] of Object.entries(landingBundles)) i18n.addResourceBundle(lng, "translation", bundle, true, false);

/** The plan an address names (`?plan=pro`), Free otherwise (where Free has a waitlist: sign-up by invite, or closed). */
export function planOfSearch(search: string): WaitlistPlan {
  return new URLSearchParams(search).get("plan") === "pro" ? "pro" : "free";
}

function PlanHead({
  testId,
  name,
  recommended,
  price,
  unit,
  note,
}: {
  testId: string;
  name: string;
  recommended?: boolean;
  price: string;
  unit?: string;
  note: string;
}) {
  const { t } = useTranslation();
  return (
    <div data-testid={testId} role="group" aria-labelledby={`${testId}-name`} className="flex min-w-0 flex-col items-center gap-4 text-center">
      <div className="flex min-h-6 flex-wrap items-center justify-center gap-2">
        <h3 id={`${testId}-name`} className="text-base leading-6 font-semibold text-fg">{name}</h3>
        {recommended && (
          <span className={cn("inline-flex items-center bg-fg px-1.5 py-0.5 text-xs font-medium tracking-label text-bg uppercase", cjkTracking)}>
            {t("Recommended")}
          </span>
        )}
      </div>
      <p className="flex flex-wrap items-baseline justify-center gap-2">
        <span className="text-4xl leading-none font-semibold tracking-tight text-fg tabular-nums">{price}</span>
        {unit && <span className="text-sm text-muted">{unit}</span>}
      </p>
      <p className="text-sm leading-5 text-balance text-muted">{note}</p>
    </div>
  );
}

function Plans({ signup }: { signup: SignupMode }) {
  const { t } = useTranslation();
  const { free, pro } = PLAN_NUMBERS;

  const yes = (
    <span className="inline-flex items-center">
      <Check aria-hidden="true" className="size-4" />
      <span className="sr-only">{t("included")}</span>
    </span>
  );
  const rows: { title: string; detail?: string; values: [ReactNode, ReactNode, ReactNode] }[] = [
    { title: t("Devices that run AI"), values: ["1", String(free.hosts), String(pro.hosts)] },
    { title: t("All your devices"), detail: t("Including phones and browsers"), values: [t("That device, and browsers on the same network"), String(free.devices), String(pro.devices)] },
    {
      title: t("Offline days"),
      detail: t("Sessions your phone can read"),
      values: [t("None"), t("{{n}} days", { n: free.retentionDays }), t("{{n}} days", { n: pro.retentionDays })],
    },
    {
      title: t("A month's transfer"),
      detail: t("Through our servers"),
      values: [
        t("None"),
        t("{{n}} GB, then {{kb}} KB/s", { n: gb(free.transferBytes), kb: THROTTLED_KB_PER_SEC }),
        t("{{n}} GB, then billed by use", { n: gb(pro.transferBytes) }),
      ],
    },
    { title: t("End-to-end encrypted"), values: [yes, yes, yes] },
  ];

  return (
    <Section id="plans">
      <div className="overflow-x-auto overscroll-x-contain">
        <table aria-label={t("Compare")} data-testid="compare-table" className="w-full min-w-[48rem] table-fixed border-collapse text-left">
          <colgroup>
            <col className="w-[14rem] max-md:w-[9rem]" />
            <col />
            <col />
            <col />
          </colgroup>
          <thead>
            <tr className="border-b border-border align-top">
              <td className="sticky left-0 z-10 border-r border-border bg-bg" />
              <th scope="col" className="border-l border-border p-6 align-top max-md:p-4">
                <PlanHead
                  testId="plan-local"
                  name={t("Local only")}
                  price="$0"
                  unit={t("forever")}
                  note={t("No account, nothing to sign up for")}
                />
              </th>
              <th scope="col" className="border-l border-border p-6 align-top max-md:p-4">
                <PlanHead
                  testId="plan-free"
                  name={t("Free")}
                  price="$0"
                  note={signup === "invite" ? t("Sign-up is by invite for now") : signup === "closed" ? t("Sign-up is closed for now") : t("Sign up with an e-mail address")}
                />
              </th>
              <th scope="col" className="border-l border-border p-6 align-top max-md:p-4">
                <PlanHead
                  testId="plan-pro"
                  name="Pro"
                  recommended
                  price={`$${PRO_PRICE.monthly}`}
                  unit={t("USD / month")}
                  note={t("or ${{price}} a year: save {{saving}}%", { price: PRO_PRICE.yearly, saving: yearlySaving() })}
                />
              </th>
            </tr>
          </thead>
          <tbody id="compare">
            {rows.map((r) => (
              <tr key={r.title} className="border-b border-border">
                <th scope="row" className="sticky left-0 z-10 border-r border-border bg-bg p-5 text-left align-top font-normal max-md:p-4">
                  <span className="block text-sm leading-5 font-semibold text-fg">{r.title}</span>
                  {r.detail && <span className="block text-xs leading-4 text-muted">{r.detail}</span>}
                </th>
                {r.values.map((v, i) => (
                  <td key={i} className="border-l border-border p-5 text-center align-top text-sm leading-5 text-muted tabular-nums max-md:p-4">
                    {v}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
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
      <div className="mx-auto flex max-w-[34rem] flex-col gap-8 px-6 py-12 max-md:px-4 max-md:py-10">
        <SectionHead
          label={t("Waitlist")}
          title={
            signup === "invite"
              ? t("Leave your address, and we'll write when it's your turn")
              : signup === "closed"
                ? t("Leave your address, and we'll write when sign-up or Pro opens")
                : t("Leave your address, and we'll write when Pro opens")
          }
          className="items-center p-0 text-center [&>h2]:font-semibold"
        />
        <WaitlistForm api={api} plan={plan} onPlan={onPlan} state={state} signup={signup} />
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
    <Section id="faq" className="pb-10 max-md:pb-8">
      <SectionHead title={t("FAQ")} className="items-center text-center [&>h2]:font-semibold" />
      {/* Kept mounted while closed: the answers stay in the page for search and for a reader whose browser does not run the page's script. */}
      <Accordion.Root data-testid="faq" multiple={false} keepMounted className="border-t border-border">
        {items.map((item) => (
          <Accordion.Item key={item.q} value={item.q} data-testid="faq-item" className="group border-b border-border">
            <Accordion.Header className="m-0">
              <Accordion.Trigger className="flex w-full cursor-pointer items-start justify-between gap-6 bg-transparent px-6 py-5 text-left transition-colors hover:bg-hover max-md:px-4">
                <span className="text-base leading-6 font-semibold text-fg">{item.q}</span>
                <Plus aria-hidden="true" className="mt-1 size-4 shrink-0 text-muted transition-transform duration-150 group-data-[open]:rotate-45 motion-reduce:transition-none" />
              </Accordion.Trigger>
            </Accordion.Header>
            <Accordion.Panel className="px-6 pb-6 max-md:px-4">
              <p className="max-w-prose text-base leading-6 text-muted">{item.a}</p>
            </Accordion.Panel>
          </Accordion.Item>
        ))}
      </Accordion.Root>
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
        <main className="mx-auto max-w-[1360px] md:border-x md:border-border xl:hatch-45">
          <div className="mx-auto max-w-[68rem] bg-bg xl:border-x xl:border-border">
            <div className="mx-auto flex max-w-[44rem] flex-col items-center gap-4 px-6 pt-16 pb-14 text-center max-md:px-4 max-md:pt-10 max-md:pb-10">
              <h1 className="text-5xl leading-display font-semibold tracking-tighter text-balance text-fg cjk-wrap md:cjk-keep max-md:text-4xl">
                {t("Start free. Go Pro when you need more.")}
              </h1>
              <p className="max-w-[32em] text-base leading-6 text-muted">
                {t("End-to-end encrypted: we can't read your sessions, code or terminals")}
              </p>
              <p className="text-sm leading-5 text-faint">{t("Prices in US dollars. Pro is not on sale yet, and nothing is charged before you choose to pay.")}</p>
            </div>
            <Plans signup={signup} />
            <Waitlist api={hub} plan={plan} onPlan={open ? undefined : setPlan} state={waitlist} signup={signup} />
            <Faq />
            <FooterLine />
          </div>
        </main>
      </div>
    </div>
  );
}
