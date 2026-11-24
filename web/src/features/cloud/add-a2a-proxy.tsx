import { type FormEvent, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { ArrowSquareOutIcon, ArrowsClockwiseIcon, CheckIcon, CircleNotchIcon } from "@phosphor-icons/react";
import { CapsLabel } from "@/components/ui/caps-label";
import { CheckboxField } from "@/components/ui/checkbox-field";
import { cn } from "@/lib/cn";
import { type A2aEnrollCode, formatA2aEnrollCode } from "@/lib/e2e";
import { type A2aClaim, AccountError, type A2aUpgradeGate, a2aUpgradeGate, type CloudAccount, deviceFingerprint, fingerprintDigits, sameFingerprint } from "@/lib/hub/account";
import { formatCountdown } from "@/lib/format";
import type { HubLink } from "@/lib/hub/link";
import { freshLimit } from "@/lib/hub/me";
import { DEVICES_KEY, type LimitHit, limitOf } from "@/lib/hub/plan";
import { useTicker } from "@/lib/ticker";
import { codeDeadline, codeTimeLeft } from "./add-computer";
import { LimitNote, useMe } from "./plan";
import { RenewalsAhead } from "./renewal";
import { CopyButton, ErrorLine, errorText, field, Note, primary, quiet, secondary } from "./ui";

/*
 * 「添加 A2A 代理」 (docs/a2a.md §7.2): an A2A proxy (`droppen a2a-proxy`, run
 * beside an A2A client in the cloud) joins the account with a `DA1-` code this
 * page makes. The steps:
 *
 * 1. Every current device that runs AI declared `a2a/1` (an older one cannot
 *    read the entry that adds a proxy and would stop following the chain:
 *    availability, not security), and the plan has room for a device; then
 *    the code, to set as `DROPPEN_ENROLL_CODE` on the proxy's own service.
 * 2. Once a proxy claimed it (the claim checked: `awaitA2aClaim`), the person
 *    pastes the fingerprint from the proxy's log; any other than the
 *    claimer's ends the round — someone else used the code.
 * 3. The devices that take its calls are picked, and one batch adds it
 *    (`addA2aProxy`): no key epoch, no key.
 *
 * The A2A proxy's own row then carries its badge and the way to change who
 * takes its calls (`A2aAcceptPanel`); its removal is the row's usual one.
 */

/** How to run and join the proxy (`docs/a2a-proxy.md`). */
export const A2A_PROXY_DOCS_URL = "https://github.com/getdroppen/droppen/blob/main/docs/a2a-proxy.md";

/** The environment variable the proxy reads its code from, on its first start. */
export const ENROLL_CODE_VARIABLE = "DROPPEN_ENROLL_CODE";

/** Why no code is made: the account is not on chain v3 yet, or the upgrade gate ({@link a2aUpgradeGate}). */
export type A2aBlock = { kind: "notUpgraded" } | Exclude<A2aUpgradeGate, null>;

/** A device that runs AI, as the steps name it. */
export interface HostChoice {
  id: string;
  name: string | null;
}

export type A2aAddState =
  | { kind: "starting" }
  | { kind: "blocked"; block: A2aBlock }
  | { kind: "limit"; hit: LimitHit }
  /** `deadline`: when the code runs out, by this page's clock (`codeDeadline`). */
  | { kind: "waiting"; code: A2aEnrollCode; deadline: number }
  /** A proxy claimed the code and its claim checks out: its fingerprint is asked for. */
  | { kind: "claimed"; claim: A2aClaim }
  /** The fingerprint matched: the devices that take its calls are picked. */
  | { kind: "choose"; claim: A2aClaim; pasted: string }
  | { kind: "done"; device: string; hosts: string[] }
  | { kind: "expired" }
  /** The round is over and nothing was added: the fingerprint did not match, the code was claimed twice, or its proof failed. */
  | { kind: "aborted"; why: "fingerprint" | "claims" | "proof"; message: string }
  | { kind: "error"; message: string };

const pad = "px-6 py-5 max-md:px-4";

/**
 * Adding an A2A proxy (see the module docs). Each round is one code; a new
 * one starts everything again.
 */
export function AddA2aProxy(props: { account: CloudAccount; link: HubLink; hosts: HostChoice[]; onDone: () => void }) {
  const [round, setRound] = useState(0);
  return <AddA2aRound key={round} {...props} onNewCode={() => setRound((n) => n + 1)} />;
}

function AddA2aRound({ account, link, hosts, onDone, onNewCode }: { account: CloudAccount; link: HubLink; hosts: HostChoice[]; onDone: () => void; onNewCode: () => void }) {
  const [state, setState] = useState<A2aAddState>({ kind: "starting" });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const started = useRef(false);
  const issued = useRef<A2aEnrollCode | null>(null);
  const me = useMe(account, link);
  useEffect(() => {
    if (started.current) return;
    started.current = true;
    const abort = new AbortController();
    void (async () => {
      try {
        if (account.chain?.isV3 !== true) return setState({ kind: "blocked", block: { kind: "notUpgraded" } });
        // The plan's devices first, with `/v1/me` read fresh — and with it, what each device that runs AI declared.
        const hit = await freshLimit(me, DEVICES_KEY, (error) => console.warn("cloud: /v1/me before adding an A2A proxy:", error));
        if (hit) return setState({ kind: "limit", hit });
        const gate = account.chain ? a2aUpgradeGate(account.chain, me?.view?.machines) : null;
        if (gate) return setState({ kind: "blocked", block: gate });
        const { code, expiresMs } = await account.startA2aEnrollment();
        issued.current = code;
        setState({ kind: "waiting", code, deadline: codeDeadline(Date.now(), expiresMs) });
        const claim = await account.awaitA2aClaim(code, abort.signal);
        setState({ kind: "claimed", claim });
      } catch (err) {
        if (abort.signal.aborted) return;
        const hit = limitOf(err);
        if (hit) return setState({ kind: "limit", hit });
        setState(endOf(err));
      }
    })();
    return () => {
      abort.abort();
      // A round given up on: its code adds nothing any more.
      if (issued.current) void account.dropA2aCode(issued.current);
    };
    // `me` is the account's one store: the same for as long as `account` is
  }, [account, link]);

  const check = async (pasted: string) => {
    if (state.kind !== "claimed") return;
    setError(null);
    if (fingerprintDigits(pasted) === null) return setError("format");
    if (!sameFingerprint(pasted, await deviceFingerprint(state.claim.signKey, state.claim.dhKey))) {
      await account.dropA2aCode(state.claim.code);
      return setState({ kind: "aborted", why: "fingerprint", message: "" });
    }
    setState({ kind: "choose", claim: state.claim, pasted });
  };
  const add = async (picked: string[]) => {
    if (state.kind !== "choose") return;
    setBusy(true);
    setError(null);
    try {
      const added = await account.addA2aProxy(state.claim, state.pasted, picked);
      issued.current = null;
      setState({ kind: "done", device: added.device, hosts: added.hosts });
      me?.soon();
    } catch (err) {
      const end = endOf(err);
      if (end.kind === "aborted") setState(end);
      else setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <A2aStepView
      state={state}
      hosts={hosts}
      busy={busy}
      error={error}
      onCheck={(pasted) => void check(pasted)}
      onAdd={(picked) => void add(picked)}
      onNewCode={onNewCode}
      onDone={onDone}
    />
  );
}

/** Where an error leaves the round: a hub that takes no A2A proxy, run out, over with nothing added, or a failure to show. */
function endOf(err: unknown): Extract<A2aAddState, { kind: "blocked" | "expired" | "aborted" | "error" }> {
  if (err instanceof AccountError) {
    if (err.kind === "noA2aProxies") return { kind: "blocked", block: { kind: "hub" } };
    if (err.kind === "expired") return { kind: "expired" };
    if (err.kind === "fingerprintMismatch") return { kind: "aborted", why: "fingerprint", message: "" };
    if (err.kind === "claims") return { kind: "aborted", why: "claims", message: errorText(err) };
    if (err.kind === "proof") return { kind: "aborted", why: "proof", message: errorText(err) };
  }
  return { kind: "error", message: errorText(err) };
}

/**
 * A round as it stands, drawn (the flow's state, apart from what moves it, so
 * each step is plain to draw and test). `now` stands in for the clock (tests).
 */
export function A2aStepView({
  state,
  hosts,
  busy = false,
  error = null,
  now,
  onCheck,
  onAdd,
  onNewCode,
  onDone,
}: {
  state: A2aAddState;
  hosts: HostChoice[];
  busy?: boolean;
  /** The step's own failure: `format` (what was pasted is no fingerprint), or a message. */
  error?: string | null;
  now?: number;
  onCheck: (pasted: string) => void;
  onAdd: (picked: string[]) => void;
  onNewCode: () => void;
  onDone: () => void;
}) {
  const { t } = useTranslation();
  switch (state.kind) {
    case "starting":
      return (
        <div className={pad}>
          <CircleNotchIcon className="size-4 animate-spin text-faint" />
        </div>
      );
    case "error":
      return (
        <div className={pad}>
          <ErrorLine message={state.message} />
        </div>
      );
    case "limit":
      return (
        <div className={pad}>
          <LimitNote hit={state.hit} action={{ label: t("devices"), onClick: onDone }} />
        </div>
      );
    case "blocked":
      return <A2aBlocked block={state.block} onAgain={onNewCode} onCancel={onDone} />;
    case "waiting":
    case "expired":
      return <A2aCodeView state={state} now={now} onNewCode={onNewCode} onCancel={onDone} />;
    case "claimed":
      return <A2aFingerprintStep device={state.claim.device} error={error} onCheck={onCheck} onCancel={onDone} />;
    case "choose":
      return <A2aChooseStep hosts={hosts} busy={busy} error={error} onAdd={onAdd} onCancel={onDone} />;
    case "aborted":
      return (
        <div data-testid="a2a-aborted" data-why={state.why} className={pad}>
          <Note tone="warn" className="max-w-xl">
            <span data-signal="warn" className="font-mono text-xs text-warn-fg">
              {state.why === "fingerprint" ? t("The fingerprint does not match: nothing was added.") : t("Nothing was added.")}
            </span>
            <p className="text-xs leading-5 text-muted">
              {state.why === "fingerprint"
                ? t("The device that claimed this code is not the A2A proxy whose log you read: someone else may have used the code. Make a new code, and set only the new one on the proxy.")
                : state.message}
            </p>
            <div className="flex flex-wrap gap-2">
              <button type="button" data-variant="primary" data-testid="a2a-new-code" className={primary} onClick={onNewCode}>
                <ArrowsClockwiseIcon className="size-3.5" />
                {t("new code")}
              </button>
              <button type="button" className={quiet} onClick={onDone}>
                {t("cancel")}
              </button>
            </div>
          </Note>
        </div>
      );
    case "done":
      return (
        <div data-testid="a2a-added" className={pad}>
          <Note tone="ok" className="max-w-xl">
            <span data-signal="ok" className="flex items-center gap-2 font-mono text-xs text-ok">
              <CheckIcon className="size-3 shrink-0" />
              {t("A2A proxy added: {{id}}", { id: state.device })}
            </span>
            <p data-testid="a2a-remove-code" className="text-xs leading-5 font-medium text-fg">
              {t("Now remove {{variable}} from the proxy's service: it has joined, and the code should not stay where others can read it.", { variable: ENROLL_CODE_VARIABLE })}
            </p>
            <p className="text-xs leading-5 text-muted">
              {state.hosts.length
                ? t("{{hosts}} take its calls. On each, allow it on an A2A entry in that device's A2A settings.", { hosts: state.hosts.join(", ") })
                : t("No device takes its calls yet: choose them on its row.")}
            </p>
            <button type="button" className={cn(quiet, "self-start")} onClick={onDone}>
              {t("done")}
            </button>
          </Note>
        </div>
      );
  }
}

/** Why no code is made yet, and what to do: upgrade the account, wait for the hub, or update the devices named. */
export function A2aBlocked({ block, onAgain, onCancel }: { block: A2aBlock; onAgain: () => void; onCancel: () => void }) {
  const { t } = useTranslation();
  return (
    <div data-testid="a2a-blocked" data-block={block.kind} className={pad}>
      <Note tone="warn" className="max-w-xl">
        <span data-signal="warn" className="font-mono text-xs text-warn-fg">
          {block.kind === "notUpgraded"
            ? t("Upgrade the account first: an A2A proxy needs chain v3.")
            : block.kind === "hub"
              ? t("Not available yet: the hub does not take A2A proxies for this account yet — that is on Droppen's side, not your devices'.")
              : t("Update Droppen on {{hosts}} first.", { hosts: block.hosts.join(", ") })}
        </span>
        {block.kind === "devices" && (
          <p data-testid="a2a-blocked-why" className="text-xs leading-5 text-muted">
            {t(
              "A device that runs AI on an older version cannot read the entry that adds an A2A proxy: it would stop following your account's devices, and could not be reached until it is updated. Once every device that runs AI is updated, the code is made here.",
            )}
          </p>
        )}
        <div className="flex flex-wrap gap-2">
          {block.kind !== "notUpgraded" && (
            <button type="button" data-testid="a2a-check-again" className={secondary} onClick={onAgain}>
              <ArrowsClockwiseIcon className="size-3.5" />
              {t("check again")}
            </button>
          )}
          <button type="button" className={quiet} onClick={onCancel}>
            {t("cancel")}
          </button>
        </div>
      </Note>
    </div>
  );
}

/**
 * The code, while it waits for the proxy: where to set it (the proxy's own
 * service, never a shared variable) and that it goes once the proxy joined,
 * the way to the proxy's docs, the time it has left, and that this page adds
 * the proxy. Once it ran out — the page's count or the hub's word — a new
 * one.
 */
export function A2aCodeView({
  state,
  now: fixedNow,
  onNewCode,
  onCancel,
}: {
  state: Extract<A2aAddState, { kind: "waiting" | "expired" }>;
  now?: number;
  onNewCode: () => void;
  onCancel: () => void;
}) {
  const { t } = useTranslation();
  const ticking = state.kind === "waiting" && fixedNow === undefined;
  const clock = useTicker(1000, ticking);
  const now = fixedNow ?? clock;
  const left = state.kind === "waiting" ? codeTimeLeft(state.deadline, now) : null;
  if (state.kind === "expired" || left === 0) {
    return (
      <div data-testid="a2a-expired" className={pad}>
        <Note tone="warn" className="max-w-xl">
          <span data-signal="warn" className="font-mono text-xs text-warn-fg">
            {t("This code has expired.")}
          </span>
          <p className="text-xs leading-5 text-muted">{t("No A2A proxy claimed it in time. Get a new code, and set the new one on the proxy.")}</p>
          <div className="flex flex-wrap gap-2">
            <button type="button" data-variant="primary" data-testid="a2a-new-code" className={primary} onClick={onNewCode}>
              <ArrowsClockwiseIcon className="size-3.5" />
              {t("new code")}
            </button>
            <button type="button" className={quiet} onClick={onCancel}>
              {t("cancel")}
            </button>
          </div>
        </Note>
      </div>
    );
  }
  const line = `${ENROLL_CODE_VARIABLE}=${formatA2aEnrollCode(state.code)}`;
  return (
    <div data-testid="a2a-code" className="grid md:grid-cols-2">
      <div className={cn(pad, "flex min-w-0 flex-col gap-3 md:border-r md:border-border")}>
        <p className="text-xs leading-5 text-muted">
          {t("An A2A proxy lets an A2A client that cannot reach your devices — a service in the cloud — start sessions on the devices that run AI and take its calls. It controls nothing else and holds none of your keys.")}
        </p>
        <CapsLabel>{t("01 · on the proxy's service, set")}</CapsLabel>
        <code data-testid="a2a-code-line" className="border border-border bg-raised px-2.5 py-2 font-mono text-2xs break-all text-fg select-all">
          {line}
        </code>
        <div className="flex">
          <CopyButton text={line} label={t("copy")} testId="a2a-copy" />
        </div>
        <p data-testid="a2a-code-where" className="text-2xs leading-4 text-fg">
          {t("Set it on the A2A proxy's own service only, never as a shared variable that other services — your A2A client among them — can read. Remove it once the proxy has joined.")}
        </p>
        <a href={A2A_PROXY_DOCS_URL} target="_blank" rel="noopener" data-testid="a2a-docs" className="inline-flex items-center gap-1.5 self-start text-2xs text-fg underline underline-offset-2 hover:no-underline">
          {t("how to run the A2A proxy")}
          <ArrowSquareOutIcon className="size-3" />
        </a>
      </div>
      <div className={cn(pad, "flex min-w-0 flex-col gap-3")}>
        <CapsLabel>{t("02 · waiting for it")}</CapsLabel>
        <p className="flex items-center gap-2 text-xs text-faint">
          <CircleNotchIcon className="size-3 shrink-0 animate-spin" />
          {t("waiting for the A2A proxy to claim this code (valid for 10 minutes, once)…")}
        </p>
        {left !== null && (
          <p data-testid="a2a-countdown" className="font-mono text-xs text-fg tabular-nums" aria-live="off">
            {t("The code expires in {{time}}.", { time: formatCountdown(left) })}
          </p>
        )}
        <p className="text-xs leading-5 text-muted">{t("Once it has claimed the code, its log shows its device fingerprint: you paste it here next.")}</p>
        <p className="text-xs leading-5 font-medium text-fg">{t("Keep this page open: it is what adds the A2A proxy.")}</p>
      </div>
    </div>
  );
}

/**
 * The fingerprint from the proxy's log, asked for — never shown here: a
 * person copies it from the proxy, so a code someone else claimed adds
 * nothing. What is not a fingerprint at all is said, and asked again; a
 * fingerprint that does not match ends the round.
 */
export function A2aFingerprintStep({ device, error, onCheck, onCancel }: { device: string; error: string | null; onCheck: (pasted: string) => void; onCancel: () => void }) {
  const { t } = useTranslation();
  const [text, setText] = useState("");
  const submit = (e: FormEvent) => {
    e.preventDefault();
    onCheck(text);
  };
  return (
    <form data-testid="a2a-fingerprint" className={cn(pad, "flex max-w-xl flex-col gap-3")} onSubmit={submit}>
      <CapsLabel>{t("02 · paste its fingerprint")}</CapsLabel>
      <p className="text-xs leading-5 text-muted">
        {t("A device claimed the code as {{id}}. In the A2A proxy's log, find the line that asks you to paste its fingerprint, and paste that fingerprint here: the proxy is added only if it is the device that claimed the code.", { id: device })}
      </p>
      <input
        data-testid="a2a-fingerprint-input"
        className={field}
        value={text}
        onChange={(e) => setText(e.target.value)}
        placeholder="ABCD-EFGH-IJKL-MNOP"
        aria-label={t("the A2A proxy's fingerprint")}
        autoComplete="off"
        spellCheck={false}
      />
      {error === "format" ? (
        <p data-testid="a2a-fingerprint-format" data-signal="warn" className="text-2xs leading-4 text-warn-fg">
          {t("That is not a device fingerprint: it looks like ABCD-EFGH-IJKL-MNOP, after fingerprint= in the proxy's log.")}
        </p>
      ) : (
        <ErrorLine message={error} />
      )}
      <div className="flex flex-wrap gap-2">
        <button type="submit" data-variant="primary" data-testid="a2a-fingerprint-check" className={primary} disabled={!text.trim()}>
          {t("check the fingerprint")}
        </button>
        <button type="button" className={quiet} onClick={onCancel}>
          {t("cancel")}
        </button>
      </div>
    </form>
  );
}

/** Which devices that run AI take the proxy's calls: none picked to start with, and what taking them means. */
export function A2aChooseStep({ hosts, busy, error, onAdd, onCancel }: { hosts: HostChoice[]; busy: boolean; error: string | null; onAdd: (picked: string[]) => void; onCancel: () => void }) {
  const { t } = useTranslation();
  const [picked, setPicked] = useState<Set<string>>(new Set());
  return (
    <div data-testid="a2a-choose" className={cn(pad, "flex max-w-xl flex-col gap-3")}>
      <span data-signal="ok" className="flex items-center gap-2 font-mono text-xs text-ok">
        <CheckIcon className="size-3 shrink-0" />
        {t("the fingerprint matches")}
      </span>
      <CapsLabel>{t("03 · which devices take its calls")}</CapsLabel>
      <A2aHostPicks hosts={hosts} picked={picked} disabled={busy} onChange={setPicked} />
      <A2aCallsNote />
      <ErrorLine message={error} />
      <div className="flex flex-wrap gap-2">
        <button type="button" data-variant="primary" data-testid="a2a-add" className={primary} disabled={busy} onClick={() => onAdd([...picked])}>
          {busy && <CircleNotchIcon className="size-3.5 animate-spin" />}
          {t("add the A2A proxy")}
        </button>
        <button type="button" className={quiet} disabled={busy} onClick={onCancel}>
          {t("cancel")}
        </button>
      </div>
    </div>
  );
}

/** What taking an A2A proxy's calls means for a device (docs/a2a.md §6), said where it is chosen. */
function A2aCallsNote() {
  const { t } = useTranslation();
  return (
    <p data-testid="a2a-calls-note" className="text-2xs leading-4 text-fg">
      {t("An A2A call starts a session on that device as its user: what the session runs can read any file that user can. Each device also needs an A2A entry that allows the proxy, in its own A2A settings.")}
    </p>
  );
}

/** The devices that run AI, each a checkbox. */
function A2aHostPicks({ hosts, picked, disabled, onChange }: { hosts: HostChoice[]; picked: ReadonlySet<string>; disabled: boolean; onChange: (next: Set<string>) => void }) {
  const { t } = useTranslation();
  if (!hosts.length) return <p className="text-2xs text-faint">{t("no device that runs AI in the account yet")}</p>;
  return (
    <ul className="flex flex-col gap-1">
      {hosts.map((h) => (
        <li key={h.id}>
          <CheckboxField
            data-testid={`a2a-host-${h.id}`}
            disabled={disabled}
            checked={picked.has(h.id)}
            onCheckedChange={(checked) => {
              const next = new Set(picked);
              if (checked) next.add(h.id);
              else next.delete(h.id);
              onChange(next);
            }}
            className="items-center text-xs text-fg"
            label={
              <>
                {h.name ?? h.id}
                {h.name && <span className="ms-1.5 font-mono text-2xs text-faint">{h.id}</span>}
              </>
            }
          />
        </li>
      ))}
    </ul>
  );
}

/**
 * Which devices that run AI take A2A proxy `proxy`'s calls, changed from its
 * row. A device taken off is exposed, as to a lost controller (docs/a2a.md
 * §6, §7.1): a session the proxy started there could have read its keys, its
 * identity key included — so it gets a new content key in the same change,
 * and renews its identity key after it, with the devices that let it control
 * them (`renewsIfNarrowed`). Said before it is saved. Adding one hands
 * nothing over.
 */
export function A2aAcceptPanel({
  proxy,
  hosts,
  accepted,
  busy,
  error,
  renewsIfNarrowed,
  onSave,
  onCancel,
}: {
  proxy: string;
  hosts: HostChoice[];
  /** The devices that take its calls now. */
  accepted: string[];
  busy: boolean;
  error?: string | null;
  /** The devices that run AI that would renew their identity key after these stop taking its calls (`hostsExposedBy`). */
  renewsIfNarrowed?: (dropped: string[]) => string[];
  onSave: (picked: string[]) => void;
  onCancel: () => void;
}) {
  const { t } = useTranslation();
  const [picked, setPicked] = useState<Set<string>>(new Set(accepted));
  const dropped = accepted.filter((id) => !picked.has(id));
  const added = [...picked].filter((id) => !accepted.includes(id));
  return (
    <div data-testid={`a2a-accept-${proxy}`} className="border-b border-border px-6 py-4 max-md:px-4">
      <div className="flex max-w-xl flex-col gap-2">
        <CapsLabel>{t("which devices take its calls")}</CapsLabel>
        <A2aHostPicks hosts={hosts} picked={picked} disabled={busy} onChange={setPicked} />
        <A2aCallsNote />
        {dropped.length > 0 && (
          <>
            <p data-testid="a2a-accept-narrowing" data-signal="warn" className="text-2xs leading-4 text-warn-fg">
              {t(
                "{{ids}} stop taking its calls and are treated as exposed: a session the proxy started there could have read their keys, the identity key included. They get a new content key now and renew their identity key after it; one that cannot renew should be removed and added again.",
                { ids: dropped.join(", ") },
              )}
            </p>
            <RenewalsAhead ids={renewsIfNarrowed?.(dropped) ?? []} />
          </>
        )}
        <ErrorLine message={error} />
        <div className="flex gap-2">
          <button
            type="button"
            data-variant="primary"
            data-testid={`a2a-accept-save-${proxy}`}
            className={primary}
            disabled={busy || (dropped.length === 0 && added.length === 0)}
            onClick={() => onSave([...picked])}
          >
            {busy && <CircleNotchIcon className="size-3.5 animate-spin" />}
            {dropped.length ? t("save and rotate their keys") : t("save")}
          </button>
          <button type="button" className={quiet} disabled={busy} onClick={onCancel}>
            {t("cancel")}
          </button>
        </div>
      </div>
    </div>
  );
}
