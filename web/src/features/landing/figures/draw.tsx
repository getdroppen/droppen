import * as React from "react";
import type { Flat, SolidPaths, Tone } from "./iso-kit";

/*
 * The drawing primitives, adapted from the anatomy kit's React bindings (MIT):
 * a solid paints its four tone sheets in order, then its creases and its edge;
 * lines and dots sit on top. Tone names are fixed so the CSS owns the palette.
 */

export function Solid({
  paths,
  tone = "hi",
  crease = "faint",
  halo = false,
  lit = false,
  flat = false,
}: {
  paths: SolidPaths;
  tone?: Tone;
  crease?: Tone | "none";
  halo?: boolean;
  lit?: boolean;
  flat?: boolean;
}) {
  return (
    <g className="iso-solid" data-lit={lit ? "" : undefined} data-seamless={paths.seamless ? "" : undefined}>
      {halo ? <path className="iso-halo" d={paths.fill} /> : null}
      <path className="iso-fill" d={paths.fill} />
      {flat ? null : paths.shades.map((d, shade) => (d ? <path key={shade} className="iso-shade" data-shade={shade} d={d} /> : null))}
      {flat ? null : <path className="iso-top" d={paths.top} />}
      {crease === "none" || !paths.crease ? null : <path className="iso-line iso-crease" data-tone={crease} d={paths.crease} />}
      {crease === "none" || !paths.bevel ? null : <path className="iso-line iso-bevel" data-tone={crease} d={paths.bevel} />}
      <path className="iso-line iso-edge" data-tone={tone} d={paths.outline} />
    </g>
  );
}

export function Line({
  d,
  tone = "lo",
  dotted = false,
  dashed = false,
  free,
  width,
}: {
  d: string;
  tone?: Tone | "lit" | "green" | "red" | "blue";
  dotted?: boolean;
  dashed?: boolean;
  free?: boolean | "start" | "end";
  width?: number;
}) {
  if (!d) return null;
  return (
    <path
      className="iso-line"
      data-tone={tone}
      data-dotted={dotted ? "" : undefined}
      data-dashed={dashed ? "" : undefined}
      data-free={free === true ? "" : free || undefined}
      style={width ? { strokeWidth: width } : undefined}
      d={d}
    />
  );
}

const tenth = (value: number) => Math.round(value * 10) / 10;

export function Dots({
  points,
  size = 0.5,
  tone = "mid",
}: {
  points: Array<Flat | [number, number, number]>;
  size?: number;
  tone?: Tone;
}) {
  return (
    <g className="iso-dots" data-tone={tone}>
      {points.map(([x, y, alpha], index) => (
        <circle
          key={index}
          cx={tenth(x)}
          cy={tenth(y)}
          r={size}
          fillOpacity={alpha !== undefined && alpha < 1 ? Math.max(0, alpha) : undefined}
        />
      ))}
    </g>
  );
}

export function FaceText({
  transform,
  children,
  size = 3,
  tone = "mid",
  anchor = "start",
}: {
  transform: string;
  children: React.ReactNode;
  size?: number;
  tone?: Tone;
  anchor?: "start" | "middle" | "end";
}) {
  return (
    <text className="iso-face-text" data-tone={tone} transform={transform} fontSize={size} textAnchor={anchor}>
      {children}
    </text>
  );
}
