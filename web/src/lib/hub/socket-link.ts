/*
 * The websocket plumbing the page's hub link (`link.ts`) and its relay links
 * (`relay-link.ts`) share: the socket both dial, channel routes, sending
 * frames, state listeners, the heartbeat's clock and the wake-up probe.
 *
 * What is *not* shared is what makes each link its own: the sign-in handshake
 * (a hub session, a relay ticket), the reconnect policy (the hub reconnects by
 * full-jitter backoff; a relay is opened by the pool and let go after its last
 * channel), what a lost connection tells its channels, and the close codes.
 */
import { encodeFrame, type Frame } from "@/lib/e2e";

/** The parts of a browser `WebSocket` this uses. */
export interface SocketLike {
  binaryType: string;
  readyState: number;
  send(data: string | ArrayBufferLike | ArrayBufferView): void;
  close(code?: number, reason?: string): void;
  onopen: ((ev: unknown) => void) | null;
  onmessage: ((ev: { data: unknown }) => void) | null;
  onclose: ((ev: { code?: number; reason?: string }) => void) | null;
  onerror: ((ev: unknown) => void) | null;
}

export type SocketLinkState = "connecting" | "open" | "closed";

type Listener<T> = (value: T) => void;

/** One websocket link: its channels, frames, state and heartbeat (see the module comment). */
export abstract class SocketLink<S extends SocketLinkState = SocketLinkState> {
  state: S = "closed" as S;

  protected socket: SocketLike | null = null;
  /** When the connection last brought anything, and when the last beat ran (the page's clock). */
  protected lastHeard = 0;
  protected lastBeat = 0;
  /** The wake-up ping sent on coming back, and when nothing has answered it. */
  protected probeTimer: ReturnType<typeof setTimeout> | null = null;
  protected probeAnswered = false;
  private nextCh = 1;
  /** Where each open channel's frames go. */
  protected readonly routes = new Map<number, (frame: Frame) => void>();
  protected readonly stateListeners = new Set<Listener<S>>();

  /** The page's clock (`now` in the options; a test may pass its own). */
  protected abstract now(): number;

  onState(fn: Listener<S>): () => void {
    this.stateListeners.add(fn);
    return () => this.stateListeners.delete(fn);
  }

  /** A fresh channel number on this page (never reused), and where its frames go. */
  reserve(route: (frame: Frame) => void): number {
    const ch = this.nextCh++;
    this.routes.set(ch, route);
    return ch;
  }

  release(ch: number): void {
    this.routes.delete(ch);
  }

  /** Send a frame now; `false` when not signed in (the frame is dropped). */
  send(frame: Frame): boolean {
    if (this.state !== "open" || !this.socket) return false;
    this.socket.send(encodeFrame(frame));
    return true;
  }

  protected setState(next: S): void {
    if (this.state === next) return;
    this.state = next;
    this.notifyState(next);
  }

  /** Tell the state listeners; a link whose listeners may unsubscribe while it tells them overrides this. */
  protected notifyState(next: S): void {
    for (const fn of this.stateListeners) fn(next);
  }

  /**
   * A message from `socket`: `false` when it is not the connection now (one
   * that was replaced). Records the connection as heard, which answers a
   * wake-up probe.
   */
  protected heard(socket: SocketLike): boolean {
    if (this.socket !== socket) return false;
    this.lastHeard = this.now();
    this.probeAnswered = true;
    this.answered();
    return true;
  }

  /** The connection answered: no probe is pending. */
  protected answered(): void {
    if (this.probeTimer) clearTimeout(this.probeTimer);
    this.probeTimer = null;
  }

  /** Give the wake-up ping `ms` to be answered: unless anything is heard, `dead` runs. */
  protected startProbe(ms: number, dead: () => void): void {
    this.probeAnswered = false;
    this.probeTimer = setTimeout(() => {
      this.probeTimer = null;
      if (this.probeAnswered) return;
      dead();
    }, ms);
  }

  /**
   * The heartbeat's clock: note the beat, and a beat far later than due means
   * this page's timers were held — what was not heard meanwhile says nothing
   * about the connection, so its silence deadline gets one more interval from
   * now. Returns the time, and whether the beat was late.
   */
  protected markBeat(heartbeat: number, silence: number): { now: number; late: boolean } {
    const now = this.now();
    const late = now - this.lastBeat > heartbeat * 1.5;
    this.lastBeat = now;
    if (late) this.lastHeard = Math.max(this.lastHeard, now - silence + heartbeat);
    return { now, late };
  }

  /** Hand a decoded frame to its channel's owner (the caller has checked its type is one this link routes). */
  protected route(frame: Frame): void {
    if (!("ch" in frame)) return;
    const route = this.routes.get(frame.ch);
    if (!route) return;
    if (frame.type === "close") this.routes.delete(frame.ch);
    route(frame);
  }
}
