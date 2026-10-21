/*
 * What a machine is called, in cloud mode.
 *
 * The hub knows a machine only by its id. Its name — the one a full device
 * gave it, else its hostname — travels sealed in the machine's status stream
 * (`displayName`, `lib/hub/streams.ts`), so the machine list reads each
 * machine's status (`lib/hub/machine-status.ts`) and the bar reads the open
 * machine's from its offline copy; the id stays as a quiet second label, and
 * stands in for the name until there is one.
 *
 * Renaming is a request to the machine itself — `POST /rh/display-name` on an
 * `rpc` pipe of its channel, which only a current full device may make — so
 * it is offered only while the machine is online. The new name shows at once
 * (what the machine says it goes by now, `GET /rh/info`) and gives way to the
 * next status record, which carries it.
 */
import { useEffect, useState, useSyncExternalStore } from "react";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import { Check, Loader2, Pencil, X } from "lucide-react";
import { hostRefusalText } from "@/api/errors";
import { cn } from "@/lib/cn";
import { type KeyStore, type MachineKey, namingKey } from "@/lib/e2e";
import type { CloudAccount } from "@/lib/hub/account";
import type { HubLink, PresenceState } from "@/lib/hub/link";
import { MachineChannel } from "@/lib/hub/machine";
import type { RelayPool } from "@/lib/hub/relays";
import { HubCopySource, signKeyAt } from "@/lib/hub/copy-source";
import { MachineStatusReader } from "@/lib/hub/machine-status";
import { type MachineMirror, mirrorSeal, type Transact } from "@/lib/hub/mirror";
import { DISPLAY_NAME_MAX, machineDisplayName, recentAlarms, type StatusState, StreamClient } from "@/lib/hub/streams";
import { type PipeOpener, tunnelTransport } from "@/lib/transport/tunnel";
import type { CopyWarning } from "@/lib/hub/plan";
import { CopyWarningLine } from "./plan";
import { errorText } from "./ui";

// ---------------------------------------------------------------------------
// Names
// ---------------------------------------------------------------------------

/** A name as typed, checked as the agent checks it; empty is "back to the hostname" (`null`). */
export type NameCheck = { ok: true; name: string | null } | { ok: false; reason: "tooLong" | "control" };

export function checkDisplayName(input: string): NameCheck {
  const name = input.trim();
  if (!name) return { ok: true, name: null };
  if (/[\u0000-\u001f\u007f-\u009f]/.test(name)) return { ok: false, reason: "control" };
  if ([...name].length > DISPLAY_NAME_MAX) return { ok: false, reason: "tooLong" };
  return { ok: true, name };
}

/**
 * Names just set from this page, until the machine's next status says so
 * itself: shown while the status they were set over is still the newest.
 */
const renamed = new Map<string, { name: string | null; base: StatusState | null }>();
let renamedVersion = 0;
const renamedWatchers = new Set<() => void>();

/** `machine` was just renamed to `name` over `base`, the status shown at the time. */
export function noteRenamed(machine: string, name: string | null, base: StatusState | null): void {
  renamed.set(machine, { name, base });
  renamedVersion++;
  for (const watch of [...renamedWatchers]) watch();
}

/** The name to show for `machine` given its newest status: a rename made here first, else the status's; `null` for none (show the id). */
export function shownName(machine: string, status: StatusState | null): string | null {
  const pending = renamed.get(machine);
  if (pending && pending.base === status) return pending.name;
  return machineDisplayName(status);
}

export function useRenamed(): number {
  return useSyncExternalStore(
    (notify) => {
      renamedWatchers.add(notify);
      return () => renamedWatchers.delete(notify);
    },
    () => renamedVersion,
    () => renamedVersion,
  );
}

// ---------------------------------------------------------------------------
// Renaming
// ---------------------------------------------------------------------------

export class RenameError extends Error {
  constructor(
    /**
     * `forbidden`: this device may not (not a current device that manages the
     * account: 403, `limited_device` among them); `stale`: the computer's view
     * of the account is not confirmed lately (412); `refused`: the machine
     * said no otherwise; `unreachable`: no answer.
     */
    readonly kind: "forbidden" | "stale" | "refused" | "unreachable",
    message: string,
  ) {
    super(message);
    this.name = "RenameError";
  }
}

/** Requests to the machine's own routes carry a placeholder origin; the tunnel sends only the path. */
const MACHINE = "http://machine.invalid";

