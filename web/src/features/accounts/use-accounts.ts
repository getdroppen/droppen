import { useQuery } from "@tanstack/react-query";
import { useCallback, useEffect, useRef, useState } from "react";
import { ApiError } from "@/api/errors";
import type { components } from "@/api/schema";
import { useApi, useMachine } from "@/lib/machines/context";
import type { MachineKey, SessionRef } from "@/lib/machines/key";
import { apiFor } from "@/lib/machines/registry";
import { mq } from "@/lib/machines/query";

export type Account = components["schemas"]["Account"];
export type AccountKind = components["schemas"]["AccountKind"];
export type AccountList = components["schemas"]["AccountList"];
/** Which CLI an account's credential is for. */
export type AccountProvider = components["schemas"]["AccountProvider"];
/** Which CLI a session belongs to, spelled the way the session routes spell it. */
export type SessionSource = components["schemas"]["Source"];
export type Quota = components["schemas"]["Quota"];
export type AccountQuota = components["schemas"]["AccountQuota"];
export type QuotaWindow = components["schemas"]["QuotaWindow"];
export type CodexQuota = NonNullable<Quota["codex"]>;
/** Whether a Codex reading is the app server's live answer or a log scrape. */
export type CodexQuotaSource = CodexQuota["source"];
/** Why an account has no live windows, as the quota probe reports it. */
export type AccountQuotaError = NonNullable<components["schemas"]["AccountQuota"]["error"]>;
/**
 * Where a Claude reading came from: a live session's rate-limit events, or the
 * `/usage` probe the server only falls back to once sessions have gone quiet.
 */
export type QuotaReadingSource = components["schemas"]["QuotaReadingSource"];
/** The machine's own login for one CLI: whether it can start a session, and its rests. */
export type LocalLogin = components["schemas"]["LocalLogin"];
/** The new session `continueOnAccount` made. */
export type ContinuedSession = components["schemas"]["ContinuedSession"];
/** Where a start that names no account runs, and why (`GET /api/accounts/start`). */
export type AccountStart = components["schemas"]["AccountStart"];
/** Why a start that names no account runs where it does. */
export type StartReason = components["schemas"]["StartReason"];

/** Shared query key so every mutation can invalidate a machine's one account list. */
export const accountsKey = (machine: MachineKey) => mq(machine, "accounts");

/** Shared query key for a machine's quota poll, which the composer and settings both read. */
export const quotaKey = (machine: MachineKey) => mq(machine, "quota");

/**
 * Stored accounts for both CLIs, plus the one each defaults to. Cached briefly:
 * the list changes only from the settings page, but several composers read it.
 */
export function useAccounts() {
  const machine = useMachine();
  const api = useApi();
  return useQuery({
    queryKey: accountsKey(machine),
    queryFn: async () => (await api.GET("/api/accounts")).data,
    staleTime: 30_000,
  });
}

/**
 * Where a start of this CLI that names no account runs, and why — the
 * server's own answer (`GET /api/accounts/start`, the rule a chat, a scheduled
 * task and a session started without `accountId` follow), so a page never
 * shows one login while the run goes to another.
 *
 * `model` is the one the start will send; the server reads nothing of it but
 * its family (a rest scoped to one family stands in the way of that family
 * only), so the family is what is asked and what keys the answer. No model is
 * a start that names none (the CLI's default). `chooseModel` is a start that
 * will send whichever model the answer names — a composer with nothing
 * stored, whose default is the head of the login's own list: then the server
 * chooses the login and the model together (the answer's `model`), because
 * asked apart they can chase each other.
 *
 * The key also carries what of the cached list the answer depends on, so a
 * mutation that writes the list into the cache asks again at once. The
 * previous answer stays (for the same CLI) while the next is asked, and
 * `ready` says whether the answer is the one for what is asked now — a start
 * that leaves the account to the server waits for it. Before any answer, and
 * on a server too old to give one, the list's own reading of the rule stands
 * in ([`startAccountFor`]).
 */
