/**
 * The re-attach policy for a terminal socket, kept apart from the pane so it
 * can be reasoned about (and tested) without a DOM.
 *
 * Terminals now outlive the server process, so a dropped socket almost always
 * means "the server is restarting", not "this terminal is gone". The pane keeps
 * retrying until the server says otherwise — the one thing it treats as final
 * is the `[terminal not found]` note the attach route sends for an unknown id
 * (`server-rs/src/terminals/routes.rs`).
 */
import type { Terminal } from "@xterm/xterm";

/** First retry lands inside a second; a server restart is usually done by then. */
export const RECONNECT_BASE_MS = 1_000;
/** Ceiling, so a long outage settles into a slow poll instead of a hot loop. */
export const RECONNECT_MAX_MS = 10_000;
/**
 * How long an attach has to hold before the backoff is forgiven. Without this a
 * server that accepts the socket and drops it right back — restarting, or up but
 * refusing — would reset the backoff on every open and hammer it once a second.
 */
export const RECONNECT_STABLE_MS = 3_000;

/**
 * Backoff for the given retry (0 = the first one after a working attach).
 *
 * Jitter only ever shortens the wait — half the capped delay to all of it — so
 * the ceiling stays honest while a screenful of panes that dropped together
 * stops re-attaching in lockstep.
 */
export function reconnectDelay(attempt: number, random: () => number = Math.random): number {
  const capped = Math.min(RECONNECT_BASE_MS * 2 ** Math.max(0, attempt), RECONNECT_MAX_MS);
  return Math.round(capped * (0.5 + random() * 0.5));
}

const TERMINAL_GONE_NOTE = "[terminal not found]";

/**
 * Whether this frame is the server refusing the attach because the terminal is
 * gone. Only ever asked of a connection's *first* frame: further along it is
 * ordinary pty output, where the same text could just be something the shell
 * printed.
 */
export function isTerminalGoneFrame(text: string): boolean {
  return text.trim() === TERMINAL_GONE_NOTE;
}

/** What the pane sends the pty ({@link terminalInput}). */
export interface TerminalInput {
  /** Write the attach replay into the terminal; what it says while parsing it is not sent. */
  writeReplay(text: string): void;
  dispose(): void;
}

/**
 * Send what the terminal produces, keys and its answers to queries alike,
 * except its answers to the attach replay.
 *
 * The replay is output the screen already showed. The programs that asked for
 * the cursor position, the device attributes or the colours in it got their
 * answers when it was new; parsing it again, the terminal answers them all a
 * second time, and that would reach whatever runs now as typed text (zsh:
 * `command not found: 2RR0`). Keys pressed while the replay is still being
 * parsed are dropped with them, as keys are while the socket is down.
 */
export function terminalInput(term: Pick<Terminal, "onData" | "write">, send: (data: string) => void): TerminalInput {
  let replays = 0;
  const data = term.onData((text) => {
    if (replays === 0) send(text);
  });
  return {
    writeReplay(text) {
      replays += 1;
      term.write(text, () => {
        replays -= 1;
      });
    },
    dispose: () => data.dispose(),
  };
}
