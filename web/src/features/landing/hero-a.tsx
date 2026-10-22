import { useId, useRef } from "react";
import { useTranslation } from "react-i18next";
import { chipOpacity, place, pointAt, type Pt, setAttr, span, useLoop, useReducedMotion, useSize } from "./motion";
import { Arrows, CipherChip, InkMark, Ledger, ledgerH, LiveDot, MessageRow, Stage, useRowRefs, Wire, WireLock, WireTag } from "./kit";
import { type Chip, chipLabel, drawRow, type Message, type Shown, textOf } from "./story";

/*
 * The hero — "the message is the focus". Three cards, two rows each: a title
 * and the one thing that matters. Your computer types a message and seals it
 * in place (a padlock appears, the text turns into its ciphertext); only then
 * does a chip of that ciphertext leave. The Droppen relay only ever shows
 * the ciphertext it passes on (its card is titled for that: what Droppen
 * sees). The agent's first message asks you something ("ready to merge?"),
 * the second reports ("all tests pass"). Your phone gets the same
 * ciphertext and opens it in place into the same message, its title showing
 * the round trip (relay 220 ms, amber). Then the direct beat: the green line
 * lights up, the next message is sealed the same way and its chip goes
 * straight across, the relay dimmed; the phone opens it and shows direct
 * 63 ms. A padlock sits on each wire leaving the computer: what leaves it is
 * encrypted already.
 *
 * Computer and phone share a row with the straight green line between them;
 * the Droppen relay sits below, between them, reached by a single L from each
 * (a wire bends at most once). Sizes are fixed: a wider
 * screen only leaves more grid around the figure. On a phone the three stack.
 */

export const HERO_LOOP = 12;

/** The two messages of a loop: the first through the Droppen relay, the second direct. Plaintext in the reader's language, ciphertext fixed. */
export const HERO_CIPHERS = ["5c1e 88d0 a3f7", "2930 bf24 1db9"] as const;

/** When each message is typed, sealed, sent, passed on, opened (seconds into the loop). */
const M0 = { type: [0.3, 1.5], seal: [1.6, 2.3], send: [2.5, 3.3], pass: [3.8, 4.6], open: [4.9, 5.7], fade: [5.7, 6.4] } as const;
const M1 = { type: [7.0, 8.0], seal: [8.1, 8.8], send: [9.0, 9.6], open: [9.9, 10.7], fade: [10.7, 11.4] } as const;
/** When each message starts (is first typed): the message whose turn it is. */
export const HERO_STARTS = [M0.type[0], M1.type[0]] as const;
/** The direct beat: the green line lit, the Droppen relay dimmed. */
const DIRECT = [M1.type[0], M1.fade[1]] as const;
/** A chip is clear of the computer's card this long after it leaves. */
const CLEAR = 0.25;

export interface HeroFrame {
  computer: Shown;
  /** The Droppen relay's card: the ciphertext it passes on. */
  hub: Shown;
  phone: Shown;
  /** Which path brought what the phone shows (`hub`: through the relay): its title's round trip. */
  path: "hub" | "direct";
  /** The direct beat: the green line lit and the Droppen relay dimmed. */
  direct: boolean;
  chips: Chip[];
}

const within = (t: number, [a, b]: readonly [number, number]) => t >= a && t < b;