export function useStartAccount(
  provider: AccountProvider,
  model?: string | null,
  chooseModel = false,
): { start: AccountStart | null; ready: boolean; confirm: () => Promise<boolean> } {
  const machine = useMachine();
  const api = useApi();
  const { data: list } = useAccounts();
  const family = chooseModel || provider !== "claude" ? null : modelFamily(model);
  const query = useQuery({
    queryKey: mq(machine, "accounts", "start", provider, chooseModel ? "choose" : family, startInputs(list, provider)),
    queryFn: async () => {
      const res = await api.GET("/api/accounts/start", {
        params: { query: { provider, ...(chooseModel ? { chooseModel: true } : family ? { model: family } : {}) } },
      });
      // A server too old to have the route: no answer, and the list's rule
      // is its rule. Anything else that fails is worth the query's retry.
      if (res.response.status === 404) return null;
      if (!res.data) throw new ApiError(res.error);
      return res.data;
    },
    // The previous answer while the next is asked — for the same CLI only: the
    // other CLI's account is never where this one starts.
    placeholderData: (previous, previousQuery) => (previousQuery?.queryKey[4] === provider ? previous : undefined),
  });
  const start = query.data ?? (list ? startAccountFor(list, provider, family) : null);
  // The answer at the moment of sending, not as it was asked: the list is cached
  // and readings are not in the key, so the rule may have moved on. `confirm`
  // asks again and says whether the answer shown still stands; when it does not,
  // the fresh one is in the cache and on screen at the next render, and the
  // caller sends on that.
  const latest = useRef(query);
  latest.current = query;
  const confirm = useCallback(async () => {
    const shown = JSON.stringify(latest.current.data ?? null);
    const fresh = await latest.current.refetch();
    // Nothing to check against: the spawn places it by the same rule anyway.
    if (fresh.isError) return true;
    return JSON.stringify(fresh.data ?? null) === shown;
  }, []);
  return { start, ready: query.isFetched && !query.isPlaceholderData, confirm };
}

/** At most this many re-asks of the start answer for one send… */
export const START_RECHECKS = 3;
/** …and at most this long held for it: then the start goes out with no account, and the server places it. */
export const START_RECHECK_MS = 8_000;

/** What a held start does now ([`useStartRecheck`]). */
export type StartVerdict = "send" | "unplaced" | "wait";

/**
 * The hold a start left to the server is under once it is pressed: it goes
 * out on the answer asked again at that moment ([`useStartAccount`]'s
 * `confirm`), and a different answer is shown first and asked once more.
 *
 * `check()` says what to do now: `"send"` — the answer on screen was just
 * asked again and stands; `"unplaced"` — it has been asked
 * [`START_RECHECKS`] times without settling, or the send has been held
 * [`START_RECHECK_MS`] (an answer that does not come): send it with no
 * account and let the server place it at the spawn; `"wait"` — an answer is
 * being asked, and the caller re-renders when it lands (or at the deadline).
 * A verdict other than `"wait"` ends the hold. `cancel()` drops it: the CLI
 * the start was for changed under it.
 */
export function useStartRecheck(start: { start: AccountStart | null; ready: boolean; confirm: () => Promise<boolean> }) {
  const confirmed = useRef<string | null>(null);
  const confirming = useRef(false);
  const asks = useRef(0);
  const since = useRef<number | null>(null);
  const deadline = useRef<ReturnType<typeof setTimeout> | null>(null);
  // A hold ended (or cancelled) while an answer was being asked is not woken by it.
  const hold = useRef(0);
  const [, wake] = useState(0);
  const cancel = useCallback(() => {
    hold.current++;
    confirmed.current = null;
    confirming.current = false;
    asks.current = 0;
    since.current = null;
    if (deadline.current) clearTimeout(deadline.current);
    deadline.current = null;
  }, []);
  useEffect(() => cancel, [cancel]);
  const check = (): StartVerdict => {
    if (since.current === null) {
      since.current = Date.now();
      deadline.current = setTimeout(() => wake((n) => n + 1), START_RECHECK_MS);
    }
    const shown = JSON.stringify(start.start);
    if (start.ready && confirmed.current === shown) {
      cancel();
      return "send";
    }
    if (Date.now() - since.current >= START_RECHECK_MS) {
      cancel();
      return "unplaced";
    }
    // An answer being asked is awaited — the last one allowed included.
    if (confirming.current) return "wait";
    if (asks.current >= START_RECHECKS) {
      cancel();
      return "unplaced";
    }
    if (start.ready) {
      confirming.current = true;
      asks.current++;
      const mine = hold.current;
      void start.confirm().then((same) => {
        if (mine !== hold.current) return;
        confirming.current = false;
        if (same) confirmed.current = shown;
        wake((n) => n + 1);
      });
    }
    return "wait";
  };
  return { check, cancel };
}

