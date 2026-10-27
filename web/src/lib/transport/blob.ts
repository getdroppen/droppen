/*
 * File bytes in cloud mode: host files, pictures, GitHub images, uploads.
 *
 * On the direct transport the browser fetches those itself — an `<img src>`,
 * an `<a href download>` with a ticket in the URL. In cloud mode the page's
 * origin is the hub, which must never see a file, so the browser has no URL
 * to give them: every byte comes over the machine channel's `blob` lane
 * (`tunnel.ts`), lowest priority, and is drawn or saved from a `blob:` URL
 * made here in the page.
 *
 * Bounds, checked here before and while the bytes arrive, so a mistake costs
 * an error message rather than the tab's memory: a picture at most
 * {@link BLOB_LIMITS.image}, a download {@link BLOB_LIMITS.download}, an
 * upload {@link BLOB_LIMITS.upload} (what the agent buffers of a request).
 */
import { formatBytes } from "@/lib/format";
import type { MachineKey } from "@/lib/machines/key";
import { BLOB_UPLOAD_MAX, BodyTooLargeError } from "./tunnel";
import type { Transport } from "./types";

const MiB = 1024 * 1024;

export const BLOB_LIMITS = {
  image: 25 * MiB,
  download: 256 * MiB,
  upload: BLOB_UPLOAD_MAX,
} as const;

export type BlobErrorKind =
  /** Larger than the bound for what it is. */
  | "tooLarge"
  /** The machine (or the offline copy standing in for it) answered with an error status. */
  | "status"
  /** No answer at all: the channel dropped, or the request was refused before it went. */
  | "network";

export class BlobError extends Error {
  constructor(
    readonly kind: BlobErrorKind,
    message: string,
    /** The HTTP status, for `status`. */
    readonly status: number | null = null,
    /** The bound that was passed, for `tooLarge`. */
    readonly limit: number | null = null,
  ) {
    super(message);
    this.name = "BlobError";
  }
}

/** Where a path is put when a `Request` needs an absolute URL; the tunnel only reads its path and query. */
function absolute(path: string): string {
  const origin = (globalThis as { location?: { origin?: string } }).location?.origin;
  return new URL(path, origin && origin !== "null" ? origin : "http://machine.invalid").toString();
}

/**
 * The sentence an error answer carries: the server's own `hint` when it wrote
 * one (the offline copy's `machine_offline` does), else its `error`.
 */
async function errorSentence(res: Response): Promise<string> {
  try {
    const body = (await res.json()) as { error?: unknown; hint?: unknown };
    if (typeof body.hint === "string" && body.hint) return body.hint;
    if (typeof body.error === "string" && body.error) return body.error;
  } catch {
    // not JSON
  }
  return res.statusText || `HTTP ${res.status}`;
}

