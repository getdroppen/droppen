import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { LogOut, Trash2 } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { ApiClient } from "@/api/client";
import { ApiError } from "@/api/errors";
import type { components } from "@/api/schema";
import { Badge } from "@/components/ui/badge";
import { Spinner } from "@/components/ui/spinner";
import { FirstAccountForm, OWNER_EXPLANATION } from "@/features/auth/setup-view";
import { authIsOff, isLocalOwner, LOGIN_PATH, logout } from "@/lib/auth";
import { formatRelative, formatTime, formatUntil } from "@/lib/format";
import { droppenCommand } from "@/lib/droppen-command";
import { cn } from "@/lib/cn";
import { disablePushOnSignOut } from "@/lib/push";
import { Sheet } from "@/components/sheet";
import { useApi, useMachine } from "@/lib/machines/context";
import type { MachineKey } from "@/lib/machines/key";
import { mq } from "@/lib/machines/query";

type AuthSession = components["schemas"]["AuthSession"];

const authStatusKey = (machine: MachineKey) => mq(machine, "auth-status");
const authSessionsKey = (machine: MachineKey) => mq(machine, "auth-sessions");

const field =
  "h-6.5 w-full rounded border border-border bg-bg px-2 font-mono text-2xs text-fg placeholder:text-faint focus:border-fg focus:outline-none";

const sectionTitle = "label-caps -mx-3 -mt-3 mb-3 flex h-9 items-center border-b border-border px-3 text-2xs text-fg";

/**
 * This tab's `GET /api/auth/status`: the answer, or the failure as an error —
 * never nothing, which the tab would have to guess about.
 */
export async function askAuthStatus(api: ApiClient) {
  const res = await api.GET("/api/auth/status");
  if (!res.data) throw new ApiError(res.error);
  return res.data;
}

/** The desktop app's own account (`desktop/core/src/accounts.rs`, `APP_ACCOUNT`). */
const DESKTOP_APP_ACCOUNT = "droppen-app";

/**
 * Whether `user` is somebody's own account on this server: not the desktop
 * app's, not this device's owner by a local-open session, not a device
 * reached through Droppen.
 */
function isPersonsAccount(user: string | null | undefined): boolean {
  return !!user && user !== DESKTOP_APP_ACCOUNT && !isLocalOwner(user) && !user.startsWith("cloud:");
}

/** Whichever sentence the server wrote, or the code it sent, for a user to read. */
function refusal(error: unknown, wrongPassword: string): string {
  if (error instanceof ApiError) {
    return error.code === "invalid credentials" ? wrongPassword : (error.hint ?? error.code);
  }
  return error instanceof Error ? error.message : String(error);
}

/** Who this browser is signed in as, and the way out. */
function AccountSection({ user }: { user: string }) {
  const { t } = useTranslation();
  const [busy, setBusy] = useState(false);

  const signOut = async () => {
    setBusy(true);
    // While the token still works: this browser's notifications stop with the sign-in (the device's row, then the subscription).
    await disablePushOnSignOut();
    await logout();
    // A document load, not a router navigation: it is the one way to be sure
    // nothing carries on running with the token that was just revoked.
    location.replace(LOGIN_PATH);
  };

  return (
    <section className="border-b border-border p-3">
      <h2 data-label="" className={sectionTitle}>{t("signed in")}</h2>
      <div className="flex max-w-xl items-center justify-between gap-4">
        <div className="min-w-0">
          <div className="font-mono text-xs">{user}</div>
          <div className="text-2xs text-faint">{t("this browser's account")}</div>
        </div>
        <button
          type="button"
          disabled={busy}
          onClick={() => void signOut()}
          className="flex h-7 shrink-0 items-center justify-center gap-1.5 rounded border border-border px-3 font-mono text-2xs text-muted hover:bg-hover hover:text-fg disabled:opacity-40"
        >
          <LogOut className="size-3" />
          {t("sign out")}
        </button>
      </div>
    </section>
  );
}

