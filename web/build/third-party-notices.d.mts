// Types for third-party-notices.mjs (plain JavaScript, so that
// scripts/notices/web-notices.mjs can run it with Node alone).
import type { Plugin, Rollup } from "vite";

export const ALLOWED: Set<string>;
export const NOTICES_FILE: string;
export function evaluate(expression: string): { ok: boolean; ids: string[] };

export interface Recorded {
  files: Set<string>;
  banners: Set<string>;
}
export function recorder(): Recorded & { record(bundle: Rollup.OutputBundle): void };

export interface Found {
  entries: { label: string; license: string | null; repository?: string; homepage?: string; texts: { name: string; text: string }[] }[];
  helpers: string[];
  failures: string[];
  unusual: string[];
  withoutText: string[];
  summary: string;
}
export function collect(webDir: string, recorded: Recorded): Found;
export function render(found: Pick<Found, "entries" | "helpers">, intro: string[]): string;
export function failureText(found: Pick<Found, "failures">): string | null;
export function thirdPartyNotices(): Plugin;