async function sentence(res: Response): Promise<string> {
  try {
    const body = (await res.json()) as { error?: unknown; hint?: unknown };
    // A refusal of a route that manages the computer (403 `limited_device`, 412 `stale`): in words.
    const host = hostRefusalText(typeof body.error === "string" ? body.error : null, body);
    if (host) return host;
    if (typeof body.hint === "string" && body.hint) return body.hint;
    if (typeof body.error === "string" && body.error) return body.error;
  } catch {
    // not JSON
  }
  return `HTTP ${res.status}`;
}

/**
 * Give the machine behind `opener` the name `name` (`null`: back to its
 * hostname), and return the name it goes by now.
 */
export async function renameMachine(opener: PipeOpener, name: string | null): Promise<string | null> {
  const via = tunnelTransport(opener);
  let res: Response;
  try {
    res = await via.fetch(
      new Request(`${MACHINE}/rh/display-name`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ name }) }),
    );
  } catch (error) {
    throw new RenameError("unreachable", error instanceof Error ? error.message : String(error));
  }
  if (res.status === 403) throw new RenameError("forbidden", await sentence(res));
  if (res.status === 412) throw new RenameError("stale", await sentence(res));
  if (!res.ok) throw new RenameError("refused", await sentence(res));
  const answered = (await res.json().catch(() => null)) as { displayName?: unknown } | null;
  if (typeof answered?.displayName === "string") return answered.displayName;
  // What it goes by now — after a reset, its hostname, which only it knows.
  try {
    const info = (await (await via.fetch(new Request(`${MACHINE}/rh/info`))).json()) as { displayName?: unknown };
    if (typeof info.displayName === "string") return info.displayName;
  } catch {
    // the name was set; the next status says what it is
  }
  return name;
}

/**
 * Rename a machine the page has no channel to (the machine list): one opened
 * for it — through the relays, like every channel (`relays`: the page's pool;
 * without one only the hub could carry it, which it no longer does after the
 * cut-over) — and closed after.
 */
export async function renameThroughNewChannel(
  link: HubLink,
  account: CloudAccount,
  machine: string,
  name: string | null,
  relays?: RelayPool | null,
): Promise<string | null> {
  const channel = new MachineChannel(link, account, machine, { relays: relays ?? undefined });
  try {
    return await renameMachine(channel, name);
  } finally {
    channel.close();
  }
}

export function renameErrorText(error: unknown, t: TFunction): string {
  if (error instanceof RenameError) {
    if (error.kind === "forbidden") return t("only a full device of the account can rename it");
    if (error.kind === "unreachable") return t("the device could not be reached");
    return error.message;
  }
  // The channel's, the relay's or the hub's refusal, or no answer: in words (`errorText`), never its own message.
  return errorText(error);
}

// ---------------------------------------------------------------------------
// Reading statuses
// ---------------------------------------------------------------------------

/** The open machine's status, from its offline copy. */
export function useMirrorStatus(mirror: MachineMirror | null | undefined): StatusState | null {
  return useSyncExternalStore(
    (notify) =>
      mirror
        ? mirror.onChange((change) => {
            if (change.kind === "status") notify();
          })
        : () => undefined,
    () => mirror?.status?.state ?? null,
    () => mirror?.status?.state ?? null,
  );
}

/** What a status reader needs of the account (`CloudAccount` is one). */
export type StatusAccount = Pick<CloudAccount, "account" | "api" | "session" | "machine" | "machineKeys" | "onChange"> & Partial<Pick<CloudAccount, "chain" | "sync">>;

/** Which key epochs a reader was opened with: a different set is a different reader. */
const epochsOf = (epochs: ReadonlyMap<number, MachineKey>) => [...epochs.keys()].sort((a, b) => a - b).join(",");

/**
 * A status reader for `machine` over the page's link, with its keys from the
 * account (`machineKeys`; the status stream is looked for under the newest
 * epoch's name first, then older ones, and every epoch opens records); one
 * with no keys is inert. The caller closes both. `epochs` says
 * which epochs it holds, so a set that has changed since can be told apart.
 */
export async function openStatusReader(
  account: StatusAccount,
  link: HubLink | StreamClientLink,
  store: KeyStore,
  machine: string,
  transact?: Transact,
): Promise<{ reader: MachineStatusReader; client: StreamClient | null; epochs: string }> {
  const info = account.machine(machine);
  const epochs: Map<number, MachineKey> = info ? await account.machineKeys(machine, store) : new Map();
  const mk = namingKey(epochs);
  const client =
    info && mk
      ? new StreamClient({
          link,
          machineSignKey: info.signKey,
          epochs,
          fetchPage: (stream, page) => account.api.streamPage(account.session.token, stream, page),
          // A copy kept from before the device renewed its identity key still resumes, under the key of the seq it was
          // verified at; what comes from the hub is checked under its current key, once more after a sync (§5.4).
          signKeyAt: (seq) => signKeyAt(account.chain, machine, seq),
          chainSeq: () => account.chain?.seq ?? null,
          resyncSignKey: async () => {
            await account.sync?.();
            return account.machine(machine)?.signKey ?? null;
          },
        })
      : null;
  const reader = new MachineStatusReader({ account: account.account, machine, source: HubCopySource.over(mk && client ? { mk, client } : null), transact, seal: mirrorSeal(store) });
  return { reader, client, epochs: epochsOf(epochs) };
}

