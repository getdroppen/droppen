import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { useTranslation } from "react-i18next";
import { Loader2 } from "lucide-react";
import { ASSURANCE_AUTO, ASSURANCE_ON_DEVICE, type Assurance } from "@/lib/e2e";
import { formatAsOf } from "@/lib/format";
import { type CloudAccount, type PauseEnding, RenewalError } from "@/lib/hub/account";
import { type QueuedWhy, renewalRunner, type RenewalRunner, type RenewalStatus } from "@/lib/hub/renewal";
import { cn } from "@/lib/cn";
import { enablePush, pushRenewalNeeded } from "@/lib/push";
import { danger, ErrorLine, errorText, field, primary, quiet, renewalErrorText, secondary } from "./ui";

/*
 * Renewing the identity key of a device that runs AI, as 「我的设备」 shows
 * it (docs/host-identity-rotation.md §4.4): each host row's state, the
 * person's own "renew identity key" (with the choice to confirm it on the
 * device), the dialog where the code that device shows is TYPED (never shown
 * here), the account's 「更换身份密钥时总在设备上确认」, and the notice above
 * the list. The renewing itself is `lib/hub/renewal.ts`'s runner.
 */


/** The page's renewal runner, re-rendering as it changes (`null`: none — local mode, or before sign-in). */
export function useRenewals(runner: RenewalRunner | null = renewalRunner()): RenewalRunner | null {
  useSyncExternalStore(
    (notify) => runner?.onChange(notify) ?? (() => undefined),
    () => runner?.version ?? 0,
    () => 0,
  );
  return runner;
}

/** Why a host the row waits for is not renewing now (`RenewalStatus` `queued`), in words. */
function queuedWhyText(why: QueuedWhy, id: string, t: (key: string, opts?: Record<string, unknown>) => string): string {
  switch (why) {
    case "needsCode":
      return t("{{id}} needs confirming on the device itself first", { id });
    case "isolated":
      return t("{{id}} is isolated: settle it on its row first", { id });
    case "blocked":
      return t("{{id}} cannot renew until who can control it is narrowed", { id });
    case "offline":
      return t("{{id}} is offline: this one renews after it is back and has renewed", { id });
    case "stuck":
      return t("{{id}} could not renew its identity key: try again on its row", { id });
    case "tooOld":
      return t("{{id}}'s Droppen is too old to renew its identity key", { id });
    case "waiting":
      return t("{{id}} waits for Droppen to be updated on other devices", { id });
  }
}

/** Why the last try failed, in the reader's language when the failure itself is at hand (else its message, as it was). */
function failureText(status: { error: string; cause?: unknown }, t: (key: string, opts?: Record<string, unknown>) => string): string {
  if (status.cause instanceof RenewalError) return renewalErrorText(status.cause, t);
  return status.cause === undefined || status.cause === null ? status.error : errorText(status.cause);
}

