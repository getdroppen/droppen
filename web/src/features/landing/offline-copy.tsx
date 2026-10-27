import { useRef } from "react";
import { useTranslation } from "react-i18next";
import { chipOpacity, ease, place, pointAt, type Pt, setAttr, setHidden, span, useLoop, useReducedMotion, useSize } from "./motion";
import { CipherChip, InkMark, Ledger, ledgerH, LiveDot, MessageRow, OffSquare, Source, Stage, useRowRefs, WEIGHT, Wire, WireLock } from "./kit";
import { type Chip, drawRow, type Message, type Shown, textOf } from "./story";

/*
 * "Readable with the computer off": the computer goes off, and the phone
 * fetches the session's pages from the encrypted copy behind Droppen Hub, one
 * at a time: each page rides sealed (a padlocked packet, no words) through
 * Droppen Hub to the phone, where it opens into its line of the transcript,
 * readable as ever. Read-only: sending and terminals wait for the computer.
 * The cards are ledgers, as in the app; every wire is straight. What the
 * reader sees is the session, not ciphertext: the encryption is the
 * packets' padlocks and the copy's "Droppen can't read it".
 */

export const OFFLINE_LOOP = 9;
const LOOP = OFFLINE_LOOP;
const OFF = 1.0;
/** When each page leaves the store. */
export const OFFLINE_STARTS = [1.6, 4.0] as const;
const PAGES = OFFLINE_STARTS;
const TRIP = 1.2;
/** The share of a trip spent between the store and Droppen Hub (the rest is on to the phone). */
const LEG = 0.4;
/** From here to the end of the loop every copy is the last one again (dimmed): the loop starts over. */
const RESET = LOOP - 0.5;

export interface OfflineFrame {
  off: boolean;
  /** Droppen Hub is passing a page on: its card lit, else dimmed. */
  passing: boolean;
  /** The phone's transcript, a row per page. */
  rows: Shown[];
  chips: Chip[];
}

/** What the offline copy shows at `t` seconds into its loop. */
export function offlineFrame(t: number): OfflineFrame {
  const chips: Chip[] = [];
  let passing = false;
  const rows: Shown[] = PAGES.map((start, i) => {
    const trip = span(t, start, start + TRIP);
    if (trip > 0 && trip < 1) {
      chips.push({ msg: i, wire: trip < LEG ? "storeHub" : "hubDevice", at: trip < LEG ? ease(trip / LEG) : ease((trip - LEG) / (1 - LEG)) });
      passing = true;
    }
    const arrive = start + TRIP;
    // Before its page arrives a row still reads (the phone's last copy), dimmed; then it is the fresh page, opened on the phone.
    if (t < arrive || t >= RESET) return { msg: i, kind: "plain", dim: true };
    if (t < arrive + 1.0) return { msg: i, kind: "plain", lock: "open" };
    if (t < arrive + 1.7) return { msg: i, kind: "plain", lock: "open", lockOpacity: 1 - span(t, arrive + 1.0, arrive + 1.7) };
    return { msg: i, kind: "plain" };
  });
  return { off: t >= OFF, passing, rows, chips };
}

/** The still frame (reduced motion): both pages opened on the phone, the last one's padlock open beside it. */
export function offlineStill(): OfflineFrame {
  const f = offlineFrame(PAGES[PAGES.length - 1]! + TRIP + 1.05);
  return { ...f, rows: f.rows.map((r) => (r.lock ? { ...r, lockOpacity: 1 } : r)), chips: [] };
}

interface Box {
  x: number;
  y: number;
  w: number;
  h: number;
}

interface Layout {
  h: number;
  narrow: boolean;
  machine: Box;
  hub: Box;
  store: Box;
  device: Box;
  machineHub: Pt[];
  storeHub: Pt[];
  hubDevice: Pt[];
}

/** Every drawn wire of an offline layout, for the geometry checks. */
export function offlineWires(L: Layout): Pt[][] {
  return [L.machineHub, L.storeHub, L.hubDevice];
}

/** The padlock on the wire leaving the computer, just outside it: it sealed every page before it went off. */
export function offlineLocks(L: Layout): Pt[] {
  const p = pointAt(L.machineHub, 0);
  return [L.narrow ? { x: p.x, y: p.y + 12 } : { x: p.x + 10, y: p.y }];
}

