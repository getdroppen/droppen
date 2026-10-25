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
  return createClient<paths>({
    baseUrl,
    fetch: async (request) => {
      const headers: Record<string, string> = {};
      request.headers.forEach((value, name) => {
        headers[name] = value;
      });
      const init: RequestInit = { method: request.method, headers };
      if (request.method !== "GET" && request.method !== "HEAD") init.body = await request.text();
      return fetchImpl(request.url, init);
    },
  });
}
