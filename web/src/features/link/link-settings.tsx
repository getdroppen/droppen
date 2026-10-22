import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import type { components } from "@/api/schema";
import { Spinner } from "@/components/ui/spinner";
import { Sheet } from "@/components/sheet";
import { cn } from "@/lib/cn";
import { formatTime } from "@/lib/format";
import { hubWebOrigin } from "./handoff";
import { cloudKey, failure, useLinkFlow } from "./link-flow";
import type { OpenerState } from "./opener";
import { BrowserSignInWaiting, startDroppenSignIn } from "./browser-sign-in";
import { useApi, useMachine } from "@/lib/machines/context";
import { SquareSwitch } from "@/features/cloud/devices";

/*
 * Settings → Droppen, on the computer's own page (never on droppen.app):
 * sign this computer in to Droppen through a droppen.app popup
 * (docs/local-aggregator.md §3.1) — or, in the Droppen desktop app and when a
 * browser blocks the popup, through a browser tab (the browser hand-off,
 * §4.9, `browser-sign-in.tsx`) — see how its link stands, disconnect it.
 * Passwords and recovery words are only ever typed into droppen.app; this
 * page hands the popup's enrollment code to the local server and nothing else.
 */

type CloudStatus = components["schemas"]["CloudStatus"];

export { cloudKey };

const sectionTitle = "label-caps -mx-3 -mt-3 mb-3 flex h-9 items-center border-b border-border px-3 text-2xs text-fg";
const primary = "flex h-7 items-center justify-center bg-fg px-3 text-xs font-medium text-bg hover:bg-fg/85 disabled:opacity-40";
const quiet =
  "flex h-7 shrink-0 items-center justify-center gap-1.5 rounded border border-border px-3 font-mono text-2xs text-muted hover:bg-hover hover:text-fg disabled:opacity-40";
const field =
  "h-6.5 w-full rounded border border-border bg-bg px-2 font-mono text-2xs text-fg placeholder:text-faint focus:border-fg focus:outline-none";

/** Two keys/values in a row of the status table. */
function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <tr className="border-b border-border">
      <td className="py-1 pr-4 align-top font-mono text-2xs whitespace-nowrap text-faint">{label}</td>
      <td className="py-1 font-mono text-xs break-all">{children}</td>
    </tr>
  );
}

function Fingerprint({ value }: { value: string | null | undefined }) {
  if (!value) return null;
  return <span data-testid="machine-fingerprint" className="font-mono text-xs tracking-wide text-fg">{value}</span>;
}

