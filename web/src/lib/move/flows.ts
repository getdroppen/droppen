/*
 * The move's four windows (see `types.ts` for the two ways in):
 * - {@link startMove}: push, the legacy origin's tab;
 * - {@link servePushReceiver}: push, the app host's `/move` popup;
 * - {@link startPull}: pull, the app host's tab;
 * - {@link servePullSender}: pull, the legacy origin's `/move` popup.
 * The popups take part only for the window that opened them and only when it
 * is the other origin of the move: one opened by anything else gets nothing
 * and can put nothing here (`protocol.ts`).
 */
import { browserEnv, type MoveEnv, type PeerWindow } from "./env";
import { readLocalDevice } from "./local";
import { LinkEnded, type Message, MSG, newId, originsProblem, peerLink } from "./protocol";
import { onlyUnfinishedHere, receiveBundle, settleMarker } from "./receiver";
import { failureOutcome, sendMove } from "./sender";
import { MOVE_PATH, type MoveOutcome, type MoveStep, type StartOptions } from "./types";

/** How long a popup may take to load and answer. */
export const READY_MS = 30_000;
/** How long the app host's tab waits for the click in the droppen.app popup. */
export const CONFIRM_MS = 10 * 60_000;
/** How long the receiving side waits to hear that the old copy went. */
export const WIPED_MS = 30_000;
/** How long a popup stays to say it is done before it closes itself. */
export const CLOSE_AFTER_MS = 1_500;
/** How often a popup says it is there until it is answered. */
const REPEAT_MS = 1_000;

type Options = StartOptions & { env?: MoveEnv };

/** The window's name: one of its own per move, so no window already open (another site's included) is reused. */
const windowName = () => `droppen-move-${newId()}`;

/**
 * Push, from the legacy origin's tab (`StartMove`): call it from the click.
 * Opens the app host's `/move`, sends it this browser's device, and forgets
 * the device here once that page signed in with it.
 */
export function startMove(opts: Options): Promise<MoveOutcome> {
  const env = opts.env ?? browserEnv();
  const step = opts.onStep ?? (() => undefined);
  const problem = originsProblem(opts.origins, env.origin, "legacy", env.pinned);
  if (problem) return Promise.resolve({ kind: "failed", reason: problem });
  if (env.userActive() === false) return Promise.resolve({ kind: "failed", reason: "a move starts only from a click" });
  step("opening");
  const popup = env.open(`${opts.origins.app}${MOVE_PATH}`, windowName());
  if (!popup) return Promise.resolve({ kind: "popupBlocked" });
  const link = peerLink(env, opts.origins.app, () => popup);
  return (async () => {
    let outcome: MoveOutcome = { kind: "failed", reason: "the move did not finish" };
    try {
      outcome = await sendMove(
        env,
        opts.origins,
        {
          link,
          async peerReady() {
            step("waiting");
            return (await link.next([MSG.ready], READY_MS)).id!;
          },
        },
        step,
      );
      return outcome;
    } finally {
      link.stop();
      // The popup closes itself once it is done; one that stayed is closed here.
      setTimeout(() => popup.close(), outcome.kind === "moved" ? CLOSE_AFTER_MS * 2 : 0);
    }
  })();
}

/** What the app host's `/move` popup shows (push). */
export type ReceiverView =
  /** Not opened by droppen.app: there is nothing to do here. */
  | { step: "noOpener" }
  | { step: "waiting" }
  | { step: "verifying" }
  /** The device is here and signed in; `wiped`: droppen.app removed its copy (`null`: not heard yet). */
  | { step: "moved"; wiped: boolean | null }
  /** The device is here, but droppen.app kept its copy (it changed meanwhile, or did not say). */
  | { step: "kept" }
  /** Taken in nothing: why. */
  | { step: "refused"; detail: string }
  | { step: "closed" }
  | { step: "failed"; detail: string };

/**
 * The app host's `/move` page in a push: say it is ready to the window that
 * opened it (only if that is the legacy origin), take the one bundle it
 * sends, prove it, answer. Returns a stop.
 */
