/**
 * When the composer's *local* queue may hand its next message to the server.
 *
 * # Why there are two queues at all
 *
 * The server has a real queue now: a message POSTed at a busy session is
 * accepted, held by the resident and run when the turn ends, and it shows up in
 * `resident.queuedMessages` while it waits. So the local queue is no longer
 * where messages wait — it is only where they wait *for this tab*, in the
 * cases the server cannot help with:
 *
 * - an upload is still in flight, so there is nothing to POST yet;
 * - a one-shot Codex process is still running, because Codex has no resident
 *   or server-side queue;
 * - a send failed, and the rest of the queue is paused so the user can look at
 *   the failure before the next one goes in after it;
 * - a send got no answer at all, and the message waits as `unknown` until
 *   the server says whether it arrived (`send-receipts.ts`).
 *
 * Everything else goes straight out over the wire.
 *
 * # Why this is a function
 *
 * The drain used to be edge-triggered: an effect compared `canSend` against its
 * previous value and dispatched on the false→true transition. That was right
 * when a busy session really did answer `canSend: false`, and became a
 * permanent stall the day the resident started accepting messages while busy —
 * `canSend` is simply true now, the edge never arrives, and a queued message
 * sat there until someone pressed ▶. Level-triggered has no such failure mode:
 * whenever the conditions hold, the next message goes.
 *
 * The cost of level-triggering is that "already sending" has to be part of the
 * condition rather than implied by the edge, which is what every field below
 * the first two is for.
 */
export interface QueueDrainState {
  /** How many messages this tab is holding. */
  queued: number;
  /** The feed's verdict on whether the session will take a message. */
  canSend: boolean;
  /**
   * The server's own record says the last injection failed.
   *
   * The queue pauses so the failure can be read before the next message lands
   * on top of it.
   */
  failed: boolean;
  /**
   * This tab's last POST failed — refused, or never arrived.
   *
   * Load-bearing in a way the edge-triggered version never had to be. A send
   * that fails puts its message back at the head of the queue, and a
   * level-triggered drain would find the same conditions one render later and
   * send it again, forever, as fast as the network allows. So a failure stops
   * the drain and the ▶ button is what resumes it — the same "look at it
   * first" rule `failed` encodes, for the half of the failures the server
   * never hears about.
   */
  sendFailed: boolean;
  /** A send is in flight. Dispatching now would reorder the queue. */
  sending: boolean;
  /**
   * The head of the queue is not a message the drain may send: one already in
   * flight, one whose send got no answer (`unknown`), or one the user has to
   * decide about (`unsent`, `failed`).
   *
   * The `unknown` case is the one this exists for. A send whose answer was lost
   * may well have arrived, so sending it again on the drain's own initiative is
   * exactly the duplicate idempotent sends were built to stop; and nothing
   * behind it goes either, because the order of a conversation is part of what
   * it says. See `send-receipts.ts`.
   */
  headBlocked?: boolean;
}

/**
 * Whether a terminal injection outcome needs a human decision before the next
 * local message is sent.
 *
 * An interrupt is recorded as a failed injection for audit/history purposes,
 * but it is the user explicitly asking to abandon that turn. Treating it as an
 * ordinary failure strands the correction they queued while pressing Stop —
 * exactly the message that should run next.
 */
export function injectionFailurePausesQueue(
  injection: { state: string; interrupted: boolean } | null | undefined,
): boolean {
  return injection?.state === "failed" && !injection.interrupted;
}

/** The action the head of this tab's queue can take right now. */
export function localQueueHeadAction(input: {
  source: "claude" | "codex";
  canSend: boolean;
  injectionRunning: boolean;
  sending: boolean;
  steering: boolean;
}): "send" | "steer" | null {
  if (input.sending || input.steering) return null;
  if (input.canSend) return "send";
  // Claude's busy messages have already crossed the wire into its resident
  // queue and are rendered by ServerQueue. A one-shot Codex run is the sole
  // case where a message is still local while a turn can be abandoned for it.
  if (input.source === "codex" && input.injectionRunning) return "steer";
  return null;
}

/** Whether the head of the local queue should be dispatched right now. */
export function shouldDrainQueue(state: QueueDrainState): boolean {
  return (
    state.queued > 0 &&
    state.canSend &&
    !state.failed &&
    !state.sendFailed &&
    !state.sending &&
    !state.headBlocked
  );
}
