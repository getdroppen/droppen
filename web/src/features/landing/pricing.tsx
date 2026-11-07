import { type ReactNode, useState } from "react";
import { useTranslation } from "react-i18next";
import { CheckIcon } from "@phosphor-icons/react";
import { Card } from "@/components/ui/card";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { type HubApi, type SignupMode, type WaitlistPlan } from "@/lib/hub/api";
import { cn } from "@/lib/cn";
import "./copy";
import { DocumentHead, FaqAccordion, PublicLayout, RecommendedBadge, Section, SectionHead, SUBHEAD, usePageMeta } from "./page-kit";
import { gb, PLAN_NUMBERS, PRO_PRICE, THROTTLED_KB_PER_SEC, yearlySaving } from "./pricing-plans";
import { SIGNUP_COPY } from "./signup-copy";
import { type WaitlistState, WaitlistForm } from "./waitlist";

export function planOfSearch(search: string): WaitlistPlan {
  return new URLSearchParams(search).get("plan") === "pro" ? "pro" : "free";
}

const SECTION_SPACING = "py-20 max-md:py-14";

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
        <h3 id={`${testId}-name`} className={SUBHEAD}>{name}</h3>
        {recommended && <RecommendedBadge>{t("Recommended")}</RecommendedBadge>}
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
      <CheckIcon aria-hidden="true" className="size-4" />
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
    <Section id="plans" rule={false} seam={false}>
      <DocumentHead
        center
        className={cn("pt-0", SECTION_SPACING)}
        title={t("Start free. Go Pro when you need more.")}
        lead={t("End-to-end encrypted: we can't read your sessions, code or terminals")}
        note={t("Prices in US dollars. Pro is not on sale yet, and nothing is charged before you choose to pay.")}
      />
      <div className="mx-auto max-w-[68rem] px-6 pb-20 max-md:px-4 max-md:pb-16">
        <Card className="gap-0 overflow-hidden p-0">
          <div className="overflow-x-auto overscroll-x-contain">
            <Table aria-label={t("Compare")} data-testid="compare-table" className="min-w-[48rem] table-fixed border-collapse text-left">
              <colgroup>
                <col className="w-[14rem] max-md:w-[9rem]" />
                <col />
                <col />
                <col />
              </colgroup>
              <TableHeader>
                <TableRow className="border-b border-border align-top hover:bg-transparent">
                  <TableHead className="sticky left-0 z-10 border-r border-border bg-bg" />
                  <TableHead scope="col" className="border-l border-border p-6 align-top whitespace-normal max-md:p-4">
                    <PlanHead
                      testId="plan-local"
                      name={t("Local only")}
                      price="$0"
                      unit={t("forever")}
                      note={t("No account, nothing to sign up for")}
                    />
                  </TableHead>
                  <TableHead scope="col" className="border-l border-border p-6 align-top whitespace-normal max-md:p-4">
                    <PlanHead
                      testId="plan-free"
                      name={t("Free")}
                      price="$0"
                      note={t(SIGNUP_COPY[signup].planNote)}
                    />
                  </TableHead>
                  <TableHead scope="col" className="border-l border-border p-6 align-top whitespace-normal max-md:p-4">
                    <PlanHead
                      testId="plan-pro"
                      name="Pro"
                      recommended
                      price={`$${PRO_PRICE.monthly}`}
                      unit={t("USD / month")}
                      note={t("or ${{price}} a year: save {{saving}}%", { price: PRO_PRICE.yearly, saving: yearlySaving() })}
                    />
                  </TableHead>
                </TableRow>
              </TableHeader>
              <TableBody id="compare">
                {rows.map((r) => (
                  <TableRow key={r.title} className="border-b border-border last:border-b-0 hover:bg-transparent">
                    <TableHead scope="row" className="sticky left-0 z-10 h-auto border-r border-border bg-bg p-5 text-left align-top font-normal whitespace-normal max-md:p-4">
                      <span className="block text-sm leading-5 font-semibold text-fg">{r.title}</span>
                      {r.detail && <span className="block text-xs leading-4 text-muted">{r.detail}</span>}
                    </TableHead>
                    {r.values.map((v, i) => (
                      <TableCell key={i} className="border-l border-border p-5 text-center align-top text-sm leading-5 whitespace-normal text-muted tabular-nums max-md:p-4">
                        {v}
                      </TableCell>
                    ))}
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </div>
        </Card>
      </div>
    </Section>
  );
}

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
  const copy = SIGNUP_COPY[signup];
  return (
    <Section id="waitlist">
      <div className={cn("mx-auto flex max-w-[34rem] flex-col gap-8 px-6 max-md:px-4", SECTION_SPACING)}>
        <SectionHead
          label={t("Waitlist")}
          title={t(copy.waitlistTitle)}
          lead={t(copy.waitlistLead)}
          className="items-center p-0 text-center [&>h2]:font-semibold [&>p]:max-w-[34em] [&>p]:text-balance"
        />
        <WaitlistForm api={api} plan={plan} onPlan={onPlan} initialState={state} signup={signup} />
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
      <div className={cn("mx-auto grid max-w-[68rem] gap-8 lg:grid-cols-[minmax(0,0.45fr)_minmax(0,1fr)] lg:items-center lg:gap-16", SECTION_SPACING)}>
        <h2 className="text-2xl leading-8 font-semibold tracking-tight text-balance text-fg cjk-wrap md:cjk-keep max-md:px-4 max-md:text-xl">{t("FAQ")}</h2>
        <FaqAccordion
          testId="faq"
          items={items}
          className="min-w-0 max-md:mx-4"
          triggerClassName="px-6 py-5 max-md:px-4"
          panelClassName="px-6 pb-6 max-md:px-4"
        />
      </div>
    </Section>
  );
}

export default function PricingPage({
  signedIn = false,
  api,
  waitlist,
  signup = "open",
}: {
  signedIn?: boolean;
  api: Pick<HubApi, "joinWaitlist">;
  waitlist?: WaitlistState;
  signup?: SignupMode;
}) {
  const { t } = useTranslation();
  const open = signup === "open";
  usePageMeta(
    t("Droppen pricing — start free, Pro for ${{price}} a month", { price: PRO_PRICE.monthly }),
    t(SIGNUP_COPY[signup].pricingDescription),
  );
  const [chosen, setPlan] = useState<WaitlistPlan>(() => planOfSearch(typeof location === "undefined" ? "" : location.search));
  const plan = open ? "pro" : chosen;
  return (
    <PublicLayout testId="pricing" signedIn={signedIn}>
      <Plans signup={signup} />
      <Waitlist api={api} plan={plan} onPlan={open ? undefined : setPlan} state={waitlist} signup={signup} />
      <Faq />
    </PublicLayout>
  );
}
