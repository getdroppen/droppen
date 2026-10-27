/*
 * One device's copies through the local gateway (`docs/local-aggregator.md`
 * §6.2, `GatewayCopySource`): the `CopySource` a `MachineMirror` follows in
 * the local app for a device it reaches only through the local install.
 *
 * The install has already opened and checked every record (it holds the
 * keys; the page holds none), so this only folds what the gateway hands over
 * with the same folds droppen.app uses (`indexFold`, `statusFold`,
 * `feedFold`): a `reset` rebuilds the copy from its bodies, anything else goes
 * on top. Nothing is kept on the device and nothing is resumed from it — a
 * copy's mark is only its place in the stream (its epoch and `seq`), which
 * the gateway uses to skip the snapshot when nothing moved since.
 *
 * A change that does not fold, or one on top of a copy this page does not
 * hold, takes the whole copy again — after a moment, longer each time in a
 * row. A subscription the install ended because the device left its list
 * (`gone`, `no_device`) is asked for again while the device is still listed.
 */
import type { CopyAt, CopyEvents, CopyFollow, CopyMark, CopyName, CopySource } from "@/lib/hub/copy-source";
import { copyFolds, type StreamFold } from "@/lib/hub/streams";
import type { GatewayFrom, GatewayLink, GatewayName } from "./link";

/** Waits (ms) before taking a copy again, the last repeated. */
const AGAIN_MS = [1000, 2000, 5000, 10_000, 30_000];

/** Where a copy stands: its place in the stream (`mark`: its epoch), and how current it is. */
function mark(epoch: number, seq: number, timeMs: number): CopyMark {
  return { mark: `e${epoch}`, seq, timeMs };
}

/** The place a mark says, if it is one of ours. */
function placeOf(at: CopyMark): GatewayFrom | null {
  const m = /^e(\d+)$/.exec(at.mark);
  return m ? { epoch: Number(m[1]), seq: at.seq } : null;
}

/** A copy's name as the gateway takes it. */
export function gatewayName(name: CopyName): GatewayName {
  if (name.kind === "session") return { kind: "session", source: name.source, id: name.id };
  return { kind: name.kind };
}

/** A copy's place, for the mirror's bookkeeping only (nothing is stored under it). */
export function gatewayPlace(name: CopyName): string {
  return name.kind === "session" ? `gateway:session/${name.source}/${name.id}` : `gateway:${name.kind}`;
}

export class GatewayCopySource implements CopySource {
  private closed = false;

  constructor(
    private readonly link: GatewayLink,
    readonly machine: string,
    /** Waits (ms) before taking a copy again, the last repeated; tests pass short ones. */
    private readonly againMs: readonly number[] = AGAIN_MS,
  ) {}

  ready(): Promise<void> {
    return Promise.resolve();
  }

  /** The install holds the keys: everything it shows can be followed. */
  get keyed(): boolean {
    return true;
  }

  onKeys(): () => void {
    return () => undefined;
  }

  async places(name: CopyName): Promise<string[]> {
    return [gatewayPlace(name)];
  }

  /** Nothing is kept on the device, so nothing is resumed from it. */
  async resume(): Promise<CopyMark | null> {
    return null;
  }

  get reachable(): boolean {
    return this.link.open.get();
  }

  follow<S>(name: CopyName, _places: readonly string[], start: { at: number; from: CopyAt<S> | null }, events: CopyEvents<S>): CopyFollow | null {
    if (this.closed) return null;
    const fold = copyFolds[name.kind] as StreamFold<S>;
    let place = start.from ? placeOf(start.from.at) : null;
    let state: S | null = place ? (start.from?.state ?? null) : null;
    let synced = false;
    let stopped = false;
    let tries = 0;
    let wait: ReturnType<typeof setTimeout> | undefined;
    let stop = () => undefined as void;
    /** Subscribe again in a moment (`fresh`: from nothing). */
    const again = (fresh: boolean) => {
      stop();
      stop = () => undefined;
      if (fresh) {
        state = null;
        place = null;
      }
      clearTimeout(wait);
      const ms = this.againMs[Math.min(tries, this.againMs.length - 1)] ?? 1000;
      tries += 1;
      wait = setTimeout(() => {
        wait = undefined;
        if (!stopped && !this.closed) stop = follow();
      }, ms);
    };
    const follow = () =>
      this.link.sub(this.machine, gatewayName(name), () => (state === null ? null : place), {
        onCopy: ({ reset, bodies, headSeq, epoch, headTimeMs }) => {
          if (!reset && state === null) {
            // A change to a copy this page does not hold: the whole copy again.
            again(true);
            return;
          }
          let next: S;
          try {
            next = reset || state === null ? fold.empty() : state;
            for (const body of bodies) next = fold.apply(next, body);
          } catch (error) {
            // Not what the device wrote: take the whole copy again.
            console.warn("gateway: a copy did not fold:", error);
            again(true);
            return;
          }
          tries = 0;
          state = next;
          place = { epoch, seq: headSeq };
          events.onState({ state: next, at: mark(epoch, headSeq, headTimeMs), reset, bodies, freshestMs: headTimeMs });
          if (!synced) {
            synced = true;
            events.onSynced?.(true);
          }
        },
        onMissing: () => {
          if (!synced) events.onSynced?.(false);
        },
        onEnd: (code) => {
          if (!synced) {
            synced = true;
            events.onSynced?.(false);
          }
          // The device left the install's list for a moment (it was cleared,
          // or listed again): while the page still lists it, ask again.
          if ((code === "gone" || code === "no_device") && this.link.lists(this.machine)) again(false);
        },
      });
    stop = follow();
    return {
      close: () => {
        stopped = true;
        clearTimeout(wait);
        stop();
      },
    };
  }

  close(): void {
    this.closed = true;
  }
}