/** Change the password, which also evicts every other device. */
function PasswordSection() {
  const machine = useMachine();
  const api = useApi();
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [confirm, setConfirm] = useState("");

  const change = useMutation({
    mutationFn: async () => {
      const res = await api.POST("/api/auth/password", {
        body: { currentPassword: current, newPassword: next },
      });
      if (!res.data) throw new ApiError(res.error);
      return res.data;
    },
    onSuccess: () => {
      setCurrent("");
      setNext("");
      setConfirm("");
      // The server revoked every other session; the list on this page is stale.
      void queryClient.invalidateQueries({ queryKey: authSessionsKey(machine) });
    },
  });

  const mismatch = confirm !== "" && next !== confirm;
  const ready = current !== "" && next !== "" && next === confirm && !change.isPending;

  return (
    <section className="border-b border-border p-3">
      <h2 data-label="" className={sectionTitle}>{t("password")}</h2>
      <div className="flex max-w-xs flex-col gap-2">
        <label className="flex flex-col gap-1">
          <span className="text-2xs text-faint">{t("current password")}</span>
          <input
            type="password"
            autoComplete="current-password"
            value={current}
            onChange={(e) => setCurrent(e.target.value)}
            className={field}
          />
        </label>
        <label className="flex flex-col gap-1">
          <span className="text-2xs text-faint">{t("new password")}</span>
          <input
            type="password"
            autoComplete="new-password"
            value={next}
            onChange={(e) => setNext(e.target.value)}
            className={field}
          />
        </label>
        <label className="flex flex-col gap-1">
          <span className="text-2xs text-faint">{t("confirm new password")}</span>
          <input
            type="password"
            autoComplete="new-password"
            value={confirm}
            onChange={(e) => setConfirm(e.target.value)}
            className={cn(field, mismatch && "border-danger/60")}
          />
        </label>
        <div className="flex items-center gap-2">
          <button
            type="button"
            disabled={!ready}
            onClick={() => change.mutate()}
            data-variant="primary" className="flex h-7 items-center justify-center bg-fg px-3 text-xs font-medium text-bg hover:bg-fg/85 disabled:opacity-40"
          >
            {change.isPending ? t("saving…") : t("change password")}
          </button>
          {change.isSuccess && (
            <span data-signal="ok" className="font-mono text-2xs text-ok">{t("password changed")}</span>
          )}
        </div>
        {mismatch && <div data-signal="warn" className="font-mono text-2xs text-warn-fg">{t("the two do not match")}</div>}
        {change.isError && (
          <div data-signal="failed" className="font-mono text-2xs break-words text-danger">
            {refusal(change.error, t("that is not the current password"))}
          </div>
        )}
        <p className="text-2xs text-faint">
          {t("Changing it signs out every other device, but not this one.")}
        </p>
      </div>
    </section>
  );
}

/** One live login: what it is, when it was last used, and a way to end it. */
function SessionRow({ session, onRevoke, busy }: { session: AuthSession; onRevoke: () => void; busy: boolean }) {
  const { t } = useTranslation();
  return (
    <tr className="grid grid-cols-[minmax(0,1fr)_auto] border-b border-border/40 @[36rem]/security:table-row">
      <td className="min-w-0 py-1 pr-3 align-top">
        <div className="flex min-w-0 flex-wrap items-center gap-1.5">
          <span className="min-w-0 max-w-full truncate font-mono text-2xs text-fg @[36rem]/security:max-w-64" title={session.label ?? ""}>
            {session.label ?? t("unknown device")}
          </span>
          {session.current && <Badge variant="outline">{t("this device")}</Badge>}
        </div>
        <div className="font-mono text-2xs text-faint">
          {t("since {{when}}", { when: formatTime(session.createdAt) })}
        </div>
      </td>
      <td className="col-start-1 py-1 pr-3 align-top font-mono text-2xs text-faint @[36rem]/security:whitespace-nowrap">
        {t("seen {{ago}} ago", { ago: formatRelative(session.lastSeenAt) })}
      </td>
      <td className="col-start-1 py-1 pr-3 align-top font-mono text-2xs text-faint @[36rem]/security:whitespace-nowrap">
        {t("expires in {{left}}", { left: formatUntil(session.expiresAt) })}
      </td>
      <td className="col-start-2 row-span-3 row-start-1 self-center py-1 text-right align-top">
        <button
          type="button"
          disabled={busy}
          title={session.current ? t("sign this device out") : t("revoke this session")}
          onClick={onRevoke} data-signal="failed" className="rounded p-1 text-muted hover:bg-hover hover:text-danger disabled:opacity-40"
        >
          <Trash2 className="size-3" />
        </button>
      </td>
    </tr>
  );
}

/** Every device holding a live token for this account. */
function SessionsSection() {
  const machine = useMachine();
  const api = useApi();
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const { data, isPending } = useQuery({
    queryKey: authSessionsKey(machine),
    queryFn: async () => (await api.GET("/api/auth/sessions")).data?.sessions ?? [],
  });

  const revoke = useMutation({
    mutationFn: async (ids: string[]) => {
      for (const id of ids) {
        await api.DELETE("/api/auth/sessions/{id}", { params: { path: { id } } });
      }
      return ids;
    },
    onSuccess: () => void queryClient.invalidateQueries({ queryKey: authSessionsKey(machine) }),
  });

  const others = (data ?? []).filter((s) => !s.current);

  const revokeOne = (session: AuthSession) => {
    revoke.mutate([session.id]);
    // Revoking the session this page is using kills the token it is holding;
    // there is nowhere to stay.
    if (session.current) location.replace(LOGIN_PATH);
  };

  return (
    <section className="@container/security p-3">
      <div className="mb-2 flex max-w-2xl items-center justify-between gap-4">
        <h2 data-label="" className={sectionTitle + " mb-0"}>{t("devices")}</h2>
        {others.length > 0 && (
          <button
            type="button"
            disabled={revoke.isPending}
            onClick={() => revoke.mutate(others.map((s) => s.id))}
            className="flex h-6 items-center justify-center rounded border border-border px-2 font-mono text-2xs text-muted hover:bg-hover hover:text-fg disabled:opacity-40"
          >
            {t("sign out everywhere else")}
          </button>
        )}
      </div>
      {isPending && <Spinner />}
      {data && data.length === 0 && (
        <div className="font-mono text-2xs text-faint">{t("no other logins")}</div>
      )}
      {data && data.length > 0 && (
        <table className="block w-full max-w-2xl @[36rem]/security:table">
          <tbody className="block @[36rem]/security:table-row-group">
            {data.map((session) => (
              <SessionRow
                key={session.id}
                session={session}
                busy={revoke.isPending}
                onRevoke={() => revokeOne(session)}
              />
            ))}
          </tbody>
        </table>
      )}
      {revoke.isError && (
        <div data-signal="failed" className="mt-1 font-mono text-2xs text-danger">{refusal(revoke.error, "")}</div>
      )}
    </section>
  );
}