type StreamClientLink = ConstructorParameters<typeof StreamClient>[0]["link"];

/**
 * The status readers of a list of machines, kept in step with the keys the
 * account holds: a machine whose key epochs change — its first key landing
 * after it was enrolled, a rotation adding an epoch — gets a reader opened
 * afresh with them (the account says when its keys change), and until the
 * new one has read anything, what the old one showed stays shown.
 */
export class StatusReaders {
  private readonly held = new Map<string, { reader: MachineStatusReader; client: StreamClient | null; epochs: string }>();
  /** Per machine, the re-check in flight: one at a time, in order. */
  private readonly queue = new Map<string, Promise<void>>();
  private machines: readonly string[] = [];
  private closed = false;
  private readonly off: () => void;

  constructor(
    private readonly opts: {
      account: StatusAccount;
      link: HubLink | StreamClientLink;
      store: KeyStore;
      transact?: Transact;
      /** A machine's status changed (`null`: none read yet). */
      onStatus(machine: string, state: StatusState | null): void;
    },
  ) {
    this.off = opts.account.onChange(() => this.recheck());
  }

  /** Follow exactly these machines. */
  set(machines: readonly string[]): void {
    this.machines = machines;
    for (const [machine, one] of this.held) {
      if (machines.includes(machine)) continue;
      this.held.delete(machine);
      void one.reader.flush();
      one.reader.close();
      one.client?.close();
    }
    this.recheck();
  }

  /** Re-open the reader of every machine whose key epochs are not the ones it was opened with. Resolves once all have been looked at. */
  recheck(): Promise<void> {
    return Promise.all(this.machines.map((machine) => this.check(machine))).then(() => undefined);
  }

  private check(machine: string): Promise<void> {
    const next = (this.queue.get(machine) ?? Promise.resolve())
      .then(async () => {
        if (this.closed || !this.machines.includes(machine)) return;
        const opened = await openStatusReader(this.opts.account, this.opts.link, this.opts.store, machine, this.opts.transact);
        const before = this.held.get(machine);
        if (this.closed || !this.machines.includes(machine) || before?.epochs === opened.epochs) {
          opened.reader.close();
          opened.client?.close();
          return;
        }
        if (before) {
          await before.reader.flush();
          before.reader.close();
          before.client?.close();
        }
        this.held.set(machine, opened);
        opened.reader.onChange(() => {
          if (this.held.get(machine) !== opened) return;
          // Nothing read yet by a reader replacing another: keep what was shown.
          if (opened.reader.state === null && before) return;
          this.opts.onStatus(machine, opened.reader.state);
        });
        // A status that cannot be read leaves the id standing in.
        await opened.reader.start().catch(() => undefined);
      })
      .catch(() => undefined);
    this.queue.set(machine, next);
    return next;
  }

  /** Save what is pending now (the page is going away). */
  flush(): void {
    for (const { reader } of this.held.values()) void reader.flush();
  }

  close(): void {
    this.closed = true;
    this.off();
    for (const { reader, client } of this.held.values()) {
      void reader.flush();
      reader.close();
      client?.close();
    }
    this.held.clear();
  }
}

/** The statuses of machines whose copies the page follows, as each copy reads or hears one; the call returned stops. */
export function followCopyStatuses(
  copies: readonly (readonly [string, Pick<MachineMirror, "status" | "onChange" | "whenLoaded">])[],
  onStatus: (machine: string, state: StatusState | null) => void,
): () => void {
  let live = true;
  const offs = copies.map(([id, mirror]) => {
    const take = () => {
      if (live) onStatus(id, mirror.status?.state ?? null);
    };
    take();
    void mirror.whenLoaded().then(take);
    return mirror.onChange((change) => {
      if (change.kind === "status") take();
    });
  });
  return () => {
    live = false;
    for (const off of offs) off();
  };
}

/**
 * Every listed machine's status, as each is read (from the device at once,
 * then from the hub), following the keys as they arrive. A machine whose
 * offline copy the page already follows (`copyOf`: droppen.app's shell holds
 * every machine of the account) is read from that copy — its status stream is
 * already followed and kept, and a reader of its own would follow and write
 * the same stream a second time; only the others get a reader.
 */
