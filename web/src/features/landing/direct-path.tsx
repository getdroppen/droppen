import { useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { latencyTone } from "@/features/cloud/cloud-bar";
import { cn } from "@/lib/cn";
import { place, pointAt, type Pt, setAttr, setText, span, useLoop, useReducedMotion, useSize } from "./motion";
import { CipherChip, InkMark, Ledger, ledgerH, LiveDot, Stage, WEIGHT, Wire, WIRE_STROKE, WireLock } from "./kit";

/*
 * "Direct when it can be": keystrokes from your phone to your computer go
 * straight across while a direct path holds, a Droppen relay standing by as
 * a dashed fallback; then the direct path drops, the keys it had not had
 * confirmed are sent again through the relay in order, typing carries on
 * through the relay, and the direct path comes back. Round trips are the
 * measured p50s. Every key is sealed on the device before it leaves (a
 * padlock on each wire leaving it, and each key a padlocked chip), whichever
 * way it goes. (`hub` in the names below is that relay.)
 */

export type Scenario = "same" | "phone";

/**
 * Measured p50 round trips, in ms (docs/cross-machine-architecture.md §9, "测量记录").
 * `hub` is the way through a relay: measured through Droppen Hub when it still
 * forwarded, in the region where the default relay now runs.
 */
export const MEASURED: Record<Scenario, { direct: [number, number]; hub: [number, number] }> = {
  same: { direct: [1, 2], hub: [170, 215] },
  phone: { direct: [63, 73], hub: [215, 227] },
};

const LOOP = 12;
const DROP = 5.0;
const FALLBACK = 5.45;
const BACK = 9.4;

/** The commands typed in one loop, and the line each prints. */
const SCRIPT = [
  { at: 0.4, cmd: "git status", out: "nothing to commit" },
  { at: 3.3, cmd: "npm test", out: "42 passed" },
  { at: 6.8, cmd: "git push", out: "main -> origin/main" },
  { at: 10.0, cmd: "ls", out: "README.md  src  web" },
];
const KEY_GAP = 0.2;
/** What the computer's terminal already shows before the loop's first line arrives: a terminal is never a blank box. */
export const TERMINAL_HISTORY = ["$ cd ~/code/shop-api", "$ git pull", "  Already up to date."] as const;

interface Key {
  /** Which command, and how many of its characters this key completes (Enter = one past the end). */
  line: number;
  upto: number;
  sent: number;
  /** Departure and arrival, and the route it arrived by. */
  depart: number;
  arrive: number;
  route: "direct" | "hub";
  /** Sent on the direct path and lost with it: where it was when the path dropped. */
  lostAt?: number;
}

/** Seconds a packet is drawn travelling, for a round trip of `ms`: slow enough to see, in proportion. */
const travel = (ms: number) => 0.14 + (ms / 1000) * 2.6;

export function schedule(scenario: Scenario): Key[] {
  const m = MEASURED[scenario];
  const direct = travel((m.direct[0] + m.direct[1]) / 2);
  const hub = travel((m.hub[0] + m.hub[1]) / 2);
  const keys: Key[] = [];
  let pending: Key[] = [];
  SCRIPT.forEach((line, li) => {
    for (let c = 1; c <= line.cmd.length + 1; c++) {
      const sent = line.at + (c - 1) * KEY_GAP;
      const onDirect = sent < DROP || sent >= BACK;
      const key: Key = { line: li, upto: c, sent, depart: sent, arrive: sent + (onDirect ? direct : hub), route: onDirect ? "direct" : "hub" };
      if (onDirect && sent < DROP && key.arrive > DROP) {
        // In flight when the path dropped: never confirmed, so sent again through the relay.
        key.lostAt = (DROP - sent) / direct;
        pending.push(key);
      }
      if (!onDirect && sent < FALLBACK) {
        // Typed while the page still thought the direct path was up: also unconfirmed.
        key.lostAt = 0;
        pending.push(key);
      }
      keys.push(key);
    }
  });
  // Resent through the relay, in their order, once the fallback kicks in.
  pending.forEach((key, i) => {
    key.route = "hub";
    key.depart = FALLBACK + i * 0.06;
    key.arrive = key.depart + hub;
  });
  pending = [];
  // Delivered in order: a key never lands before the one typed ahead of it.
  keys.forEach((key, i) => {
    if (i > 0) key.arrive = Math.max(key.arrive, keys[i - 1]!.arrive + 0.01);
  });
  return keys;
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
  device: Box;
  machine: Box;
  hub: Pt;
  /** The drawn wires, each straight or one L: the direct path, and the two halves of the way through the relay. */
  direct: Pt[];
  devHub: Pt[];
  hubMach: Pt[];
  /** The route a packet takes through the relay (both halves, under the relay's card between them). */
  viaHub: Pt[];
  cut: Pt;
  directReadout: Pt;
  hubReadout: Pt;
  /** The readouts' boxes (a readout is at most this wide), for the geometry checks. */
  labels: { x: number; y: number; w: number; h: number }[];
}

/** The Droppen relay's card in this diagram. */
export const DIRECT_HUB = { w: 156, h: 28 };

export function directLayout(w: number): Layout {
  const pad = 12;
  if (w < 560) {
    // A column: the device on top, the computer below; the relay path on the left, the direct line on the right.
    const device = { x: pad, y: pad, w: w - 2 * pad, h: ledgerH(1) };
    const hubY = device.y + device.h + 104;
    const machine = { x: pad, y: hubY + 84, w: w - 2 * pad, h: 120 };
    const h = machine.y + machine.h + 14;
    // The direct line near the right edge; its readout to its left, the relay's under the relay card: no line crosses either.
    const dx = w - 40;
    const hx = Math.round(w * 0.22);
    const hub = { x: Math.max(hx, pad + DIRECT_HUB.w / 2), y: hubY };
    const hubX = hub.x;
    const directReadoutY = device.y + device.h + 40;
    return {
      h,
      narrow: true,
      device,
      machine,
      hub,
      direct: [
        { x: dx, y: device.y + device.h },
        { x: dx, y: machine.y },
      ],
      devHub: [{ x: hubX, y: device.y + device.h }, { x: hubX, y: hub.y - DIRECT_HUB.h / 2 }],
      hubMach: [{ x: hubX, y: hub.y + DIRECT_HUB.h / 2 }, { x: hubX, y: machine.y }],
      viaHub: [{ x: hubX, y: device.y + device.h }, hub, { x: hubX, y: machine.y }],
      cut: { x: dx, y: hubY },
      directReadout: { x: dx, y: directReadoutY },
      labels: [
        { x: dx - 100, y: directReadoutY - 12, w: 90, h: 24 },
        { x: hubX + 10, y: hub.y + DIRECT_HUB.h / 2 + 10, w: Math.min(120, dx - 20 - hubX - 10), h: 24 },
      ],
      // Right of the line through the relay, never across it.
      hubReadout: { x: hubX + 10, y: hub.y + DIRECT_HUB.h / 2 + 22 },
    };
  }
  const cardW = Math.round(Math.min(236, w * 0.34));
  const top = 128;
  const device = { x: pad, y: top, w: cardW, h: ledgerH(1) };
  const machine = { x: w - pad - cardW, y: top, w: cardW, h: ledgerH(4) };
  const h = top + ledgerH(4) + 26;
  const mid = top + Math.round(ledgerH(1) / 2);
  const hub = { x: Math.round(w / 2), y: 44 };
  const dx = device.x + Math.round(cardW / 2);
  const mx = machine.x + Math.round(cardW / 2);
  const half = DIRECT_HUB.w / 2;
  return {
    h,
    narrow: false,
    device,
    machine,
    hub,
    direct: [
      { x: device.x + device.w, y: mid },
      { x: machine.x, y: mid },
    ],
    // Up from the phone and along into the relay; out of the relay and down to the computer: one L each.
    devHub: [{ x: dx, y: top }, { x: dx, y: hub.y }, { x: hub.x - half, y: hub.y }],
    hubMach: [{ x: hub.x + half, y: hub.y }, { x: mx, y: hub.y }, { x: mx, y: top }],
    viaHub: [{ x: dx, y: top }, { x: dx, y: hub.y }, hub, { x: mx, y: hub.y }, { x: mx, y: top }],
    cut: { x: Math.round(w / 2), y: mid },
    directReadout: { x: Math.round(w / 2), y: mid + 26 },
    labels: [
      { x: Math.round(w / 2) - 70, y: mid + 14, w: 140, h: 24 },
      { x: hub.x - 70, y: hub.y + DIRECT_HUB.h / 2 + 10, w: 140, h: 24 },
    ],
    hubReadout: { x: hub.x, y: hub.y + DIRECT_HUB.h / 2 + 22 },
  };
}

/** The padlocks on the wires leaving the device (the direct line and the way to the relay), just outside it. */
export function directLocks(L: Layout): Pt[] {
  const d = pointAt(L.direct, 0);
  const h = pointAt(L.devHub, 0);
  return L.narrow
    ? [
        { x: d.x, y: d.y + 12 },
        { x: h.x, y: h.y + 12 },
      ]
    : [
        { x: d.x + 10, y: d.y },
        { x: h.x, y: h.y - 12 },
      ];
}

/** Every drawn wire of a direct-path layout, for the geometry checks. */
export function directWires(L: Layout): Pt[][] {
  return [L.direct, L.devHub, L.hubMach];
}

/** A round trip within the measured range, varying key to key. */
function sample([lo, hi]: [number, number], i: number): number {
  return lo + ((i * 7919) % 11) / 10 * (hi - lo);
}

export function DirectPath() {
  const { t } = useTranslation();
  // The headline is about typing from a phone, so the phone on 4G is what shows first.
  const [scenario, setScenario] = useState<Scenario>("phone");
  const box = useRef<HTMLDivElement>(null);
  const { w, h: boxH } = useSize(box, 680);
  const L = directLayout(w);
  const reduced = useReducedMotion();
  const keys = useMemo(() => schedule(scenario), [scenario]);
  const m = MEASURED[scenario];

  const root = useRef<HTMLDivElement>(null);
  const directWire = useRef<SVGPathElement>(null);
  // The way through the relay, drawn twice: standing by (dashed, light) and carrying (solid ink).
  const hubIdle = useRef<SVGGElement>(null);
  const hubLive = useRef<SVGGElement>(null);
  const cut = useRef<HTMLSpanElement>(null);
  const typed = useRef<HTMLSpanElement>(null);
  const term = useRef<HTMLSpanElement>(null);
  const directValue = useRef<HTMLSpanElement>(null);
  const hubValue = useRef<HTMLSpanElement>(null);
  const directBox = useRef<HTMLSpanElement>(null);
  const hubBox = useRef<HTMLSpanElement>(null);
  const status = useRef<HTMLSpanElement>(null);
  const counts = useRef<HTMLSpanElement>(null);
  const devPath = useRef<HTMLSpanElement>(null);
  const hubCard = useRef<HTMLDivElement>(null);
  const packets = useRef<(SVGGElement | null)[]>([]);

  const directLive = t("direct path up");
  const directLost = t("direct path lost: back on the Droppen relay, unconfirmed keys sent again in order");
  const directBack = t("network back: direct again");
  const lostWord = t("lost");

  const tone = (ms: number) => latencyTone(ms);
  const fmt = (ms: number) => `${Math.round(ms)} ms`;

  const show = (time: number) => {
    const phase = time < DROP ? "direct" : time < BACK ? "hub" : "back";
    setAttr(root.current, "data-path", phase === "hub" ? "hub" : "direct");
    // Wires: the live path inked (green when it is the direct one), the other dashed.
    const dropped = time >= DROP && time < BACK;
    setAttr(directWire.current, "stroke", dropped ? WIRE_STROKE.danger : WIRE_STROKE.ok);
    setAttr(directWire.current, "stroke-dasharray", dropped ? WEIGHT.direct.dash : "none");
    setAttr(hubIdle.current, "visibility", dropped ? "hidden" : "visible");
    setAttr(hubLive.current, "visibility", dropped ? "visible" : "hidden");
    setAttr(cut.current, "data-on", dropped ? "true" : "false");
    // Packets.
    let sent = 0;
    let delivered = 0;
    let lastDirect: number | null = null;
    let lastHub: number | null = null;
    keys.forEach((key, i) => {
      const el = packets.current[i];
      if (time >= key.sent) sent++;
      if (time >= key.arrive) {
        delivered++;
        if (key.route === "direct") lastDirect = sample(m.direct, i);
        else lastHub = sample(m.hub, i);
      }
      let at: Pt | null = null;
      if (key.lostAt !== undefined && time >= key.sent && time < DROP) {
        // On its way across the direct path when it drops.
        at = pointAt(L.direct, span(time, key.sent, key.sent + (DROP - key.sent) / Math.max(key.lostAt, 0.001)));
      } else if (time >= key.depart && time < key.arrive) {
        at = pointAt(key.route === "direct" ? L.direct : L.viaHub, span(time, key.depart, key.arrive));
      }
      place(el, at);
    });
    // What has been typed (the phone) and what has arrived (the computer).
    let line = 0;
    let upto = 0;
    keys.forEach((key) => {
      if (time >= key.sent) {
        line = key.line;
        upto = key.upto;
      }
    });
    const cmd = SCRIPT[line]!.cmd;
    setText(typed.current, `$ ${cmd.slice(0, Math.min(upto, cmd.length))}${upto > cmd.length ? " ⏎" : ""}`);
    const lines: string[] = [...TERMINAL_HISTORY];
    SCRIPT.forEach((s, li) => {
      const arrived = keys.filter((k) => k.line === li && time >= k.arrive).length;
      if (arrived === 0) return;
      lines.push(`$ ${s.cmd.slice(0, Math.min(arrived, s.cmd.length))}`);
      if (arrived > s.cmd.length) lines.push(`  ${s.out}`);
    });
    // The newest lines that fit the card.
    const fit = Math.max(1, Math.floor((L.machine.h - 26 - 12) / 18));
    setText(term.current, lines.slice(-fit).join("\n"));
    // Readouts.
    const directMs = phase === "hub" ? null : (lastDirect ?? sample(m.direct, 0));
    const hubMs = lastHub ?? sample(m.hub, 3);
    setText(directValue.current, directMs === null ? lostWord : fmt(directMs));
    setAttr(directValue.current, "data-latency", directMs === null ? "danger" : tone(directMs));
    setText(hubValue.current, fmt(hubMs));
    setAttr(hubValue.current, "data-latency", tone(hubMs));
    setAttr(directBox.current, "data-active", phase === "hub" ? "false" : "true");
    setAttr(hubBox.current, "data-active", phase === "hub" ? "true" : "false");
    setText(status.current, phase === "direct" ? directLive : phase === "hub" ? directLost : directBack);
    setAttr(status.current, "data-tone", phase === "hub" ? "danger" : "ok");
    setText(counts.current, t("keys {{sent}} · delivered {{delivered}} · lost 0", { sent, delivered }));
    // The device's own path tag (the path and its round trip), and what Droppen is doing.
    const activeMs = phase === "hub" ? hubMs : (directMs ?? hubMs);
    setText(devPath.current, `${phase === "hub" ? t("relay") : t("direct")} ${fmt(activeMs)}`);
    setAttr(devPath.current, "data-latency", tone(activeMs));
    setAttr(hubCard.current, "data-dim", phase === "hub" ? "false" : "true");
  };

  useLoop(
    box,
    LOOP,
    {
      frame: show,
      still: () => {
        // The steady state, annotated: the direct path live, the relay dashed beside it.
        show(DROP - 0.6);
        keys.forEach((_, i) => place(packets.current[i], null));
        place(packets.current[0], pointAt(L.direct, 0.5));
        setText(status.current, t("if the direct path drops, the Droppen relay takes over within 1.5 s and no keystroke is lost"));
        setAttr(status.current, "data-tone", "ok");
      },
    },
    reduced,
    [w, scenario, t],
  );

  const readout =
    "pointer-events-none absolute flex h-6 items-center gap-1.5 border bg-bg px-2 font-mono text-[11px] whitespace-nowrap tabular-nums data-[active=false]:border-dashed data-[active=false]:border-border-strong data-[active=true]:border-fg";
  const value = "data-[latency=ok]:text-ok data-[latency=warn]:text-warn-fg data-[latency=danger]:text-danger";
  const scenarios: { id: Scenario; label: string }[] = [
    { id: "phone", label: t("on 4G") },
    { id: "same", label: t("same computer") },
  ];
  return (
    <div ref={root} data-path="direct" className="flex h-full flex-col">
      <div className="flex flex-wrap items-center gap-x-4 gap-y-2 border-b border-border px-6 py-3 max-md:px-4">
        <div role="tablist" aria-label={t("measured on")} className="flex h-9 border border-border-strong">
          {scenarios.map((s) => (
            <button
              key={s.id}
              type="button"
              role="tab"
              aria-selected={scenario === s.id}
              onClick={() => setScenario(s.id)}
              className={cn(
                "flex items-center border-r border-border px-3 font-mono text-2xs font-medium last:border-r-0",
                scenario === s.id ? "bg-fg text-bg" : "text-muted hover:text-fg",
              )}
            >
              {s.label}
            </button>
          ))}
        </div>
        <span className="font-mono text-2xs text-faint tabular-nums">
          {t("measured p50 round trip · direct {{d}} · via a Droppen relay {{h}}", {
            d: `${m.direct[0]}–${m.direct[1]} ms`,
            h: `${m.hub[0]}–${m.hub[1]} ms`,
          })}
        </span>
      </div>
      <Stage
      name="direct"
        ref={box}
        width={w}
        height={Math.max(boxH, L.h)}
        drawn={L.h}
        className="flex-1"
        still={reduced}
        svg={
          <>
            <g ref={hubIdle}>
              <Wire points={L.devHub} tone="faint" weight="fallback" dashed />
              <Wire points={L.hubMach} tone="faint" weight="fallback" dashed />
            </g>
            <g ref={hubLive} visibility="hidden">
              <Wire points={L.devHub} />
              <Wire points={L.hubMach} />
            </g>
            <Wire ref={directWire} points={L.direct} tone="ok" weight="direct" />
            {directLocks(L).map((at, i) => (
              <WireLock key={i} at={at} title={t("encrypted locally")} />
            ))}
            {keys.map((_, i) => (
              <CipherChip key={i} ref={(el) => void (packets.current[i] = el)} />
            ))}
          </>
        }
      >
        <div ref={hubCard} data-dim="false" className="contents">
        <Ledger
            box={{ x: L.hub.x - DIRECT_HUB.w / 2, y: L.hub.y - DIRECT_HUB.h / 2, w: DIRECT_HUB.w, h: DIRECT_HUB.h }}
            ink
            head={
              <>
                <InkMark size={14} />
                {t("Droppen relay")}
              </>
            }
            className="transition-opacity duration-300 [[data-dim=true]>&]:opacity-35"
            rows={[]}
          />
        </div>
        <span
          ref={hubBox}
          data-active="false"
          className={readout}
          style={L.narrow ? { left: L.hubReadout.x, top: L.hubReadout.y - 12, maxWidth: L.labels[1]!.w } : { left: L.hubReadout.x - 70, top: L.hubReadout.y - 12, width: 140, justifyContent: "center" }}
        >
          <span className="text-muted">{t("relay")}</span>
          <span ref={hubValue} data-signal="latency" className={value} />
        </span>
        <span
          ref={directBox}
          data-active="true"
          className={readout}
          style={L.narrow ? { left: L.directReadout.x - 100, top: L.directReadout.y - 12, width: 90, justifyContent: "flex-end" } : { left: L.directReadout.x - 70, top: L.directReadout.y - 12, width: 140, justifyContent: "center" }}
        >
          <span className="text-muted">{t("direct")}</span>
          <span ref={directValue} data-signal="latency" className={value} />
        </span>
        <span
          ref={cut}
          data-on="false"
          data-signal="failed"
          className="absolute flex size-4 items-center justify-center bg-bg font-mono text-[13px] leading-none text-danger data-[on=false]:hidden"
          style={{ left: L.cut.x - 8, top: L.cut.y - 8 }}
        >
          ×
        </span>
        <Ledger
          box={L.device}
          head={
            scenario === "phone" ? (
              <>
                {t("your phone")}
              </>
            ) : (
              <>
                {t("browser")}
              </>
            )
          }
          status={<span ref={devPath} data-signal="latency" className={cn("min-w-[11ch] text-right tabular-nums", value)} />}
          rows={[
            <span key="t" className="text-xs text-fg">
              {/* The caret is painted at the end of the line's own box: it grows with the line, nothing moves. */}
              <span ref={typed} className="bg-[linear-gradient(to_left,var(--color-fg)_6px,transparent_6px)] pr-[7px]">
                ${" "}
              </span>
            </span>,
          ]}
        />
        <Ledger
          box={L.machine}
          head={
            <>
              {t("your computer")}
            </>
          }
          status={<LiveDot />}
          rows={[]}
        >
          <span ref={term} className="block px-2.5 py-1.5 font-mono text-xs leading-[18px] whitespace-pre text-fg">
            {TERMINAL_HISTORY.join("\n")}
          </span>
        </Ledger>
      </Stage>
      {/* Fixed height and a fixed-width count: a status that changes length moves nothing on the page. */}
      {/* Fixed height and a fixed-width count: a status that changes length moves nothing on the page. */}
      <div className="grid h-10 grid-cols-[minmax(0,1fr)_auto] items-center gap-4 border-t border-border px-6 max-md:h-14 max-md:px-4">
        <span
          ref={status}
          data-tone="ok"
          data-signal="path"
          className="line-clamp-2 font-mono text-2xs leading-4 data-[tone=danger]:text-danger data-[tone=ok]:text-ok"
        >
          {directLive}
        </span>
        <span ref={counts} className="min-w-[24ch] text-right font-mono text-2xs text-muted tabular-nums" />
      </div>
    </div>
  );
}