/** A start request with no account in it: the server's to place. */
export function withoutAccount<T extends { accountId?: string | null; accountFrom?: "start" }>(request: T): T {
  const rest = { ...request };
  delete rest.accountId;
  delete rest.accountFrom;
  return rest;
}

/** What of the list a start's answer depends on, as one string for a query key. */
function startInputs(list: AccountList | undefined, provider: AccountProvider): string {
  if (!list) return "";
  const local = localLoginOf(list, provider);
  return JSON.stringify([
    defaultAccountIdFor(list, provider),
    local?.state ?? null,
    local?.isDefault ?? null,
    accountsForProvider(list.accounts, provider).map((a) => [a.id, a.rotation, (a.rest ?? []).map((r) => [r.scope, r.untilAt])]),
  ]);
}

/**
 * The provider an account belongs to.
 *
 * `provider` is always sent by a current server; the fallback is for the window
 * where a new web build talks to an older one, in which every stored account is
 * a Claude account and must not vanish out of the settings page.
 */
export function accountProvider(account: Pick<Account, "provider">): AccountProvider {
  return account.provider ?? "claude";
}

/** The accounts one CLI can run under, in the order the server lists them. */
export function accountsForProvider(accounts: readonly Account[], provider: AccountProvider): Account[] {
  return accounts.filter((account) => accountProvider(account) === provider);
}

/**
 * Which stored account a new session of this CLI starts under, or `null` for
 * the CLI's own local login.
 *
 * Each CLI keeps its own default, so a Claude default must never decide what a
 * Codex session runs under. A default that has since been deleted — or that
 * belongs to the other CLI — is no default: the list is the authority on which
 * ids still exist and what they are for.
 */
export function defaultAccountIdFor(
  list: AccountList | undefined,
  provider: AccountProvider,
): string | null {
  if (!list) return null;
  const id = (provider === "codex" ? list.defaultCodexAccountId : list.defaultAccountId) ?? null;
  if (!id) return null;
  return list.accounts.some((account) => account.id === id && accountProvider(account) === provider)
    ? id
    : null;
}

/**
 * The machine's own login for one CLI, or `null` from a server too old to say.
 *
 * The local login is the one a session with no account runs on — the user's own
 * `~/.claude` (or `~/.codex`) — and it can be signed out without anything here
 * having done it: an OAuth session that expired, a `/logout` in a terminal.
 */
export function localLoginOf(list: AccountList | undefined, provider: AccountProvider): LocalLogin | null {
  const local = list?.local;
  if (!local) return null;
  return (provider === "codex" ? local.codex : local.claude) ?? null;
}

/**
 * Whether the local login is known not to work: a turn on it was refused for
 * its credential, or the usage probe found nobody signed in, and nothing has
 * worked on it since. `unknown` is not this — a login nobody has asked about
 * yet is offered as it always was.
 */
export function localLoginSignedOut(login: LocalLogin | null | undefined): boolean {
  return login?.state === "signed_out";
}

/** The local login's name in a picker, saying so when it is signed out. */
export function localLoginLabel(login: LocalLogin | null | undefined, t: (key: string) => string): string {
  return localLoginSignedOut(login) ? t("local login (not signed in)") : t("local login");
}