/** What the hero shows at `t` seconds into its loop. */
export function heroFrame(t: number): HeroFrame {
  // The computer: the last message, then the next typed, sealed in place, held until its chip is clear, and back to its plaintext.
  const sender = (m: typeof M0 | typeof M1, msg: number): Shown | null => {
    if (within(t, m.type)) return { msg, kind: "plain", typed: span(t, m.type[0], m.type[1]) };
    if (t >= m.type[1] && t < m.seal[0]) return { msg, kind: "plain" };
    if (within(t, m.seal)) return { msg, kind: "seal", p: span(t, m.seal[0], m.seal[1]), lock: "shut", lockOpacity: span(t, m.seal[0], m.seal[0] + 0.15) };
    if (t >= m.seal[1] && t < m.send[0] + CLEAR) return { msg, kind: "cipher", lock: "shut" };
    return null;
  };
  let computer = sender(M0, 0) ?? sender(M1, 1);
  if (!computer) computer = { msg: t >= M0.send[0] && t < M1.type[0] ? 0 : 1, kind: "plain" };

  // The chips: the first message to the Droppen relay and on to the phone, the second straight across.
  const chips: Chip[] = [];
  if (within(t, M0.send)) chips.push({ msg: 0, wire: "toHub", at: span(t, M0.send[0], M0.send[1]) });
  if (within(t, M0.pass)) chips.push({ msg: 0, wire: "toPhone", at: span(t, M0.pass[0], M0.pass[1]) });
  if (within(t, M1.send)) chips.push({ msg: 1, wire: "direct", at: span(t, M1.send[0], M1.send[1]) });

  // The Droppen relay: the ciphertext it passes on, the same from the chip's arrival; dimmed while another message has the turn.
  const hub: Shown = { msg: 0, kind: "cipher", dim: !(t >= M0.send[1] && t < M1.type[0]) };

  // The phone: the last message (dimmed once the next is under way), then the next arriving as ciphertext and opening in place.
  const receiver = (m: { open: readonly [number, number]; fade: readonly [number, number] }, arrive: number, msg: number): Shown | null => {
    if (t >= arrive && t < m.open[0]) return { msg, kind: "cipher", lock: "shut" };
    if (within(t, m.open)) return { msg, kind: "open", p: span(t, m.open[0], m.open[1]), lock: "open" };
    if (within(t, m.fade)) return { msg, kind: "plain", lock: "open", lockOpacity: 1 - span(t, m.fade[0], m.fade[1]) };
    return null;
  };
  let phone = receiver(M0, M0.pass[1], 0) ?? receiver(M1, M1.send[1], 1);
  if (!phone) {
    if (t >= M0.fade[1] && t < M1.type[0]) phone = { msg: 0, kind: "plain" };
    else if (t >= M1.type[0] && t < M1.send[1]) phone = { msg: 0, kind: "plain", dim: true };
    else if (t >= M1.fade[1] || t < M0.type[0]) phone = { msg: 1, kind: "plain" };
    else phone = { msg: 1, kind: "plain", dim: true };
  }
  return {
    computer,
    hub,
    phone,
    path: t >= M0.pass[1] && t < M1.send[1] ? "hub" : "direct",
    direct: within(t, DIRECT),
    chips,
  };
}

/** The still frame (reduced motion): the first message delivered through the Droppen relay, the phone's padlock open beside it. */
export function heroStill(): HeroFrame {
  const f = heroFrame(M0.fade[0] + 0.05);
  return { ...f, phone: { ...f.phone, lockOpacity: 1 }, chips: [] };
}