const HUB_W = 156;

/** The phone card's width in a row: room for its longest message row in every language (English's, 254px). */
const DEVICE_W = 280;
/** Below this the three cards and their gaps do not fit in a row with the phone at `DEVICE_W`: they stack. */
const ROW_MIN_W = 740;

export function offlineLayout(w: number): Layout {
  const pad = 12;
  if (w < ROW_MIN_W) {
    // The computer on top; Droppen with the store beside it; the phone below.
    const machine = { x: pad, y: pad, w: w - 2 * pad, h: ledgerH(1) };
    const hub = { x: pad, y: machine.y + machine.h + 44, w: Math.min(HUB_W, Math.round(w * 0.42)), h: ledgerH(1) };
    const hubCx = Math.round(hub.x + hub.w / 2);
    const hubCy = Math.round(hub.y + hub.h / 2);
    const store = { x: hub.x + hub.w + 16, y: hubCy - Math.round(ledgerH(1) / 2), w: w - pad - (hub.x + hub.w + 16), h: ledgerH(1) };
    const device = { x: pad, y: Math.max(store.y + store.h, hub.y + hub.h) + 30, w: w - 2 * pad, h: ledgerH(2) };
    return {
      h: device.y + device.h + 14,
      narrow: true,
      machine,
      hub,
      store,
      device,
      machineHub: [{ x: hubCx, y: machine.y + machine.h }, { x: hubCx, y: hub.y }],
      storeHub: [{ x: store.x, y: hubCy }, { x: hub.x + hub.w, y: hubCy }],
      hubDevice: [{ x: hubCx, y: hub.y + hub.h }, { x: hubCx, y: device.y }],
    };
  }
  const gap = 36;
  const machineW = Math.min(208, Math.round(w * 0.28));
  const deviceW = Math.min(DEVICE_W, w - 2 * pad - machineW - HUB_W - 2 * gap);
  const x0 = Math.round((w - (machineW + HUB_W + deviceW + 2 * gap)) / 2);
  const top = 24;
  const mid = top + Math.round(ledgerH(2) / 2);
  const machine = { x: x0, y: mid - Math.round(ledgerH(1) / 2), w: machineW, h: ledgerH(1) };
  const hub = { x: x0 + machineW + gap, y: mid - Math.round(ledgerH(1) / 2), w: HUB_W, h: ledgerH(1) };
  const device = { x: hub.x + HUB_W + gap, y: top, w: deviceW, h: ledgerH(2) };
  const store = { x: hub.x + Math.round((HUB_W - 196) / 2), y: hub.y + hub.h + 36, w: 196, h: ledgerH(1) };
  const cx = Math.round(hub.x + HUB_W / 2);
  return {
    h: Math.max(device.y + device.h, store.y + store.h) + 24,
    narrow: false,
    machine,
    hub,
    store,
    device,
    machineHub: [{ x: machine.x + machineW, y: mid }, { x: hub.x, y: mid }],
    storeHub: [{ x: cx, y: store.y }, { x: cx, y: hub.y + hub.h }],
    hubDevice: [{ x: hub.x + HUB_W, y: mid }, { x: device.x, y: mid }],
  };
}