/** The model families a claude rest can be scoped to, as the server names them. */
const FAMILIES = ["fable", "opus", "sonnet", "haiku"] as const;

/** The family a claude model id or alias belongs to (`claude-opus-4-1`, `opus[1m]`), or `null`. */
export function modelFamily(model: string | null | undefined): string | null {
  const lower = model?.toLowerCase();
  return (lower && FAMILIES.find((family) => lower.includes(family))) || null;
}

/**
 * Whether one of an account's active rests stands in the way of a session of
 * `family`: a whole-account rest always, a family's own only for that family —
 * and, for a session whose family is unknown, any rest at all, since the model
 * it ends up on may be the resting one. The server's `rest_covers`.
 */
export function restingFor(account: Pick<Account, "rest">, family: string | null, now: number = Date.now()): boolean {
  return (account.rest ?? []).some(
    (rest) =>
      (rest.untilAt === null || Date.parse(rest.untilAt) > now) &&
      (rest.scope === "all" || family === null || rest.scope === family),
  );
}

/**
 * Where a start of this CLI that names no account runs, read off the list —
 * the server's `start_choice`, without what only the server knows (the quota
 * readings that rank the pool, a window about to run out). What a page shows
 * until the server's own answer ([`useStartAccount`]) is in:
 *
 * 1. the configured default ([`defaultAccountIdFor`]);
 * 2. the local login when somebody chose it as the default;
 * 3. the local login when nothing says it is signed out;
 * 4. signed out: an account of this CLI *in the auto-rotation pool* that is
 *    not resting under a scope covering `model` — the first in the list here;
 *    the server prefers the one with the most headroom — and never one out of
 *    the pool (an env block behind a gateway, a codex key nobody put in), which
 *    is not something to bill on somebody's behalf because their own login
 *    lapsed;
 * 5. nothing free there: the local login after all, labelled, with how to
 *    sign in again.
 *
 * A server that predates the rule (its local login says nothing of being
 * chosen, `isDefault`) still starts on the default, else the local login,
 * whatever state that is in — and that is what is shown for it.
 */
export function startAccountFor(
  list: AccountList,
  provider: AccountProvider,
  model?: string | null,
  now: number = Date.now(),
): AccountStart {
  const on = (accountId: string | null, reason: StartReason): AccountStart => ({ accountId, reason, model: null, placed: true });
  const configured = defaultAccountIdFor(list, provider);
  if (configured) return on(configured, "default");
  const local = localLoginOf(list, provider);
  if (!local || !knowsStartRule(list, provider)) return on(null, "local");
  if (local.isDefault) return on(null, "local_chosen");
  if (!localLoginSignedOut(local)) return on(null, "local");
  const family = provider === "claude" ? modelFamily(model) : null;
  const standIn = accountsForProvider(list.accounts, provider).find(
    (account) => account.rotation && !restingFor(account, family, now),
  );
  return standIn ? on(standIn.id, "fallback") : on(null, "local_signed_out");
}

/**
 * Whether the device's server has the start rule at all — it tells a chosen
 * local login from no choice (`isDefault`). An older one starts an unnamed
 * session on the default, else the local login, and a chat there takes no
 * `accountId`.
 */
export function knowsStartRule(list: AccountList | undefined, provider: AccountProvider): boolean {
  return typeof localLoginOf(list, provider)?.isDefault === "boolean";
}

/**
 * Carry a claude session stuck on the local login onto a stored account: the
 * server copies the conversation into a new session bound to the account and
 * answers with it. The session it came from is left exactly as it was.
 */
export async function continueOnAccount(
  session: SessionRef & { source: SessionSource },
  accountId: string,
): Promise<ContinuedSession> {
  const res = await apiFor(session.machine).POST("/api/sessions/{source}/{id}/continue", {
    params: { path: { source: session.source, id: session.id } },
    body: { accountId },
  });
  if (res.error || !res.data) throw new ApiError(res.error);
  return res.data;
}