export function useMachineStatuses(
  account: StatusAccount,
  link: HubLink,
  store: KeyStore,
  machines: readonly string[],
  copyOf: (machine: string) => MachineMirror | null = () => null,
): ReadonlyMap<string, StatusState | null> {
  const [statuses, setStatuses] = useState<ReadonlyMap<string, StatusState | null>>(() => new Map());
  const [readers, setReaders] = useState<StatusReaders | null>(null);
  useEffect(() => {
    const set = new StatusReaders({ account, link, store, onStatus: (machine, state) => setStatuses((prev) => new Map(prev).set(machine, state)) });
    const flush = () => set.flush();
    globalThis.addEventListener?.("pagehide", flush);
    setReaders(set);
    return () => {
      globalThis.removeEventListener?.("pagehide", flush);
      set.close();
    };
  }, [account, link, store]);
  const key = machines.join("\n");
  useEffect(() => {
    const ids = key ? key.split("\n") : [];
    const copies = ids.map((id) => [id, copyOf(id)] as const);
    readers?.set(copies.filter(([, mirror]) => !mirror).map(([id]) => id));
    return followCopyStatuses(
      copies.filter((c): c is readonly [string, MachineMirror] => c[1] !== null),
      (id, state) => setStatuses((prev) => (prev.has(id) && prev.get(id) === state ? prev : new Map(prev).set(id, state))),
    );
  }, [readers, key, copyOf]);
  return statuses;
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

/** A machine's name with its id as a quiet second label, or the id alone. */
export function MachineLabel({ id, name, className }: { id: string; name: string | null; className?: string }) {
  return (
    <span className={cn("flex min-w-0 flex-col", className)} title={id}>
      <span className="truncate font-sans text-sm font-semibold text-fg">{name ?? id}</span>
      {name && <span className="truncate font-mono text-2xs text-faint">{id}</span>}
    </span>
  );
}

/** Type a new name; empty goes back to the hostname. */
export function RenameMachineForm({
  initial,
  onRename,
  onDone,
  compact,
}: {
  initial: string;
  onRename: (name: string | null) => Promise<void>;
  onDone: () => void;
  /** The bar's height. */
  compact?: boolean;
}) {
  const { t } = useTranslation();
  const [value, setValue] = useState(initial);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (busy) return;
    const checked = checkDisplayName(value);
    if (!checked.ok) {
      setError(checked.reason === "tooLong" ? t("a name is at most {{n}} characters", { n: DISPLAY_NAME_MAX }) : t("a name cannot hold control characters"));
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await onRename(checked.name);
      onDone();
    } catch (err) {
      setError(renameErrorText(err, t));
      setBusy(false);
    }
  };
  const icon = compact ? "size-3" : "size-3.5";
  const button = cn("flex shrink-0 items-center justify-center text-muted hover:bg-hover hover:text-fg disabled:opacity-40", compact ? "size-5" : "size-9");
  return (
    <form onSubmit={submit} data-testid="rename-machine" className={cn("flex min-w-0 items-center gap-1", compact ? "flex-1" : "flex-wrap")}>
      <input
        value={value}
        onChange={(e) => setValue(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            e.stopPropagation();
            onDone();
          }
        }}
        autoFocus
        maxLength={DISPLAY_NAME_MAX * 2}
        aria-label={t("device name")}
        placeholder={t("empty: the device's hostname")}
        spellCheck={false}
        disabled={busy}
        className={cn(
          "min-w-0 border border-border bg-bg px-2 font-mono text-fg placeholder:text-faint focus:border-fg focus:outline-none",
          compact ? "h-5 w-40 text-2xs" : "h-9 flex-1 text-xs max-md:text-base",
        )}
      />
      <button type="submit" disabled={busy} title={t("save")} aria-label={t("save")} className={button}>
        {busy ? <Loader2 className={cn(icon, "animate-spin")} /> : <Check className={icon} />}
      </button>
      <button type="button" disabled={busy} title={t("back")} aria-label={t("back")} onClick={onDone} className={button}>
        <X className={icon} />
      </button>
      {error && (
        <span role="alert" data-signal="failed" title={error} className={cn("min-w-0 truncate font-mono text-2xs text-danger", !compact && "basis-full")}>
          {error}
        </span>
      )}
    </form>
  );
}

/**
 * A machine's presence, by the drafting sheet's rule: online is the normal,
 * quiet state (an ink square); offline is red, because the machine cannot be
 * driven; unknown is a hollow square until the hub says.
 */