export function DroppenSection({ onOpenSecurity }: { onOpenSecurity?: () => void }) {
  const machine = useMachine();
  const api = useApi();
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  // The page's one sign-in: the first-run card may have started it (`link-flow.ts`).
  const { flow, setFlow, opener } = useLinkFlow();
  const [hubInput, setHubInput] = useState("");
  const [advanced, setAdvanced] = useState(false);
  const [deleteCopies, setDeleteCopies] = useState(false);
  const [confirmUnlink, setConfirmUnlink] = useState(false);

  const busy =
    flow.kind === "starting" || flow.kind === "waiting" || flow.kind === "finishing" || flow.kind === "enrolling" || flow.kind === "blocked" || flow.kind === "browser";
  const { data: status, isPending, error } = useQuery({
    queryKey: cloudKey(machine),
    queryFn: async () => {
      const res = await api.GET("/api/cloud");
      if (!res.data) throw await failure(res);
      return res.data;
    },
    refetchInterval: (q) => (busy || q.state.data?.state === "enrolling" ? 1000 : 5000),
  });

  // Linked while the flow waited (the popup's `done` may never come if it was
  // closed; a browser hand-off's answer may be missed between two looks): the flow is over.
  useEffect(() => {
    if (status?.state === "linked" && (flow.kind === "enrolling" || flow.kind === "closed" || flow.kind === "browser" || flow.kind === "finishing" || flow.kind === "expired"))
      setFlow({ kind: "idle" });
  }, [status?.state, flow.kind, setFlow]);

  const unlink = useMutation({
    mutationFn: async () => {
      const res = await api.POST("/api/cloud/unlink", { body: { deleteCopies } });
      if (!res.data) throw await failure(res);
      return res.data;
    },
    onSuccess: (data) => {
      queryClient.setQueryData(cloudKey(machine), data);
      setConfirmUnlink(false);
      setFlow({ kind: "idle" });
    },
  });

  // Which local accounts see the other devices here (S1-7, §4.6).
  const access = useMutation({
    mutationFn: async ({ user, remote }: { user: string; remote: boolean }) => {
      const res = await api.PUT("/api/cloud/access", { body: { user, remote } });
      if (!res.data) throw await failure(res);
      return res.data;
    },
    onSuccess: (data) => queryClient.setQueryData(cloudKey(machine), data),
  });

  // "I added this device": the owner of a link enrolled from the command line (S1-7).
  const claim = useMutation({
    mutationFn: async () => {
      const res = await api.POST("/api/cloud/claim");
      if (!res.data) throw await failure(res);
      return res.data;
    },
    onSuccess: (data) => queryClient.setQueryData(cloudKey(machine), data),
  });

  // The aggregator (S1-4): this computer controlling the other devices with its one identity.
  const aggregator = useMutation({
    mutationFn: async (enabled: boolean) => {
      const res = await api.POST("/api/cloud/aggregator", { body: { enabled } });
      if (!res.data) throw await failure(res);
      return res.data;
    },
    onSuccess: (data) => queryClient.setQueryData(cloudKey(machine), data),
  });

  // Its identity key renewed elsewhere, and only the retired key left here (docs/host-identity-rotation.md §3.8
  // 「只剩退役钥时」): its key put aside for good, a fresh one made, and it signs in again under a new id.
  const fresh = useMutation({
    mutationFn: async () => {
      // Read as R3's agent serves it until the generated API types name it.
      const post = api as unknown as { POST(path: string, init: { body: unknown }): Promise<{ data?: unknown; error?: unknown; response: Response }> };
      const res = await post.POST("/api/cloud/re-enroll-fresh", { body: {} });
      if (!res.data) throw await failure(res);
      return res.data;
    },
    onSuccess: () => {
      setFlow({ kind: "idle" });
      void queryClient.invalidateQueries({ queryKey: cloudKey(machine) });
    },
  });

  const typedHub = advanced && hubInput.trim() ? hubWebOrigin(hubInput) : null;
  const hubWeb = typedHub ?? status?.hubWeb ?? null;
  const signIn = () => {
    // Synchronously, in the click: a popup blocker lets only these through.
    if (!hubWeb) return;
    startDroppenSignIn(opener, hubWeb, { hub: typedHub ?? undefined });
  };

  if (isPending) {
    return (
      <div className="p-3">
        <Spinner />
      </div>
    );
  }
  if (!status) {
    return <div data-signal="failed" className="p-3 font-mono text-2xs text-danger">{error instanceof Error ? error.message : t("something went wrong")}</div>;
  }

  return (
    <Sheet>
      <section className="border-b border-border p-3" data-testid="droppen-section" data-state={status.state} data-flow={flow.kind}>
        <h2 data-label="" className={sectionTitle}>
          Droppen
        </h2>
        <div className="flex max-w-xl flex-col gap-3">
          {!status.localAuth ? (
            <NoLocalAccount onOpenSecurity={onOpenSecurity} />
          ) : busy || flow.kind === "closed" || flow.kind === "expired" || (flow.kind === "error" && status.state === "none") ? (
            <InProgress
              flow={flow}
              status={status}
              onReopen={() => opener.reopen()}
              onCancel={() => opener.cancel()}
              onRetry={signIn}
              onContinueInBrowser={() => opener.continueInBrowser()}
            />
          ) : status.state === "linked" || status.state === "not_hosting" || status.state === "enrolling" ? (
            <Linked status={status} onAggregator={(enabled) => aggregator.mutate(enabled)} aggregatorBusy={aggregator.isPending} />
          ) : status.state === "removed" ? (
            <Removed onRejoin={signIn} canManage={status.canManage} />
          ) : status.state === "broken" ? (
            <p data-signal="failed" className="font-mono text-2xs break-words text-danger">{status.link?.error ?? t("this device's Droppen state does not load")}</p>
          ) : (
            <SignedOut
              status={status}
              done={flow.kind === "done" ? flow : null}
              advanced={advanced}
              setAdvanced={setAdvanced}
              hubInput={hubInput}
              setHubInput={setHubInput}
              hubValid={!advanced || !hubInput.trim() || typedHub !== null}
              onSignIn={signIn}
            />
          )}
          {linkRotation(status) && (
            <RotationNote
              rotation={linkRotation(status)!}
              canManage={status.canManage}
              busy={fresh.isPending}
              error={fresh.isError ? fresh.error.message : null}
              onFresh={() => fresh.mutate()}
            />
          )}
        </div>
      </section>
      {(status.state === "linked" || status.state === "not_hosting") && status.localAuth && (
        <section className="border-b border-border p-3">
          <h2 data-label="" className={sectionTitle}>{t("who sees your other devices here")}</h2>
          <RemoteAccess
            status={status}
            busy={access.isPending || claim.isPending}
            onSet={(user, remote) => access.mutate({ user, remote })}
            onClaim={() => claim.mutate()}
            error={access.isError ? access.error.message : claim.isError ? claim.error.message : null}
          />
        </section>
      )}
      {(status.state === "linked" ||
        status.state === "not_hosting" ||
        status.state === "enrolling" ||
        status.state === "removed" ||
        status.state === "broken") &&
        status.localAuth && (
        <section className="border-b border-border p-3">
          <h2 data-label="" className={sectionTitle}>{t("disconnect this device")}</h2>
          <div className="flex max-w-xl flex-col gap-2">
            <p className="text-2xs text-faint">
              {t("Stops its Droppen link and deletes its Droppen keys here. It keeps working locally exactly as before. Remove it on droppen.app too, if it is still listed there.")}
            </p>
            {!status.canManage ? (
              <p className="text-2xs text-faint">{t("Only {{owner}}, who signed this device in, can disconnect it.", { owner: status.owner ?? "—" })}</p>
            ) : !confirmUnlink ? (
              <div>
                <button type="button" className={quiet} data-testid="droppen-unlink" onClick={() => setConfirmUnlink(true)}>
                  {t("disconnect this device")}
                </button>
              </div>
            ) : (
              <div className="flex flex-col gap-2">
                {status.state === "linked" && (
                  <label className="flex items-center gap-2 text-2xs text-muted">
                    <input type="checkbox" checked={deleteCopies} onChange={(e) => setDeleteCopies(e.target.checked)} />
                    {t("also delete its encrypted copy on droppen.app")}
                  </label>
                )}
                <div className="flex items-center gap-2">
                  <button type="button" data-variant="primary" className={primary} disabled={unlink.isPending} data-testid="droppen-unlink-confirm" onClick={() => unlink.mutate()}>
                    {unlink.isPending ? t("disconnecting…") : t("disconnect")}
                  </button>
                  <button type="button" className={quiet} onClick={() => setConfirmUnlink(false)}>
                    {t("cancel")}
                  </button>
                </div>
                {unlink.isError && <div data-signal="failed" className="font-mono text-2xs break-words text-danger">{unlink.error.message}</div>}
              </div>
            )}
          </div>
        </section>
      )}
    </Sheet>
  );
}

