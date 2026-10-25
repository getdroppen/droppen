import { type ReactNode, useState } from "react";
import { useTranslation } from "react-i18next";
import { Check, Copy } from "lucide-react";
import { BrandMark } from "@/components/brand-mark";
import { CapsLabel } from "@/components/ui/caps-label";
import { RegMark } from "@/components/reg-mark";
import { cn } from "@/lib/cn";
import { formatMoment } from "@/lib/format";
import i18next from "i18next";
import { authErrorKind, CODE_PAIRING_WINDOW_MS, HubApiError, hubErrorKind, isNetworkError } from "@/lib/hub/api";
import { ChainError, RULES } from "@/lib/e2e/chain";
import { AccountError, RenewalError, renewalRefusal } from "@/lib/hub/account";
import { ChannelIsolated, ChannelRefused } from "@/lib/hub/machine";
import { DEVICES_KEY, HOSTS_KEY, limitOf, MACHINES_KEY, STORAGE_KEY } from "@/lib/hub/plan";
import { limitText } from "./plan";

/*
 * The pieces the cloud screens share (sign-in, set-up, recovery, pairing,
 * machines, devices), in the drafting-sheet language: a 56px top bar and a
 * page framed by hairlines with registration marks at its seams — edge to
 * edge below 1280px, then 32px in from the window and centred past 2400px —
 * ink primary buttons, white fields that ink on focus, and colour only for
 * state. Cloud pages use the 36px control size
 * (docs/ui-spec.md §2).
 */

export const field =
  "h-9 w-full border border-border bg-bg px-2.5 font-mono text-xs text-fg placeholder:text-faint focus:border-fg focus:outline-none max-md:text-base";
export const primary =
  "flex h-9 items-center justify-center gap-1.5 border border-fg bg-fg px-4 text-[13px] font-medium text-bg hover:bg-fg/85 disabled:pointer-events-none disabled:opacity-40";
export const secondary =
  "flex h-9 items-center justify-center gap-1.5 border border-border-strong bg-bg px-4 text-[13px] font-medium text-fg hover:bg-hover disabled:pointer-events-none disabled:opacity-40";
export const quiet =
  "flex h-9 items-center justify-center gap-1.5 border border-transparent px-3 text-xs text-muted hover:bg-hover hover:text-fg disabled:pointer-events-none disabled:opacity-40";
export const danger =
  "flex h-9 items-center justify-center gap-1.5 border border-danger/55 bg-bg px-4 text-[13px] font-medium text-danger hover:bg-danger/8 disabled:pointer-events-none disabled:opacity-40";

/** The limits the page has words for (another key: the hub's own message). */
const PLAN_LIMITS = new Set([DEVICES_KEY, HOSTS_KEY, MACHINES_KEY, STORAGE_KEY]);

