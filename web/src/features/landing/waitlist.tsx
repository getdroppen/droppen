import { type FormEvent, useState } from "react";
import { useTranslation } from "react-i18next";
import { CheckIcon, CircleNotchIcon } from "@phosphor-icons/react";
import { Button } from "@/components/ui/button";
import { CapsLabel } from "@/components/ui/caps-label";
import { Input } from "@/components/ui/input";
import { type HubApi, HubApiError, type SignupMode, type WaitlistPlan } from "@/lib/hub/api";
import { cn } from "@/lib/cn";
import { PRIVACY_EMAIL, SUBHEAD } from "./page-kit";
import { SIGNUP_COPY } from "./signup-copy";
import { PRO_PRICE } from "./pricing-plans";


export type WaitlistState =
  | { kind: "idle" }
  | { kind: "sending" }
  | { kind: "done"; plan: WaitlistPlan }
  | { kind: "error"; reason: WaitlistProblem; retryMinutes?: number };

type WaitlistProblem = "email" | "disposable" | "rate" | "unreachable";

export function looksLikeEmail(text: string): boolean {
  const email = text.trim();
  return email.length <= 254 && /^[^\s@]+@[^\s@.]+(\.[^\s@.]+)+$/.test(email);
}

export function waitlistProblem(error: unknown): { reason: WaitlistProblem; retryMinutes?: number } {
  if (error instanceof HubApiError) {
    if (error.status === 429) return { reason: "rate", retryMinutes: Math.max(1, Math.ceil((error.retryAfterMs ?? 60_000) / 60_000)) };
    if (error.code === "disposable_email") return { reason: "disposable" };
    if (error.status === 400) return { reason: "email" };
  }
  return { reason: "unreachable" };
}

export async function submitWaitlist(
  api: Pick<HubApi, "joinWaitlist">,
  input: { email: string; plan: WaitlistPlan; locale?: string },
): Promise<WaitlistState> {
  if (!looksLikeEmail(input.email)) return { kind: "error", reason: "email" };
  try {
    await api.joinWaitlist({ email: input.email.trim(), plan: input.plan, locale: input.locale });
    return { kind: "done", plan: input.plan };
  } catch (error) {
    return { kind: "error", ...waitlistProblem(error) };
  }
}

type T = (key: string, opts?: Record<string, unknown>) => string;

function problemText(state: { reason: WaitlistProblem; retryMinutes?: number }, t: T): string {
  switch (state.reason) {
    case "email":
      return t("That does not look like an e-mail address. Check it and try again.");
    case "disposable":
      return t("Use an address you keep: throwaway mail providers are not accepted.");
    case "rate":
      return t("Too many tries from your network. Try again in {{n}} min.", { n: state.retryMinutes ?? 1 });
    case "unreachable":
      return t("Could not reach Droppen. Check your connection and try again.");
  }
}

function PlanChoice({ plan, onPlan }: { plan: WaitlistPlan; onPlan: (plan: WaitlistPlan) => void }) {
  const { t } = useTranslation();
  const options: { id: WaitlistPlan; name: string; price: string }[] = [
    { id: "free", name: t("Free"), price: "$0" },
    { id: "pro", name: "Pro", price: t("${{price}} / month", { price: PRO_PRICE.monthly }) },
  ];
  return (
    <fieldset className="flex flex-col gap-1.5">
      <legend className="mb-1.5">
        <CapsLabel variant="section">{t("Plan")}</CapsLabel>
      </legend>
      <div className="grid grid-cols-2 gap-px border border-border-strong bg-border-strong">
        {options.map((o) => (
          <label
            key={o.id}
            className={cn(
              "flex h-9 cursor-pointer items-center justify-center gap-2 bg-bg px-3 text-sm font-medium text-fg hover:bg-hover max-md:h-10",
              "has-[:focus-visible]:outline-2 has-[:focus-visible]:-outline-offset-2 has-[:focus-visible]:outline-accent",
              plan === o.id && "bg-fg text-bg hover:bg-fg",
            )}
          >
            <input type="radio" name="plan" value={o.id} checked={plan === o.id} onChange={() => onPlan(o.id)} data-testid={`waitlist-plan-${o.id}`} className="sr-only" />
            <span>{o.name}</span>
            <span className={cn("text-xs font-semibold tabular-nums", plan === o.id ? "text-bg/70" : "text-faint")}>{o.price}</span>
          </label>
        ))}
      </div>
    </fieldset>
  );
}