/** Short chip text for an account's credential kind. */
export function accountKindLabel(kind: AccountKind): string {
  return kind === "env" ? "env" : "OAuth";
}

/** The variable names an env account sets, for a row that lists them. */
export function accountEnvKeys(account: Pick<Account, "envKeys">): string[] {
  return account.envKeys ?? [];
}

/**
 * Whether an account row offers the auto-rotation toggle.
 *
 * Codex accounts never rotate in this phase, so they have nothing to toggle. A
 * Claude env account does get one — off by default, but a relay can still be a
 * deliberate fallback. An account with no `rotation` field is one from a server
 * too old to rotate at all, and a toggle there would change nothing.
 */
export function accountRotationToggleShown(account: Pick<Account, "provider" | "rotation">): boolean {
  return accountProvider(account) !== "codex" && typeof account.rotation === "boolean";
}

/**
 * The list with one account replaced by the server's fresh copy of it, for a
 * mutation that answers with the account rather than the whole list. An id the
 * list does not hold leaves it as it was: the next fetch is the authority on
 * which accounts exist.
 */
export function withAccount(list: AccountList, account: Account): AccountList {
  if (!list.accounts.some((a) => a.id === account.id)) return list;
  return { ...list, accounts: list.accounts.map((a) => (a.id === account.id ? account : a)) };
}

/** Pin a session of either CLI to an account, or unbind it. */
export async function bindSessionAccount(
  session: SessionRef & { source: SessionSource },
  accountId: string | null,
): Promise<void> {
  const res = await apiFor(session.machine).PUT("/api/sessions/{source}/{id}/account", {
    params: { path: { source: session.source, id: session.id } },
    body: { accountId },
  });
  if (res.error) throw new ApiError(res.error);
}

/**
 * What an account's /usage login is worth right now.
 *
 * `usageEnabled` is the verified state of `claude auth login`: the server
 * stores false until a usage-verify succeeds. The quota probe then reports
 * `not_logged_in` when that login has since expired. Both mean "run the
 * wizard", but they read differently: `login_needed` was never set up,
 * `login_expired` was and stopped working. `probe_failed` is the probe itself
 * breaking (a CLI hiccup, a timeout); it is transient and must never be shown
 * as a missing login.
 */
export type AccountUsageState = "ready" | "login_needed" | "login_expired" | "unavailable" | "not_applicable";

export function accountUsageState(
  kind: AccountKind,
  usageEnabled: boolean,
  error: AccountQuotaError | undefined,
): AccountUsageState {
  // Only an OAuth login has a `/usage` endpoint behind it. A pasted env block —
  // Claude's or Codex's — never gets one, so it is not a login waiting to be
  // fixed.
  if (kind === "env") return "not_applicable";
  if (error === "probe_failed") return "unavailable";
  if (!usageEnabled) return "login_needed";
  // Enabled once, and the probe now finds nobody logged in: the login expired.
  // The server still hands back the last good windows in this state, so the
  // caller can show them as old numbers instead of pretending there are none.
  return error === "not_logged_in" ? "login_expired" : "ready";
}

/**
 * The window closest to running out, which is the one a one-line summary must
 * show: an account with 80% of its week left but 4% of its five hours is out of
 * fuel right now. Ties keep the first window, so the server's order decides.
 */
export function tightestWindow(windows: QuotaWindow[]): QuotaWindow | null {
  let tightest: QuotaWindow | null = null;
  for (const w of windows) {
    if (!tightest || w.remainingPercent < tightest.remainingPercent) tightest = w;
  }
  return tightest;
}

/**
 * How old a reading may be before it is worth saying so. The server re-probes
 * on its own budget, so a number older than this is a stale number, not a
 * current one, and the UI must not present it as live.
 */
export const QUOTA_STALE_MS = 10 * 60_000;