/** Whether this page came over the network unencrypted (`http://` to an address that is not this computer's own). */
export function unencryptedOverNetwork(loc: { protocol: string; hostname: string } | null = typeof window === "undefined" ? null : window.location): boolean {
  if (!loc || loc.protocol !== "http:") return false;
  const host = loc.hostname.replace(/^\[|\]$/g, "");
  return !(host === "localhost" || host.endsWith(".localhost") || host === "::1" || /^127\./.test(host));
}

/** The short warning beside who sees the other devices, on a page that came over the network unencrypted. */
function UnencryptedNote() {
  const { t } = useTranslation();
  if (!unencryptedOverNetwork()) return null;
  return (
    <p data-testid="droppen-remote-unencrypted" data-signal="warn" className="max-w-xl font-mono text-2xs text-warn-fg">
      {t("This page is open over the network without encryption (http://): others on the network can read what it shows, your other devices' sessions included.")}
    </p>
  );
}

/**
 * Which local accounts of this device see your other devices in its app
 * (S1-7, `docs/local-aggregator.md` §4.6; the user's decision of
 * 2026-10-19): the owner — the one that signed this device in — always
 * does; any other only once allowed here. A device with no owner here (it
 * joined from the command line, or its owner's account is gone) shows them
 * to nobody until one who manages it says "I added this device". Only
 * whoever manages the link sees this list and can change it; any other
 * account is told whether it sees them, and who can allow it.
 */
