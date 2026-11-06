export type Flat = [number, number];
export type Point3 = [number, number, number];
export type Plane = [number, number];

export interface Projection {
  origin: Flat;
  scale: number;
  azimuth?: number;
  elevation?: number;
}

export interface Plan {
  x: number;
  y: number;
  w: number;
  d: number;
  r: number;
}

export interface SolidPaths {
  fill: string;
  outline: string;
  crease: string;
  top: string;
  shades: string[];
  bevel?: string;
  seamless?: boolean;
}

export type Tone = "hi" | "mid" | "lo" | "faint";
export type LineTone = Tone | "lit" | "red" | "green" | "blue";

const AZIMUTH = 45;
const ELEVATION = 30;
const SHADES = 4;

const RADIANS = Math.PI / 180;
const UNIT = Math.sqrt(1.6);

interface Camera {
  sinA: number;
  cosA: number;
  sinE: number;
  cosE: number;
  k: number;
}

function cameraOf({ scale, azimuth = AZIMUTH, elevation = ELEVATION }: Projection): Camera {
  const a = azimuth * RADIANS;
  const e = elevation * RADIANS;
  return { sinA: Math.sin(a), cosA: Math.cos(a), sinE: Math.sin(e), cosE: Math.cos(e), k: scale * UNIT };
}

export function iso([x, y, z]: Point3, projection: Projection): Flat {
  const { origin } = projection;
  const { sinA, cosA, sinE, cosE, k } = cameraOf(projection);
  return [origin[0] + (x * sinA - y * cosA) * k, origin[1] + ((x * cosA + y * sinA) * sinE - z * cosE) * k];
}

function towardViewer(projection: Projection): Plane {
  const { sinA, cosA } = cameraOf(projection);
  return [cosA, sinA];
}

export function depthOf([x, y, z]: Point3, projection: Projection) {
  const { sinA, cosA, sinE, cosE } = cameraOf(projection);
  return (x * cosA + y * sinA) * cosE + z * sinE;
}

const tenth = (value: number) => {
  const text = (Math.round(value * 10) / 10).toFixed(1).replace(/\.0$/, "");
  return text === "-0" ? "0" : text;
};

export function pathOf(points: Flat[], closed = false) {
  if (!points.length) return "";
  return points.map(([x, y], at) => `${at ? "L" : "M"}${tenth(x)} ${tenth(y)}`).join("") + (closed ? "Z" : "");
}


function roundedPlan({ x, y, w, d, r }: Plan, steps = 8): Plane[] {
  const radius = Math.max(0, Math.min(r, w / 2, d / 2));
  const corners: [number, number, number][] = [
    [x + w - radius, y + radius, -90],
    [x + w - radius, y + d - radius, 0],
    [x + radius, y + d - radius, 90],
    [x + radius, y + radius, 180],
  ];
  const out: Plane[] = [];
  for (const [cx, cy, start] of corners) {
    const count = radius > 0 ? steps : 0;
    for (let step = 0; step <= count; step++) {
      const angle = (start + (count ? (90 * step) / count : 45)) * RADIANS;
      out.push([cx + radius * Math.cos(angle), cy + radius * Math.sin(angle)]);
    }
  }
  return out;
}

function signedArea(points: Plane[] | Flat[]) {
  let area = 0;
  for (let index = 0; index < points.length; index++) {
    const [ax, ay] = points[index]!;
    const [bx, by] = points[(index + 1) % points.length]!;
    area += ax * by - bx * ay;
  }
  return area / 2;
}

const oriented = (points: Flat[]) => (signedArea(points) < 0 ? [...points].reverse() : points);

