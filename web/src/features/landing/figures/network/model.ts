import * as k from "../iso-kit";
import type { Flat, LineTone, SolidPaths, Tone } from "../iso-kit";
import type { ArtItem } from "../art";

/*
 * The connection path as three objects on a bench: your phone, Droppen's relay
 * node, and your computer. The capsule crosses straight between the devices;
 * when that cable is cut, it climbs to the relay and comes back down. No rails,
 * no lifts: the devices and the relay are the subject.
 */

export const WIDTH = 660;
export const HEIGHT = 340;

const AZ = 58;
const EL = 32;

const BASE = { x: -150, y: -58, w: 300, d: 100, r: 7 };
const BASE_H = 10;
const FOOT_H = 5;

const PHONE = { x: -112, y: 6 };
const COMPUTER = { x: 104, y: 6 };
const RELAY = { x: 0, y: -26 };

export const NETWORK = { direct: [63, 73] as const, relay: [215, 227] as const };

export const PROJECTION = k.fitProjection(
  [
    ...k.boxCorners(BASE, -BASE_H - FOOT_H, 0),
    [RELAY.x, RELAY.y, 96],
    [PHONE.x, PHONE.y, 62],
    [COMPUTER.x, COMPUTER.y, 64],
  ],
  WIDTH,
  HEIGHT,
  { pad: 28, azimuth: AZ, elevation: EL },
);

export const at = (point: [number, number, number]): Flat => k.iso(point, PROJECTION);

type SolidStyle = { tone?: Tone; crease?: Tone | "none" };
type LineStyle = { tone?: LineTone; dashed?: boolean; dotted?: boolean; free?: boolean; width?: number };

const S = (paths: SolidPaths, style: SolidStyle = {}): ArtItem => ({ kind: "solid", paths, ...style });
const L = (d: string, style: LineStyle = {}): ArtItem => ({ kind: "line", d, ...style });
const solid = (plan: k.Plan, z: number, h: number, steps = 6, bevel = 0) => k.slabOf(plan, z, h, PROJECTION, steps, bevel);
const cyl = (x: number, y: number, r: number, z: number, h: number, steps = 32, bevel = 0) => k.cylinder(x, y, r, z, h, PROJECTION, steps, bevel);
const byDepth = (points: Flat[]) => [...points].sort((a, b) => k.depthOf([a[0], a[1], 0], PROJECTION) - k.depthOf([b[0], b[1], 0], PROJECTION));
const join = (list: string[]) => list.join("");

const faceLine = (x0: number, z0: number, x1: number, z1: number, y: number) => k.pathOf([at([x0, y, z0]), at([x1, y, z1])]);
const faceRect = (x0: number, z0: number, x1: number, z1: number, y: number) =>
  k.pathOf([at([x0, y, z0]), at([x1, y, z0]), at([x1, y, z1]), at([x0, y, z1])], true);

function plate(): ArtItem[] {
  const out: ArtItem[] = [];
  out.push({ kind: "halo", d: k.haloOf(BASE, -BASE_H - FOOT_H, BASE_H, PROJECTION) });
  for (const [x, y] of byDepth(k.corners(BASE, 24))) {
    out.push(S(cyl(x, y, 7.5, -BASE_H - FOOT_H, 1.6, 28), { tone: "lo" }));
    out.push(S(cyl(x, y, 5.6, -BASE_H - FOOT_H + 1.6, FOOT_H - 1.6, 28), { tone: "mid" }));
  }
  out.push(S(solid(BASE, -BASE_H, BASE_H, 8, 1.6), { tone: "mid" }));
  out.push(L(k.planOutline(k.insetPlan(BASE, 5), 0, PROJECTION), { tone: "lo" }));
  const screws = k.corners(BASE, 11);
  out.push(L(join(screws.map(([x, y]) => k.ring(x, y, 2.3, 0, PROJECTION, 16))), { tone: "lo" }));
  out.push({ kind: "dots", points: screws.map(([x, y]) => at([x, y, 0])), size: 0.5 });

  const strip = { x: -134, y: BASE.y + BASE.d - 15, w: 268, d: 7, r: 1 };
  out.push(S(solid(strip, 0, 0.7, 3), { tone: "lo", crease: "none" }));
  const ticks = k.topTicks(strip.x + 4, strip.x + strip.w - 4, 4, 5, strip.y + 0.9, 0.7, [1.8, 3.4], PROJECTION);
  out.push(L(ticks.minor, { tone: "lo" }), L(ticks.major, { tone: "mid" }));
  return out;
}