export function servePushReceiver(opts: { origins: Options["origins"]; env?: MoveEnv; onView: (view: ReceiverView) => void }): () => void {
  const env = opts.env ?? browserEnv();
  const { origins, onView } = opts;
  const problem = originsProblem(origins, env.origin, "app", env.pinned);
  if (problem) {
    onView({ step: "failed", detail: problem });
    return () => undefined;
  }
  const opener = env.opener();
  if (!opener) {
    onView({ step: "noOpener" });
    return () => undefined;
  }
  const id = newId();
  const link = peerLink(env, origins.legacy, () => opener);
  let repeat: ReturnType<typeof setInterval> | undefined;
  void (async () => {
    onView({ step: "waiting" });
    const ready = () => link.send({ t: MSG.ready, id });
    ready();
    repeat = setInterval(ready, REPEAT_MS);
    let message: Message;
    try {
      message = await link.next([MSG.bundle], 2 * READY_MS, id);
    } catch (error) {
      onView(error instanceof LinkEnded && error.why === "closed" ? { step: "closed" } : { step: "failed", detail: error instanceof Error ? error.message : String(error) });
      return;
    } finally {
      clearInterval(repeat);
    }
    onView({ step: "verifying" });
    const got = await receiveBundle(env, origins, message.bundle);
    if (!got.ok) {
      link.send({ t: MSG.refused, id, reason: got.reason, detail: got.detail, holds: got.holds });
      onView({ step: "refused", detail: got.detail });
      return;
    }
    link.send({ t: MSG.verified, id, digest: got.digest, account: got.account, device: got.device, resumed: got.resumed });
    onView({ step: "moved", wiped: null });
    try {
      const end = await link.next([MSG.wiped, MSG.kept], WIPED_MS, id);
      if (end.t === MSG.wiped) {
        onView({ step: "moved", wiped: true });
        setTimeout(() => env.closeSelf(), CLOSE_AFTER_MS);
      } else onView({ step: "kept" });
    } catch {
      onView({ step: "kept" });
    }
  })();
  return () => {
    clearInterval(repeat);
    link.stop();
  };
}

/**
 * Pull, from the app host's tab (`StartPull`): call it from the click. Opens
 * the legacy origin's `/move`, asks it for its device, and takes it in once
 * the person said so there.
 */
export function startPull(opts: Options): Promise<MoveOutcome> {
  const env = opts.env ?? browserEnv();
  const step = opts.onStep ?? (() => undefined);
  const { origins } = opts;
  const problem = originsProblem(origins, env.origin, "app", env.pinned);
  if (problem) return Promise.resolve({ kind: "failed", reason: problem });
  if (env.userActive() === false) return Promise.resolve({ kind: "failed", reason: "a move starts only from a click" });
  step("opening");
  const popup: PeerWindow | null = env.open(`${origins.legacy}${MOVE_PATH}`, windowName());
  if (!popup) return Promise.resolve({ kind: "popupBlocked" });
  const link = peerLink(env, origins.legacy, () => popup);
  return (async (): Promise<MoveOutcome> => {
    let keepOpen = false;
    try {
      step("waiting");
      const hello = await link.next([MSG.hello, MSG.none], READY_MS);
      if (hello.t === MSG.none) return { kind: "nothingToMove" };
      // A device here already (and not only what a move left unfinished, nor this device from a move cut off):
      // the click over there would be for nothing.
      const here = await readLocalDevice(env);
      if (here && !onlyUnfinishedHere(env.storage, here) && settleMarker(env.storage, here)?.state !== "verified") return { kind: "targetHasDevice" };
      const id = newId();
      link.send({ t: MSG.request, id });
      step("confirm");
      const sent = await link.next([MSG.bundle, MSG.cancel], CONFIRM_MS, id);
      if (sent.t === MSG.cancel) return { kind: "cancelled" };
      step("verifying");
      const got = await receiveBundle(env, origins, sent.bundle);
      if (!got.ok) {
        link.send({ t: MSG.refused, id, reason: got.reason, detail: got.detail, holds: got.holds });
        keepOpen = true;
        if (got.reason === "targetHasDevice") return { kind: "targetHasDevice" };
        if (got.reason === "busy") return { kind: "busy" };
        if (got.reason === "verifyFailed") return { kind: "verifyFailed", reason: got.detail };
        return { kind: "failed", reason: got.detail };
      }
      link.send({ t: MSG.verified, id, digest: got.digest, account: got.account, device: got.device, resumed: got.resumed });
      step("wiping");
      keepOpen = true;
      let senderWiped = false;
      try {
        senderWiped = (await link.next([MSG.wiped, MSG.kept], WIPED_MS, id)).t === MSG.wiped;
      } catch {
        // closed or silent: droppen.app keeps its copy until the move is offered there again
      }
      return { kind: "moved", account: got.account, device: got.device, pushWasOn: got.pushWasOn, resumed: got.resumed, senderWiped };
    } catch (error) {
      return failureOutcome(error);
    } finally {
      link.stop();
      // The popup says how it ended and closes itself; one with nothing more to say is closed now.
      if (!keepOpen) popup.close();
    }
  })();
}