export function RemoteAccess({
  status,
  busy,
  onSet,
  onClaim,
  error = null,
}: {
  status: CloudStatus;
  busy: boolean;
  onSet: (user: string, remote: boolean) => void;
  onClaim?: () => void;
  error?: string | null;
}) {
  const { t } = useTranslation();
  if (!status.canManage) {
    return (
      <div className="flex max-w-xl flex-col gap-2">
        <p data-testid="droppen-remote-self" data-remote={status.seesOtherDevices ? "on" : "off"} className="text-2xs text-faint">
          {status.seesOtherDevices
            ? t("This local account sees your other devices here.")
            : status.owner
              ? t("This local account does not see your other devices here. {{owner}}, who signed this device in, can allow it here.", { owner: status.owner })
              : t("Nobody sees your other devices here until whoever added this device says so in these settings.")}
        </p>
        {status.seesOtherDevices && <UnencryptedNote />}
      </div>
    );
  }
  if (status.canClaim) {
    return (
      <div data-testid="droppen-remote-claim" className="flex max-w-xl flex-col gap-2">
        <p className="text-2xs text-faint">
          {t("Nobody sees your other devices here yet: this device joined Droppen without signing in here (or the local account that did is gone). If you added it, say so: then you see them here, and decide who else does.")}
        </p>
        <div>
          <button type="button" className={quiet} data-testid="droppen-claim" disabled={busy} onClick={onClaim}>
            {t("I added this device")}
          </button>
        </div>
        <UnencryptedNote />
        {error && <p data-signal="failed" className="font-mono text-2xs break-words text-danger">{error}</p>}
      </div>
    );
  }
  const accounts = status.accounts ?? [];
  return (
    <div data-testid="droppen-remote-access" className="flex max-w-xl flex-col gap-2">
      <p className="text-2xs text-faint">
        {t("Your other devices' sessions show in this device's app only to these local accounts. Anyone using one of them sees what those devices copy here.")}
      </p>
      <UnencryptedNote />
      <ul className="flex flex-col border-t border-border">
        {accounts.map((account) => (
          <li key={account.user} data-remote-user={account.user} className="flex min-h-9 flex-wrap items-center gap-x-4 border-b border-border py-0.5">
            <span className="min-w-0 flex-1 truncate font-mono text-xs text-fg">{account.user}</span>
            {account.owner ? (
              <span className="font-mono text-2xs text-faint">{t("signed this device in: always sees them")}</span>
            ) : (
              <SquareSwitch
                on={account.remote}
                blocked={busy}
                label={t("sees your other devices")}
                testId={`droppen-remote-${account.user}`}
                onChange={(on) => onSet(account.user, on)}
              />
            )}
          </li>
        ))}
      </ul>
      {error && <p data-signal="failed" className="font-mono text-2xs break-words text-danger">{error}</p>}
    </div>
  );
}

function NoLocalAccount({ onOpenSecurity }: { onOpenSecurity?: () => void }) {
  const { t } = useTranslation();
  return (
    <>
      <p className="text-xs text-fg" data-testid="droppen-no-local-account">
        {t("Create a local account for this device first, in Security. Once it is signed in to Droppen, your other devices are reachable through it, so it must not be open to anyone.")}
      </p>
      {onOpenSecurity && (
        <div>
          <button type="button" className={quiet} onClick={onOpenSecurity}>
            {t("open Security")}
          </button>
        </div>
      )}
    </>
  );
}