function phone(): ArtItem[] {
  const out: ArtItem[] = [];
  const dock = { x: PHONE.x - 17, y: PHONE.y - 10, w: 34, d: 26, r: 4 };
  out.push(S(solid(dock, 0, 9, 4, 0.8), { tone: "mid" }));
  out.push(L(k.planOutline(k.insetPlan(dock, 2.6), 9, PROJECTION, 4), { tone: "faint" }));
  out.push({ kind: "dots", points: k.corners(dock, 5).map(([x, y]) => at([x, y, 9])), size: 0.45 });

  const body = { x: PHONE.x - 13, y: PHONE.y - 3, w: 26, d: 7, r: 3.4 };
  out.push(S(solid(body, 9, 46, 5, 0.8), { tone: "hi", crease: "lo" }));
  const face = PHONE.y + 4.2;
  out.push(L(faceRect(PHONE.x - 9.6, 17, PHONE.x + 9.6, 48, face), { tone: "faint" }));
  out.push(L(faceLine(PHONE.x - 6, 43, PHONE.x + 5, 43, face), { tone: "lo" }));
  out.push(L(faceLine(PHONE.x - 6, 39, PHONE.x + 2, 39, face), { tone: "lo" }));
  out.push(L(faceLine(PHONE.x - 6, 35, PHONE.x - 1, 35, face), { tone: "lo" }));
  out.push(L(faceLine(PHONE.x - 6, 30, PHONE.x + 6, 30, face), { tone: "faint" }));
  out.push(L(faceLine(PHONE.x - 6, 22, PHONE.x + 6, 22, face), { tone: "faint" }));
  out.push({ kind: "dots", points: [at([PHONE.x, face, 12.4])], size: 0.6, tone: "mid" });
  // A single button on the near edge.
  out.push(S(solid({ x: PHONE.x - 2.6, y: PHONE.y + 3.4, w: 5.2, d: 1.4, r: 0.6 }, 9, 1.2, 2), { tone: "lo" }));
  return out;
}

function computer(): ArtItem[] {
  const out: ArtItem[] = [];
  const base = { x: COMPUTER.x - 24, y: COMPUTER.y - 12, w: 48, d: 28, r: 3.4 };
  out.push(S(solid(base, 0, 5, 4, 0.6), { tone: "mid" }));
  out.push(L(k.planOutline(k.insetPlan(base, 2.4), 5, PROJECTION, 4), { tone: "faint" }));
  // Keyboard in front of the screen.
  const keys = { x: COMPUTER.x - 20, y: COMPUTER.y + 2, w: 40, d: 11, r: 1.2 };
  out.push(S(solid(keys, 5, 1.8, 3, 0.3), { tone: "hi", crease: "lo" }));
  for (let i = 0; i < 7; i++) {
    out.push(L(k.lineOnTop([keys.x + 3 + i * 5.2, keys.y + 2], [keys.x + 3 + i * 5.2, keys.y + keys.d - 2], 6.8, PROJECTION), { tone: "faint", free: true }));
  }
  // The screen, on a short neck.
  out.push(S(solid({ x: COMPUTER.x - 4, y: COMPUTER.y - 3, w: 8, d: 5, r: 1 }, 5, 14, 3, 0.4), { tone: "mid" }));
  const frame = { x: COMPUTER.x - 22, y: COMPUTER.y - 4.5, w: 44, d: 5, r: 2.6 };
  out.push(S(solid(frame, 17, 38, 5, 0.7), { tone: "hi", crease: "lo" }));
  const face = COMPUTER.y - 5.2;
  out.push(L(faceRect(COMPUTER.x - 18, 21, COMPUTER.x + 18, 50, face), { tone: "faint" }));
  out.push(L(faceLine(COMPUTER.x - 14, 46, COMPUTER.x + 6, 46, face), { tone: "lo" }));
  out.push(L(faceLine(COMPUTER.x - 14, 42, COMPUTER.x + 2, 42, face), { tone: "lo" }));
  out.push(L(faceLine(COMPUTER.x - 14, 38, COMPUTER.x - 4, 38, face), { tone: "lo" }));
  out.push({ kind: "dots", points: [at([COMPUTER.x, face, 19.6])], size: 0.6, tone: "mid" });
  return out;
}

function relay(): ArtItem[] {
  const out: ArtItem[] = [];
  const cab = { x: RELAY.x - 17, y: RELAY.y - 14, w: 34, d: 28, r: 1.8 };
  out.push(S(solid(cab, 0, 46, 5, 0.7), { tone: "mid" }));
  out.push(L(k.planOutline(k.insetPlan(cab, 3), 46, PROJECTION, 5), { tone: "faint" }));
  out.push(L(k.sideSeam(cab, 30, PROJECTION, 4), { tone: "faint" }));
  // Vents on the top.
  for (let i = 0; i < 5; i++) {
    out.push(L(k.lineOnTop([cab.x + 6 + i * 5.4, cab.y + 6], [cab.x + 6 + i * 5.4, cab.y + 16], 46, PROJECTION), { tone: "faint" }));
  }
  // A status light on the near face.
  out.push(L(k.ring(RELAY.x - 9, RELAY.y + 14.2, 2.1, 34, PROJECTION, 16), { tone: "lo" }));
  // The antenna: a mast, a ring, and a crown.
  const mast = { x: RELAY.x - 1.4, y: RELAY.y - 21, w: 2.8, d: 2.8, r: 0.6 };
  out.push(S(solid(mast, 46, 42, 3, 0.3), { tone: "mid" }));
  out.push(S(cyl(RELAY.x, RELAY.y - 19.6, 3.4, 84, 2.4, 20), { tone: "mid" }));
  out.push(L(k.ring(RELAY.x, RELAY.y - 19.6, 4.6, 86.4, PROJECTION, 20), { tone: "lo" }));
  out.push(S(cyl(RELAY.x, RELAY.y - 19.6, 1.1, 88, 6, 12), { tone: "hi" }));
  // Two guy wires down to the cabinet.
  out.push(L(k.pathOf([at([RELAY.x + 1, RELAY.y - 19.6, 86]), at([RELAY.x + 15, RELAY.y + 12, 46])]), { tone: "faint" }));
  out.push(L(k.pathOf([at([RELAY.x - 1, RELAY.y - 19.6, 86]), at([RELAY.x - 15, RELAY.y + 12, 46])]), { tone: "faint" }));
  return out;
}