function Joined({ plan, signup, onAgain }: { plan: WaitlistPlan; signup: SignupMode; onAgain: () => void }) {
  const { t } = useTranslation();
  return (
    <div role="status" data-testid="waitlist-done" data-plan={plan} className="flex w-full flex-col gap-3 border border-border bg-raised px-5 py-5 max-md:px-4">
      <p className={cn("flex items-start gap-2.5", SUBHEAD)}>
        <CheckIcon aria-hidden="true" className="mt-1 size-4 shrink-0" />
        <span>
          {plan === "pro" ? t("You're on the list: we'll e-mail you when Pro opens.") : t(SIGNUP_COPY[signup].joined)}
        </span>
      </p>
      <p className="text-sm leading-6 text-muted">{t("We'll send nothing else. To be taken off the list, write to {{email}}.", { email: PRIVACY_EMAIL })}</p>
      <Button
        type="button"
        variant="ghost"
        onClick={onAgain}
        data-testid="waitlist-again"
        className="h-auto w-fit px-0 text-sm text-fg underline underline-offset-4 hover:bg-transparent hover:no-underline"
      >
        {t("Add another address")}
      </Button>
    </div>
  );
}

export function WaitlistForm({
  api,
  plan,
  onPlan,
  initialState,
  signup = "open",
}: {
  api: Pick<HubApi, "joinWaitlist">;
  plan: WaitlistPlan;
  onPlan?: (plan: WaitlistPlan) => void;
  initialState?: WaitlistState;
  signup?: SignupMode;
}) {
  const { t, i18n } = useTranslation();
  const [email, setEmail] = useState("");
  const [state, setState] = useState<WaitlistState>(initialState ?? { kind: "idle" });
  if (state.kind === "done") return <Joined plan={state.plan} signup={signup} onAgain={() => setState({ kind: "idle" })} />;
  const sending = state.kind === "sending";
  const refused = state.kind === "error" ? state : null;
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (sending) return;
    setState({ kind: "sending" });
    setState(await submitWaitlist(api, { email, plan, locale: i18n.language }));
  };
  return (
    <form noValidate onSubmit={(e) => void submit(e)} data-testid="waitlist-form" data-plan={plan} className="flex w-full flex-col gap-5">
      {onPlan && <PlanChoice plan={plan} onPlan={onPlan} />}
      <div className="flex mx-auto w-full max-w-[26rem] flex-col gap-2 max-md:max-w-none">
        <Input
          id="waitlist-email"
          type="email"
          name="email"
          autoComplete="email"
          inputMode="email"
          spellCheck={false}
          required
          maxLength={254}
          value={email}
          onChange={(e) => {
            setEmail(e.target.value);
            if (refused) setState({ kind: "idle" });
          }}
          placeholder={t("you@example.com")}
          aria-label={t("E-mail address")}
          aria-invalid={refused?.reason === "email" || refused?.reason === "disposable" ? true : undefined}
          aria-describedby={refused ? "waitlist-error" : undefined}
          className="h-10 text-sm max-md:h-11"
        />
        <Button type="submit" variant="primary" size="lg" data-testid="waitlist-submit" disabled={sending} className="h-10 w-full">
          {sending ? <CircleNotchIcon aria-hidden="true" className="size-3.5 animate-spin" /> : <CheckIcon className="size-3.5" />}
          {t("Join the waitlist")}
        </Button>
        {refused && (
          <p id="waitlist-error" role="alert" data-signal="failed" data-testid="waitlist-error" data-reason={refused.reason} className="text-xs font-semibold leading-4 text-danger">
            {problemText(refused, t)}
          </p>
        )}
      </div>
    </form>
  );
}