/**
 * How much a reading's age is worth saying, in three values rather than two.
 *
 * `unknown` is a percentage with no timestamp: not a fresh percentage, it is
 * one nobody can date, and drawing it exactly like a reading taken a second
 * ago is how a panel came to show a three-day-old Codex window as the current
 * one. Every place that prints a number prints this beside it.
 *
 * A never-probed *account* is a different thing and stays `unknown` here on
 * purpose: its row already says "warming up" through its usage state, and the
 * callers that have that state show it instead.
 */
export type QuotaAge = "fresh" | "stale" | "unknown";

export function quotaAge(updatedAt: string | null | undefined, now = Date.now()): QuotaAge {
  if (!updatedAt) return "unknown";
  const at = new Date(updatedAt).getTime();
  if (Number.isNaN(at)) return "unknown";
  return now - at > QUOTA_STALE_MS ? "stale" : "fresh";
}

/**
 * The line an account's usage state earns beside its readings.
 *
 * An account's numbers come mainly from its own sessions now — any account's,
 * whether or not it ever had a /usage login — and the `/usage` probe is only
 * the fallback for when no session has run in a while. So windows are drawn
 * whenever there are any, in every usage state, and this line is a note about
 * the fallback alone:
 *
 * - `login_needed` with windows becomes `login_needed_idle`: the numbers are
 *   there without /usage, which only the idle-time refresh would have used;
 * - `login_expired` behind a fresh reading becomes `login_expired_idle`: still
 *   said (the login needs redoing before the next idle stretch), but faintly,
 *   as what it costs rather than as though the numbers were wrong;
 * - `unavailable` behind a fresh reading becomes `none`: a transient probe
 *   failure behind numbers sessions keep current says nothing worth a line.
 *
 * An aged reading keeps the old lines — then the fallback was what should
 * have kept it current — and with no windows every state reads exactly as it
 * did before session readings existed. `not_applicable` never has a line.
 */
export type UsageNotice =
  | "none"
  | "unavailable"
  | "login_needed"
  | "login_needed_idle"
  | "login_expired"
  | "login_expired_idle";

export function usageNotice(
  usage: AccountUsageState,
  updatedAt: string | null | undefined,
  hasWindows: boolean,
  now = Date.now(),
): UsageNotice {
  const current = hasWindows && quotaAge(updatedAt, now) === "fresh";
  switch (usage) {
    case "ready":
    case "not_applicable":
      return "none";
    case "login_needed":
      return hasWindows ? "login_needed_idle" : "login_needed";
    case "unavailable":
      return current ? "none" : "unavailable";
    case "login_expired":
      return current ? "login_expired_idle" : "login_expired";
  }
}

/**
 * Why a gauge's numbers deserve the toolbar's `~`, or `null` when they do not.
 *
 * A fresh reading draws no doubt, whatever the probe last said: the numbers are
 * current because sessions keep them so. Only once the reading has aged does a
 * failed probe or an expired /usage login explain *why* it aged, and it is
 * named ahead of the bare age because it is the more useful thing to know.
 */
export type QuotaDoubt = "login_expired" | "unavailable" | "stale" | "unknown";

export function quotaDoubt(
  usage: AccountUsageState,
  updatedAt: string | null | undefined,
  now = Date.now(),
): QuotaDoubt | null {
  const age = quotaAge(updatedAt, now);
  if (age === "fresh") return null;
  if (usage === "login_expired" || usage === "unavailable") return usage;
  return age;
}

/** An enabled account the server has not finished probing yet. */
function quotaIsWarmingUp(data: Quota | undefined): boolean {
  return (data?.accounts ?? []).some((a) => a.usageEnabled && a.updatedAt === null && !a.error);
}

/**
 * Rolling-window quota for both CLIs and every account. A cold server answers
 * with empty accounts and fills them in the background, so polling stays fast
 * until each enabled account has reported once.
 */
export function useQuota() {
  const machine = useMachine();
  const api = useApi();
  return useQuery({
    queryKey: quotaKey(machine),
    queryFn: async () => (await api.GET("/api/stats/quota")).data,
    staleTime: 60_000,
    refetchInterval: (query) => (quotaIsWarmingUp(query.state.data) ? 5_000 : 120_000),
  });
}
