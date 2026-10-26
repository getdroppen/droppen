import createClient, { type Client } from "openapi-fetch";
import type { paths } from "./hub-schema";

/**
 * A typed client for the hub's public API, generated from
 * `openapi/hub.json` (`pnpm --filter @remote-hand/web gen:api:hub`).
 *
 * The page's own fetch is kept underneath: it may be a transport (a machine's
 * tunnel) or a test's in-process hub, and it has always taken a URL string and
 * a plain `RequestInit`. Headers therefore go over as a record, not `Headers`.
 */
export type HubClient = Client<paths>;

export function createHubClient(baseUrl: string, fetchImpl: (input: string, init?: RequestInit) => Promise<Response>): HubClient {
  // A page with no hub marker sends to its own origin: `baseUrl` is empty and
  // the paths stay relative. openapi-fetch builds a `Request`, which needs an
  // absolute URL, so it gets a placeholder and the wrapper hands the path back.
  const relative = baseUrl === "";
  const base = relative ? "http://hub.invalid" : baseUrl;
  return createClient<paths>({
    baseUrl: base,
    fetch: async (request) => {
      const headers: Record<string, string> = {};
      request.headers.forEach((value, name) => {
        headers[name] = value;
      });
      const init: RequestInit = { method: request.method, headers };
      if (request.method !== "GET" && request.method !== "HEAD") {
        // A DELETE has no body; sending `""` makes some in-process hubs parse
        // an empty string as JSON, and an empty body is what the old client
        // sent too.
        const body = await request.text();
        if (body !== "") init.body = body;
      }
      const url = new URL(request.url);
      // The keyring's names span path segments (`mk/m-1/0/…`), while OpenAPI
      // has one `{name}` parameter and openapi-fetch encodes the whole value:
      // put the slashes back for that path.
      if (url.pathname.startsWith("/v1/keys/")) url.pathname = url.pathname.replace(/%2F/gi, "/");
      if (!relative) return fetchImpl(url.toString(), init);
      return fetchImpl(`${url.pathname}${url.search}`, init);
    },
  });
}