function convexHull(points: Flat[]): Flat[] {
  const sorted = [...points].sort((a, b) => a[0] - b[0] || a[1] - b[1]);
  if (sorted.length < 3) return sorted;
  const cross = (o: Flat, a: Flat, b: Flat) => (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
  const lower: Flat[] = [];
  for (const point of sorted) {
    while (lower.length >= 2 && cross(lower[lower.length - 2]!, lower[lower.length - 1]!, point) <= 0) lower.pop();
    lower.push(point);
  }
  const upper: Flat[] = [];
  for (let index = sorted.length - 1; index >= 0; index--) {
    const point = sorted[index]!;
    while (upper.length >= 2 && cross(upper[upper.length - 2]!, upper[upper.length - 1]!, point) <= 0) upper.pop();
    upper.push(point);
  }
  return [...lower.slice(0, -1), ...upper.slice(0, -1)];
}

function runsOf(flags: boolean[], want: boolean) {
  const count = flags.length;
  const runs: number[][] = [];
  const start = flags.findIndex((flag, index) => flag !== flags[(index + count - 1) % count]);
  if (start < 0) return flags[0] === want ? [Array.from({ length: count + 1 }, (_, index) => index % count)] : [];
  let run: number[] | null = null;
  for (let step = 0; step < count; step++) {
    const index = (start + step) % count;
    if (flags[index] === want) {
      if (!run) run = [index];
      run.push((index + 1) % count);
    } else if (run) {
      runs.push(run);
      run = null;
    }
  }
  if (run) runs.push(run);
  return runs;
}

interface ExtrudeOptions {
  convex?: boolean;
  bevel?: number;
}

function extrude(ring: Plane[], z: number, height: number, projection: Projection, options: ExtrudeOptions = {}): SolidPaths {
  const convex = options.convex ?? true;
  const count = ring.length;
  const counter = signedArea(ring) > 0;
  const [vx, vy] = towardViewer(projection);
  const { sinA, cosA } = cameraOf(projection);
  const normals = ring.map(([ax, ay], index) => {
    const [bx, by] = ring[(index + 1) % count]!;
    const dx = bx - ax;
    const dy = by - ay;
    const length = Math.hypot(dx, dy) || 1;
    return (counter ? [dy / length, -dx / length] : [-dy / length, dx / length]) as Plane;
  });
  const facing = normals.map(([nx, ny]) => nx * vx + ny * vy > 1e-9);
  const top = ring.map(([x, y]) => iso([x, y, z + height], projection));
  const bottom = ring.map(([x, y]) => iso([x, y, z], projection));
  const outline: string[] = [];
  const crease: string[] = [];
  for (const run of runsOf(facing, false)) outline.push(pathOf(run.map((index) => top[index]!)));
  for (const run of runsOf(facing, true)) {
    crease.push(pathOf(run.map((index) => top[index]!)));
    if (height > 0) outline.push(pathOf(run.map((index) => bottom[index]!)));
  }
  if (height > 0) {
    for (let index = 0; index < count; index++) {
      if (facing[(index + count - 1) % count] !== facing[index]) outline.push(pathOf([top[index]!, bottom[index]!]));
    }
  }
  let fill: string;
  if (convex) {
    fill = pathOf(convexHull([...top, ...bottom]), true);
  } else {
    const parts = [pathOf(oriented(top), true), pathOf(oriented(bottom), true)];
    for (let index = 0; index < count; index++) {
      if (!facing[index]) continue;
      const next = (index + 1) % count;
      parts.push(pathOf(oriented([top[index]!, top[next]!, bottom[next]!, bottom[index]!]), true));
    }
    fill = parts.join("");
  }
  const buckets: string[][] = Array.from({ length: SHADES }, () => []);
  if (height > 0) {
    const [lx, ly] = [-sinA, cosA];
    for (let index = 0; index < count; index++) {
      if (!facing[index]) continue;
      const [nx, ny] = normals[index]!;
      const lit = (nx * lx + ny * ly + 1) / 2;
      const shade = Math.min(SHADES - 1, Math.max(0, Math.floor(lit * SHADES)));
      const next = (index + 1) % count;
      buckets[shade]!.push(pathOf(oriented([top[index]!, top[next]!, bottom[next]!, bottom[index]!]), true));
    }
  }
  let bevel: string | undefined;
  if (options.bevel && options.bevel > 0) {
    const by = options.bevel;
    const inset = ring.map(([x, y], index) => {
      const [ax, ay] = normals[(index + count - 1) % count]!;
      const [bx, by2] = normals[index]!;
      const mx = ax + bx;
      const my = ay + by2;
      const length = Math.hypot(mx, my) || 1;
      return [x - (mx / length) * by, y - (my / length) * by] as Plane;
    });
    const insetTop = inset.map(([x, y]) => iso([x, y, z + height], projection));
    bevel = runsOf(facing, true)
      .map((run) => pathOf([top[run[0]!]!, ...run.slice(1, -1).map((index) => insetTop[index]!), top[run[run.length - 1]!]!]))
      .join("");
  }
  return {
    fill,
    outline: outline.join(""),
    crease: crease.join(""),
    top: pathOf(oriented(top), true),
    shades: buckets.map((paths) => paths.join("")),
    bevel,
  };
}

const ringOf = (plan: Plan, steps = 8): Plane[] => roundedPlan(plan, steps);

export function slabOf(plan: Plan, z: number, height: number, projection: Projection, steps = 8, bevel = 0) {
  return extrude(ringOf(plan, steps), z, height, projection, { convex: true, bevel });
}

function circleRing(cx: number, cy: number, radius: number, steps = 40): Plane[] {
  return Array.from({ length: steps }, (_, step) => {
    const angle = (step / steps) * Math.PI * 2;
    return [cx + radius * Math.cos(angle), cy + radius * Math.sin(angle)] as Plane;
  });
}

export function cylinder(cx: number, cy: number, radius: number, z: number, height: number, projection: Projection, steps = 32, bevel = 0) {
  return extrude(circleRing(cx, cy, radius, steps), z, height, projection, { convex: true, bevel });
}

export const insetPlan = ({ x, y, w, d, r }: Plan, by: number): Plan => ({ x: x + by, y: y + by, w: w - by * 2, d: d - by * 2, r: Math.max(0, r - by) });

function onTop(points: Plane[], z: number, projection: Projection, closed = false) {
  return pathOf(
    points.map(([x, y]) => iso([x, y, z], projection)),
    closed,
  );
}

export const planOutline = (plan: Plan, z: number, projection: Projection, steps = 8) => onTop(ringOf(plan, steps), z, projection, true);

export const lineOnTop = (from: Plane, to: Plane, z: number, projection: Projection) => onTop([from, to], z, projection);

export const ring = (cx: number, cy: number, radius: number, z: number, projection: Projection, steps = 40) =>
  onTop(circleRing(cx, cy, radius, steps), z, projection, true);

export function corners({ x, y, w, d }: Plan, inset = 0): Plane[] {
  return [
    [x + inset, y + inset],
    [x + w - inset, y + inset],
    [x + inset, y + d - inset],
    [x + w - inset, y + d - inset],
  ];
}

export function boxCorners({ x, y, w, d }: Plan, z0: number, z1: number): Point3[] {
  const out: Point3[] = [];
  for (const px of [x, x + w]) for (const py of [y, y + d]) for (const z of [z0, z1]) out.push([px, py, z]);
  return out;
}

export function sideArc(cx: number, cy: number, radius: number, z: number, projection: Projection, steps = 24) {
  const [vx, vy] = towardViewer(projection);
  const facing = Math.atan2(vy, vx);
  const points: Flat[] = [];
  for (let step = 0; step <= steps; step++) {
    const angle = facing - Math.PI / 2 + (step / steps) * Math.PI;
    points.push(iso([cx + radius * Math.cos(angle), cy + radius * Math.sin(angle), z], projection));
  }
  return pathOf(points);
}

function ringSeam(ring: Plane[], z: number, projection: Projection) {
  const count = ring.length;
  const counter = signedArea(ring) > 0;
  const [vx, vy] = towardViewer(projection);
  const facing = ring.map(([ax, ay], index) => {
    const [bx, by] = ring[(index + 1) % count]!;
    const dx = bx - ax;
    const dy = by - ay;
    const [nx, ny] = counter ? [dy, -dx] : [-dy, dx];
    return nx * vx + ny * vy > 1e-9;
  });
  return runsOf(facing, true).map((run) => pathOf(run.map((index) => iso([ring[index]![0], ring[index]![1], z], projection)))).join("");
}

export const sideSeam = (plan: Plan, z: number, projection: Projection, steps = 8) => ringSeam(ringOf(plan, steps), z, projection);

export interface Ticks {
  minor: string;
  major: string;
}

export function topTicks(from: number, to: number, step: number, every: number, edge: number, z: number, lengths: [number, number], projection: Projection, axis: "x" | "y" = "x"): Ticks {
  const minor: string[] = [];
  const major: string[] = [];
  const first = Math.ceil(from / step) * step;
  for (let at = first, index = Math.round(first / step); at <= to + 1e-9; at += step, index++) {
    const big = index % every === 0;
    const length = big ? lengths[1] : lengths[0];
    const a: Plane = axis === "x" ? [at, edge] : [edge, at];
    const b: Plane = axis === "x" ? [at, edge + length] : [edge + length, at];
    (big ? major : minor).push(lineOnTop(a, b, z, projection));
  }
  return { minor: minor.join(""), major: major.join("") };
}

export const haloOf = (plan: Plan, z: number, height: number, projection: Projection) => slabOf(plan, z, height, projection).fill;

export interface FitOptions {
  pad?: number;
  headroom?: number;
  azimuth?: number;
  elevation?: number;
}

export function fitProjection(points: Point3[], width: number, height: number, { pad = 24, headroom = 0, azimuth = AZIMUTH, elevation = ELEVATION }: FitOptions = {}): Projection {
  const unit: Projection = { origin: [0, 0], scale: 1, azimuth, elevation };
  const flat = points.map((point) => iso(point, unit));
  const xs = flat.map(([x]) => x);
  const ys = flat.map(([, y]) => y);
  const left = Math.min(...xs);
  const right = Math.max(...xs);
  const up = Math.min(...ys);
  const down = Math.max(...ys);
  const scale = Math.min((width - pad * 2) / (right - left), (height - pad * 2 - headroom) / (down - up));
  return {
    origin: [width / 2 - ((left + right) / 2) * scale, headroom + (height - headroom) / 2 - ((up + down) / 2) * scale],
    scale,
    azimuth,
    elevation,
  };
}

