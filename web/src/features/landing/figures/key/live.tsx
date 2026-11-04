import { useCallback, useEffect, useRef } from "react";
import { useAnimationFrame, useInView, useReducedMotion } from "motion/react";
import { Art } from "../art";
import type { Flat } from "../iso-kit";
import {
  CAP_END,
  CAP_START,
  CAPSULE_ITEMS,
  CAPSULE_START,
  CABLE_SCREEN,
  COPY_ITEMS,
  HEIGHT,
  KEY_CABLE_PATH,
  LAMP_AT,
  LID_ITEMS,
  SEAL_AT,
  STATIC_ART,
  TOKEN_ITEMS,
  TOKEN_REST,
  WIDTH,
} from "./model";

/*
 * The live key bench. A calm 11.5 s tour: the two devices negotiate a key over
 * the cable above the Hub, the capsule is sealed at the computer, leaves an
 * encrypted copy at the Hub, and opens at the phone. The clock runs on screen
 * only; hovering holds it, reduced motion draws one delivery.
 */

export interface KeyLabels {
  keys: string;
  copy: string;
  opened: string;
  ciphertext: string;
}

type Phase = "handshake" | "seal" | "run" | "copy" | "deliver" | "open";

const LOOP = 11.5;
const ease = (t: number) => {
  const u = Math.min(1, Math.max(0, t));
  return u * u * (3 - 2 * u);
};

const script = (t: number): { phase: Phase; p: number; k: number; copy: boolean; lid: number } => {
  const u = t % LOOP;
  if (u < 0.3) return { phase: "handshake", p: 0, k: 0, copy: false, lid: 0 };
  if (u < 2.8) return { phase: "handshake", p: 0, k: ease((u - 0.3) / 2.5), copy: false, lid: 0 };
  if (u < 3.4) return { phase: "seal", p: 0, k: 1, copy: false, lid: 0 };
  if (u < 6.2) return { phase: "run", p: ease((u - 3.4) / 2.8) * 0.5, k: 1, copy: false, lid: 0 };
  if (u < 7.4) return { phase: "copy", p: 0.5, k: 1, copy: true, lid: 0 };
  if (u < 9.8) return { phase: "deliver", p: 0.5 + ease((u - 7.4) / 2.4) * 0.5, k: 1, copy: true, lid: 0 };
  return { phase: "open", p: 1, k: 1, copy: true, lid: 1 };
};

const mix = (a: Flat, b: Flat, t: number): Flat => [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
const round = (n: number) => Math.round(n * 100) / 100;
const move = (p: Flat) => `translate(${round(p[0])} ${round(p[1])})`;

const atCable = (u: number): Flat => {
  const f = Math.min(0.999999, Math.max(0, u)) * (CABLE_SCREEN.length - 1);
  const i = Math.floor(f);
  const j = Math.min(CABLE_SCREEN.length - 1, i + 1);
  return mix(CABLE_SCREEN[i]!, CABLE_SCREEN[j]!, f - i);
};

const GREY = "var(--anatomy-lo)";
const OK = "var(--anatomy-green)";

export function KeyLive({ label, labels, onReadout }: { label: string; labels: KeyLabels; onReadout: (text: string) => void }) {
  const svg = useRef<SVGSVGElement>(null);
  const capsule = useRef<SVGGElement>(null);
  const lid = useRef<SVGGElement>(null);
  const seal = useRef<SVGCircleElement>(null);
  const token = useRef<SVGGElement>(null);
  const copy = useRef<SVGGElement>(null);
  const glow = useRef<SVGPathElement>(null);
  const lamp = useRef<SVGCircleElement>(null);

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
    const p = Math.min(1, Math.max(0, s.p));

    if (capsule.current) capsule.current.setAttribute("transform", move(mix(CAP_START, CAP_END, p)));
    if (lid.current) lid.current.setAttribute("transform", `translate(0 ${round(-5 * s.lid)})`);

    if (copy.current) copy.current.style.display = s.copy ? "" : "none";

    const tokenOn = s.phase === "handshake";
    if (token.current) {
      token.current.style.display = tokenOn ? "" : "none";
      if (tokenOn) token.current.setAttribute("transform", move(atCable(s.k)));
    }
    if (glow.current) glow.current.style.opacity = tokenOn ? "0.5" : "0";

    if (lamp.current) {
      lamp.current.style.stroke = s.copy ? OK : GREY;
      lamp.current.style.fill = s.copy ? OK : "none";
    }
    if (seal.current) {
      const sealed = s.phase !== "handshake";
      seal.current.style.stroke = sealed ? OK : GREY;
      seal.current.style.fill = sealed ? OK : "none";
    }

    if (lastPhase.current !== s.phase) {
      lastPhase.current = s.phase;
      readRef.current(
        s.phase === "handshake"
          ? labelsRef.current.keys
          : s.phase === "copy"
            ? labelsRef.current.copy
            : s.phase === "open"
              ? labelsRef.current.opened
              : labelsRef.current.ciphertext,
      );
    }
  }, []);

  useEffect(() => {
    if (calm) draw(9);
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
        <path ref={glow} className="iso-line" data-tone="green" d={KEY_CABLE_PATH} style={{ strokeWidth: 2.2, opacity: 0 }} />
        <g ref={token} transform={move(TOKEN_REST)} style={{ display: "none" }}>
          <Art items={TOKEN_ITEMS} />
        </g>
        <circle ref={lamp} cx={LAMP_AT[0]} cy={LAMP_AT[1]} r={2} fill="none" style={{ stroke: GREY }} />
        <g ref={copy} transform="translate(0 0)" style={{ display: "none" }}>
          <Art items={COPY_ITEMS} />
        </g>
        <g ref={capsule} transform={CAPSULE_START}>
          <Art items={CAPSULE_ITEMS} />
          <circle ref={seal} cx={SEAL_AT[0]} cy={SEAL_AT[1]} r={1.6} fill="none" style={{ stroke: GREY }} />
          <g ref={lid} transform="translate(0 0)">
            <Art items={LID_ITEMS} />
          </g>
        </g>
      </svg>
    </div>
  );
}