function SignedOut(props: {
  status: CloudStatus;
  done: { email: string; machine: string } | null;
  advanced: boolean;
  setAdvanced: (v: boolean) => void;
  hubInput: string;
  setHubInput: (v: string) => void;
  hubValid: boolean;
  onSignIn: () => void;
}) {
  const { t } = useTranslation();
  const { status } = props;
  return (
    <>
      <p className="text-xs text-fg">
        {t("Sign in to use this device from your phone on droppen.app, and to see your other devices here too. Everything works without signing in as well.")}
      </p>
      {!status.canManage && status.owner && (
        <p className="text-2xs text-faint">{t("Only {{owner}} can sign this device in.", { owner: status.owner })}</p>
      )}
      <div className="flex flex-wrap items-center gap-2">
        <button type="button" data-variant="primary" className={primary} data-testid="droppen-sign-in" disabled={!status.canManage || !props.hubValid} onClick={props.onSignIn}>
          {t("Sign in to Droppen")}
        </button>
        <button type="button" className="font-mono text-2xs text-faint underline-offset-2 hover:text-fg hover:underline" onClick={() => props.setAdvanced(!props.advanced)}>
          {props.advanced ? t("hide advanced") : t("advanced")}
        </button>
      </div>
      {props.advanced && (
        <label className="flex max-w-xs flex-col gap-1">
          <span className="text-2xs text-faint">{t("hub address")}</span>
          <input
            value={props.hubInput}
            onChange={(e) => props.setHubInput(e.target.value)}
            placeholder={status.hubWeb}
            spellCheck={false}
            className={cn(field, !props.hubValid && "border-danger/60")}
          />
        </label>
      )}
    </>
  );
}

function InProgress({
  flow,
  status,
  onReopen,
  onCancel,
  onRetry,
  onContinueInBrowser,
}: {
  flow: OpenerState;
  status: CloudStatus;
  onReopen: () => void;
  onCancel: () => void;
  onRetry: () => void;
  onContinueInBrowser?: () => void;
}) {
  const { t } = useTranslation();
  if (flow.kind === "browser") return <BrowserSignInWaiting attempt={flow.attempt} onReopen={onReopen} onCancel={onCancel} />;
  const attempt = "attempt" in flow ? flow.attempt : undefined;
  const fingerprint = attempt?.machine.fingerprint ?? status.machine.fingerprint;
  if (flow.kind === "error" || flow.kind === "expired") {
    return (
      <>
        <p data-signal="failed" className="font-mono text-2xs break-words text-danger" data-testid="droppen-error">
          {flow.kind === "expired"
            ? t("The sign-in ran out of time. Sign in again — if droppen.app asked this browser to wait for approval, once it is approved.")
            : flow.message}
        </p>
        <div className="flex gap-2">
          <button type="button" data-variant="primary" className={primary} onClick={onRetry}>
            {t("try again")}
          </button>
          <button type="button" className={quiet} onClick={onCancel}>
            {t("cancel")}
          </button>
        </div>
      </>
    );
  }
  const line =
    flow.kind === "starting"
      ? t("opening droppen.app…")
      : flow.kind === "blocked"
        ? t("The browser blocked the droppen.app window.")
        : flow.kind === "closed"
          ? t("The droppen.app window was closed.")
          : flow.kind === "finishing"
            ? t("adding this device…")
            : flow.kind === "enrolling"
              ? t("waiting for droppen.app to add this device…")
              : t("Continue in the droppen.app window.");
  return (
    <>
      <p className="flex items-center gap-2 text-xs text-fg" data-testid="droppen-progress">
        {flow.kind !== "blocked" && flow.kind !== "closed" && <Spinner />}
        {line}
      </p>
      {fingerprint && (
        <div className="flex flex-col gap-1">
          <span className="text-2xs text-faint">{t("this device's fingerprint — droppen.app shows the same")}</span>
          <Fingerprint value={fingerprint} />
        </div>
      )}
      <div className="flex flex-wrap gap-2">
        {flow.kind === "blocked" ? (
          <button type="button" data-variant="primary" className={primary} data-testid="droppen-open-popup" onClick={onReopen}>
            {t("open droppen.app")}
          </button>
        ) : flow.kind === "waiting" || flow.kind === "closed" ? (
          <button type="button" className={quiet} data-testid="droppen-reopen" onClick={onReopen}>
            {t("open the window again")}
          </button>
        ) : null}
        {(flow.kind === "blocked" || flow.kind === "closed") && attempt?.handoffUrl && onContinueInBrowser && (
          <button type="button" className={quiet} data-testid="droppen-continue-in-browser" onClick={onContinueInBrowser}>
            {t("continue in a browser tab instead")}
          </button>
        )}
        {flow.kind !== "finishing" && flow.kind !== "enrolling" && (
          <button type="button" className={quiet} data-testid="droppen-cancel" onClick={onCancel}>
            {t("cancel")}
          </button>
        )}
      </div>
    </>
  );
}