export function OfflineCopy() {
  const { t } = useTranslation();
  const box = useRef<HTMLDivElement>(null);
  const { w, h: boxH } = useSize(box, 680);
  const L = offlineLayout(w);
  const reduced = useReducedMotion();

  const machineCard = useRef<HTMLDivElement>(null);
  const live = useRef<HTMLSpanElement>(null);
  const off = useRef<HTMLSpanElement>(null);
  const machineRow = useRef<HTMLSpanElement>(null);
  const offRow = useRef<HTMLSpanElement>(null);
  const machineWire = useRef<SVGPathElement>(null);
  const hubCard = useRef<HTMLDivElement>(null);
  const rows = [useRowRefs(), useRowRefs()] as const;
  const chips = useRef<(SVGGElement | null)[]>([]);

  const roles = ["USER", "CLAUDE"];
  // Plaintext only: the phone shows the session as it was, and nothing on the way shows any words.
  const messages: Message[] = [
    { p: t("darken the sign-in button"), c: "" },
    { p: t("done, tests pass"), c: "" },
  ];
  const titles = { shut: t("encrypted locally"), open: t("decrypted locally") };
  const onlineRow = t("online");

  const draw = (f: OfflineFrame, tick: number) => {
    setAttr(machineCard.current, "data-off", f.off ? "true" : "false");
    setHidden(live.current, f.off);
    setHidden(off.current, !f.off);
    setHidden(machineRow.current, f.off);
    setHidden(offRow.current, !f.off);
    setAttr(machineWire.current, "stroke-dasharray", f.off ? WEIGHT.fallback.dash : "none");
    setAttr(hubCard.current, "data-dim", f.passing ? "false" : "true");
    f.rows.forEach((row, i) => drawRow(rows[i]!, row, messages, tick));
    const wires: Record<string, Pt[]> = { storeHub: L.storeHub, hubDevice: L.hubDevice };
    PAGES.forEach((_, i) => {
      const chip = f.chips.find((c) => c.msg === i);
      place(chips.current[i], chip ? pointAt(wires[chip.wire]!, chip.at) : null, chip ? chipOpacity(chip.at) : 0);
    });
  };

  useLoop(
    box,
    LOOP,
    { frame: (time) => draw(offlineFrame(time), Math.floor(time * 14)), still: () => draw(offlineStill(), 0) },
    reduced,
    [w, t],
  );
  const first = reduced ? offlineStill() : offlineFrame(0);

  return (
    <Stage
      name="offline"
      ref={box}
      width={w}
      height={Math.max(boxH, L.h)}
      drawn={L.h}
      className="h-full"
      still={reduced}
      svg={
        <>
          <Wire ref={machineWire} points={L.machineHub} tone="faint" weight="fallback" />
          <Wire points={L.storeHub} />
          <Wire points={L.hubDevice} />
          {offlineLocks(L).map((at, i) => (
            <WireLock key={i} at={at} title={titles.shut} />
          ))}
        </>
      }
      over={PAGES.map((_, i) => (
        <CipherChip key={i} ref={(el) => void (chips.current[i] = el)} />
      ))}
    >
      <div ref={machineCard} data-off="false" className="contents">
        <Ledger
          box={L.machine}
          className="[[data-off=true]>&]:border-dashed"
          head={
            <>
              {t("your computer")}
            </>
          }
          status={
            <>
              <span ref={live} className="flex">
                <LiveDot />
              </span>
              <span ref={off} hidden className="flex">
                <OffSquare />
              </span>
            </>
          }
          rows={[
            <span key="r" className="flex items-center">
              <span ref={machineRow} className="text-muted">
                {onlineRow}
              </span>
              <span ref={offRow} hidden data-signal="offline" className="text-danger">
                {t("computer off · {{time}}", { time: "14:00" })}
              </span>
            </span>,
          ]}
        />
      </div>
      <div ref={hubCard} data-dim={first.passing ? "false" : "true"} className="contents">
        <Ledger
          box={L.hub}
          ink
          className="transition-opacity duration-300 [[data-dim=true]>&]:opacity-60"
          head={
            <>
              <InkMark size={14} />
              Droppen Hub
            </>
          }
          rows={[
            // What it does with a page: passes it on, sealed as it came.
            <span key="p" className="opacity-80">
              {t("passes the copy on")}
            </span>,
          ]}
        />
      </div>
      <Ledger
        box={L.store}
        head={
          <>
            {t("encrypted copy")}
          </>
        }
        rows={[
          <span key="c" className="text-fg">
            {t("Droppen can't read it")}
          </span>,
        ]}
      />
      <Ledger
        box={L.device}
        head={
          <>
            {t("your phone")}
          </>
        }
        status={<span className="text-muted">{t("read-only")}</span>}
        rows={messages.map((_, i) => (
          <span key={i} className="flex min-w-0 items-center gap-1.5">
            <Source>{roles[i]}</Source>
            <MessageRow refs={rows[i]!} titles={titles}>
              {textOf(first.rows[i]!, messages)}
            </MessageRow>
          </span>
        ))}
      />
    </Stage>
  );
}