export function Presence({ presence, compact }: { presence: PresenceState; compact?: boolean }) {
  const { t } = useTranslation();
  return (
    <span
      data-presence={presence}
      data-signal={presence === "offline" ? "offline" : undefined}
      className={cn(
        "flex shrink-0 items-center gap-1.5 font-mono text-2xs",
        presence === "online" ? "text-fg" : presence === "offline" ? "text-danger" : "text-faint",
      )}
    >
      <span
        aria-hidden="true"
        className={cn(
          "size-[7px] shrink-0",
          presence === "online" ? "bg-fg" : presence === "offline" ? "shadow-[inset_0_0_0_1.25px_var(--color-danger)]" : "shadow-[inset_0_0_0_1.25px_currentColor]",
        )}
      />
      <span className={compact ? "sr-only" : undefined}>{t(presence)}</span>
    </span>
  );
}

/**
 * What only droppen.app has to say about a machine on its row of 「我的设备」
 * (`device-row.tsx`'s `notes`): a chain alarm its
 * status carries (a fork it was shown); `oldKey`, it still uses a key epoch
 * older than the chain's newest for it (its status says so, or handing it the
 * new key failed) — the amber "needs you" state: opening it hands it the new
 * key; and, once it has been away for days, what that means for its offline
 * copy in the cloud (the plan's retention). Nothing when there is nothing.
 */
export function MachineNotes({
  id,
  status,
  oldKey = false,
  copyWarning = null,
}: {
  id: string;
  status: StatusState | null;
  oldKey?: boolean;
  /** Away for days: its cloud copy goes soon, or went (the plan's retention, relay-and-plans §3.8). */
  copyWarning?: { warning: CopyWarning; plan: string } | null;
}) {
  const { t } = useTranslation();
  const alarm = recentAlarms(status)[0];
  if (!alarm && !oldKey && !copyWarning) return null;
  return (
    <div className="flex min-w-0 flex-col gap-1" data-old-key={oldKey ? "" : undefined} data-chain-alarm={alarm ? "" : undefined}>
      {alarm && (
        <span role="alert" data-signal="alarm" data-testid={`chain-alarm-${id}`} className="flex min-w-0 items-center gap-2 text-2xs text-muted" title={alarm.what}>
          <span className="inline-flex h-4 shrink-0 items-center bg-danger px-1 font-mono text-[10px] leading-none font-semibold text-on-signal">{t("chain alarm")}</span>
          <span className="truncate">{t("it was shown a device chain that conflicts with its own: {{what}}", { what: alarm.what })}</span>
        </span>
      )}
      {oldKey && (
        <span data-signal="warn" className="flex min-w-0 items-center gap-2 text-2xs text-muted">
          <span className="inline-flex h-4 shrink-0 items-center bg-warn px-1 font-mono text-[10px] leading-none font-semibold text-on-signal">{t("old key")}</span>
          <span className="truncate">{t("it still uses an old key — open it to update")}</span>
        </span>
      )}
      {copyWarning && <CopyWarningLine warning={copyWarning.warning} plan={copyWarning.plan} />}
    </div>
  );
}

/**
 * The open machine in the cloud bar: its name (the id beside it, quietly),
 * and — while the page is live on it — a way to rename it over its channel.
 */
export function BarMachineName({
  machine,
  mirror,
  channel,
  canRename,
}: {
  machine: string;
  mirror: MachineMirror | null | undefined;
  channel: PipeOpener | null | undefined;
  canRename: boolean;
}) {
  const { t } = useTranslation();
  useRenamed();
  const status = useMirrorStatus(mirror);
  const [editing, setEditing] = useState(false);
  const name = shownName(machine, status);
  if (editing && canRename && channel) {
    return (
      <RenameMachineForm
        compact
        initial={name ?? ""}
        onRename={async (next) => noteRenamed(machine, await renameMachine(channel, next), status)}
        onDone={() => setEditing(false)}
      />
    );
  }
  return (
    <>
      <span data-testid="bar-machine-name" className="truncate text-fg" title={machine}>
        {name ?? machine}
      </span>
      {name && (
        <span className="truncate text-faint max-sm:hidden" title={machine}>
          {machine}
        </span>
      )}
      {canRename && channel && (
        <button
          type="button"
          title={t("rename device")}
          aria-label={t("rename device")}
          onClick={() => setEditing(true)}
          className="flex size-5 shrink-0 items-center justify-center text-faint hover:bg-hover hover:text-fg"
        >
          <Pencil className="size-3" />
        </button>
      )}
    </>
  );
}
