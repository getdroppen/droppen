import { type FormEvent, useState } from "react";
import { useTranslation } from "react-i18next";
import { Check, Loader2 } from "lucide-react";
import { CapsLabel } from "@/components/ui/caps-label";
import { field, primary } from "@/features/cloud/ui";
import { type HubApi, HubApiError, type WaitlistPlan } from "@/lib/hub/api";
import { cn } from "@/lib/cn";
import { PRO_PRICE } from "./pricing-plans";
import { publicLink, PRIVACY_PATH } from "./route";

/*
 * The waitlist form on droppen.app/pricing (`#waitlist`): an e-mail address,
 * for Pro (word when it can be bought), or — where sign-up is by invite — a
 * plan to choose too: Free (an invite) or Pro. Submitting is joining: no
 * confirmation mail, and the hub
 * answers the same whether the address was on the list already
 * (`POST /v1/waitlist`, docs/hub-api/http.md). Its outcomes, said plainly:
 * on the list; not an address; a throwaway address; too many tries from this
 * network (with when to try again); the hub out of reach.
 */

/** Where the form is: idle, sending, joined, or refused and why. */
export type WaitlistState =
  | { kind: "idle" }
  | { kind: "sending" }
  | { kind: "done"; plan: WaitlistPlan }
  | { kind: "error"; reason: WaitlistProblem; retryMinutes?: number };

export type WaitlistProblem = "email" | "disposable" | "rate" | "unreachable";

/** Whether `text` could be an address at all (the hub has the last word). */
export function looksLikeEmail(text: string): boolean {
  const email = text.trim();
  return email.length <= 254 && /^[^\s@]+@[^\s@.]+(\.[^\s@.]+)+$/.test(email);
}

/** Why the hub refused, as the form tells it. */
export function waitlistProblem(error: unknown): { reason: WaitlistProblem; retryMinutes?: number } {
  if (error instanceof HubApiError) {
    if (error.status === 429) return { reason: "rate", retryMinutes: Math.max(1, Math.ceil((error.retryAfterMs ?? 60_000) / 60_000)) };
    if (error.code === "disposable_email") return { reason: "disposable" };
    if (error.status === 400) return { reason: "email" };
  }
  return { reason: "unreachable" };
}

/** Join, and say how it went (an address that cannot be one is not sent). */
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