/** An error as a person reads it: the hub's caps and refusals in words, else its message. */
export function errorText(error: unknown): string {
  const t = (key: string, vars?: Record<string, unknown>) => i18next.t(key, vars);
  // An entitlement used up (409 `limit`): the plan's words for it (relay-and-plans §3.8).
  const limit = limitOf(error);
  if (limit && PLAN_LIMITS.has(limit.key)) return limitText(limit, t);
  switch (hubErrorKind(error)) {
    case "mailRate":
      return t("the hub is holding back this device's mail for a minute: try again shortly");
    case "mailboxFull":
      return t("that device's mailbox at the hub is full: open it there so it takes its mail, then try again");
    case "keyringFull":
      return t("this account's keyring at the hub is full");
    case "notFullDevice":
      return t("the hub says this device is not a full device of the account");
    case "notAnAdmin":
      return t("only a full device of the account may receive that");
    case "rotationInProgress":
      return t("another of your devices is renewing it now");
    case "rotationNotReady":
      return renewalErrorText(renewalRefusal(error) as RenewalError, t);
    case "chainV3NotReady": {
      const waiting = (error as HubApiError).waitingHosts ?? [];
      return waiting.length
        ? t("update Droppen on these devices first: {{hosts}}", { hosts: waiting.join(", ") })
        : t("this change is not available on Droppen yet");
    }
    case "exists":
      return t("the hub already holds another key under that name: nothing was overwritten");
    case "removed":
      return t("the hub says this device was removed from the account");
    case "emailUnverified":
      return t("verify this account's e-mail first: open the link we sent you");
    case "rate":
      return retryText(error);
    case "deviceRate":
      return t("this account added as many new devices as it may today: try again tomorrow");
    case "tooManyPendingDevices":
      return t("too many new devices are waiting to be added to this account: add them from a device already in it, or try again in {{n}} minutes", { n: Math.round(CODE_PAIRING_WINDOW_MS / 60_000) });
    case "tooManyEnrollments":
      return t("too many devices are being added at once: finish adding them, or let them lapse, then try again");
    case "pairingWindowClosed":
      return t("this browser signed in too long ago to make a code: sign in again, or use the QR code");
    case "tooManyPairings":
      return t("too many codes are open in this account: try again in 10 minutes, or use the QR code");
    case "pairingFull":
      return t("this code was tried too often: start again for a new one");
    case "unavailable":
      return t("Droppen is not answering properly right now: try again in a moment");
    case "reauthFailed":
      return t("that did not prove it is you: check the password, or use one of this account's passkeys");
    case "reauthRequired":
      return t("prove it is you first: the current password, or one of this account's passkeys");
    case "lastCredential":
      return t("the account's last way to sign in: set a password or add another passkey first");
    case "unlockOn":
      return t("new devices can still join with that passkey: turn that off first");
    case "tooManyPasskeys":
      return t("this account has as many passkeys as it may: remove one first");
    case "passkeyExists":
      return t("that passkey is in an account already");
    case "noPasskey":
      return t("this account has no passkey: give its current password");
    case "passkeyRefused":
      return t("that passkey could not be checked: try again");
    case "confirmMismatch":
      return t("that is not this account's e-mail address: type it exactly as shown");
    case "accountSuspended":
      return t("this account is suspended: it cannot be used for now");
    case "accountDeleted":
      return t("this account was deleted");
    case "recentlyDeleted":
      return recentlyDeletedText(error);
    case null:
      break;
  }
  // No answer at all (offline, a dropped connection): the browser's own words differ, and are not the reader's language.
  if (isNetworkError(error)) return t("Droppen could not be reached: check the connection and try again");
  // A refusal with no words here: the hub's message is not the reader's
  // language (nor meant for a person) — its code, for a report, in a
  // sentence that is. A limit is the exception: its key names it (above).
  if (error instanceof HubApiError) return limitOf(error) ? error.message || error.code : t("Droppen could not do that ({{code}})", { code: error.code });
  // The page's own errors whose message is an i18n key (a channel that did
  // not open, an enrollment proof that did not check out): in words.
  // A join or an enrollment with an identity key a renewal retired (`RULES.keyRetired`, docs/host-identity-rotation.md
  // §3.4 rule 8): that device was renewed elsewhere, or restored from before a renewal — it comes back under a new identity.
  if (error instanceof AccountError && error.kind === "invalidEntry" && error.rule === RULES.keyRetired) {
    return t("That device's identity key was replaced by a renewal, so it cannot join with it again. Add it again under a new identity: on that device, choose “add this device again with a new identity”, then use a new registration code.");
  }
  if (error instanceof AccountError && i18next.exists(error.key)) return t(error.key, error.vars);
  // A host's identity key renewal (docs/host-identity-rotation.md §4.2), and a host isolated after one (§3.8).
  if (error instanceof RenewalError) return renewalErrorText(error, t);
  if (error instanceof ChannelIsolated) return t("{{id}} is isolated: confirm its identity key on the device itself first", { id: error.machine });
  // The relay's or the hub's own reason, inside the reader's sentence.
  if (error instanceof ChannelRefused) {
    if (!error.why) return t(error.by === "relay" ? "the relay refused the channel" : "the hub refused the channel");
    return t(error.by === "relay" ? "the relay refused the channel: {{why}}" : "the hub refused the channel: {{why}}", { why: error.why });
  }
  // An entry of a chain version this page cannot read: the account moved on
  // past this build of Droppen (local-aggregator §3.5). A reload fetches the
  // build that reads it — nothing here is an attack.
  if (error instanceof ChainError && error.kind === "version") {
    return t("this page is older than your account: reload it to update Droppen");
  }
  return error instanceof Error ? knownText(error.message) : String(error);
}

