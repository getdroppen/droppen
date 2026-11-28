/**
 * Parent-side bridge for a sandboxed cloud plugin iframe (`/plugin-frame.html`,
 * `docs/plugins/cloud.md` §3–§6).
 *
 * Pure transport/protocol logic: no React, no Vite virtual modules, and no
 * direct hub/e2e imports — every call crosses `transportFor(machine)` with
 * `X-Rh-Plugin: <pluginId>` stamped by the parent so the target machine's
 * server can enforce `enabled && cloud` on every request.
 */
import { PluginStorageError, type Doc, type DocMeta, type DocWrite } from "@droppen/plugin-sdk";
import type { MachineKey } from "@/lib/machines/key";
import { perMachine, transportFor } from "@/lib/machines/registry";
import type {
  FramePackageData,
  HttpRequestParams,
  HttpResponseResult,
  SerializedError,
} from "./frame/protocol";
import { storageFailure } from "./host";

/** Structural subset of a machine transport needed by the cloud plugin bridge. */
export interface FetchTransport {
  fetch(request: Request): Promise<Response>;
}

/** Headers a plugin inside a cloud frame may never forge on relative `/api/...` calls. */
const STRIPPED_REQUEST_HEADERS = new Set([
  "authorization",
  "cookie",
  "host",
  "x-rh-agent",
  "x-rh-agent-device",
  "x-rh-plugin",
]);

/** Synthesise a base URL so relative `/api/...` paths can become `Request` objects for `Transport.fetch`. */
const MACHINE_BASE = "http://droppen.machine";

/** Encode bytes into base64 without blowing the call stack on multi-megabyte payloads. */
export function bytesToBase64(bytes: Uint8Array): string {
  if (bytes.byteLength === 0) return "";
  const chunkSize = 0x8000;
  let binary = "";
  for (let i = 0; i < bytes.byteLength; i += chunkSize) {
    const slice = bytes.subarray(i, Math.min(i + chunkSize, bytes.byteLength));
    binary += String.fromCharCode(...slice);
  }
  return btoa(binary);
}

