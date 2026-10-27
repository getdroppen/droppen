/**
 * The server can replace its own binary and restart in place while the page
 * stays open, so the SPA may end up talking to a build it was never loaded
 * against. We only notice, never react: the rule below latches a flag that the
 * shell turns into a "reload when convenient" notice. Reloading is always the
 * user's call — an automatic reload would throw away composer drafts and
 * terminal state.
 */
export interface VersionDriftState {
  /** First non-empty serverVersion seen this page load; the baseline. */
  readonly seen: string | null;
  /** Latched once a different non-empty version arrives; only a reload clears it. */
  readonly drifted: boolean;
}

export const NO_DRIFT: VersionDriftState = { seen: null, drifted: false };

/**
 * The build a `settings` frame says the page is talking to: none for one
 * played from the machine's offline copy (`offline-transport.ts`). That is the
 * build the machine ran when it last wrote its status, any number of updates
 * ago, and a cloud page starts on the copy until the machine's channel opens:
 * taken as the baseline, it made the live build drift on every load, so no
 * reload could clear the banner.
 */
export function liveVersion(
  settings: { serverVersion?: string; fromCopy?: boolean } | null | undefined,
): string | undefined {
  return settings?.fromCopy ? undefined : settings?.serverVersion;
}

/**
 * Folds one observed serverVersion into the drift state. An absent version on
 * either side means "cannot tell" and is ignored, so an older server that does
 * not report a version never trips the banner. Idempotent for a repeated
 * version, which keeps StrictMode's double-invoked effects honest.
 */
export function observeVersion(
  state: VersionDriftState,
  version: string | null | undefined,
): VersionDriftState {
  const next = version?.trim();
  if (!next || state.drifted) return state;
  if (state.seen === null) return { seen: next, drifted: false };
  if (state.seen === next) return state;
  return { seen: state.seen, drifted: true };
}