function Linked({ status, onAggregator, aggregatorBusy }: { status: CloudStatus; onAggregator: (enabled: boolean) => void; aggregatorBusy: boolean }) {
  const { t } = useTranslation();
  const link = status.link;
  const connection = useMemo(() => {
    if (status.state === "enrolling") return t("waiting for another device to add this one");
    // Still in the account, but a device stopped it running AI there: no
    // streams, no channels, until one makes it run AI again.
    if (status.state === "not_hosting") return t("in the account · not running AI in it");
    if (link?.refused) return t("the hub refuses this device · trying again · {{why}}", { why: link.refused });
    if (!link?.running) return t("not running");
    if (link.connected) return link.sinceMs ? t("connected · since {{when}}", { when: formatTime(new Date(link.sinceMs).toISOString()) }) : t("connected");
    const since = link.sinceMs ? formatTime(new Date(link.sinceMs).toISOString()) : "—";
    return link.error ? t("disconnected · since {{when}} · {{why}}", { when: since, why: link.error }) : t("disconnected · since {{when}}", { when: since });
  }, [link, status.state, t]);
  return (
    <>
      <table className="w-full max-w-xl" data-testid="droppen-linked">
        <tbody>
          <Row label={t("account")}>
            {status.account?.email ?? status.account?.id ?? "—"}
            {status.account && <div className="text-2xs text-faint">{status.account.fingerprint}</div>}
          </Row>
          <Row label={t("this device")}>
            {status.machine.hostname} · {status.machine.id ?? "—"}
            <div className="text-2xs text-faint">{status.machine.fingerprint}</div>
          </Row>
          <Row label={t("link")}>
            <span data-signal={link?.connected ? "ok" : "warn"} className={link?.connected ? "text-ok" : "text-warn-fg"} data-testid="droppen-connection">
              {connection}
            </span>
          </Row>
          {link?.machines != null && (
            <Row label={t("in the account")}>{t("devices that run AI: {{machines}} · other devices: {{devices}}", { machines: link.machines, devices: link.devices ?? 0 })}</Row>
          )}
          <Row label={t("aggregator")}>
            <AggregatorLine aggregator={status.aggregator} canManage={status.canManage} busy={aggregatorBusy} onSet={onAggregator} />
          </Row>
          {status.owner && <Row label={t("signed in by")}>{status.owner}</Row>}
        </tbody>
      </table>
      {link?.alarms.map((alarm) => (
        <p key={alarm} data-signal="failed" className="font-mono text-2xs break-words text-danger">
          {alarm}
        </p>
      ))}
    </>
  );
}

/**
 * The aggregator (S1-4): whether this computer controls the account's other
 * devices with its one identity, and the content keys it holds of the
 * devices that accept its control (an admin seals them to it; it asks for
 * what it lacks). The owner turns it off (its keys here are deleted) or on.
 */