/** Decode base64 into an `ArrayBuffer`. */
export function base64ToArrayBuffer(b64: string): ArrayBuffer {
  if (!b64) return new ArrayBuffer(0);
  const binary = atob(b64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes.buffer;
}

/** Serialize an Error or `PluginStorageError` so the frame can reconstruct its exact type. */
export function serializeError(error: unknown): SerializedError {
  if (error instanceof PluginStorageError) {
    return {
      kind: "storage",
      code: error.code,
      message: error.message,
      ...(error.code === "conflict" ? { doc: error.doc, current: error.current } : {}),
      ...(error.code === "too_large" ? { limit: error.limit } : {}),
    };
  }
  return {
    kind: "error",
    message: error instanceof Error ? error.message : String(error),
  };
}

/** Build a `Request` aimed at `path` on the target machine, stamped with `X-Rh-Plugin`. */
function machineRequest(
  path: string,
  pluginId: string,
  init: RequestInit = {},
): Request {
  const url = new URL(path, MACHINE_BASE).href;
  const headers = new Headers(init.headers);
  headers.set("X-Rh-Plugin", pluginId);
  return new Request(url, { ...init, headers });
}

/** Cache downloaded packages per `(machine, id@build)`; cleared when the machine closes. */
const packagesOf = perMachine(() => new Map<string, Promise<FramePackageData>>());

/**
 * Download a cloud-enabled plugin's package bundle (`GET /api/plugins/{id}/package`)
 * over the machine's encrypted channel, cached per `(machine, id, build)`.
 */
export function fetchPluginPackage(
  machine: MachineKey,
  pluginId: string,
  build: string,
  transport: FetchTransport = transportFor(machine),
): Promise<FramePackageData> {
  const cache = packagesOf(machine);
  const key = `${pluginId}@${build}`;
  let pending = cache.get(key);
  if (!pending) {
    pending = (async () => {
      const req = machineRequest(`/api/plugins/${encodeURIComponent(pluginId)}/package`, pluginId, {
        method: "GET",
      });
      const res = await transport.fetch(req);
      if (!res.ok) {
        let message = `HTTP ${res.status}`;
        try {
          const body = (await res.json()) as { error?: string; message?: string };
          message = body.message ?? body.error ?? message;
        } catch {
          // keep status text
        }
        throw new Error(message);
      }
      const data = (await res.json()) as FramePackageData;
      if (!data || data.id !== pluginId || !Array.isArray(data.files)) {
        throw new Error(`invalid plugin package response for ${pluginId}`);
      }
      return data;
    })().catch((error) => {
      cache.delete(key);
      throw error;
    });
    cache.set(key, pending);
  }
  return pending;
}

/**
 * Forward one `fetch` / `XMLHttpRequest` call from a sandboxed cloud plugin frame
 * to the target machine:
 *
 * - Relative paths (`/api/...`, not protocol-relative `//...`) go directly to the
 *   machine's HTTP API through its E2E transport, with sensitive headers stripped
 *   and `X-Rh-Plugin: <pluginId>` stamped.
 * - External `http://` / `https://` URLs go to `POST /api/plugins/{id}/fetch` on
 *   the machine, where the machine executes the HTTP request itself (no CORS,
 *   LAN/VPN reachable) and returns the response through the E2E channel.
 * - All other URL schemes are refused.
 */
export async function forwardFrameHttp(
  machine: MachineKey,
  pluginId: string,
  params: HttpRequestParams,
  signal?: AbortSignal,
  transport: FetchTransport = transportFor(machine),
): Promise<HttpResponseResult> {
  const rawUrl = (params.url ?? "").trim();
  const method = (params.method || "GET").toUpperCase();

  // 1. Relative path against the machine's own API.
  if (rawUrl.startsWith("/") && !rawUrl.startsWith("//")) {
    const headers = new Headers();
    for (const [name, value] of params.headers ?? []) {
      if (STRIPPED_REQUEST_HEADERS.has(name.toLowerCase())) continue;
      try {
        headers.append(name, value);
      } catch {
        // Ignore invalid header names/values from plugin code.
      }
    }
    headers.set("X-Rh-Plugin", pluginId);
    const hasBody = method !== "GET" && method !== "HEAD" && params.body !== null;
    const req = new Request(new URL(rawUrl, MACHINE_BASE).href, {
      method,
      headers,
      body: hasBody ? params.body : undefined,
      signal,
    });
    const res = await transport.fetch(req);
    const body = await res.arrayBuffer();
    const outHeaders: [string, string][] = [];
    res.headers.forEach((value, name) => outHeaders.push([name, value]));
    return {
      status: res.status,
      statusText: res.statusText,
      url: rawUrl,
      headers: outHeaders,
      body: body.byteLength > 0 ? body : null,
    };
  }

  // 2. External http(s):// URL — proxy via the target machine's `POST /api/plugins/{id}/fetch`.
  let parsed: URL;
  try {
    parsed = new URL(rawUrl);
  } catch {
    throw new Error(`unsupported URL: ${rawUrl}`);
  }
  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
    throw new Error(`refusing non-http(s) URL: ${parsed.protocol}`);
  }

  const proxyHeaders = (params.headers ?? []).map(([name, value]) => ({ name, value }));
  const bodyBase64 =
    params.body && params.body.byteLength > 0 && method !== "GET" && method !== "HEAD"
      ? bytesToBase64(new Uint8Array(params.body))
      : null;

  const proxyReq = machineRequest(
    `/api/plugins/${encodeURIComponent(pluginId)}/fetch`,
    pluginId,
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        url: parsed.href,
        method,
        headers: proxyHeaders,
        bodyBase64,
      }),
      signal,
    },
  );
  const proxyRes = await transport.fetch(proxyReq);
  if (!proxyRes.ok) {
    let message = `HTTP ${proxyRes.status}`;
    try {
      const err = (await proxyRes.json()) as { error?: string; message?: string };
      message = err.message ?? err.error ?? message;
    } catch {
      // keep status text
    }
    throw new Error(message);
  }
  const payload = (await proxyRes.json()) as {
    status: number;
    statusText: string;
    url: string;
    headers: { name: string; value: string }[];
    bodyBase64: string;
  };
  const respBody = payload.bodyBase64 ? base64ToArrayBuffer(payload.bodyBase64) : null;
  return {
    status: payload.status,
    statusText: payload.statusText ?? "",
    url: payload.url || parsed.href,
    headers: (payload.headers ?? []).map((h) => [h.name, h.value]),
    body: respBody,
  };
}