/** Why a renewal did not go through, in words. */
export function renewalErrorText(error: RenewalError, t: (key: string, opts?: Record<string, unknown>) => string): string {
  switch (error.kind) {
    case "tooOld":
      return t("its Droppen is too old to renew its identity key: update it");
    case "disabled":
      return t("Droppen does not renew identity keys yet");
    case "needsAssurance":
      return t("only a renewal confirmed on the device itself goes now");
    case "inProgress":
      return t("another of your devices is renewing it now");
    case "offline":
      return t("the device is offline");
    case "hubNotReady":
      return t("it cannot reach Droppen's hub to renew its identity key yet");
    case "waiting":
      return t("waiting for Droppen to be updated on {{ids}}", { ids: (error.hosts ?? []).join(", ") });
    case "renewFirst":
      return t("renew the identity key of {{ids}} first: it lets them control it", { ids: (error.hosts ?? []).join(", ") });
    case "superseded":
      return t("a device that could control it was removed while it renewed: try again");
    case "policyUnset":
      return t("choose how identity keys are renewed first (under my devices)");
    case "badAnswer":
      return t("its answer did not check out, so nothing was changed (another copy of its key may have answered)");
    case "wrongCode":
      return t("that is not the code the device shows, so nothing was changed");
    case "cancelled":
      return t("no code was typed");
    case "voided":
      return error.detail === "not_acked" || error.detail === "conflict"
        ? t("another copy of its old identity key signed in meanwhile: confirm on the device itself")
        : t("the renewal lapsed: try again");
    case "rate":
      return t("it renewed its identity key too often lately: try again later");
    case "refused":
      return t("it refused to renew its identity key ({{why}})", { why: error.detail ?? "" });
    case "unconfirmed":
      // Its own 412 `stale`: only another full device's word on the account's newest change counts for this device's request.
      if (error.hosts?.length) {
        return t("it waits for another of your full devices to confirm the account's latest change: open Droppen on {{ids}}, then try again", { ids: error.hosts.join(", ") });
      }
      return t("this device's word on the account's latest change did not reach it: check the connection, then try again");
    case "behind":
      return t("it is still catching up with the account's latest changes: try again in a moment");
    case "fork":
      return t("it sees the account's devices differently from what your full devices confirmed, so nothing was changed: look at Droppen on that device itself");
    case "lapsed":
      return t("it takes no control from this device now: open Droppen on another of your full devices and try again, or renew it from that device");
    case "failed":
      return t("could not reach it: {{why}}", { why: errorText(new Error(error.message)) });
  }
}

/** A sign-up refused for an address whose account was deleted lately (409 `recently_deleted`): from when it may sign up again, if the hub said. */
function recentlyDeletedText(error: unknown): string {
  const until = error instanceof HubApiError ? error.untilMs : undefined;
  if (until === undefined) return i18next.t("an account with this e-mail address was deleted a short while ago: it can sign up again in a few days");
  return i18next.t("an account with this e-mail address was deleted a short while ago: it can sign up again from {{when}}", { when: formatMoment(until, i18next.language) });
}

/** `message` in the reader's language when it is one of the page's i18n keys (as the English), else as it is. */
export function knownText(message: string): string {
  return i18next.exists(message) ? i18next.t(message) : message;
}

