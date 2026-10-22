import { type RefObject, useEffect, useLayoutEffect, useRef, useState } from "react";

/*
 * The landing page's motion kit: no animation runtime, just SVG wires in the
 * page's own pixels, a few nodes, and one requestAnimationFrame loop per
 * diagram that writes transforms and text straight into the DOM (React only
 * renders the layout, never a frame).
 *
 * Every diagram:
 * - runs only while some of it is on screen (IntersectionObserver) and the tab
 *   is visible (rAF stops by itself in a hidden tab), so the page idles once
 *   the reader has scrolled past;
 * - is drawn 1:1 in CSS pixels (the SVG's viewBox is its measured size), so a
 *   1px hairline lands on a pixel and stays crisp at any width, on retina or
 *   not; wires and boxes snap to the grid, only the moving packets do not;
 * - holds a still, annotated frame under `prefers-reduced-motion`.
 */

export interface Pt {
  x: number;
  y: number;
}

export const clamp01 = (n: number) => (n < 0 ? 0 : n > 1 ? 1 : n);

/** Progress of `t` through the window [a, b], clamped to 0..1. */
export const span = (t: number, a: number, b: number) => clamp01((t - a) / (b - a));

/** Ease in and out (cubic). */
export const ease = (p: number) => (p < 0.5 ? 4 * p * p * p : 1 - Math.pow(-2 * p + 2, 3) / 2);

/** A coordinate on a hairline: the middle of a pixel, so a 1px stroke fills exactly one. */
export const px = (n: number) => Math.round(n) + 0.5;

export function polyLength(points: Pt[]): number {
  let total = 0;
  for (let i = 1; i < points.length; i++) total += Math.hypot(points[i]!.x - points[i - 1]!.x, points[i]!.y - points[i - 1]!.y);
  return total;
}

/** The point a fraction `f` of the way along a polyline. */
export function pointAt(points: Pt[], f: number): Pt {
  const first = points[0];
  if (!first) return { x: 0, y: 0 };
  if (points.length === 1 || f <= 0) return first;
  let left = clamp01(f) * polyLength(points);
  for (let i = 1; i < points.length; i++) {
    const a = points[i - 1]!;
    const b = points[i]!;
    const d = Math.hypot(b.x - a.x, b.y - a.y);
    if (left <= d || i === points.length - 1) {
      const k = d === 0 ? 0 : Math.min(1, left / d);
      return { x: a.x + (b.x - a.x) * k, y: a.y + (b.y - a.y) * k };
    }
    left -= d;
  }
  return points[points.length - 1]!;
}

/** An SVG path through `points`, each snapped to a pixel centre. */
export function pathD(points: Pt[], offset = 0.5): string {
  return points.map((p, i) => `${i ? "L" : "M"}${Math.round(p.x) + offset} ${Math.round(p.y) + offset}`).join("");
}

/**
 * How many times a wire turns. The drafting sheet is right-angled and a
 * wire bends at most once: a straight line or a single L.
 */
export function corners(points: Pt[]): number {
  let turns = 0;
  for (let i = 1; i < points.length - 1; i++) {
    const a = points[i - 1]!;
    const b = points[i]!;
    const c = points[i + 1]!;
    const cross = (b.x - a.x) * (c.y - b.y) - (b.y - a.y) * (c.x - b.x);
    if (Math.abs(cross) > 1e-9) turns++;
  }
  return turns;
}

const HEX = "0123456789abcdef";

/** A small deterministic generator, so a scramble looks the same every loop and in every test. */
function hash(a: number, b: number): number {
  let h = (a * 374761393 + b * 668265263) | 0;
  h = Math.imul(h ^ (h >>> 13), 1274126177);
  return (h ^ (h >>> 16)) >>> 0;
}

export const wide = (ch: string) => /[ᄀ-ᅟ⺀-꓏가-힣豈-﫿︰-﹏＀-｠￠-￦　-〿]/.test(ch);

/**
 * Ciphertext the width of `text`: hex, two digits under each wide (CJK)
 * character and one under every other, spaces kept, so a line keeps its shape
 * as it seals and opens in a monospace box.
 */