/**
 * Execute one `host.storage` RPC (`storage:get|list|put|batch`) for a cloud
 * plugin against its machine, stamping `X-Rh-Plugin: <pluginId>` and turning
 * server failures into `PluginStorageError`.
 */
export async function handleFrameStorage(
  machine: MachineKey,
  pluginId: string,
  method: "storage:get" | "storage:list" | "storage:put" | "storage:batch",
  params: Record<string, unknown>,
  transport: FetchTransport = transportFor(machine),
): Promise<unknown> {
  const send = async (req: Request, allowNotFound = false): Promise<unknown> => {
    let res: Response;
    try {
      res = await transport.fetch(req);
    } catch (error) {
      throw new PluginStorageError({
        code: "offline",
        message: error instanceof Error ? error.message : String(error),
      });
    }
    let body: unknown = undefined;
    try {
      body = await res.json();
    } catch {
      body = undefined;
    }
    if (res.ok) return body;
    if (allowNotFound && res.status === 404 && (body as { error?: string } | undefined)?.error === "doc_not_found") {
      return null;
    }
    throw new PluginStorageError(storageFailure(pluginId, { response: res, error: body }));
  };

  const encId = encodeURIComponent(pluginId);
  switch (method) {
    case "storage:get": {
      const name = String(params.name ?? "");
      const req = machineRequest(`/api/plugins/${encId}/docs/${encodeURIComponent(name)}`, pluginId, { method: "GET" });
      return (await send(req, true)) as Doc<unknown> | null;
    }
    case "storage:list": {
      const prefix = String(params.prefix ?? "");
      const qs = prefix ? `?prefix=${encodeURIComponent(prefix)}` : "";
      const req = machineRequest(`/api/plugins/${encId}/docs${qs}`, pluginId, { method: "GET" });
      const data = (await send(req)) as { docs?: DocMeta[] } | null;
      return data?.docs ?? [];
    }
    case "storage:put": {
      const name = String(params.name ?? "");
      const req = machineRequest(`/api/plugins/${encId}/docs/${encodeURIComponent(name)}`, pluginId, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          expectedRevision: (params.expectedRevision as number | null | undefined) ?? null,
          value: params.value,
        }),
      });
      return (await send(req)) as Doc<unknown>;
    }
    case "storage:batch": {
      const writes = (params.writes as DocWrite[] | undefined) ?? [];
      const req = machineRequest(`/api/plugins/${encId}/batch`, pluginId, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ writes }),
      });
      const data = (await send(req)) as { docs?: Doc<unknown>[] } | null;
      return data?.docs ?? [];
    }
  }
}

/**
 * Snapshot the host document's theme mode (`light` / `dark`) and design-token
 * CSS custom properties (`--color-*`, `--font-*`) to pass into a sandboxed frame.
 */
export function readHostTheme(): { themeMode: "light" | "dark"; cssVars: Record<string, string> } {
  if (typeof document === "undefined") {
    return { themeMode: "light", cssVars: {} };
  }
  const root = document.documentElement;
  const themeMode: "light" | "dark" = root.classList.contains("dark") ? "dark" : "light";
  const computed = getComputedStyle(root);
  const cssVars: Record<string, string> = {};
  const tokenNames = [
    "--color-bg",
    "--color-panel",
    "--color-surface",
    "--color-hover",
    "--color-fg",
    "--color-muted",
    "--color-faint",
    "--color-border",
    "--color-border-strong",
    "--color-accent",
    "--color-danger",
    "--color-warn",
    "--color-warn-fg",
    "--color-ok",
    "--font-sans",
    "--font-mono",
  ];
  for (const name of tokenNames) {
    const value = computed.getPropertyValue(name).trim();
    if (value) cssVars[name] = value;
  }
  // Also copy any custom `--color-*` / `--font-*` defined directly on stylesheets.
  for (let i = 0; i < computed.length; i++) {
    const name = computed[i];
    if (name && (name.startsWith("--color-") || name.startsWith("--font-"))) {
      const value = computed.getPropertyValue(name).trim();
      if (value) cssVars[name] = value;
    }
  }
  return { themeMode, cssVars };
}