/** What the legacy origin's `/move` popup shows (pull). */
export type SenderView =
  /** Not opened by the app host: nothing to do here. */
  | { step: "noOpener" }
  /** No device here to move. */
  | { step: "nothing" }
  | { step: "waiting" }
  /** Asked for this device: the person confirms here. */
  | { step: "confirm"; account: string; device: string }
  /** Sending, then waiting for the app host's sign-in, then removing the copy here. */
  | { step: Exclude<MoveStep, "opening" | "waiting" | "confirm"> }
  | { step: "done"; outcome: MoveOutcome };

export interface PullSender {
  /** The person's click: send. */
  confirm(): void;
  /** The person's click: do not. */
  cancel(): void;
  stop(): void;
}

/**
 * The legacy origin's `/move` page in a pull: tell the window that opened it
 * (only if that is the app host) whether there is a device here, and send it
 * on the person's click here once that window asked for it.
 */
export function servePullSender(opts: { origins: Options["origins"]; env?: MoveEnv; onView: (view: SenderView) => void }): PullSender {
  const env = opts.env ?? browserEnv();
  const { origins, onView } = opts;
  const idle: PullSender = { confirm: () => undefined, cancel: () => undefined, stop: () => undefined };
  const problem = originsProblem(origins, env.origin, "legacy", env.pinned);
  if (problem) {
    onView({ step: "done", outcome: { kind: "failed", reason: problem } });
    return idle;
  }
  const opener = env.opener();
  if (!opener) {
    onView({ step: "noOpener" });
    return idle;
  }
  const link = peerLink(env, origins.app, () => opener);
  let id: string | null = null;
  let started = false;
  let repeat: ReturnType<typeof setInterval> | undefined;
  void (async () => {
    try {
      const here = await readLocalDevice(env);
      if (!here?.primary || !here.keys) {
        link.send({ t: MSG.none });
        onView({ step: "nothing" });
        return;
      }
      onView({ step: "waiting" });
      const hello = () => link.send({ t: MSG.hello });
      hello();
      repeat = setInterval(hello, REPEAT_MS);
      const request = await link.next([MSG.request], 2 * READY_MS);
      id = request.id ?? null;
      if (!id) throw new Error("the app host asked without an id");
      onView({ step: "confirm", account: here.primary.account, device: here.primary.device });
    } catch (error) {
      onView({ step: "done", outcome: failureOutcome(error) });
    } finally {
      clearInterval(repeat);
    }
  })();
  return {
    confirm() {
      if (!id || started || env.userActive() === false) return;
      started = true;
      const asked = id;
      void sendMove(env, origins, { link, peerReady: async () => asked }, (step) => {
        if (step === "sending" || step === "verifying" || step === "wiping") onView({ step });
      }).then((outcome) => {
        onView({ step: "done", outcome });
        link.stop();
        if (outcome.kind === "moved") setTimeout(() => env.closeSelf(), CLOSE_AFTER_MS);
      });
    },
    cancel() {
      if (started) return;
      started = true;
      if (id) link.send({ t: MSG.cancel, id });
      link.stop();
      onView({ step: "done", outcome: { kind: "cancelled" } });
      env.closeSelf();
    },
    stop() {
      clearInterval(repeat);
      link.stop();
    },
  };
}