/** A host row's renewal (§4.4): where it stands, and what the person may do about it. */
export function RenewalLine({
  id,
  status,
  viewerAdmin,
  onCode,
  onRetry,
  onRemove,
  onAddComputer,
  onNarrow,
}: {
  id: string;
  status: RenewalStatus;
  viewerAdmin: boolean;
  /** "Enter the code shown on the device" / "confirm on the device": the renewal confirmed there. */
  onCode?: () => void;
  /** "Try again", after the automatic tries ran out. */
  onRetry?: () => void;
  onRemove?: () => void;
  /** Where a new registration code is made (「添加可运行 AI 的设备」). */
  onAddComputer?: () => void;
  /** Its 「谁可以控制它」, to narrow it (a host an exposed device that cannot renew controls). */
  onNarrow?: () => void;
}) {
  const { t } = useTranslation();
  const link = "text-muted underline underline-offset-2 hover:text-fg";
  switch (status.kind) {
    case "none":
      return null;
    case "renewing":
      return (
        <span data-testid={`renewal-${id}`} data-renewal="renewing" className="flex items-center gap-1.5 text-2xs leading-4 text-muted">
          <Loader2 className="size-3 animate-spin" />
          {t("renewing its identity key… (about 2 minutes)")}
        </span>
      );
    case "offline":
      return (
        <span data-testid={`renewal-${id}`} data-renewal="offline" className="text-2xs leading-4 text-muted">
          {t("renews its identity key once it is back online")}
        </span>
      );
    case "queued":
      // §4.1: after every exposed host it lets control it — and why one of those is not renewing now, when it is stuck.
      return (
        <span data-testid={`renewal-${id}`} data-renewal="queued" className="flex flex-col items-start gap-0.5 text-2xs leading-4 text-muted">
          <span>{t("renews its identity key after {{ids}}", { ids: status.after.join(", ") })}</span>
          {status.stuck.map((s) => (
            <span key={s.host} data-testid={`renewal-after-${id}-${s.host}`} data-why={s.why} data-signal="warn" className="text-warn-fg">
              {queuedWhyText(s.why, s.host, t)}
            </span>
          ))}
        </span>
      );
    case "waiting":
      return (
        <span data-testid={`renewal-${id}`} data-renewal="waiting" data-signal="warn" className="text-2xs leading-4 text-warn-fg">
          {t("renews its identity key once Droppen is updated on {{ids}}", { ids: status.hosts.join(", ") })}
        </span>
      );
    case "retrying":
      return (
        <span data-testid={`renewal-${id}`} data-renewal="retrying" data-signal="warn" className="text-2xs leading-4 text-warn-fg">
          {t("could not renew its identity key: {{why}} · trying again at {{time}}", { why: failureText(status, t), time: formatAsOf(status.atMs) })}
        </span>
      );
    case "stuck":
      return (
        <span data-testid={`renewal-${id}`} data-renewal="stuck" data-signal="warn" className="flex flex-wrap items-baseline gap-x-2 text-2xs leading-4 text-warn-fg">
          <span>{t("could not renew its identity key: {{why}}", { why: failureText(status, t) })}</span>
          {viewerAdmin && onRetry && (
            <button type="button" data-testid={`renewal-retry-${id}`} className={link} onClick={onRetry}>
              {t("try again")}
            </button>
          )}
        </span>
      );
    case "blocked":
      // §4.1 case 3: an exposed device that cannot renew still controls it — no renewal of this one helps.
      return (
        <span data-testid={`renewal-${id}`} data-renewal="blocked" data-signal="warn" className="flex flex-col items-start gap-0.5 text-2xs leading-4 text-warn-fg">
          <span>{t("It lets {{ids}} control it, which may have had its identity key copied and cannot renew it: renewing this one would not help.", { ids: status.by.join(", ") })}</span>
          <span className="flex flex-wrap items-baseline gap-x-2 text-muted">
            <span>{t("Narrow who can control it, or add {{ids}} again.", { ids: status.by.join(", ") })}</span>
            {viewerAdmin && onNarrow && (
              <button type="button" data-testid={`renewal-narrow-${id}`} className={link} onClick={onNarrow}>
                {t("who can control it")}
              </button>
            )}
          </span>
        </span>
      );
    case "tooOld":
      return (
        <span data-testid={`renewal-${id}`} data-renewal="tooOld" data-signal="warn" className="flex flex-col items-start gap-0.5 text-2xs leading-4 text-warn-fg">
          <span>{t("Droppen is too old: it renews its identity key once updated")}</span>
          {viewerAdmin && <span className="text-muted">{t("or, in its ⋯ menu: remove it, then add it again")}</span>}
        </span>
      );
    case "needsCode":
      return (
        <span data-testid={`renewal-${id}`} data-renewal="needsCode" data-signal="warn" className="flex flex-col items-start gap-0.5 text-2xs leading-4 text-warn-fg">
          <span>{t("needs confirming on the device itself")}</span>
          <span className="text-muted">
            {status.why === "policy"
              ? t("Your account renews identity keys only with the code the device shows.")
              : status.why === "device"
                ? t("This device signs only renewals confirmed on the device.")
                : t("Another copy of its identity key signed in: only the code its own screen shows tells them apart.")}
          </span>
          {viewerAdmin && onCode && (
            <button type="button" data-testid={`renewal-code-${id}`} className={link} onClick={onCode}>
              {t("enter the code shown on the device")}
            </button>
          )}
        </span>
      );
    case "policyUnset":
      return (
        <span data-testid={`renewal-${id}`} data-renewal="policyUnset" data-signal="warn" className="text-2xs leading-4 text-warn-fg">
          {t("renews its identity key once the account chose how renewals are confirmed")}
        </span>
      );
    case "isolated":
      return (
        <span data-testid={`renewal-${id}`} data-renewal="isolated" data-signal="failed" className="flex flex-col items-start gap-1 text-2xs leading-4 text-danger">
          <span>
            {t("An identity key {{id}} renewed away from signed in again — for example, it was restored from a backup made before the renewal, or another copy of its old key is in use. Nothing here connects to it until this is settled.", { id })}
          </span>
          {viewerAdmin && (
            // §3.8 「页面」: first look at Droppen on that device; what it shows
            // picks the main action — "replaced elsewhere": remove the id, then add it again under a new identity; a code:
            // confirm on the device. Removing stays one click away in both, and the row says all along why it matters.
            <span data-testid={`renewal-fresh-${id}`} className="flex flex-col items-start gap-1.5 text-muted">
              <span className="text-fg">
                {t("Until {{id}} is removed or confirmed on the device, the devices that let it control them still accept whoever holds its current key.", { id })}
              </span>
              <ol className="flex list-decimal flex-col gap-1.5 pl-4">
                <li data-testid={`renewal-look-${id}`} className="text-fg">
                  {t("Look at Droppen on {{id}} (Settings → Access → Droppen).", { id })}
                </li>
                <li className="flex flex-col items-start gap-1.5">
                  <span data-testid={`renewal-replaced-${id}`} className="flex flex-col items-start gap-1">
                    <span className="text-fg">{t("It says “this device's identity key was replaced elsewhere”:")}</span>
                    {onRemove && (
                      <button type="button" data-testid={`renewal-remove-${id}`} className={cn(danger, "h-7 px-3 text-xs")} onClick={onRemove}>
                        {t("remove {{id}}", { id })}
                      </button>
                    )}
                    <span>
                      {t("then, on that device, choose “add this device again with a new identity” and sign it in with a new registration code.")}
                      {onAddComputer && (
                        <>
                          {" "}
                          <button type="button" data-testid={`renewal-add-${id}`} className={link} onClick={onAddComputer}>
                            {t("make a registration code")}
                          </button>
                        </>
                      )}
                    </span>
                  </span>
                  <span data-testid={`renewal-shows-code-${id}`} className="flex flex-col items-start gap-1">
                    <span className="text-fg">{t("It does not say “replaced elsewhere”: confirm on the device, with the code it shows.")}</span>
                    <span className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
                      {onCode && (
                        <button type="button" data-testid={`renewal-code-${id}`} className={cn(primary, "h-7 px-3 text-xs")} onClick={onCode}>
                          {t("confirm on the device")}
                        </button>
                      )}
                      {onRemove && (
                        <button type="button" data-testid={`renewal-remove-also-${id}`} className={link} onClick={onRemove}>
                          {t("or remove {{id}}", { id })}
                        </button>
                      )}
                    </span>
                  </span>
                </li>
              </ol>
            </span>
          )}
        </span>
      );
    case "renewed":
      return (
        <span data-testid={`renewal-${id}`} data-renewal="renewed" className="flex flex-col items-start gap-0.5 text-2xs leading-4 text-muted">
          <span className="font-mono">{t("renewed identity key · {{time}}", { time: formatAsOf(status.atMs) })}</span>
          <span>{t("A device that could control {{id}} may have read its Claude login and SSH keys: change them where they were issued.", { id })}</span>
        </span>
      );
  }
}

