import { useMediaQuery } from "usehooks-ts";

/**
 * Whether the reader asked the system for less motion.
 *
 * The app's only motion is the 1 Hz liveness blink ([`LiveDot`] and the
 * working pane's glow), and under this setting both hold still in their
 * colour: the state stays readable, nothing moves.
 */
const QUERY = "(prefers-reduced-motion: reduce)";

export function usePrefersReducedMotion(): boolean {
  return useMediaQuery(QUERY, { defaultValue: false });
}