export interface Box {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface HeroALayout {
  h: number;
  narrow: boolean;
  computer: Box;
  droppen: Box;
  phone: Box;
  direct: Pt[];
  toHub: Pt[];
  toPhone: Pt[];
  directLabel: Pt;
}

export function heroALayout(w: number): HeroALayout {
  const cardH = ledgerH(1);
  if (w < 480) {
    // A column: computer, Droppen relay, phone, with the direct line straight down a lane to the left of the relay's card.
    const colW = Math.min(w - 24, 320);
    const x0 = Math.round((w - colW) / 2);
    const lane = x0 + 44;
    const computer = { x: x0, y: 16, w: colW, h: cardH };
    const droppen = { x: x0 + 96, y: computer.y + cardH + 36, w: colW - 96, h: cardH };
    const phone = { x: x0, y: droppen.y + cardH + 36, w: colW, h: cardH };
    const relayX = Math.round(droppen.x + droppen.w / 2);
    return {
      h: phone.y + cardH + 16,
      narrow: true,
      computer,
      droppen,
      phone,
      direct: [{ x: lane, y: computer.y + cardH }, { x: lane, y: phone.y }],
      toHub: [{ x: relayX, y: computer.y + cardH }, { x: relayX, y: droppen.y }],
      toPhone: [{ x: relayX, y: droppen.y + cardH }, { x: relayX, y: phone.y }],
      directLabel: { x: lane, y: Math.round((computer.y + cardH + phone.y) / 2) },
    };
  }
  // Fixed, compact sizes: the computer and the phone on one row, the direct
  // line straight between them; the Droppen relay below, between them, reached by
  // one L from each. A wider screen only leaves more grid around the figure.
  const sideW = 184;
  const gap = 96;
  const inset = 36;
  const midW = sideW + gap - 2 * inset;
  const x0 = Math.round((w - (2 * sideW + gap)) / 2);
  const top = 40;
  const computer = { x: x0, y: top, w: sideW, h: cardH };
  const phone = { x: x0 + sideW + gap, y: top, w: sideW, h: cardH };
  const c1 = Math.round(computer.x + sideW / 2);
  const c3 = Math.round(phone.x + sideW / 2);
  const droppen = { x: c1 + inset, y: top + cardH + 30, w: midW, h: cardH };
  const mid = Math.round(droppen.y + cardH / 2);
  const directY = Math.round(top + cardH / 2);
  return {
    h: droppen.y + cardH + 40,
    narrow: false,
    computer,
    droppen,
    phone,
    direct: [{ x: computer.x + sideW, y: directY }, { x: phone.x, y: directY }],
    // Down out of the computer, then along into the Droppen relay; out of it, then up into the phone: one bend each.
    toHub: [{ x: c1, y: top + cardH }, { x: c1, y: mid }, { x: droppen.x, y: mid }],
    toPhone: [{ x: droppen.x + midW, y: mid }, { x: c3, y: mid }, { x: c3, y: top + cardH }],
    directLabel: { x: Math.round((computer.x + sideW + phone.x) / 2), y: directY },
  };
}

export function heroAWires(L: HeroALayout): Pt[][] {
  return [L.direct, L.toHub, L.toPhone];
}

/** Where the padlocks sit: on each wire leaving the computer, just outside it. */
export function heroALocks(L: HeroALayout): Pt[] {
  return [pointAt(L.direct, 0), pointAt(L.toHub, 0)].map((p, i) =>
    i === 0 && !L.narrow ? { x: p.x + 10, y: p.y } : { x: p.x, y: p.y + 12 },
  );
}

/** The direct line at rest: solid and plainly there, in a muted green; full green while it carries the beat. */
const DIRECT_IDLE = "color-mix(in srgb, var(--color-ok) 45%, var(--color-bg))";
const DIRECT_LIVE = "var(--color-ok)";

export function HeroA() {
  const { t } = useTranslation();
  const box = useRef<HTMLDivElement>(null);
  const { w, h: boxH } = useSize(box, 560);
  const L = heroALayout(w);
  const reduced = useReducedMotion();
  const arrows = `ha-${useId().replace(/[^a-z0-9]/gi, "")}`;
  const messages: Message[] = [
    { p: t("ready to merge?"), c: HERO_CIPHERS[0] },
    { p: t("all tests pass"), c: HERO_CIPHERS[1] },
  ];
  const titles = { shut: t("encrypted locally"), open: t("decrypted locally") };

  const computer = useRowRefs();
  const hub = useRowRefs();
  const phone = useRowRefs();
  const hubCard = useRef<HTMLDivElement>(null);
  const pathDirect = useRef<HTMLSpanElement>(null);
  const pathHub = useRef<HTMLSpanElement>(null);
  const directWire = useRef<SVGPathElement>(null);
  const directTag = useRef<SVGGElement>(null);
  const relay = useRef<SVGGElement>(null);
  const hop = useRef<SVGGElement>(null);

  const draw = (f: HeroFrame, tick: number) => {
    drawRow(computer, f.computer, messages, tick);
    drawRow(hub, f.hub, messages, tick);
    drawRow(phone, f.phone, messages, tick);
    setAttr(pathDirect.current, "data-off", f.path === "direct" ? "false" : "true");
    setAttr(pathHub.current, "data-off", f.path === "hub" ? "false" : "true");
    setAttr(directWire.current, "stroke", f.direct ? DIRECT_LIVE : DIRECT_IDLE);
    setAttr(directTag.current, "data-on", f.direct ? "true" : "false");
    setAttr(hubCard.current, "data-dim", f.direct ? "true" : "false");
    const wires: Record<string, Pt[]> = { toHub: L.toHub, toPhone: L.toPhone, direct: L.direct };
    for (const [msg, el] of [
      [0, relay.current],
      [1, hop.current],
    ] as const) {
      const chip = f.chips.find((c) => c.msg === msg);
      place(el, chip ? pointAt(wires[chip.wire]!, chip.at) : null, chip ? chipOpacity(chip.at) : 0);
    }
  };

  useLoop(
    box,
    HERO_LOOP,
    { frame: (time) => draw(heroFrame(time), Math.floor(time * 14)), still: () => draw(heroStill(), 0) },
    reduced,
    [w, t],
  );

  // The first frame, as the loop starts it (and as a page without scripts shows it).
  const first = reduced ? heroStill() : heroFrame(0);
  return (
    <Stage
      name="hero"
      ref={box}
      width={w}
      height={Math.max(boxH, L.h)}
      drawn={L.h}
      className="h-full"
      grid={22}
      svg={
        <>
          <Arrows id={arrows} />
          <Wire points={L.toHub} arrow={`${arrows}-ink`} />
          <Wire points={L.toPhone} arrow={`${arrows}-ink`} />
          <Wire ref={directWire} points={L.direct} tone="ok" weight="direct" color={first.direct ? DIRECT_LIVE : DIRECT_IDLE} />
          {heroALocks(L).map((at, i) => (
            <WireLock key={i} at={at} title={titles.shut} />
          ))}
          {/* On its own line, by design: the tag names the direct path (under the chips, which ride over it). */}
          <WireTag ref={directTag} at={L.directLabel} text={t("direct")} />
        </>
      }
      over={
        <>
          <CipherChip ref={relay} label={chipLabel(messages[0]!.c)} />
          <CipherChip ref={hop} label={chipLabel(messages[1]!.c)} />
        </>
      }
    >
      <Ledger
        box={L.computer}
        head={t("your computer")}
        status={<LiveDot />}
        rows={[
          <MessageRow key="m" refs={computer} titles={titles}>
            {textOf(first.computer, messages)}
          </MessageRow>,
        ]}
      />
      <div ref={hubCard} data-dim="false" className="contents">
        <Ledger
          box={L.droppen}
          ink
          className="transition-opacity duration-300 [[data-dim=true]>&]:opacity-35"
          head={
            <>
              <InkMark size={14} />
              {t("what Droppen sees")}
            </>
          }
          rows={[
            // Only ever the ciphertext it passes on: no padlock here, nothing is sealed or opened in the relay.
            <MessageRow key="c" refs={hub} lock={false} className="text-[13px] font-medium tabular-nums">
              {textOf(first.hub, messages)}
            </MessageRow>,
          ]}
        />
      </div>
      <Ledger
        box={L.phone}
        head={t("your phone")}
        status={
          // The latency of the path that brought it; both tags share one grid cell, one invisible, so nothing moves.
          <span className="grid justify-items-end">
            <span ref={pathHub} data-off={first.path === "hub" ? "false" : "true"} data-signal="latency" className="col-start-1 row-start-1 text-warn-fg tabular-nums data-[off=true]:invisible">
              {t("relay")} 220 ms
            </span>
            <span ref={pathDirect} data-off={first.path === "direct" ? "false" : "true"} data-signal="latency" className="col-start-1 row-start-1 text-ok tabular-nums data-[off=true]:invisible">
              {t("direct")} 63 ms
            </span>
          </span>
        }
        rows={[
          <MessageRow key="m" refs={phone} titles={titles}>
            {textOf(first.phone, messages)}
          </MessageRow>,
        ]}
      />
    </Stage>
  );
}