/**
 * The security tab: who you are, your password, and every device holding a
 * token for the account.
 *
 * With authentication switched off (`auth.enabled = false`) there is nothing
 * here to configure, so the tab says why instead of drawing controls that
 * would have nothing to act on. A page opened signed in from the device itself
 * while it has no account (this device's owner) is offered the first account,
 * without the setup code — under the desktop app too, where "Open in Browser"
 * opens such a page. The desktop app's own window's login is the app's; a
 * person's own account on its server is an ordinary one.
 * A status the server did not give is said to be missing, never read as
 * authentication switched off.
 */
export function SecuritySection() {
  const machine = useMachine();
  const api = useApi();
  const { t } = useTranslation();
  // Asked fresh rather than read from the module cache in `lib/auth`: this is
  // the page where the answer can change while it is open.
  const {
    data: status,
    isPending,
    error,
  } = useQuery({
    queryKey: authStatusKey(machine),
    queryFn: () => askAuthStatus(api),
  });

  if (isPending) {
    return (
      <div className="p-3">
        <Spinner />
      </div>
    );
  }

  const command = "mt-1.5 rounded border border-border bg-raised px-2 py-1.5 font-mono text-2xs text-fg";

  if (!status) {
    return (
      <section className="p-3" data-testid="security-status-failed">
        <h2 data-label="" className={sectionTitle}>{t("login")}</h2>
        <div data-signal="failed" className="max-w-xl font-mono text-2xs text-danger">
          {t("Could not ask this device's server about its login.")} {error ? refusal(error, "") : null}
        </div>
      </section>
    );
  }

  if (authIsOff(status)) {
    return (
      <section className="p-3" data-testid="security-auth-off">
        <h2 data-label="" className={sectionTitle}>{t("login")}</h2>
        <div className="max-w-xl text-2xs text-faint">
          <div className="mb-1 text-xs text-fg">{t("switched off")}</div>
          {t(
            "Local login is switched off on this device (auth.enabled = false), so its page answers anyone who can reach it. To require a login, remove this setting from the server's configuration and restart the server:",
          )}
          <pre className={command}>APP_AUTH__ENABLED=false</pre>
          <div className="mt-2">{t("Then make its first account on this device:")}</div>
          <pre className={command}>{`${droppenCommand(status)} user add <name>`}</pre>
        </div>
      </section>
    );
  }

  // This device's owner, by a page opened signed in from the device itself:
  // no account yet, so nothing to sign out of or change — only the first one
  // to make, which needs no code from them. Under the desktop app too: its
  // "Open in Browser" opens exactly such a page, and the app's own account
  // is not one a person signs in with.
  if (isLocalOwner(status.user)) {
    return (
      <section className="p-3" data-testid="security-local-owner">
        <h2 data-label="" className={sectionTitle}>{t("login")}</h2>
        <div className="max-w-xl text-2xs text-faint">
          <div className="mb-1 text-xs text-fg">{t("no account yet")}</div>
          <p className="mb-3">{t(OWNER_EXPLANATION)}</p>
          <FirstAccountForm withCode={false} size="settings" />
        </div>
      </section>
    );
  }

  // Under the Droppen desktop app the window's login is the app's: it made the
  // password nobody knows and signs in again by itself, so changing the
  // password or signing out would only lead nowhere — and its account is the
  // app's own, so "no account yet" is not for this page to say either. A
  // person's own account on the app's server (made on a page "Open in
  // Browser" opened) is an ordinary one, and gets the ordinary view.
  if (status.desktopApp && !isPersonsAccount(status.user)) {
    return (
      <Sheet>
        <section className="border-b border-border p-3" data-testid="security-desktop-app">
          <h2 data-label="" className={sectionTitle}>{t("signed in")}</h2>
          <div className="max-w-xl text-2xs text-faint">{t("The Droppen app signs in for you on this device.")}</div>
        </section>
        <SessionsSection />
      </Sheet>
    );
  }

  return (
    <Sheet>
      {status.user && <AccountSection user={status.user} />}
      <PasswordSection />
      <SessionsSection />
    </Sheet>
  );
}