export function AggregatorLine({
  aggregator,
  canManage,
  busy,
  onSet,
}: {
  aggregator: CloudStatus["aggregator"];
  canManage: boolean;
  busy: boolean;
  onSet: (enabled: boolean) => void;
}) {
  const { t } = useTranslation();
  const hosts = aggregator.hosts ?? [];
  const held = hosts.filter((h) => h.latestEpoch == null || h.epochs.includes(h.latestEpoch));
  const missing = hosts.filter((h) => !held.includes(h)).map((h) => h.id);
  const outdated = hosts.filter((h) => h.policy === false).map((h) => h.id);
  const line = !aggregator.enabled
    ? t("off: this device does not control other devices")
    : aggregator.state !== "on"
      ? t("on · starts once this device is in the account and connected")
      : hosts.length === 0
        ? t("on · no device accepts control from this one yet: allow it on droppen.app, in My devices")
        : t("on · content keys of {{held}} of {{total}} devices that accept its control", { held: held.length, total: hosts.length });
  return (
    <div className="flex flex-col gap-1" data-testid="droppen-aggregator" data-state={aggregator.enabled ? aggregator.state : "off"}>
      <div className="flex flex-wrap items-center gap-2">
        <span className={aggregator.enabled ? "text-fg" : "text-faint"}>{line}</span>
        {canManage && (
          <button type="button" className={quiet} disabled={busy} data-testid="droppen-aggregate" onClick={() => onSet(!aggregator.enabled)}>
            {aggregator.enabled ? t("turn off") : t("turn on")}
          </button>
        )}
      </div>
      {aggregator.enabled && missing.length > 0 && (
        <span className="text-2xs text-faint" data-testid="droppen-aggregator-missing">
          {aggregator.askedMs
            ? t("missing: {{ids}} · asked the account's full devices at {{when}}", { ids: missing.join(", "), when: formatTime(new Date(aggregator.askedMs).toISOString()) })
            : t("missing: {{ids}}", { ids: missing.join(", ") })}
        </span>
      )}
      {aggregator.enabled && outdated.length > 0 && (
        <span data-signal="warn" className="text-2xs text-warn-fg">
          {t("update Droppen on {{ids}}: until then this device cannot control them", { ids: outdated.join(", ") })}
        </span>
      )}
      {aggregator.enabled && aggregator.error && (
        <span data-signal="warn" className="text-2xs break-words text-warn-fg">
          {aggregator.error}
        </span>
      )}
      {aggregator.enabled && canManage && <span className="text-2xs text-faint">{t("turning it off deletes the other devices' content keys here")}</span>}
    </div>
  );
}

/**
 * `GET /api/cloud`'s `link.rotation` (the agent, R3; docs/host-identity-rotation.md
 * §3.5, §3.7, §3.8): `staged` with the code a renewal
 * confirmed on the device shows while it lasts, `renewed` after one, `foreign`
 * once a verified renewal of its own id names keys it never made (`reason`
 * `renewed_elsewhere`: its own backup from before a renewal looks the same), and
 * `refused` while the hub refuses its sign-in and it syncs the chain to see why.
 * Read as the agent serves it until the generated API types name it.
 */
export interface LinkRotation {
  state: "idle" | "staged" | "renewed" | "foreign" | "refused";
  code?: string | null;
  codeExpiresMs?: number | null;
  renewedMs?: number | null;
  previousFingerprint?: string | null;
  reason?: string | null;
}

/** This device's renewal as its link reports it, when there is anything to say. */
export function linkRotation(status: Pick<CloudStatus, "link">): LinkRotation | null {
  const r = (status.link as { rotation?: unknown } | null | undefined)?.rotation as Partial<LinkRotation> | null | undefined;
  if (!r || typeof r !== "object" || (r.state !== "staged" && r.state !== "renewed" && r.state !== "foreign" && r.state !== "refused")) return null;
  return r as LinkRotation;
}

/**
 * This device's identity key renewal, on its own Droppen settings: the code to
 * type on the device that asked (a renewal confirmed on the device, §3.7), when
 * it last renewed, that the hub refused its sign-in while it syncs (no alarm:
 * nothing is known yet), or — in red — that its identity key was replaced
 * elsewhere, with the way back: put its key aside for good and sign it in
 * again under a new identity (§3.8 「只剩退役钥时」). Worded so as not to alarm:
 * restoring this device's own backup from before a renewal ends here too.
 */
