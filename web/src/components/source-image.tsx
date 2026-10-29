/**
 * The pieces a picture needs when it does not come from the host's disk: a
 * private GitHub file fetched through the server's `gh` proxy, the object URL
 * that draws it, and the card that stands in for any picture that cannot be
 * drawn at all. `components/image-lightbox.tsx` puts them together.
 *
 * The proxy is called with the typed client rather than put in an `<img src>`:
 * an `<img>` cannot send the `Authorization` header, so a URL would need a
 * download ticket — and `/api/fs/download` is deliberately the only plain-GET
 * route that takes one (`server-rs/crates/droppen-auth/src/lib.rs`, `ticket_offered`). A fetch
 * also gets to read *why* it failed, which is what lets the fallback card say
 * "gh is not logged in on the server" instead of showing a broken glyph.
 */
import { useQuery } from "@tanstack/react-query";
import type { TFunction } from "i18next";
import { ImageOff } from "lucide-react";
import { useEffect, useReducer } from "react";
import { useTranslation } from "react-i18next";
import { blobErrorText } from "@/components/blob-transfers";
import { cn } from "@/lib/cn";
import type { Transport } from "@/lib/transport";
import { BLOB_LIMITS, BlobError, fetchBlob } from "@/lib/transport/blob";
import { useMachineRuntime } from "@/lib/machines/context";
import type { MachineKey } from "@/lib/machines/key";
import { mq } from "@/lib/machines/query";

/** A proxy refusal, with the status it came with. */
export class MediaFetchError extends Error {
  readonly status: number;
  /** In cloud mode, the channel's own account of it. */
  readonly blobError: BlobError | null;

  constructor(message: string, status: number, blobError: BlobError | null = null) {
    super(message);
    this.name = "MediaFetchError";
    this.status = status;
    this.blobError = blobError;
  }
}

/** Why a GitHub picture is not drawn, in the page's language where it is ours to say. */
export function mediaErrorText(error: Error, t: TFunction): string {
  return error instanceof MediaFetchError && error.blobError ? blobErrorText(error.blobError, t) : error.message;
}

/**
 * Cloud mode: the proxy over the machine channel's `blob` lane, since the
 * page's origin is the hub and the typed client's lane is for small answers.
 */
export async function githubThroughChannel(url: string, signal: AbortSignal, via: Transport): Promise<Blob> {
  try {
    return (await fetchBlob(`/api/media/github?url=${encodeURIComponent(url)}`, { via, limit: BLOB_LIMITS.image, signal })).blob;
  } catch (error) {
    if (error instanceof BlobError) throw new MediaFetchError(error.message, error.status ?? 0, error);
    throw error;
  }
}

/** The query key a GitHub image is cached under on `machine`, shared with the tests. */
export function githubImageKey(machine: MachineKey, url: string) {
  return mq(machine, "media-github", url);
}

/**
 * The bytes of a GitHub image, through `GET /api/media/github`.
 *
 * Never stale while the page is open: the thumbnail and the full-size overlay
 * share one fetch, and a picture scrolled out of the virtualised transcript
 * and back in within five minutes costs nothing. One retry, only for the two
 * statuses that can be transient (`gh` or GitHub failing, or `gh` timing out);
 * a 404 is the same 404 the second time, and each one is up to four `gh` runs.
 */
export function useGithubImage(url: string) {
  const runtime = useMachineRuntime();
  const machine = runtime.key;
  const api = runtime.api;
  return useQuery({
    queryKey: githubImageKey(machine, url),
    queryFn: async ({ signal }) => {
      if (runtime.transport.tunneled) return githubThroughChannel(url, signal, runtime.transport);
      const res = await api.GET("/api/media/github", {
        params: { query: { url } },
        parseAs: "blob",
        signal,
      });
      if (res.error || !res.data) {
        throw new MediaFetchError(res.error?.error ?? "image could not be loaded", res.response.status);
      }
      return res.data;
    },
    staleTime: Infinity,
    gcTime: 5 * 60_000,
    retry: (failures, error) =>
      failures < 1 && error instanceof MediaFetchError && (error.status === 502 || error.status === 504),
  });
}

/** How long an object URL outlives its last user. See {@link useObjectUrl}. */
const RELEASE_DELAY_MS = 1000;

/** One object URL per blob, shared by every picture drawing it. */
const objectUrls = new Map<Blob, { url: string; users: number }>();

function objectUrlFor(blob: Blob) {
  let entry = objectUrls.get(blob);
  if (!entry) {
    entry = { url: URL.createObjectURL(blob), users: 0 };
    objectUrls.set(blob, entry);
  }
  return entry;
}

/**
 * A `blob:` URL for a blob, revoked once nothing on the page draws it.
 *
 * Built during render so the first paint already has its picture, and counted
 * in an effect so it can be released. The release waits a moment instead of
 * revoking on the spot: when one copy of a picture unmounts in the same commit
 * as another mounts — a transcript row re-keyed, the thumbnail and overlay
 * trading places — the cleanup runs first, and revoking there would hand the
 * newcomer a dead URL. If that happens anyway the effect notices and redraws.
 */
export function useObjectUrl(blob: Blob | undefined): string | undefined {
  const [, redraw] = useReducer((n: number) => n + 1, 0);
  const url = blob ? objectUrlFor(blob).url : undefined;
  useEffect(() => {
    if (!blob) return;
    const entry = objectUrlFor(blob);
    entry.users += 1;
    if (entry.url !== url) redraw();
    return () => {
      entry.users -= 1;
      setTimeout(() => {
        if (entry.users > 0 || objectUrls.get(blob) !== entry) return;
        objectUrls.delete(blob);
        URL.revokeObjectURL(entry.url);
      }, RELEASE_DELAY_MS);
    };
  }, [blob, url]);
  return url;
}

/**
 * What stands in for a picture that cannot be drawn: the agent's alt text, a
 * short reason when there is one, and the link itself so the user can try it
 * in a browser that may be logged in where this one is not.
 *
 * All phrasing content, because markdown puts images inside a `<p>`.
 */
export function ImageFallback({
  alt,
  href,
  reason,
  className,
}: {
  alt: string;
  /** Null inside a link, where a second anchor would be nested in the first. */
  href?: string | null;
  reason?: string | null;
  className?: string;
}) {
  const { t } = useTranslation();
  return (
    <span
      data-image-fallback=""
      title={reason ?? undefined}
      className={cn(
        "inline-flex max-w-full items-center gap-1.5 rounded border border-border bg-raised px-2 py-1 align-middle text-2xs text-muted",
        className,
      )}
    >
      <ImageOff aria-hidden className="size-3.5 shrink-0 text-faint" />
      <span className="min-w-0 truncate">{alt || t("image")}</span>
      {reason && <span className="min-w-0 truncate text-faint">{reason}</span>}
      {href && (
        <a href={href} target="_blank" rel="noreferrer" className="shrink-0 text-accent hover:underline">
          {t("open link")}
        </a>
      )}
    </span>
  );
}

/**
 * Where a picture will be while its bytes are still on the way. Still, not
 * pulsing: see the fence in `test/live-dot.test.tsx` for why nothing here does.
 */
export function ImagePending({ alt }: { alt: string }) {
  return (
    <span
      role="img"
      aria-label={alt}
      aria-busy="true"
      className="inline-block h-24 w-40 max-w-full rounded border border-border bg-raised align-top"
    />
  );
}