export function cipherOf(text: string, seed = 1, narrow = false): string {
  let out = "";
  let i = 0;
  for (const ch of text) {
    if (ch === " " || ch === "\n") out += ch;
    else {
      // `narrow`: one digit per character, so a one-line row never gets wider as it seals.
      const n = wide(ch) && !narrow ? 2 : 1;
      for (let k = 0; k < n; k++) out += HEX[hash(seed, i++) % 16];
    }
  }
  return out;
}

/**
 * `text` sealing into its ciphertext as `p` goes 0 → 1: a band of noise sweeps
 * left to right, plaintext before it, ciphertext behind it. `tick` changes the
 * noise (a few times a second reads as churning, not flicker).
 */
export function seal(text: string, p: number, seed = 1, tick = 0, narrow = false): string {
  if (p <= 0) return text;
  const cipher = cipherOf(text, seed, narrow);
  if (p >= 1) return cipher;
  // Cells: one per character of `text`, holding the cipher digits under it.
  const cells: { plain: string; cipher: string }[] = [];
  let at = 0;
  for (const ch of text) {
    const n = ch === " " || ch === "\n" ? 1 : wide(ch) && !narrow ? 2 : 1;
    cells.push({ plain: ch, cipher: cipher.slice(at, at + n) });
    at += n;
  }
  const band = 0.25;
  const front = p * (1 + band);
  let out = "";
  cells.forEach((cell, i) => {
    const x = cells.length <= 1 ? 0 : i / (cells.length - 1);
    if (cell.plain === " " || cell.plain === "\n") out += cell.plain;
    else if (x < front - band) out += cell.cipher;
    else if (x < front) {
      for (let k = 0; k < cell.cipher.length; k++) out += HEX[hash(tick * 31 + i, k + 7) % 16];
    } else out += cell.plain;
  });
  return out;
}

/**
 * `from` turning into `to` in place as `p` goes 0 → 1: a plaintext sealing
 * into its ciphertext, or a ciphertext opening into its plaintext. A band of
 * churning hex digits sweeps left to right, `to` behind it and `from` ahead
 * of it, so the line is never blank; `tick` changes the churn.
 */
export function morph(from: string, to: string, p: number, tick = 0): string {
  if (p <= 0) return from;
  if (p >= 1) return to;
  const a = [...from];
  const b = [...to];
  const n = Math.max(a.length, b.length);
  const band = 0.25;
  const front = p * (1 + band);
  let out = "";
  for (let i = 0; i < n; i++) {
    const x = n <= 1 ? 0 : i / (n - 1);
    if (x < front - band) out += b[i] ?? "";
    else if (x < front) out += b[i] === " " && a[i] === " " ? " " : HEX[hash(tick * 31 + i, 7) % 16];
    else out += a[i] ?? "";
  }
  return out;
}

/** Ciphertext grouped in fours, as a hex dump would print it: `9f3a c1e0 7b2d`. */
export function hexBlocks(count: number, seed: number): string {
  const groups: string[] = [];
  for (let g = 0; g < count; g++) {
    let s = "";
    for (let k = 0; k < 4; k++) s += HEX[hash(seed, g * 4 + k) % 16];
    groups.push(s);
  }
  return groups.join(" ");
}

/** Whether the reader asked for less motion (live). */
export function useReducedMotion(): boolean {
  const [reduced, setReduced] = useState(() =>
    typeof matchMedia === "function" ? matchMedia("(prefers-reduced-motion: reduce)").matches : false,
  );
  useEffect(() => {
    if (typeof matchMedia !== "function") return;
    const mq = matchMedia("(prefers-reduced-motion: reduce)");
    const change = () => setReduced(mq.matches);
    mq.addEventListener("change", change);
    return () => mq.removeEventListener("change", change);
  }, []);
  return reduced;
}

/**
 * The width of an element in whole CSS pixels, measured before the first
 * paint and on every resize. `fallback` is what a render without a DOM (the
 * tests, a prerender) draws.
 */