/** The file name a `Content-Disposition` header gives, if any (`filename*=UTF-8''…` first). */
export function dispositionName(header: string | null): string | null {
  if (!header) return null;
  const star = /filename\*\s*=\s*([^']*)'[^']*'([^;]+)/i.exec(header);
  if (star) {
    try {
      return decodeURIComponent(star[2]!.trim().replace(/^"|"$/g, "")) || null;
    } catch {
      // fall through to the plain one
    }
  }
  const plain = /filename\s*=\s*("((?:[^"\\]|\\.)*)"|[^;]+)/i.exec(header);
  if (!plain) return null;
  const name = plain[2] !== undefined ? plain[2].replace(/\\(.)/g, "$1") : plain[1]!.trim();
  return name || null;
}

export interface FetchBlobOptions {
  /** The most bytes accepted; past it the transfer is abandoned. */
  limit: number;
  signal?: AbortSignal;
  /** Bytes in so far, and the whole if the answer said (`content-length`). */
  onProgress?: (received: number, total: number | null) => void;
  /** The transport of the machine the file is on. */
  via: Transport;
}

export interface FetchedBlob {
  blob: Blob;
  /** The name the answer's `Content-Disposition` gives, if any. */
  name: string | null;
}

/**
 * `GET path` on the `blob` lane, read whole into a `Blob` — or a
 * {@link BlobError} saying why not. An abort through `signal` rejects with the
 * abort's own reason, as `fetch` does.
 */
export async function fetchBlob(path: string, opts: FetchBlobOptions): Promise<FetchedBlob> {
  const via = opts.via;
  let res: Response;
  try {
    res = await via.fetch(new Request(absolute(path), { signal: opts.signal }), { class: "blob" });
  } catch (error) {
    if (opts.signal?.aborted) throw error;
    throw new BlobError("network", error instanceof Error ? error.message : String(error));
  }
  if (!res.ok) {
    const sentence = await errorSentence(res);
    throw new BlobError("status", sentence, res.status);
  }
  const declared = Number(res.headers.get("content-length"));
  const total = Number.isFinite(declared) && declared >= 0 && res.headers.has("content-length") ? declared : null;
  if (total !== null && total > opts.limit) {
    void res.body?.cancel().catch(() => undefined);
    throw new BlobError("tooLarge", `larger than ${formatBytes(opts.limit, true)}`, null, opts.limit);
  }
  const type = res.headers.get("content-type") ?? "";
  const name = dispositionName(res.headers.get("content-disposition"));
  const chunks: Uint8Array[] = [];
  let received = 0;
  opts.onProgress?.(0, total);
  if (res.body) {
    const reader = res.body.getReader();
    for (;;) {
      let step: ReadableStreamReadResult<Uint8Array>;
      try {
        step = await reader.read();
      } catch (error) {
        if (opts.signal?.aborted) throw error;
        throw new BlobError("network", error instanceof Error ? error.message : String(error));
      }
      if (step.done) break;
      received += step.value.byteLength;
      if (received > opts.limit) {
        void reader.cancel().catch(() => undefined);
        throw new BlobError("tooLarge", `larger than ${formatBytes(opts.limit, true)}`, null, opts.limit);
      }
      chunks.push(step.value);
      opts.onProgress?.(received, total);
    }
  }
  return { blob: new Blob(chunks as BlobPart[], type ? { type } : undefined), name };
}

/**
 * Send an upload (`POST /api/fs/upload`) on the `blob` lane, refusing one of
 * `size` bytes past {@link BLOB_LIMITS.upload} before anything is sent when
 * the page is tunneled. The direct transport sends it as it always did.
 */
export async function sendUpload(request: Request, size: number, via: Transport): Promise<Response> {
  if (via.tunneled && size > BLOB_LIMITS.upload) {
    throw new BlobError("tooLarge", `larger than ${formatBytes(BLOB_LIMITS.upload, true)}`, null, BLOB_LIMITS.upload);
  }
  try {
    return await via.fetch(request, { class: "blob" });
  } catch (error) {
    if (error instanceof BodyTooLargeError) throw new BlobError("tooLarge", `larger than ${formatBytes(error.limit, true)}`, null, error.limit);
    throw error;
  }
}

// ---------------------------------------------------------------------------
// The pictures already fetched
// ---------------------------------------------------------------------------

/**
 * The last few blobs fetched, by URL, least recently used dropped first —
 * so a picture scrolled out of the transcript and back, or opened full size
 * from its thumbnail, or saved from the overlay, is not fetched again. Bounded
 * by count and by bytes; a blob larger than the byte bound is handed out but
 * not kept. Loads of one URL already on their way are shared.
 */
export class BlobCache {
  private readonly entries = new Map<string, Blob>();
  private readonly loading = new Map<string, Promise<Blob>>();
  private bytes = 0;

  constructor(
    readonly maxEntries = 32,
    readonly maxBytes = 96 * MiB,
  ) {}

  /** The blob for `key`, now, if it is held (and it becomes the most recently used). */
  get(key: string): Blob | undefined {
    const blob = this.entries.get(key);
    if (blob) {
      this.entries.delete(key);
      this.entries.set(key, blob);
    }
    return blob;
  }

  put(key: string, blob: Blob): void {
    const old = this.entries.get(key);
    if (old) {
      this.entries.delete(key);
      this.bytes -= old.size;
    }
    if (blob.size > this.maxBytes) return;
    this.entries.set(key, blob);
    this.bytes += blob.size;
    for (const [k, b] of this.entries) {
      if (this.entries.size <= this.maxEntries && this.bytes <= this.maxBytes) break;
      this.entries.delete(k);
      this.bytes -= b.size;
    }
  }

  /** The blob for `key`: held, on its way, or loaded now with `load`. A failure is not kept. */
  load(key: string, load: () => Promise<Blob>): Promise<Blob> {
    const held = this.get(key);
    if (held) return Promise.resolve(held);
    let pending = this.loading.get(key);
    if (!pending) {
      pending = load().then(
        (blob) => {
          this.loading.delete(key);
          this.put(key, blob);
          return blob;
        },
        (error: unknown) => {
          this.loading.delete(key);
          throw error;
        },
      );
      this.loading.set(key, pending);
    }
    return pending;
  }

  get size(): number {
    return this.entries.size;
  }

  get totalBytes(): number {
    return this.bytes;
  }

  clear(): void {
    this.entries.clear();
    this.bytes = 0;
  }
}

/** The page's pictures from its machines, by {@link hostBlobKey}. */
export const hostBlobs = new BlobCache();

/** What a picture is kept under in {@link hostBlobs}: its machine and its `/api/fs/download` route. */
export function hostBlobKey(machine: MachineKey, route: string): string {
  return `${machine}|${route}`;
}