// The direct cable leaves the phone at its top corner, bows forward so it
// passes clearly in front of the relay, and reaches the computer's screen.
const PHONE_ANCHOR: [number, number, number] = [PHONE.x + 13, PHONE.y + 4, 46];
const COMPUTER_ANCHOR: [number, number, number] = [COMPUTER.x - 24, COMPUTER.y - 4, 42];
const RELAY_ANCHOR: [number, number, number] = [RELAY.x, RELAY.y - 19.6, 92];

const A = at(PHONE_ANCHOR);
const C = at(COMPUTER_ANCHOR);
const R = at(RELAY_ANCHOR);

const directControl: [number, number, number] = [0, 38, 20];
const directPoints = Array.from({ length: 81 }, (_, i) => {
  const t = i / 80;
  const u = 1 - t;
  const point: [number, number, number] = [
    u * u * PHONE_ANCHOR[0] + 2 * u * t * directControl[0] + t * t * COMPUTER_ANCHOR[0],
    u * u * PHONE_ANCHOR[1] + 2 * u * t * directControl[1] + t * t * COMPUTER_ANCHOR[1],
    u * u * PHONE_ANCHOR[2] + 2 * u * t * directControl[2] + t * t * COMPUTER_ANCHOR[2],
  ];
  return at(point);
});

const arc = (from: Flat, to: Flat, lift: number, count = 60): Flat[] =>
  Array.from({ length: count + 1 }, (_, i) => {
    const t = i / count;
    return [from[0] + (to[0] - from[0]) * t, from[1] + (to[1] - from[1]) * t - Math.sin(Math.PI * t) * lift] as Flat;
  });

export const DIRECT_PATH = directPoints;
export const RELAY_PATH = [...arc(A, R, 18).slice(0, -1), ...arc(R, C, 18)];

// The cut is a real gap in the cable: two ends retract and the middle is left open.
export const DIRECT_LEFT = k.pathOf(directPoints.slice(0, 41));
export const DIRECT_RIGHT = k.pathOf(directPoints.slice(40));
export const DIRECT_LEFT_CUT = k.pathOf(directPoints.slice(0, 37));
export const DIRECT_RIGHT_CUT = k.pathOf(directPoints.slice(44));
export const CUT_AT = directPoints[40]!;

const RELAY_LEFT = k.pathOf(RELAY_PATH.slice(0, 61));
const RELAY_RIGHT = k.pathOf(RELAY_PATH.slice(60));
export { RELAY_LEFT, RELAY_RIGHT };

function capsule(): ArtItem[] {
  const out: ArtItem[] = [];
  out.push(S(solid({ x: -6.5, y: -5, w: 13, d: 10, r: 1.6 }, 0, 2.6, 4, 0.4), { tone: "hi", crease: "lo" }));
  out.push(L(k.planOutline(k.insetPlan({ x: -6.5, y: -5, w: 13, d: 10, r: 1.6 }, 1.2), 2.6, PROJECTION, 4), { tone: "faint" }));
  out.push(S(cyl(0, 0, 5.6, 2.6, 12, 32, 0.5), { tone: "hi", crease: "lo" }));
  out.push(L(k.sideArc(0, 0, 5.6, 7.4, PROJECTION, 26), { tone: "faint" }));
  out.push(S(cyl(0, 0, 3.4, 14.6, 1.8, 22, 0.3), { tone: "mid" }));
  out.push(S(cyl(0, 0, 1.3, 16.4, 1.1, 14), { tone: "hi" }));
  return out;
}

const ORIGIN = at([0, 0, 0]);
export function shiftTo(point: Flat) {
  return `translate(${Math.round((point[0] - ORIGIN[0]) * 100) / 100} ${Math.round((point[1] - ORIGIN[1]) * 100) / 100})`;
}

export const CAPSULE_ITEMS = capsule();
export const CAPSULE_START = shiftTo(directPoints[0]!);
export const LAMP_AT = at([RELAY.x, RELAY.y - 19.6, 96]);

export const STATIC_ART: ArtItem[] = [...plate(), ...relay(), ...phone(), ...computer()];
