/**
 * Request shims (`fetch` and `XMLHttpRequest`) installed inside a sandboxed
 * cloud plugin frame before the plugin's code runs (`docs/plugins/cloud.md` §4).
 *
 * The frame has `connect-src 'none'`. Every `fetch` / `XMLHttpRequest` goes
 * over the frame's MessagePort to the parent page, which forwards `/api/…` to
 * the plugin's machine over the end-to-end channel and external `http(s)://`
 * URLs to `POST /api/plugins/{id}/fetch` on that machine. Local `blob:` and
 * `data:` URLs continue to use the browser's native `fetch`.
 */
import type { HttpRequestParams, HttpResponseResult } from "./protocol";

export type PortHttpTransport = (
  params: HttpRequestParams,
  signal?: AbortSignal | null,
) => Promise<HttpResponseResult>;

const NULL_BODY_STATUS = new Set([101, 204, 205, 304]);

function isLocalScheme(url: string): boolean {
  return url.startsWith("blob:") || url.startsWith("data:");
}

function normalizeUrl(raw: string): string {
  if (/^https?:\/\//i.test(raw)) return raw;
  if (raw.startsWith("//")) return `https:${raw}`;
  // Relative URL against a virtual base so `./api/x`, `/api/x`, `?a=1` all resolve cleanly.
  const resolved = new URL(raw, "http://droppen.plugin.local/");
  if (resolved.origin === "http://droppen.plugin.local") {
    return `${resolved.pathname}${resolved.search}${resolved.hash}`;
  }
  return resolved.href;
}

function collectHeaders(initHeaders?: HeadersInit): [string, string][] {
  const out: [string, string][] = [];
  if (!initHeaders) return out;
  const h = new Headers(initHeaders);
  h.forEach((value, key) => {
    out.push([key, value]);
  });
  return out;
}

async function bodyToBuffer(body: BodyInit | null | undefined, headers: [string, string][]): Promise<ArrayBuffer | null> {
  if (body === null || body === undefined) return null;
  if (typeof body === "string") {
    const encoded = new TextEncoder().encode(body);
    return encoded.buffer.slice(encoded.byteOffset, encoded.byteOffset + encoded.byteLength);
  }
  if (body instanceof ArrayBuffer) return body.slice(0);
  if (ArrayBuffer.isView(body)) {
    return body.buffer.slice(body.byteOffset, body.byteOffset + body.byteLength) as ArrayBuffer;
  }
  if (body instanceof Blob) {
    if (body.type && !headers.some(([k]) => k.toLowerCase() === "content-type")) {
      headers.push(["content-type", body.type]);
    }
    return await body.arrayBuffer();
  }
  if (body instanceof URLSearchParams) {
    if (!headers.some(([k]) => k.toLowerCase() === "content-type")) {
      headers.push(["content-type", "application/x-www-form-urlencoded;charset=UTF-8"]);
    }
    const encoded = new TextEncoder().encode(body.toString());
    return encoded.buffer.slice(encoded.byteOffset, encoded.byteOffset + encoded.byteLength);
  }
  // FormData / ReadableStream: serialize through a temporary Request.
  const temp = new Request("http://droppen.plugin.local/", { method: "POST", body });
  const ct = temp.headers.get("content-type");
  if (ct && !headers.some(([k]) => k.toLowerCase() === "content-type")) {
    headers.push(["content-type", ct]);
  }
  return await temp.arrayBuffer();
}

export function createShimmedFetch(transport: PortHttpTransport, nativeFetch: typeof fetch = globalThis.fetch.bind(globalThis)): typeof fetch {
  return async function shimmedFetch(input: RequestInfo | URL, init?: RequestInit): Promise<Response> {
    const rawUrl =
      typeof input === "string"
        ? input
        : input instanceof URL
          ? input.href
          : input.url;

    if (isLocalScheme(rawUrl)) {
      return nativeFetch(input, init);
    }

    let method = init?.method;
    let headers: [string, string][];
    let bodyBuffer: ArrayBuffer | null = null;
    const signal = init?.signal ?? (input instanceof Request ? input.signal : undefined);

    if (input instanceof Request) {
      method ??= input.method;
      headers = collectHeaders(init?.headers ?? input.headers);
      if (init?.body !== undefined) {
        bodyBuffer = await bodyToBuffer(init.body, headers);
      } else if (method.toUpperCase() !== "GET" && method.toUpperCase() !== "HEAD") {
        const buf = await input.clone().arrayBuffer();
        bodyBuffer = buf.byteLength > 0 ? buf : null;
      }
    } else {
      method ??= "GET";
      headers = collectHeaders(init?.headers);
      bodyBuffer = await bodyToBuffer(init?.body, headers);
    }

    const res = await transport(
      {
        url: normalizeUrl(rawUrl),
        method: method.toUpperCase(),
        headers,
        body: bodyBuffer,
      },
      signal,
    );

    const responseHeaders = new Headers();
    for (const [name, value] of res.headers) {
      try {
        responseHeaders.append(name, value);
      } catch {
        // Ignore headers rejected by the Fetch Headers constructor.
      }
    }
    const responseBody = NULL_BODY_STATUS.has(res.status) ? null : res.body;
    const response = new Response(responseBody, {
      status: res.status,
      statusText: res.statusText,
      headers: responseHeaders,
    });
    if (res.url) {
      try {
        Object.defineProperty(response, "url", { value: res.url });
      } catch {
        // Read-only in some environments; non-fatal.
      }
    }
    return response;
  };
}

export function createShimmedXHR(transport: PortHttpTransport): typeof XMLHttpRequest {
  class ShimmedXMLHttpRequest extends EventTarget {
    static readonly UNSENT = 0;
    static readonly OPENED = 1;
    static readonly HEADERS_RECEIVED = 2;
    static readonly LOADING = 3;
    static readonly DONE = 4;

    readonly UNSENT = 0;
    readonly OPENED = 1;
    readonly HEADERS_RECEIVED = 2;
    readonly LOADING = 3;
    readonly DONE = 4;

    readyState = 0;
    status = 0;
    statusText = "";
    responseURL = "";
    responseType: XMLHttpRequestResponseType = "";
    response: unknown = "";
    responseText = "";
    responseXML: Document | null = null;
    timeout = 0;
    withCredentials = false;
    upload = new EventTarget();

    onreadystatechange: ((this: XMLHttpRequest, ev: Event) => unknown) | null = null;
    onloadstart: ((this: XMLHttpRequest, ev: ProgressEvent) => unknown) | null = null;
    onprogress: ((this: XMLHttpRequest, ev: ProgressEvent) => unknown) | null = null;
    onabort: ((this: XMLHttpRequest, ev: ProgressEvent) => unknown) | null = null;
    onerror: ((this: XMLHttpRequest, ev: ProgressEvent) => unknown) | null = null;
    onload: ((this: XMLHttpRequest, ev: ProgressEvent) => unknown) | null = null;
    ontimeout: ((this: XMLHttpRequest, ev: ProgressEvent) => unknown) | null = null;
    onloadend: ((this: XMLHttpRequest, ev: ProgressEvent) => unknown) | null = null;

    private method = "GET";
    private url = "";
    private reqHeaders: [string, string][] = [];
    private resHeaders: [string, string][] = [];
    private controller: AbortController | null = null;

    open(method: string, url: string | URL, async = true): void {
      if (async === false) {
        throw new Error("synchronous XMLHttpRequest is not supported in cloud plugin frames");
      }
      this.method = method.toUpperCase();
      this.url = normalizeUrl(typeof url === "string" ? url : url.href);
      this.reqHeaders = [];
      this.resHeaders = [];
      this.status = 0;
      this.statusText = "";
      this.responseURL = "";
      this.response = "";
      this.responseText = "";
      this.setState(1);
    }

    setRequestHeader(name: string, value: string): void {
      this.reqHeaders.push([name, value]);
    }

    overrideMimeType(_mime: string): void {
      // No-op in shim.
    }

    getResponseHeader(name: string): string | null {
      const lower = name.toLowerCase();
      const matched = this.resHeaders.filter(([k]) => k.toLowerCase() === lower).map(([, v]) => v);
      return matched.length ? matched.join(", ") : null;
    }

    getAllResponseHeaders(): string {
      return this.resHeaders.map(([k, v]) => `${k.toLowerCase()}: ${v}\r\n`).join("");
    }

    abort(): void {
      if (this.controller) {
        this.controller.abort();
        this.controller = null;
      }
    }

    send(body?: Document | XMLHttpRequestBodyInit | null): void {
      const controller = new AbortController();
      this.controller = controller;
      let timer: ReturnType<typeof setTimeout> | undefined;
      let timedOut = false;
      if (this.timeout > 0) {
        timer = setTimeout(() => {
          timedOut = true;
          controller.abort();
        }, this.timeout);
      }
      this.emit("loadstart");

      void (async () => {
        try {
          const payload =
            typeof Document !== "undefined" && body instanceof Document
              ? new TextEncoder().encode(new XMLSerializer().serializeToString(body)).buffer
              : await bodyToBuffer(body as BodyInit | null | undefined, this.reqHeaders);
          const res = await transport(
            {
              url: this.url,
              method: this.method,
              headers: this.reqHeaders,
              body: payload,
            },
            controller.signal,
          );
          if (timer) clearTimeout(timer);
          this.controller = null;
          this.status = res.status;
          this.statusText = res.statusText;
          this.responseURL = res.url || this.url;
          this.resHeaders = res.headers;
          this.setState(2);
          this.setState(3);

          const bytes = res.body ? new Uint8Array(res.body) : new Uint8Array(0);
          const text = new TextDecoder().decode(bytes);
          if (this.responseType === "" || this.responseType === "text") {
            this.responseText = text;
            this.response = text;
          } else if (this.responseType === "json") {
            try {
              this.response = JSON.parse(text);
            } catch {
              this.response = null;
            }
          } else if (this.responseType === "arraybuffer") {
            this.response = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
          } else if (this.responseType === "blob") {
            this.response = new Blob([bytes], { type: this.getResponseHeader("content-type") ?? "" });
          } else {
            this.response = text;
          }
          this.setState(4);
          this.emit("load");
          this.emit("loadend");
        } catch (err) {
          if (timer) clearTimeout(timer);
          this.controller = null;
          this.setState(4);
          if (timedOut) {
            this.emit("timeout");
          } else if (err instanceof DOMException && err.name === "AbortError") {
            this.emit("abort");
          } else {
            this.emit("error");
          }
          this.emit("loadend");
        }
      })();
    }

    private setState(next: number) {
      this.readyState = next;
      const ev = new Event("readystatechange");
      this.onreadystatechange?.call(this as unknown as XMLHttpRequest, ev);
      this.dispatchEvent(ev);
    }

    private emit(name: "loadstart" | "progress" | "abort" | "error" | "load" | "timeout" | "loadend") {
      const ev = typeof ProgressEvent !== "undefined" ? new ProgressEvent(name) : new Event(name);
      const handler = this[`on${name}`] as ((this: XMLHttpRequest, ev: Event) => unknown) | null;
      handler?.call(this as unknown as XMLHttpRequest, ev);
      this.dispatchEvent(ev);
    }
  }

  return ShimmedXMLHttpRequest as unknown as typeof XMLHttpRequest;
}

/** Install the shims onto `globalThis` before any plugin code is imported. */
export function installRequestShims(transport: PortHttpTransport): void {
  const nativeFetch = globalThis.fetch.bind(globalThis);
  globalThis.fetch = createShimmedFetch(transport, nativeFetch);
  globalThis.XMLHttpRequest = createShimmedXHR(transport);
}