/**
 * Sign-up, sign-in and the verification mail's refusals in words (`context`:
 * what was being done, for the one refusal that reads differently), else
 * {@link errorText}.
 */
export function authErrorText(error: unknown, context: "signup" | "login" | "resend" = "signup"): string {
  const t = (key: string) => i18next.t(key);
  switch (authErrorKind(error)) {
    case "badLogin":
      return t("wrong e-mail or password");
    case "signupClosed":
      return t("sign-up is closed on this hub");
    case "badInvite":
      return t("that invite code is unknown, used up or expired");
    case "turnstileRequired":
      return t("complete the human check first");
    case "turnstileRetry":
      return t("the human check expired: do it again");
    case "turnstileFailed":
      return t("the human check failed: try again");
    case "turnstileUnavailable":
      return context === "signup"
        ? t("the human check cannot be reached right now, so sign-up is paused: try again in a few minutes")
        : t("the human check cannot be reached right now: try again in a few minutes");
    case "disposableEmail":
      return t("use an e-mail address you keep: throwaway mail services are not accepted");
    case "emailTaken":
      return t("an account with this e-mail exists already: sign in instead");
    case "alreadyVerified":
      return t("this e-mail is verified already");
    case "unauthorized":
      return t("your sign-in expired: sign in again");
    case "badPasskey":
      return context === "signup" ? t("the hub could not accept this passkey: try again") : t("this passkey could not sign you in: try again");
    case "unknownPasskey":
      return t("this passkey is no longer registered here: use another passkey or your password");
    case "passkeysOff":
      return t("passkeys are turned off on this hub: use your password");
    case "accountSuspended":
      return t("this account is suspended, so it cannot sign in for now");
    case "recentlyDeleted":
      return recentlyDeletedText(error);
    case "rate":
      return retryText(error);
    default:
      // What was typed did not pass the hub's checks: its message names the
      // field (`docs/hub-api`), for the form's own words — the password, the
      // e-mail address, or neither said.
      if (context !== "resend" && error instanceof HubApiError && error.status === 400 && error.code === "bad_request") {
        if (/password/i.test(error.message)) return t("Droppen did not accept that password: use 10 to 1024 characters");
        if (/e-?mail/i.test(error.message)) return t("Droppen did not accept that e-mail address: check it and try again");
        return t("Droppen did not accept what was typed: check it and try again");
      }
      return errorText(error);
  }
}

/** A 429 in words: when to try again, in whole seconds, if the hub said. */
export function retryText(error: unknown): string {
  const ms = error instanceof HubApiError ? error.retryAfterMs : undefined;
  if (ms === undefined) return i18next.t("too many attempts: try again shortly");
  return i18next.t("too many attempts: try again in {{n}} s", { n: Math.max(1, Math.ceil(ms / 1000)) });
}

/**
 * A whole cloud page: the top bar with the mark, then the framed column. The
 * registration marks sit where the bar's bottom line meets the frame.
 *
 * The bar sticks inside the same scroller as the page, so the scrollbar's
 * gutters move both by the same amount and their frame lines stay one line.
 */
export function CloudPage({ children, aside }: { children: ReactNode; aside?: ReactNode }) {
  return (
    <div className="flex h-full w-full flex-col overflow-y-auto bg-bg [scrollbar-gutter:stable_both-edges]">
      <header className="sticky top-0 z-10 h-14 shrink-0 border-b border-border bg-bg xl:px-8">
        <div className="relative mx-auto flex h-full max-w-[2400px] items-center gap-3 px-6 max-md:px-4 md:border-x md:border-border">
          <BrandMark size={20} title="Droppen" />
          <span className="font-mono text-sm font-medium text-fg">Droppen</span>
          <span className="flex-1" />
          {aside}
          <RegMark className="-bottom-[5px] -left-[5px] md:hidden xl:block" />
          <RegMark className="-right-[5px] -bottom-[5px] md:hidden xl:block" />
        </div>
      </header>
      <div className="flex flex-1 flex-col xl:px-8">
        <div className="mx-auto w-full max-w-[2400px] flex-1 md:border-x md:border-border">{children}</div>
      </div>
    </div>
  );
}