/** A refusal in words. */
export function problemText(state: { reason: WaitlistProblem; retryMinutes?: number }, t: T): string {
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

/** The privacy contact, for removals and requests about data (the owner's word, 2026-10-23). */
export const PRIVACY_EMAIL = "privacy@droppen.app";

/** The two plans as one segmented choice: native radios, so keyboards and screen readers get them for free. */
function PlanChoice({ plan, onPlan }: { plan: WaitlistPlan; onPlan: (plan: WaitlistPlan) => void }) {
  const { t } = useTranslation();
  const options: { id: WaitlistPlan; name: string; price: string }[] = [
    { id: "free", name: t("Free"), price: "$0" },
    { id: "pro", name: "Pro", price: t("${{price}} / month", { price: PRO_PRICE.monthly }) },
  ];
  return (
    <fieldset className="flex flex-col gap-1.5">
      <legend className="mb-1.5">
        <CapsLabel>{t("Plan")}</CapsLabel>
      </legend>
      <div className="grid grid-cols-2 gap-px border border-border-strong bg-border-strong">
        {options.map((o) => (
          <label
            key={o.id}
            className={cn(
              "flex h-9 cursor-pointer items-center justify-center gap-2 bg-bg px-3 text-[13px] font-medium text-fg hover:bg-hover",
              "has-[:focus-visible]:outline-2 has-[:focus-visible]:-outline-offset-2 has-[:focus-visible]:outline-accent",
              plan === o.id && "bg-fg text-bg hover:bg-fg",
            )}
          >
            <input type="radio" name="plan" value={o.id} checked={plan === o.id} onChange={() => onPlan(o.id)} data-testid={`waitlist-plan-${o.id}`} className="sr-only" />
            <span>{o.name}</span>
            <span className={cn("font-mono text-2xs", plan === o.id ? "text-bg/70" : "text-faint")}>{o.price}</span>
          </label>
        ))}
      </div>
    </fieldset>
  );
}

/** Joined: what happens next, and nothing else to do. */
function Joined({ plan, onAgain }: { plan: WaitlistPlan; onAgain: () => void }) {
  const { t } = useTranslation();
  return (
    <div role="status" data-testid="waitlist-done" data-plan={plan} className="flex max-w-[34rem] flex-col gap-3 border border-border bg-raised px-5 py-5 max-md:px-4">
      <p className="flex items-start gap-2.5 text-[15px] leading-6 font-semibold text-fg">
        <Check aria-hidden="true" className="mt-1 size-4 shrink-0" />
        <span>{plan === "pro" ? t("You're on the list: we'll e-mail you when Pro opens.") : t("You're on the list: we'll e-mail you an invite.")}</span>
      </p>
      <p className="text-xs leading-5 text-muted">{t("We'll send nothing else. To be taken off the list, write to {{email}}.", { email: PRIVACY_EMAIL })}</p>
      <button type="button" onClick={onAgain} data-testid="waitlist-again" className="w-fit text-xs text-fg underline underline-offset-4 hover:no-underline">
        {t("Add another address")}
      </button>
    </div>
  );
}

/**
 * The form. `plan` is the page's (the plan cards choose it too); with
 * `onPlan` the form offers both plans to choose from, without it only `plan`
 * is joined. `state` stands in for the form's own (tests, previews).
 */
export function WaitlistForm({
  api,
  plan,
  onPlan,
  state: given,
}: {
  api: Pick<HubApi, "joinWaitlist">;
  plan: WaitlistPlan;
  onPlan?: (plan: WaitlistPlan) => void;
  state?: WaitlistState;
}) {
  const { t, i18n } = useTranslation();
  const [email, setEmail] = useState("");
  const [own, setOwn] = useState<WaitlistState>({ kind: "idle" });
  const state = given ?? own;
  if (state.kind === "done") return <Joined plan={state.plan} onAgain={() => setOwn({ kind: "idle" })} />;
  const sending = state.kind === "sending";
  const refused = state.kind === "error" ? state : null;
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (sending) return;
    setOwn({ kind: "sending" });
    setOwn(await submitWaitlist(api, { email, plan, locale: i18n.language }));
  };
  return (
    <form noValidate onSubmit={(e) => void submit(e)} data-testid="waitlist-form" data-plan={plan} className="flex max-w-[34rem] flex-col gap-5">
      {onPlan && <PlanChoice plan={plan} onPlan={onPlan} />}
      <label className="flex flex-col gap-1.5">
        <CapsLabel>{t("E-mail address")}</CapsLabel>
        <input
          id="waitlist-email"
          type="email"
          name="email"
          autoComplete="email"
          inputMode="email"
          required
          maxLength={254}
          value={email}
          onChange={(e) => {
            setEmail(e.target.value);
            if (refused) setOwn({ kind: "idle" });
          }}
          placeholder="you@example.com"
          aria-invalid={refused?.reason === "email" || refused?.reason === "disposable" ? true : undefined}
          aria-describedby={refused ? "waitlist-error" : undefined}
          className={field}
        />
      </label>
      {refused && (
        <p id="waitlist-error" role="alert" data-signal="failed" data-testid="waitlist-error" data-reason={refused.reason} className="font-mono text-2xs leading-4 text-danger">
          {problemText(refused, t)}
        </p>
      )}
      <div className="flex flex-col gap-3">
        <button type="submit" data-variant="primary" data-testid="waitlist-submit" disabled={sending} className={cn(primary, "w-full sm:w-fit")}>
          {sending && <Loader2 aria-hidden="true" className="size-3.5 animate-spin" />}
          {t("Join the waitlist")}
        </button>
        <p className="text-xs leading-5 text-muted">
          {t("No confirmation mail and no newsletter: we use your address only to send what you asked for.")}{" "}
          <a {...publicLink(PRIVACY_PATH)} data-testid="waitlist-privacy" className="text-fg underline underline-offset-4 hover:no-underline">
            {t("Privacy notice")}
          </a>
        </p>
      </div>
    </form>
  );
}
