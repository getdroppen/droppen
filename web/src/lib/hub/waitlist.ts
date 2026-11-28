import { createHubClient } from "@/api/hub";
import type { components } from "@/api/hub-schema";
import { HubApiError, type WaitlistPlan } from "./api-views";
import { hubHttpOrigin } from "./config";

/** The minimal hub surface the public pricing waitlist needs — without pulling in `HubApi` or E2E crypto. */
export interface WaitlistApi {
  joinWaitlist(input: { email: string; plan: WaitlistPlan; locale?: string }): Promise<void>;
}

/**
 * Lightweight client for `POST /v1/waitlist` on the public site host (`www.droppen.app`).
 * Keeps `SiteApp` independent of `HubApi` and the end-to-end protocol bundle.
 */
export function createWaitlistApi(baseUrl = hubHttpOrigin() ?? "", fetchImpl: (input: string, init?: RequestInit) => Promise<Response> = (input, init) => fetch(input, init)): WaitlistApi {
  const client = createHubClient(baseUrl, fetchImpl);
  return {
    async joinWaitlist(input) {
      const body: components["schemas"]["Join"] = { email: input.email, plan: input.plan };
      if (input.locale) body.locale = input.locale;
      const out = await client.POST("/v1/waitlist", { body });
      if (out.response.ok || out.response.status === 204) return;
      const d = (out.error && typeof out.error === "object" ? out.error : {}) as {
        error?: string;
        message?: string;
        retry_after_ms?: unknown;
      };
      let retryAfterMs: number | undefined;
      if (typeof d.retry_after_ms === "number" && Number.isFinite(d.retry_after_ms) && d.retry_after_ms >= 0) {
        retryAfterMs = d.retry_after_ms;
      } else {
        const seconds = Number(out.response.headers?.get("retry-after") ?? Number.NaN);
        if (Number.isFinite(seconds) && seconds >= 0) retryAfterMs = seconds * 1000;
      }
      throw new HubApiError(out.response.status, d.error ?? "error", d.message || out.response.statusText || `${d.error ?? "error"} (${out.response.status})`, {
        retryAfterMs,
      });
    },
  };
}
