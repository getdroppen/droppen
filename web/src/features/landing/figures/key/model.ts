import * as k from "../iso-kit";
import type { Flat, LineTone, SolidPaths, Tone } from "../iso-kit";
import type { ArtItem } from "../art";

/*
 * The key boundary with three actors: your computer, Droppen Hub, your phone.
 * A sealed capsule crosses between the devices on a low cable; the Hub keeps a
 * copy of the ciphertext and has no key. The key handshake travels on its own
 * cable, directly from one device to the other, over the Hub.
 */

export const WIDTH = 660;
export const HEIGHT = 340;

const AZ = 56;
const EL = 33;

const BASE = { x: -150, y: -56, w: 300, d: 100, r: 7 };
const BASE_H = 10;
const FOOT_H = 5;

const COMPUTER = { x: -106, y: 6 };
const PHONE = { x: 106, y: 6 };
const HUB = { x: -30, y: -42, w: 60, d: 26, r: 1.4, h: 54 };

const CARRY_FROM: [number, number, number] = [COMPUTER.x + 20, 21, 16];
const CARRY_TO: [number, number, number] = [PHONE.x - 14, 21, 16];
const HUB_FACE = HUB.y + HUB.d;

export const PROJECTION = k.fitProjection(
  [
    ...k.boxCorners(BASE, -BASE_H - FOOT_H, 0),
    [HUB.x + HUB.w, HUB.y, HUB.h + 8],
    [COMPUTER.x, COMPUTER.y, 62],
    [PHONE.x, PHONE.y, 60],
    [0, -24, 108],
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

const faceRing = (cx: number, cz: number, r: number, y: number, count = 20) =>
  k.pathOf(
    Array.from({ length: count + 1 }, (_, i) => {
      const a = (i / count) * Math.PI * 2;
      return at([cx + Math.cos(a) * r, y, cz + Math.sin(a) * r]);
    }),
    false,
  );
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

function computer(): ArtItem[] {
  const out: ArtItem[] = [];
  const dock = { x: COMPUTER.x - 24, y: COMPUTER.y - 12, w: 48, d: 28, r: 3.4 };
  out.push(S(solid(dock, 0, 9, 4, 0.7), { tone: "mid" }));
  out.push(L(k.planOutline(k.insetPlan(dock, 2.4), 9, PROJECTION, 4), { tone: "faint" }));
  out.push(S(solid({ x: COMPUTER.x - 4, y: COMPUTER.y - 3, w: 8, d: 5, r: 1 }, 9, 12, 3, 0.4), { tone: "mid" }));
  const frame = { x: COMPUTER.x - 22, y: COMPUTER.y - 4.5, w: 44, d: 5, r: 2.6 };
  out.push(S(solid(frame, 21, 36, 5, 0.7), { tone: "hi", crease: "lo" }));
  const face = COMPUTER.y - 5.2;
  out.push(L(faceRect(COMPUTER.x - 18, 25, COMPUTER.x + 18, 52, face), { tone: "faint" }));
  out.push(L(faceLine(COMPUTER.x - 14, 48, COMPUTER.x + 6, 48, face), { tone: "lo" }));
  out.push(L(faceLine(COMPUTER.x - 14, 44, COMPUTER.x + 2, 44, face), { tone: "lo" }));
  out.push(L(faceLine(COMPUTER.x - 14, 40, COMPUTER.x - 4, 40, face), { tone: "lo" }));
  return out;
}

function phone(): ArtItem[] {
  const out: ArtItem[] = [];
  const dock = { x: PHONE.x - 17, y: PHONE.y - 10, w: 34, d: 26, r: 4 };
  out.push(S(solid(dock, 0, 9, 4, 0.8), { tone: "mid" }));
  out.push(L(k.planOutline(k.insetPlan(dock, 2.6), 9, PROJECTION, 4), { tone: "faint" }));
  const body = { x: PHONE.x - 13, y: PHONE.y - 3, w: 26, d: 7, r: 3.4 };
  out.push(S(solid(body, 9, 44, 5, 0.8), { tone: "hi", crease: "lo" }));
  const face = PHONE.y + 4.2;
  out.push(L(faceRect(PHONE.x - 9.6, 16, PHONE.x + 9.6, 46, face), { tone: "faint" }));
  out.push(L(faceLine(PHONE.x - 6, 41, PHONE.x + 5, 41, face), { tone: "lo" }));
  out.push(L(faceLine(PHONE.x - 6, 37, PHONE.x + 2, 37, face), { tone: "lo" }));
  out.push(L(faceLine(PHONE.x - 6, 33, PHONE.x - 1, 33, face), { tone: "lo" }));
  out.push({ kind: "dots", points: [at([PHONE.x, face, 11.4])], size: 0.6, tone: "mid" });
  return out;
}

/** The key, kept on the device: a key ring lying on the dock, tied to the body. */
function deviceKey(x: number, y: number, tetherTo: [number, number, number]): ArtItem[] {
  const out: ArtItem[] = [];
  const z = 9.8;
  out.push(L(k.ring(x, y, 2.2, z, PROJECTION, 18), { tone: "mid" }));
  out.push(L(k.lineOnTop([x + 2.2, y], [x + 8.4, y], z, PROJECTION), { tone: "mid" }));
  out.push(L(k.lineOnTop([x + 6.2, y], [x + 6.2, y + 1.9], z, PROJECTION), { tone: "mid" }));
  out.push(L(k.lineOnTop([x + 7.8, y], [x + 7.8, y + 1.5], z, PROJECTION), { tone: "mid" }));
  out.push(L(k.pathOf([at([x, y, z]), at(tetherTo)]), { tone: "mid", dashed: true }));
  out.push({ kind: "dots", points: [at([x, y, z])], size: 0.4 });
  return out;
}

function hub(): ArtItem[] {
  const out: ArtItem[] = [];
  const b = HUB;
  out.push(S(solid(b, 0, b.h, 5, 0.8), { tone: "mid" }));
  out.push(L(k.planOutline(k.insetPlan(b, 3), b.h, PROJECTION, 5), { tone: "faint" }));
  out.push(L(k.sideSeam(b, 34, PROJECTION, 4), { tone: "faint" }));
  for (let i = 0; i < 6; i++) {
    out.push(L(k.lineOnTop([b.x + 8 + i * 5.4, b.y + 6], [b.x + 8 + i * 5.4, b.y + 15], b.h, PROJECTION), { tone: "faint" }));
  }
  // The open bay: a dark back panel, a shelf, and the encrypted copies already kept.
  const bay = { x: b.x + 8, y: HUB_FACE - 0.2, w: b.w - 16, d: 0.8, r: 0.5 };
  out.push(S(solid(bay, 8, 24, 2), { tone: "lo", crease: "none" }));
  out.push(L(k.planOutline(bay, 8, PROJECTION, 2), { tone: "faint" }));
  out.push(L(k.planOutline(bay, 32, PROJECTION, 2), { tone: "faint" }));
  out.push(S(solid({ x: b.x + 9, y: HUB_FACE + 0.3, w: b.w - 18, d: 5.4, r: 0.5 }, 8, 1.4, 2, 0.2), { tone: "mid", crease: "none" }));
  for (const cx of [b.x + 14, b.x + 24, b.x + 34]) {
    out.push(S(cyl(cx, HUB_FACE + 2.6, 3.4, 9.4, 6, 22, 0.4), { tone: "hi", crease: "lo" }));
    out.push(L(k.sideArc(cx, HUB_FACE + 2.6, 3.4, 11.6, PROJECTION, 16), { tone: "faint" }));
    out.push(S(cyl(cx, HUB_FACE + 2.6, 1.9, 15.4, 1.1, 14), { tone: "mid" }));
  }
  // Above the bay, the empty key socket: a keyhole with no key in the Hub.
  out.push(L(faceRing(0, 43, 4.4, HUB_FACE + 0.2, 24), { tone: "lo" }));
  out.push(L(faceRing(0, 43.6, 1.5, HUB_FACE + 0.2, 16), { tone: "mid" }));
  out.push(L(faceLine(-0.6, 42.4, -0.6, 38.8, HUB_FACE + 0.2), { tone: "mid" }));
  out.push(L(faceLine(0.6, 42.4, 0.6, 38.8, HUB_FACE + 0.2), { tone: "mid" }));
  return out;
}

const COMPUTER_TOP: [number, number, number] = [COMPUTER.x, COMPUTER.y - 2, 58];
const PHONE_TOP: [number, number, number] = [PHONE.x, PHONE.y + 1, 54];

const arc = (from: Flat, to: Flat, lift: number, count = 60): Flat[] =>
  Array.from({ length: count + 1 }, (_, i) => {
    const t = i / count;
    return [from[0] + (to[0] - from[0]) * t, from[1] + (to[1] - from[1]) * t - Math.sin(Math.PI * t) * lift] as Flat;
  });

const KEY_CABLE = arc(at(COMPUTER_TOP), at(PHONE_TOP), 68);
export const KEY_CABLE_PATH = k.pathOf(KEY_CABLE);
export const CABLE_SCREEN = KEY_CABLE.map(([x, y]) => [x, y - 4.5] as Flat);

const CARRY = arc(at(CARRY_FROM), at(CARRY_TO), 2, 60);
export const CARRY_PATH = k.pathOf(CARRY);

const ORIGIN = at([0, 0, 0]);
export function shiftTo(point: Flat) {
  return `translate(${Math.round((point[0] - ORIGIN[0]) * 100) / 100} ${Math.round((point[1] - ORIGIN[1]) * 100) / 100})`;
}

export const CAP_START: Flat = [at(CARRY_FROM)[0] - ORIGIN[0], at(CARRY_FROM)[1] - ORIGIN[1]];
export const CAP_END: Flat = [at(CARRY_TO)[0] - ORIGIN[0], at(CARRY_TO)[1] - ORIGIN[1]];
export const CAPSULE_START = shiftTo(at(CARRY_FROM));
export const TOKEN_REST = CABLE_SCREEN[0]!;
export const SEAL_AT = at([6, 5.4, 4]);
export const LAMP_AT = at([0, HUB.y + HUB.d / 2, HUB.h + 2.4]);
export const HUB_COPY_AT: [number, number, number] = [HUB.x + HUB.w - 13, HUB_FACE + 2.6, 9.4];

function capsule(): ArtItem[] {
  const out: ArtItem[] = [];
  out.push(S(solid({ x: -9, y: -5, w: 18, d: 10, r: 4 }, 0, 7, 5, 0.5), { tone: "hi", crease: "lo" }));
  out.push(L(k.planOutline(k.insetPlan({ x: -9, y: -5, w: 18, d: 10, r: 4 }, 2.2), 7, PROJECTION, 4), { tone: "faint" }));
  out.push(S(solid({ x: -1.1, y: -5, w: 2.2, d: 10, r: 0.6 }, 0, 7, 2), { tone: "mid", crease: "none" }));
  return out;
}

function lid(): ArtItem[] {
  const out: ArtItem[] = [];
  out.push(S(solid({ x: -9, y: -5, w: 18, d: 10, r: 4 }, 7, 1.7, 4, 0.4), { tone: "mid" }));
  out.push(L(k.ring(0, 0, 2.2, 8.7, PROJECTION, 16), { tone: "lo" }));
  return out;
}

function token(): ArtItem[] {
  const out: ArtItem[] = [];
  out.push(S(cyl(0, 0, 3.4, 0, 1.8, 20, 0.4), { tone: "hi" }));
  out.push(L(k.ring(0, 0, 1.6, 1.8, PROJECTION, 14), { tone: "lo" }));
  out.push(S(solid({ x: 3.4, y: -0.9, w: 7, d: 1.8, r: 0.4 }, 0, 1.8, 2, 0.2), { tone: "hi" }));
  out.push(S(solid({ x: 7.2, y: 0.9, w: 1.5, d: 2.2, r: 0.3 }, 0, 1.8, 2, 0.2), { tone: "hi" }));
  out.push(S(solid({ x: 9.2, y: 0.9, w: 1.5, d: 1.8, r: 0.3 }, 0, 1.8, 2, 0.2), { tone: "hi" }));
  return out;
}

function copy(): ArtItem[] {
  const [x, y, z] = HUB_COPY_AT;
  return [
    S(cyl(x, y, 3.4, z, 6, 22, 0.4), { tone: "hi", crease: "lo" }),
    L(k.sideArc(x, y, 3.4, z + 2.2, PROJECTION, 16), { tone: "faint" }),
    S(cyl(x, y, 1.9, z + 6, 1.1, 14), { tone: "mid" }),
  ];
}

export const CAPSULE_ITEMS = capsule();
export const LID_ITEMS = lid();
export const TOKEN_ITEMS = token();
export const COPY_ITEMS = copy();

export const STATIC_ART: ArtItem[] = [
  ...plate(),
  ...hub(),
  ...computer(),
  ...phone(),
  ...deviceKey(COMPUTER.x + 4, COMPUTER.y + 23, [COMPUTER.x, COMPUTER.y + 3, 24]),
  ...deviceKey(PHONE.x - 4, PHONE.y + 23, [PHONE.x, PHONE.y + 3, 24]),
  L(CARRY_PATH, { tone: "hi" }),
  L(KEY_CABLE_PATH, { tone: "green" }),
];