export function useSize(ref: RefObject<HTMLElement | null>, fallback: number): { w: number; h: number } {
  const [size, setSize] = useState({ w: fallback, h: 0 });
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const read = () => {
      const r = el.getBoundingClientRect();
      const next = { w: Math.max(1, Math.floor(r.width)), h: Math.floor(r.height) };
      setSize((s) => (s.w === next.w && s.h === next.h ? s : next));
    };
    read();
    if (typeof ResizeObserver !== "function") return;
    const ro = new ResizeObserver(read);
    ro.observe(el);
    return () => ro.disconnect();
  }, [ref]);
  return size;
}

export interface Loop {
  /** Jump to `t` seconds into the loop (and draw it). */
  seek(t: number): void;
}

/**
 * Drive `frame(t, round)` from requestAnimationFrame while `root` is on
 * screen, with `t` in seconds, wrapping at `duration`, and `round` counting
 * the wraps (a count that only ever grows, like pages kept). Time stands still while the diagram
 * is off screen or the tab hidden, and picks up where it stopped. Under
 * reduced motion `still()` draws once and nothing runs.
 *
 * `frame` and `still` are read through a ref, so a new closure each render is
 * fine; `deps` restarts the loop (a new layout, a new language).
 */
export function useLoop(
  root: RefObject<HTMLElement | null>,
  duration: number,
  draw: { frame: (t: number, round: number) => void; still: () => void },
  reduced: boolean,
  deps: unknown[],
): Loop {
  const drawRef = useRef(draw);
  drawRef.current = draw;
  const time = useRef(0);
  const round = useRef(0);
  const loop = useRef<Loop>({ seek: () => undefined });
  useEffect(() => {
    const el = root.current;
    if (!el) return;
    if (reduced) {
      drawRef.current.still();
      loop.current = { seek: () => drawRef.current.still() };
      return;
    }
    let raf = 0;
    let last = 0;
    const tick = (now: number) => {
      // A long gap (a stalled frame, a tab coming back) advances by one frame, not by the gap.
      const dt = last ? Math.min(now - last, 50) : 0;
      last = now;
      const next = time.current + dt / 1000;
      if (next >= duration) round.current++;
      time.current = next % duration;
      drawRef.current.frame(time.current, round.current);
      raf = requestAnimationFrame(tick);
    };
    const start = () => {
      if (raf) return;
      last = 0;
      raf = requestAnimationFrame(tick);
    };
    const stop = () => {
      if (raf) cancelAnimationFrame(raf);
      raf = 0;
    };
    loop.current = {
      seek: (t) => {
        time.current = ((t % duration) + duration) % duration;
        drawRef.current.frame(time.current, round.current);
      },
    };
    drawRef.current.frame(time.current, round.current);
    if (typeof IntersectionObserver !== "function") {
      start();
      return stop;
    }
    const io = new IntersectionObserver((entries) => {
      if (entries.some((e) => e.isIntersecting)) start();
      else stop();
    });
    io.observe(el);
    return () => {
      io.disconnect();
      stop();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [root, duration, reduced, ...deps]);
  return { seek: (t) => loop.current.seek(t) };
}

/** Set an attribute only when it changes (a frame writes the same value most of the time). */
export function setAttr(el: Element | null | undefined, name: string, value: string): void {
  if (el && el.getAttribute(name) !== value) el.setAttribute(name, value);
}

/** Show or hide an element with the `hidden` attribute, only when that changes. */
export function setHidden(el: Element | null | undefined, hidden: boolean): void {
  if (el && el.hasAttribute("hidden") !== hidden) el.toggleAttribute("hidden", hidden);
}

export function setText(el: Element | null | undefined, value: string): void {
  if (el && el.textContent !== value) el.textContent = value;
}

/** How strongly a chip `at` (0 → 1) along its wire is drawn: it fades in as it leaves a card and out as it reaches one. */
export const chipOpacity = (at: number) => Math.max(0, Math.min(1, at / 0.15, (1 - at) / 0.15));

/** Place a packet (an SVG group) at `p`, or hide it. */
export function place(el: SVGGraphicsElement | null | undefined, p: Pt | null, opacity = 1): void {
  if (!el) return;
  if (!p || opacity <= 0) {
    setAttr(el, "opacity", "0");
    return;
  }
  setAttr(el, "opacity", opacity >= 1 ? "1" : opacity.toFixed(2));
  el.setAttribute("transform", `translate(${p.x.toFixed(2)} ${p.y.toFixed(2)})`);
}