/** A page with one narrow column of content: the set-up, recovery and pairing steps. */
export function Centered({ children, aside }: { children: ReactNode; aside?: ReactNode }) {
  return (
    <CloudPage aside={aside}>
      <div className="flex justify-center px-6 py-14 max-md:px-4 max-md:py-8">{children}</div>
    </CloudPage>
  );
}

/**
 * One step's column: a caps label, a display headline, then the content. The
 * headline is the screen's one large type moment (docs/ui-spec.md §1).
 */
export function Panel({ title, label, children, wide }: { title: string; label?: string; children: ReactNode; wide?: boolean }) {
  return (
    <div className={cn("flex w-full flex-col gap-3", wide ? "max-w-xl" : "max-w-[26rem]")}>
      <CapsLabel>{label ?? "Droppen cloud"}</CapsLabel>
      <h1 className="mb-2 text-[28px] leading-[1.1] font-semibold tracking-[-0.03em] text-fg">{title}</h1>
      {children}
    </div>
  );
}

/** A boxed note inside a step: success, a danger to confirm, or a warning. */
export function Note({ tone, children, className }: { tone?: "ok" | "danger" | "warn"; children: ReactNode; className?: string }) {
  return (
    <div
      data-signal={tone}
      className={cn(
        "flex flex-col gap-2 border border-border p-3",
        tone === "ok" && "shadow-[inset_2px_0_0_var(--color-ok)]",
        tone === "danger" && "bg-danger/4 shadow-[inset_2px_0_0_var(--color-danger)]",
        tone === "warn" && "bg-warn/7 shadow-[inset_2px_0_0_var(--color-warn)]",
        className,
      )}
    >
      {children}
    </div>
  );
}

export function ErrorLine({ message }: { message: string | null | undefined }) {
  if (!message) return null;
  return (
    <div role="alert" data-signal="failed" className="font-mono text-2xs break-words text-danger">
      {message}
    </div>
  );
}

/** A button that copies `text`; `primary` when it is the view's one primary action. */
export function CopyButton({ text, label, primary: main = false, testId }: { text: string; label: string; primary?: boolean; testId?: string }) {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  return (
    <button
      type="button"
      data-variant={main ? "primary" : undefined}
      data-testid={testId}
      className={main ? primary : secondary}
      onClick={() => {
        void navigator.clipboard?.writeText(text).then(() => {
          setCopied(true);
          setTimeout(() => setCopied(false), 1500);
        });
      }}
    >
      {copied ? <Check className="size-3.5" /> : <Copy className="size-3.5" />}
      {copied ? t("copied") : label}
    </button>
  );
}

/** A device's keys as both screens show them, to compare by eye. */
export function Fingerprint({ value }: { value: string }) {
  return <span className="font-mono text-xs tracking-wider text-fg select-all">{value}</span>;
}

/** Tabs as the auth screen draws them: joined boxes, the chosen one inked in. */
export function Tabs<T extends string>({ value, options, onChange }: { value: T; options: { id: T; label: string }[]; onChange: (id: T) => void }) {
  return (
    <div role="tablist" className="flex h-9 w-full border border-border-strong">
      {options.map((o) => (
        <button
          key={o.id}
          type="button"
          role="tab"
          aria-selected={value === o.id}
          onClick={() => onChange(o.id)}
          className={cn(
            "flex flex-1 items-center justify-center border-r border-border px-3 font-mono text-2xs font-medium last:border-r-0",
            value === o.id ? "border-fg bg-fg text-bg" : "text-muted hover:text-fg",
          )}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

/** A labelled field: caps label over the input. */
export function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <label className="flex flex-col gap-1.5">
      <CapsLabel>{label}</CapsLabel>
      {children}
    </label>
  );
}
