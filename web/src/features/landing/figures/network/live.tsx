import { useCallback, useEffect, useRef } from "react";
import { useAnimationFrame, useInView, useReducedMotion } from "motion/react";
import { Art } from "../art";
import type { Flat } from "../iso-kit";
import {
  CAPSULE_ITEMS,
  CAPSULE_START,
  CUT_AT,
  DIRECT_LEFT,
  DIRECT_LEFT_CUT,
  DIRECT_PATH,
  DIRECT_RIGHT,
  DIRECT_RIGHT_CUT,
  HEIGHT,
  LAMP_AT,
  RELAY_LEFT,
  RELAY_PATH,
  RELAY_RIGHT,
  STATIC_ART,
  WIDTH,
  shiftTo,
} from "./model";

/*
 * The live bench. A calm 10.5 s tour: the capsule crosses straight from the
 * phone to the computer, the direct cable is cut, the capsule climbs through
 * the relay and comes down, and the direct cable is restored. The clock runs
 * only on screen; hovering holds it, and reduced motion draws one crossing.
 */

export interface NetworkLabels {
  direct: string;
  relay: string;
  switching: string;
}

type Phase = "load" | "direct" | "fault" | "relay" | "restore";

const LOOP = 10.5;
const ease = (t: number) => {
  const u = Math.min(1, Math.max(0, t));
  return u * u * (3 - 2 * u);
};

const script = (t: number): { phase: Phase; which: "direct" | "relay" | "none"; p: number; cut: boolean } => {
  const u = t % LOOP;
  if (u < 0.5) return { phase: "load", which: "none", p: 0, cut: false };
  if (u < 3.3) return { phase: "direct", which: "direct", p: ease((u - 0.5) / 2.8), cut: false };
  if (u < 4.3) return { phase: "fault", which: "none", p: 1, cut: true };
  if (u < 8.2) return { phase: "relay", which: "relay", p: ease((u - 4.3) / 3.9), cut: true };
  return { phase: "restore", which: "none", p: 0, cut: false };
};

const pointOf = (path: Flat[], u: number): Flat => {
  const f = Math.min(0.999999, Math.max(0, u)) * (path.length - 1);
  const i = Math.floor(f);
  const j = Math.min(path.length - 1, i + 1);
  const t = f - i;
  return [path[i]![0] + (path[j]![0] - path[i]![0]) * t, path[i]![1] + (path[j]![1] - path[i]![1]) * t];
};

const round = (n: number) => Math.round(n * 100) / 100;
const GREY = "var(--anatomy-lo)";
const OK = "var(--anatomy-green)";

export function NetworkLive({ label, labels, onReadout }: { label: string; labels: NetworkLabels; onReadout: (text: string) => void }) {
  const svg = useRef<SVGSVGElement>(null);
  const capsule = useRef<SVGGElement>(null);
  const left = useRef<SVGPathElement>(null);
  const right = useRef<SVGPathElement>(null);
  const relayLeft = useRef<SVGPathElement>(null);
  const relayRight = useRef<SVGPathElement>(null);
  const lamp = useRef<SVGCircleElement>(null);
  const cut = useRef<SVGGElement>(null);

  const inView = useInView(svg, { margin: "120px 0px" });
  const calm = useReducedMotion();
  const held = useRef(false);
  const clock = useRef(0);
  const lastPhase = useRef<Phase | null>(null);
  const labelsRef = useRef(labels);
  labelsRef.current = labels;
  const readRef = useRef(onReadout);
  readRef.current = onReadout;
  const calmRef = useRef(Boolean(calm));
  calmRef.current = Boolean(calm);

  const draw = useCallback((t: number) => {
    const s = script(t);

    if (capsule.current) {
      capsule.current.style.display = s.which === "none" ? "none" : "";
      if (s.which === "direct") capsule.current.setAttribute("transform", shiftTo(pointOf(DIRECT_PATH, s.p)));
      if (s.which === "relay") capsule.current.setAttribute("transform", shiftTo(pointOf(RELAY_PATH, s.p)));
    }

    if (left.current) {
      left.current.setAttribute("d", s.cut ? DIRECT_LEFT_CUT : DIRECT_LEFT);
      left.current.style.stroke = s.cut ? "var(--anatomy-faint)" : "var(--anatomy-hi)";
    }
    if (right.current) {
      right.current.setAttribute("d", s.cut ? DIRECT_RIGHT_CUT : DIRECT_RIGHT);
      right.current.style.stroke = s.cut ? "var(--anatomy-faint)" : "var(--anatomy-hi)";
    }
    for (const path of [relayLeft.current, relayRight.current]) {
      if (path) path.style.stroke = s.phase === "relay" ? "var(--anatomy-hi)" : "var(--anatomy-lo)";
    }

    if (lamp.current) {
      const on = s.phase === "relay";
      lamp.current.style.stroke = on ? OK : GREY;
      lamp.current.style.fill = on ? OK : "none";
    }
    if (cut.current) cut.current.style.display = s.cut ? "" : "none";

    if (lastPhase.current !== s.phase) {
      lastPhase.current = s.phase;
      const next =
        s.phase === "fault" ? labelsRef.current.switching : s.phase === "relay" ? labelsRef.current.relay : labelsRef.current.direct;
      readRef.current(next);
    }
  }, []);

  useEffect(() => {
    if (calm) draw(1.9);
  }, [calm, draw]);

  useAnimationFrame((_time, delta) => {
    if (!inView || calm || held.current) return;
    clock.current += Math.min(delta, 33) / 1000;
    draw(clock.current);
  });

  return (
    <div className="fig-iso">
      <svg
        ref={svg}
        className="iso-svg"
        viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
        role="img"
        aria-label={label}
        focusable="false"
        onPointerEnter={(event) => {
          if (event.pointerType === "mouse") held.current = true;
        }}
        onPointerLeave={(event) => {
          if (event.pointerType === "mouse") held.current = false;
        }}
      >
        <Art items={STATIC_ART} />
        <g ref={cut} style={{ display: "none" }}>
          <circle cx={round(CUT_AT[0])} cy={round(CUT_AT[1])} r={2.2} fill="var(--color-warn)" />
          <path className="iso-line" data-tone="red" d={`M${round(CUT_AT[0] - 6)} ${round(CUT_AT[1] - 4)}l-3 8`} style={{ stroke: "var(--color-warn)" }} />
          <path className="iso-line" data-tone="red" d={`M${round(CUT_AT[0] + 6)} ${round(CUT_AT[1] - 4)}l3 8`} style={{ stroke: "var(--color-warn)" }} />
        </g>
        <path ref={relayLeft} className="iso-line" data-tone="lo" data-dashed="" d={RELAY_LEFT} />
        <path ref={relayRight} className="iso-line" data-tone="lo" data-dashed="" d={RELAY_RIGHT} />
        <path ref={left} className="iso-line" data-tone="hi" d={DIRECT_LEFT} />
        <path ref={right} className="iso-line" data-tone="hi" d={DIRECT_RIGHT} />
        <g ref={capsule} transform={CAPSULE_START}>
          <Art items={CAPSULE_ITEMS} />
        </g>
        <circle ref={lamp} cx={LAMP_AT[0]} cy={LAMP_AT[1]} r={2.2} fill="none" style={{ stroke: GREY }} />
      </svg>
    </div>
  );
}
