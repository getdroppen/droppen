/**
 * Downloads in cloud mode, while they are on their way.
 *
 * A download on the direct transport is the browser's own: it shows its
 * progress in its download shelf from the first byte. In cloud mode the bytes
 * come over the machine channel into the page first (`lib/transport/blob.ts`)
 * and only reach the browser's shelf once they are all in, so for a large
 * file the page says what it is doing meanwhile — and, when it fails, why
 * (the machine went offline, the file is past what the hub carries).
 */
import type { TFunction } from "i18next";
import { Download, Loader2, X } from "lucide-react";
import { useSyncExternalStore } from "react";
import { useTranslation } from "react-i18next";
import { formatBytes } from "@/lib/format";
import { BlobError } from "@/lib/transport/blob";

/** What a failed transfer says, in the page's language where it is ours to say. */
export function blobErrorText(error: unknown, t: TFunction): string {
  if (error instanceof BlobError) {
    if (error.kind === "tooLarge") return t("larger than {{limit}}: too large to move through the hub", { limit: formatBytes(error.limit ?? 0, true) });
    if (error.kind === "network") return t("the device could not be reached");
    // The machine's own sentence, or the offline copy's (already translated).
    return error.message;
  }
  return error instanceof Error ? error.message : String(error);
}

export interface Transfer {
  id: number;
  name: string;
  /** The machine the file comes from (a page may hold several). */
  machine?: string;
  received: number;
  total: number | null;
  /** Set once it has failed; it is then shown for a while and dropped. */
  error?: unknown;
}

/** How long a failure stays up. */
export const FAILED_SHOWN_MS = 8000;

let transfers: readonly Transfer[] = [];
let nextId = 1;
const watchers = new Set<() => void>();

function publish(next: readonly Transfer[]): void {
  transfers = next;
  for (const watch of [...watchers]) watch();
}

function update(id: number, change: (t: Transfer) => Transfer): void {
  if (!transfers.some((t) => t.id === id)) return;
  publish(transfers.map((t) => (t.id === id ? change(t) : t)));
}

/** A download from `machine` has started. */
export function startTransfer(name: string, machine?: string): number {
  const id = nextId++;
  publish([...transfers, { id, name, received: 0, total: null, ...(machine !== undefined ? { machine } : {}) }]);
  return id;
}

/**
 * Bytes have come in. Published only when the shown figure moves — a whole
 * percent of a known size, or another 256 KiB of an unknown one — since a
 * blob arrives in 16 KiB pieces and a render per piece is thousands.
 */
export function progressTransfer(id: number, received: number, total: number | null): void {
  const held = transfers.find((t) => t.id === id);
  if (!held) return;
  const step = (n: number) => (total ? Math.floor((n / total) * 100) : Math.floor(n / (256 * 1024)));
  if (held.total === total && step(held.received) === step(received) && received !== total) return;
  update(id, (t) => ({ ...t, received, total }));
}

/** It is in the browser's hands now. */
export function endTransfer(id: number): void {
  publish(transfers.filter((t) => t.id !== id));
}

/** It failed: said for {@link FAILED_SHOWN_MS}, then gone. */
export function failTransfer(id: number, error: unknown): void {
  update(id, (t) => ({ ...t, error }));
  const timer = setTimeout(() => endTransfer(id), FAILED_SHOWN_MS);
  // Not a reason for a non-browser host (a test runner) to stay up.
  (timer as { unref?: () => void }).unref?.();
}

export function transfersNow(): readonly Transfer[] {
  return transfers;
}

function subscribe(watch: () => void): () => void {
  watchers.add(watch);
  return () => watchers.delete(watch);
}

export function useTransfers(): readonly Transfer[] {
  return useSyncExternalStore(subscribe, transfersNow, transfersNow);
}

/**
 * The downloads under way, bottom right; nothing when there are none. The
 * list is the page's, whichever machine is in view: `labelOf` names the
 * machine of a download that is not the one in view.
 */
export function BlobTransfers({ labelOf }: { labelOf?: (machine: string | undefined) => string | null }) {
  const list = useTransfers();
  if (list.length === 0) return null;
  return <TransferList transfers={list} labelOf={labelOf} />;
}

/** Exported for its test: the list as given, no store. */
export function TransferList({ transfers: list, labelOf }: { transfers: readonly Transfer[]; labelOf?: (machine: string | undefined) => string | null }) {
  const { t } = useTranslation();
  return (
    <div data-testid="blob-transfers" className="pointer-events-none fixed right-3 bottom-3 z-50 flex max-w-80 flex-col gap-1.5">
      {list.map((item) => {
        const failed = item.error !== undefined;
        const percent = item.total ? Math.min(100, Math.floor((item.received / item.total) * 100)) : null;
        const from = labelOf?.(item.machine) ?? null;
        return (
          <div
            key={item.id}
            role={failed ? "alert" : "status"}
            className="pointer-events-auto flex flex-col gap-1 rounded border border-border bg-panel px-2.5 py-1.5 font-mono text-2xs shadow-lg"
          >
            <div className="flex min-w-0 items-center gap-1.5">
              {failed ? <Download className="size-3 shrink-0 text-danger" /> : <Loader2 className="size-3 shrink-0 animate-spin text-faint" />}
              <span className="min-w-0 flex-1 truncate text-fg" title={item.name}>
                {item.name}
              </span>
              {from && (
                <span data-transfer-machine="" className="max-w-[40%] shrink-0 truncate text-faint" title={from}>
                  {from}
                </span>
              )}
              {!failed && (
                <span className="shrink-0 text-faint">
                  {percent !== null ? `${percent}%` : formatBytes(item.received)}
                </span>
              )}
              {failed && (
                <button type="button" aria-label={t("dismiss")} onClick={() => endTransfer(item.id)} className="shrink-0 text-faint hover:text-fg">
                  <X className="size-3" />
                </button>
              )}
            </div>
            {failed ? (
              <span className="break-words text-danger">{t("could not download: {{reason}}", { reason: blobErrorText(item.error, t) })}</span>
            ) : (
              <span className="h-0.5 w-full overflow-hidden rounded bg-border">
                <span className="block h-full bg-accent" style={{ width: `${percent ?? 0}%` }} />
              </span>
            )}
          </div>
        );
      })}
    </div>
  );
}