export function RotationNote({
  rotation,
  canManage,
  busy,
  error,
  onFresh,
  now = Date.now(),
}: {
  rotation: LinkRotation;
  canManage: boolean;
  busy: boolean;
  error: string | null;
  onFresh: () => void;
  now?: number;
}) {
  const { t } = useTranslation();
  const [confirming, setConfirming] = useState(false);
  const when = (ms: number) => formatTime(new Date(ms).toISOString());
  if (rotation.state === "staged") {
    const live = !!rotation.code && (rotation.codeExpiresMs == null || rotation.codeExpiresMs > now);
    if (!live) return null;
    return (
      <div data-testid="droppen-rotation" data-state="staged" data-signal="warn" className="flex flex-col gap-1 border border-border p-3 shadow-[inset_2px_0_0_var(--color-warn)]">
        <span className="font-mono text-2xs text-fg">{t("a device is renewing this device's identity key")}</span>
        <span data-testid="droppen-rotation-code" className="font-mono text-2xl tracking-[0.2em] text-fg select-all">
          {rotation.code}
        </span>
        <span className="text-2xs text-muted">
          {rotation.codeExpiresMs != null
            ? t("Type this code on the device that asked, to confirm it is renewing this one. Valid until {{time}}.", { time: when(rotation.codeExpiresMs) })
            : t("Type this code on the device that asked, to confirm it is renewing this one.")}
        </span>
      </div>
    );
  }
  if (rotation.state === "renewed") {
    return (
      <div data-testid="droppen-rotation" data-state="renewed" className="flex flex-col gap-0.5 text-2xs text-muted">
        <span className="font-mono">{rotation.renewedMs != null ? t("identity key renewed · {{time}}", { time: when(rotation.renewedMs) }) : t("identity key renewed")}</span>
        {rotation.previousFingerprint && <span className="font-mono text-faint">{t("previous fingerprint {{fp}}", { fp: rotation.previousFingerprint })}</span>}
      </div>
    );
  }
  if (rotation.state === "refused") {
    return (
      <p data-testid="droppen-rotation" data-state="refused" className="font-mono text-2xs text-muted">
        {t("the hub refused this device's sign-in · syncing the account before trying again")}
      </p>
    );
  }
  return (
    <div data-testid="droppen-rotation" data-state="foreign" data-signal="failed" className="flex flex-col gap-2 border border-border bg-danger/4 p-3 shadow-[inset_2px_0_0_var(--color-danger)]">
      <span className="font-mono text-xs text-danger">{t("this device's identity key was replaced elsewhere")}</span>
      <span className="text-2xs leading-4 text-muted">
        {t("This device's identity key was replaced elsewhere (for example, it was restored from a backup made before a renewal, or another copy of its old key renewed it). To keep using it here, add it again under a new identity.")}
      </span>
      <span className="text-2xs leading-4 text-muted">
        {t("Its key here is put aside for good, a new one is made, and you sign it in with a new registration code. Remove its old entry on droppen.app too.")}
      </span>
      {canManage &&
        (confirming ? (
          <div className="flex flex-wrap items-center gap-2">
            <button type="button" data-variant="primary" data-testid="droppen-fresh-confirm" className={primary} disabled={busy} onClick={onFresh}>
              {busy ? t("making a new identity…") : t("add this device again with a new identity")}
            </button>
            <button type="button" className={quiet} disabled={busy} onClick={() => setConfirming(false)}>
              {t("cancel")}
            </button>
          </div>
        ) : (
          <div>
            <button type="button" data-testid="droppen-fresh" className={quiet} onClick={() => setConfirming(true)}>
              {t("add this device again with a new identity")}
            </button>
          </div>
        ))}
      {error && <div data-signal="failed" className="font-mono text-2xs break-words text-danger">{error}</div>}
    </div>
  );
}

function Removed({ onRejoin, canManage }: { onRejoin: () => void; canManage: boolean }) {
  const { t } = useTranslation();
  return (
    <>
      <p className="text-xs text-fg" data-testid="droppen-removed">
        {t("This device was removed from the account on droppen.app. It keeps working locally.")}
      </p>
      <div>
        <button type="button" data-variant="primary" className={primary} disabled={!canManage} onClick={onRejoin}>
          {t("join again")}
        </button>
      </div>
    </>
  );
}