/**
 * The person's own renewal of `host` (the row's "renew identity key", or the
 * code a host that needs it asks for): how it is confirmed — automatically, or
 * on the device (`fixed`: only that way now) — then, confirmed on the device,
 * the code typed in ({@link EnterCode}); where it is; and, done, the reminder
 * that renewing the key does not change what else that device held (§7
 * decision 9).
 */
export function RenewPanel({
  host,
  runner,
  fixed,
  onDone,
}: {
  host: string;
  runner: Pick<RenewalRunner, "renew">;
  /** Only a renewal confirmed on the device goes (the account's setting, clone evidence, an isolated host). */
  fixed?: boolean;
  onDone: () => void;
}) {
  const { t } = useTranslation();
  const [assurance, setAssurance] = useState<Assurance>(fixed ? ASSURANCE_ON_DEVICE : ASSURANCE_AUTO);
  const [busy, setBusy] = useState(false);
  const [step, setStep] = useState<string | null>(null);
  const [asking, setAsking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState(false);
  // The open code prompt's answer. The panel closing (another row's menu opened, say) gives it up: a renewal never waits
  // on a prompt nobody can see — the runner does one at a time, and checks the hub's disputes in the same queue.
  const answer = useRef<((code: string | null) => void) | null>(null);
  const reply = (code: string | null) => {
    const give = answer.current;
    answer.current = null;
    setAsking(false);
    give?.(code);
  };
  useEffect(
    () => () => {
      const give = answer.current;
      answer.current = null;
      give?.(null);
    },
    [],
  );
  const go = async () => {
    setBusy(true);
    setError(null);
    try {
      await runner.renew(host, {
        assurance,
        // The code is asked for once the device answered (it shows the code then), and is never shown here.
        askCode: () =>
          new Promise<string | null>((resolve) => {
            answer.current?.(null);
            answer.current = resolve;
            setAsking(true);
          }),
        onStep: (s) => setStep(s),
      });
      setDone(true);
    } catch (err) {
      setError(errorText(err));
    } finally {
      answer.current = null;
      setAsking(false);
      setStep(null);
      setBusy(false);
    }
  };
  if (done) {
    return (
      <div data-testid={`renew-done-${host}`} data-signal="ok" className="flex flex-col gap-1 border border-border p-3 shadow-[inset_2px_0_0_var(--color-ok)]">
        <span className="font-mono text-xs text-ok">{t("{{id}} has a new identity key", { id: host })}</span>
        <span className="text-xs leading-5 text-muted">
          {t("A device that could control {{id}} may have read its Claude login and SSH keys: change them where they were issued.", { id: host })}
        </span>
        <button type="button" className={cn(quiet, "self-start")} onClick={onDone}>
          {t("done")}
        </button>
      </div>
    );
  }
  return (
    <div role="dialog" data-testid={`renew-${host}`} className="flex flex-col gap-2 border border-border p-3">
      <span className="font-mono text-xs text-fg">{t("renew the identity key of {{id}}?", { id: host })}</span>
      <p className="text-xs leading-5 text-muted">
        {t("It gets a new identity key and keeps its name, its settings and its copies. A device that copied the old key can no longer sign in as it, nor read anything new.")}
      </p>
      {!fixed && (
        <div className="flex flex-col gap-1">
          {(
            [
              [ASSURANCE_AUTO, t("automatically (about 2 minutes)")],
              [ASSURANCE_ON_DEVICE, t("confirm on the device: type the code it shows")],
            ] as const
          ).map(([value, label]) => (
            <label key={value} className="flex items-center gap-2 text-xs text-fg">
              <input type="radio" name={`renew-${host}`} checked={assurance === value} disabled={busy} onChange={() => setAssurance(value)} />
              {label}
            </label>
          ))}
        </div>
      )}
      {asking ? (
        <EnterCode host={host} onCode={(code) => reply(code)} onCancel={() => reply(null)} />
      ) : (
        <>
          {busy && step && (
            <span data-testid={`renew-step-${host}`} className="flex items-center gap-1.5 text-2xs text-muted">
              <Loader2 className="size-3 animate-spin" />
              {step === "waiting" ? t("waiting for every sign-in with the old key to confirm the new one (about 2 minutes)…") : t("renewing its identity key…")}
            </span>
          )}
          <ErrorLine message={error} />
          <div className="flex gap-2">
            <button type="button" data-variant="primary" data-testid={`renew-start-${host}`} className={primary} disabled={busy} onClick={() => void go()}>
              {busy && <Loader2 className="size-3.5 animate-spin" />}
              {t("renew identity key")}
            </button>
            <button type="button" className={quiet} disabled={busy} onClick={onDone}>
              {t("cancel")}
            </button>
          </div>
        </>
      )}
    </div>
  );
}

/**
 * The high-assurance dialog (§3.7): the person opens Droppen on that device
 * and types the code it shows. Typed, not compared — a "they match" is
 * clicked through too easily; the page never knows what to show.
 */
export function EnterCode({ host, onCode, onCancel }: { host: string; onCode: (code: string) => void; onCancel: () => void }) {
  const { t } = useTranslation();
  const [code, setCode] = useState("");
  const ready = code.toUpperCase().replace(/[^A-Z2-7]/g, "").length === 8;
  return (
    <form
      role="alertdialog"
      data-testid={`renew-code-${host}`}
      data-signal="warn"
      className="flex flex-col gap-2 border border-border bg-warn/7 p-3 shadow-[inset_2px_0_0_var(--color-warn)]"
      onSubmit={(e) => {
        e.preventDefault();
        if (ready) onCode(code);
      }}
    >
      <span className="font-mono text-xs text-fg">{t("type the code {{id}} shows", { id: host })}</span>
      <span className="text-xs leading-5 text-muted">
        {t("Open Droppen on {{id}} itself — Settings → Access → Droppen, or droppen agent status on a server — and type the 8 characters it shows. If it shows none, or another code, cancel: nothing is changed then.", { id: host })}
      </span>
      <input
        className={cn(field, "max-w-[12rem] tracking-[0.2em] uppercase")}
        aria-label={t("the code shown on the device")}
        autoComplete="off"
        autoCapitalize="characters"
        spellCheck={false}
        maxLength={12}
        placeholder="XXXX-XXXX"
        value={code}
        onChange={(e) => setCode(e.target.value)}
        data-testid={`renew-code-input-${host}`}
      />
      <div className="flex gap-2">
        <button type="submit" className={secondary} disabled={!ready} data-testid={`renew-code-confirm-${host}`}>
          {t("confirm")}
        </button>
        <button type="button" className={quiet} onClick={onCancel}>
          {t("cancel")}
        </button>
      </div>
    </form>
  );
}

/**
 * The account's 「更换身份密钥时总在设备上确认」 (§7 decision 1, §4.4): off by
 * default, written on the chain (`setRenewalPolicy`) and read back from it, so
 * every device of the account — and the chain's own verifiers — hold to it.
 * Beside it, this device's own guard: while it is set, this device signs no
 * renewal that is not confirmed on the device, whatever the chain or the hub
 * says (kept in this browser).
 */
export function AlwaysConfirmSetting({
  runner,
  account,
}: {
  runner: Pick<RenewalRunner, "setAlwaysAssured">;
  account: Pick<CloudAccount, "alwaysAssured" | "localAssured" | "setLocalAssured"> & { renewalPolicyChosen?: boolean };
}) {
  const { t } = useTranslation();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [local, setLocal] = useState(() => account.localAssured());
  // Renewing on their own is asked about first: every device of the account is asked "is this you?" after.
  const [askingOff, setAskingOff] = useState(false);
  const badge = "px-1 text-[10px] tracking-[0.08em] text-fg uppercase shadow-[inset_0_0_0_1px_var(--color-border-strong)]";
  const write = async (on: boolean) => {
    setBusy(true);
    setError(null);
    try {
      await runner.setAlwaysAssured(on);
      setAskingOff(false);
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };
  const chosen = account.renewalPolicyChosen !== false;
  return (
    <div className="flex flex-col gap-3 border-b border-border px-6 py-3.5 max-md:px-4">
      {!chosen && (
        // §5.6 「打开的顺序」: nothing renews on its own until an admin chose, once, either way.
        <div data-testid="renewal-policy-choose" data-signal="warn" className="flex flex-col gap-2 border border-border p-3 shadow-[inset_2px_0_0_var(--color-warn)]">
          <span className="font-mono text-xs text-fg">{t("Choose how identity keys are renewed")}</span>
          <span className="text-xs leading-5 text-muted">{t("Until you choose, no device that runs AI renews its identity key on its own.")}</span>
          <span className="flex flex-wrap gap-2">
            <button type="button" data-testid="renewal-policy-assured" className={cn(secondary, "h-7 px-3 text-xs")} disabled={busy} onClick={() => void write(true)}>
              {t("always confirm on the device")}
            </button>
            <button type="button" data-testid="renewal-policy-auto" className={cn(quiet, "h-7 px-3 text-xs")} disabled={busy} onClick={() => setAskingOff(true)}>
              {t("renew on their own")}
            </button>
          </span>
        </div>
      )}
      {askingOff && (
        <div role="alertdialog" data-testid="renewal-policy-off-confirm" data-signal="warn" className="flex flex-col gap-2 border border-border p-3 shadow-[inset_2px_0_0_var(--color-warn)]">
          <span className="font-mono text-xs text-fg">{t("Let devices that run AI renew their identity key on their own?")}</span>
          <span className="text-xs leading-5 text-muted">
            {t("Each renewal then goes without the code its screen shows. Every device of the account is asked whether this was you.")}
          </span>
          <span className="flex flex-wrap gap-2">
            <button type="button" data-testid="renewal-policy-off-yes" className={cn(primary, "h-7 px-3 text-xs")} disabled={busy} onClick={() => void write(false)}>
              {t("renew on their own")}
            </button>
            <button type="button" data-testid="renewal-policy-off-no" className={cn(quiet, "h-7 px-3 text-xs")} disabled={busy} onClick={() => setAskingOff(false)}>
              {t("cancel")}
            </button>
          </span>
        </div>
      )}
      <label data-testid="always-confirm" className="flex cursor-pointer items-start gap-3">
        <input
          type="checkbox"
          checked={chosen && account.alwaysAssured}
          disabled={busy || !chosen}
          className="mt-0.5"
          onChange={(e) => {
            if (e.target.checked) void write(true);
            else setAskingOff(true);
          }}
        />
        <span className="flex min-w-0 flex-col gap-0.5">
          <span className="flex items-center gap-2 font-mono text-xs text-fg">
            {t("Always confirm on the device when renewing an identity key")}
            <span className={badge}>{t("account")}</span>
          </span>
          <span className="text-xs leading-5 text-muted">
            {t("Off: a device that runs AI renews its identity key on its own, in about 2 minutes, after a device that could control it is removed. On: each renewal waits for you to type the code that device shows.")}
          </span>
        </span>
      </label>
      <label data-testid="local-assured" className="flex cursor-pointer items-start gap-3">
        <input
          type="checkbox"
          checked={local}
          className="mt-0.5"
          onChange={(e) => {
            account.setLocalAssured(e.target.checked);
            setLocal(e.target.checked);
          }}
        />
        <span className="flex min-w-0 flex-col gap-0.5">
          <span className="flex items-center gap-2 font-mono text-xs text-fg">
            {t("On this device, only sign renewals confirmed on the device")}
            <span className={badge}>{t("this device")}</span>
          </span>
          <span className="text-xs leading-5 text-muted">
            {t("Whatever the account says, this device then signs no automatic renewal: each one it does waits for the code.")}
          </span>
        </span>
      </label>
      <ErrorLine message={error} />
    </div>
  );
}

/** Why an edge a renewal pause holds stays held (§4.1), in words. */
export function pauseHeldText(h: PauseEnding["held"][number], t: (key: string, opts?: Record<string, unknown>) => string): string {
  switch (h.why) {
    case "cannotRenew":
      return t("{{id}} stays paused from controlling {{host}}: {{who}} cannot renew its identity key (its Droppen is too old): remove it, or update it", h);
    case "exposed":
      return t("{{id}} stays paused from controlling {{host}}: {{who}} has not renewed its identity key since it may have been copied", h);
    case "isolated":
      return t("{{id}} stays paused from controlling {{host}}: {{who}} is isolated", h);
    case "flagged":
      return t("{{id}} stays paused from controlling {{host}}: Droppen's hub holds {{who}} isolated", h);
  }
}

/**
 * 「结束暂停」, asked first (§4.1 「卡住的暂停」, R6 review): exactly which control comes
 * back — each device and the host it may control again — and, where a host's
 * list lets every device in, that it lets in every device again, those added
 * while it was paused among them. Refused while one of them must stay held:
 * it says which, and why.
 */
export function EndPauseConfirm({
  ending,
  busy,
  error,
  onConfirm,
  onCancel,
}: {
  ending: PauseEnding;
  busy: boolean;
  error?: string | null;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const { t } = useTranslation();
  const blocked = ending.held.length > 0;
  return (
    <div role="alertdialog" data-testid="end-pause-confirm" data-signal="warn" className="flex flex-col gap-2 border border-border p-3 shadow-[inset_2px_0_0_var(--color-warn)]">
      <span className="font-mono text-xs text-fg">{t("End this pause of control?")}</span>
      {ending.edges.length ? (
        <ul data-testid="end-pause-edges" className="flex list-disc flex-col gap-0.5 pl-4 text-xs leading-5 text-fg">
          {ending.edges.map((e) => (
            <li key={`${e.host}<${e.id}`}>{t("{{id}} can control {{host}} again", e)}</li>
          ))}
        </ul>
      ) : (
        <span className="text-xs leading-5 text-muted">{t("No control comes back: the lists changed since.")}</span>
      )}
      {ending.all.map((host) => (
        <span key={host} data-testid={`end-pause-all-${host}`} className="text-xs leading-5 text-muted">
          {t("{{host}} lets every device of the account control it: ending the pause gives back only the control listed above.", { host })}
        </span>
      ))}
      {blocked && (
        <span data-testid="end-pause-held" className="flex flex-col gap-0.5 text-xs leading-5 text-warn-fg">
          <span>{t("Not now: it would let a device control another while it must stay held.")}</span>
          {ending.held.map((h) => (
            <span key={`${h.host}<${h.id}`}>{pauseHeldText(h, t)}</span>
          ))}
        </span>
      )}
      <span className="flex flex-wrap gap-2">
        <button type="button" data-testid="end-pause-yes" className={cn(danger, "h-7 px-3 text-xs")} disabled={busy || blocked} onClick={onConfirm}>
          {busy && <Loader2 className="size-3.5 animate-spin" />}
          {t("end pause")}
        </button>
        <button type="button" data-testid="end-pause-no" className={cn(quiet, "h-7 px-3 text-xs")} disabled={busy} onClick={onCancel}>
          {t("cancel")}
        </button>
      </span>
      <ErrorLine message={error ?? null} />
    </div>
  );
}

/** The account notice (§4.4): devices renewing their identity key now, and those that need the person. */
export function RenewalNotice({ runner }: { runner: RenewalRunner }) {
  const { t } = useTranslation();
  const { renewing, needYou } = runner.notice();
  if (!renewing.length && !needYou.length) return null;
  return (
    <>
      {needYou.length > 0 && (
        <div data-testid="renewal-needs-you" data-signal="warn" className="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-border bg-warn/7 px-6 py-2.5 shadow-[inset_2px_0_0_var(--color-warn)] max-md:px-4">
          <span className="min-w-0 flex-1 text-xs leading-5 text-fg">{t("{{ids}} need confirming on the device for their identity key", { ids: needYou.join(", ") })}</span>
        </div>
      )}
      {renewing.length > 0 && (
        <div data-testid="renewal-renewing" className="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-border px-6 py-2.5 max-md:px-4">
          <span className="flex min-w-0 flex-1 items-center gap-1.5 text-xs leading-5 text-muted">
            <Loader2 className="size-3 animate-spin" />
            {t("renewing the identity key of {{ids}}", { ids: renewing.join(", ") })}
          </span>
        </div>
      )}
    </>
  );
}

/**
 * The device this browser's notifications came from renewed its identity key,
 * and the browser would not subscribe again without a tap (or notifications
 * are not allowed now): one tap turns them back on (§2.2 推送).
 */
export function PushRenewNotice({ needed = pushRenewalNeeded, enable = enablePush }: { needed?: () => string | null; enable?: (machine: string) => Promise<void> }) {
  const { t } = useTranslation();
  const [machine, setMachine] = useState(needed);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  if (!machine) return null;
  return (
    <div data-testid="push-renew" data-signal="warn" className="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-border bg-warn/7 px-6 py-2.5 shadow-[inset_2px_0_0_var(--color-warn)] max-md:px-4">
      <span className="min-w-0 flex-1 text-xs leading-5 text-fg">{t("Notifications from {{id}} stopped when it renewed its identity key.", { id: machine })}</span>
      <button
        type="button"
        data-testid="push-renew-on"
        className={cn(quiet, "h-7 px-2")}
        disabled={busy}
        onClick={async () => {
          setBusy(true);
          setError(null);
          try {
            await enable(machine);
            setMachine(null);
          } catch (err) {
            setError(errorText(err));
          } finally {
            setBusy(false);
          }
        }}
      >
        {t("turn notifications back on")}
      </button>
      <ErrorLine message={error} />
    </div>
  );
}

/** A confirmation's line on what a change starts (§4.4): the devices that run AI that will renew their identity key. */
export function RenewalsAhead({ ids }: { ids: string[] }) {
  const { t } = useTranslation();
  if (!ids.length) return null;
  return (
    <p data-testid="renewals-ahead" className="text-xs leading-5 text-fg">
      {ids.length === 1
        ? t("{{ids}} will renew its identity key automatically (in about 2 minutes, once it is online).", { ids: ids[0] })
        : t("{{n}} devices that run AI will renew their identity key automatically (those online in about 2 minutes): {{ids}}.", { n: ids.length, ids: ids.join(", ") })}
    </p>
  );
}

