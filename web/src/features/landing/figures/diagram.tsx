import { useId, type CSSProperties, type ReactNode } from "react";
import { Box, project, type Vec3 } from "react-isokit";
import { usePrefersReducedMotion } from "@/lib/reduced-motion";

/* Isometric device geometry adapted from isokit's MIT-licensed Agent Relay example. */
export const INK = {
  fg: "var(--color-fg)",
  muted: "var(--color-muted)",
  border: "var(--color-border-strong)",
  faint: "var(--color-border)",
  bg: "var(--color-bg)",
  top: "color-mix(in srgb, var(--color-fg) 3%, var(--color-bg))",
  left: "color-mix(in srgb, var(--color-fg) 5%, var(--color-bg))",
  right: "color-mix(in srgb, var(--color-fg) 8%, var(--color-bg))",
  screen: "color-mix(in srgb, var(--color-fg) 7%, var(--color-bg))",
  ok: "var(--color-ok)",
  danger: "var(--color-danger)",
} as const;

export const MODEL_STYLE = {
  "--ik-panel": INK.bg,
  "--ik-top": INK.top,
  "--ik-front": INK.left,
  "--ik-side": INK.right,
  "--ik-well": INK.screen,
  "--ik-line": INK.fg,
  "--ik-detail": INK.border,
  "--ik-live": INK.ok,
  "--ik-ink": INK.fg,
  "--ik-ink-hi": INK.fg,
  "--ik-screen": INK.screen,
  "--ik-screen-ink": INK.muted,
  "--ik-border": INK.border,
} as CSSProperties;

const PC_PORT: Vec3 = [114, 16, 8];
const RELAY_LEFT: Vec3 = [0, 54, 18];
const RELAY_RIGHT: Vec3 = [66, 0, 18];
const CLIENT_PORT: Vec3 = [4, 15, 27];
const PC_AT = [104, 136] as const;
const SERVER_AT = [445, 147] as const;
const CLIENT_AT = [756, 178] as const;
const point = (at: Vec3, offset: readonly [number, number]) => {
  const [x, y] = project(...at);
  return `${x + offset[0]} ${y + offset[1]}`;
};
export const PORTS = {
  pc: point(PC_PORT, PC_AT),
  relayLeft: point(RELAY_LEFT, SERVER_AT),
  relayRight: point(RELAY_RIGHT, SERVER_AT),
  client: point(CLIENT_PORT, CLIENT_AT),
} as const;
export const LABEL_AT = {
  pc: [130, 255],
  relay: [450, 255],
  client: [770, 255],
} as const;

export function Ground() {
  const id = useId();
  return (
    <g aria-hidden="true">
      <defs>
        <pattern id={id} width={28} height={14} patternUnits="userSpaceOnUse">
          <path d="M14 0 28 7 14 14 0 7Z" fill="none" stroke={INK.faint} strokeWidth={0.6} opacity={0.55} />
        </pattern>
      </defs>
      <rect x={-1000} y={-1000} width={2000} height={2000} fill={`url(#${id})`} opacity={0.5} />
    </g>
  );
}

export function DiagramText({
  x,
  y,
  children,
  ink = "muted",
  anchor = "middle",
  size = 10,
  className = "",
}: {
  x: number;
  y: number;
  children: ReactNode;
  ink?: keyof typeof INK;
  anchor?: "start" | "middle" | "end";
  size?: number;
  className?: string;
}) {
  return (
    <text x={x} y={y} fill={INK[ink]} fontSize={size} textAnchor={anchor} className={`font-mono ${className}`} style={{ fontFamily: "var(--font-mono)" }}>
      {children}
    </text>
  );
}

const LID = { x: 0, y: 12, z: 10, w: 114, d: 4, h: 78 };
const DECK = { x: 0, y: 16, z: 8, w: 114, d: 64, h: 5 };

export function Computer() {
  return (
    <g transform={`translate(${PC_AT.join(" ")})`}>
      <Box {...LID} r={2} front={
        <>
          <rect className="ik-screen" x={3} y={3} width={LID.w - 6} height={LID.h - 6} rx={3} />
          <path className="ik-detail" d={`M3 9.5h${LID.w - 6}`} />
          <rect className="ik-well" x={10} y={16} width={LID.w - 20} height={55} rx={2.5} />
          <path className="ik-detail" d={`M10 21.5h${LID.w - 20}`} />
          {[0, 1, 2].map((item) => <circle key={item} className="ik-fill" cx={14 + item * 3.6} cy={18.8} r={0.8} />)}
          {[[28, 56], [33, 38], [38, 64], [43, 30], [48, 48], [53, 42]].map(([line, width]) => (
            <path key={line} className="ik-screen-line" d={`M15 ${line}h${width}`} />
          ))}
        </>
      } />
      <Box {...DECK} r={4} top={
        <>
          <rect className="ik-well" x={10} y={8} width={DECK.w - 20} height={30} rx={2} />
          {[14, 20, 26, 32].map((line) => <path key={line} className="ik-detail" d={`M12 ${line}h${DECK.w - 24}`} />)}
          {Array.from({ length: 14 }, (_, item) => <path key={item} className="ik-detail" d={`M${18 + item * 6} 10v26`} />)}
          <rect className="ik-detail" x={DECK.w / 2 - 20} y={42} width={40} height={18} rx={2} />
        </>
      } front={<path className="ik-detail" d={`M${DECK.w / 2 - 8} 1.4h16`} />} />
      <circle cx={project(...PC_PORT)[0]} cy={project(...PC_PORT)[1]} r={2.4} fill={INK.bg} stroke={INK.fg} strokeWidth={1} />
    </g>
  );
}

const STAND = { x: 0, y: 0, z: 8, w: 62, d: 30, h: 5 };
const PHONE = { x: 4, y: 10, z: 13, w: 54, d: 5, h: 108 };

export function Phone() {
  return (
    <g transform={`translate(${CLIENT_AT.join(" ")})`}>
      <Box {...STAND} r={6} top={<rect className="ik-well" x={5} y={9} width={52} height={10} rx={3} />} />
      <Box {...PHONE} r={4} side={<path className="ik-detail" d="M2 22v8M2 34v8" />} front={
        <>
          <rect className="ik-screen" x={2} y={2} width={PHONE.w - 4} height={PHONE.h - 4} rx={8} />
          {[0, 1, 2, 3].map((item) => <rect key={item} className="ik-fill" x={6.4 + item * 1.5} y={8.2 - item * 0.7} width={1} height={1.2 + item * 0.7} rx={0.3} />)}
          <rect className="ik-detail" x={41.2} y={5.6} width={6.4} height={3} rx={0.9} />
          <rect className="ik-fill" x={42} y={6.4} width={4.4} height={1.4} rx={0.4} />
          <text className="ik-screen-text" x={PHONE.w / 2} y={22} fontSize={3.1} textAnchor="middle">Monday 5 October</text>
          <text className="ik-screen-text" x={PHONE.w / 2} y={36} fontSize={13} textAnchor="middle">9:41</text>
          <rect className="ik-well" x={6} y={46} width={42} height={24} rx={5} />
          <path className="ik-screen-line" d="M11 55h28M11 61h32" />
          <rect className="ik-well" x={6} y={75} width={42} height={17} rx={5} />
          <path className="ik-screen-line" d="M11 83h24" />
          <rect className="ik-fill" x={PHONE.w / 2 - 8} y={4.2} width={16} height={4.8} rx={2.4} />
          <circle className="ik-detail" cx={10.6} cy={PHONE.h - 11} r={4} />
          <circle className="ik-detail" cx={PHONE.w - 10.6} cy={PHONE.h - 11} r={4} />
          <rect className="ik-fill" x={PHONE.w / 2 - 9} y={PHONE.h - 4.6} width={18} height={1.2} rx={0.6} />
        </>
      } />
      <circle cx={project(...CLIENT_PORT)[0]} cy={project(...CLIENT_PORT)[1]} r={2.4} fill={INK.bg} stroke={INK.fg} strokeWidth={1} />
    </g>
  );
}

function Packet({ x, y }: { x: number; y: number }) {
  return (
    <g transform={`translate(${x} ${y})`}>
      <Box x={-6} y={-6} z={0} w={12} d={12} h={5} r={1.2} top={<path d="M2 6h8" stroke={INK.ok} strokeWidth={1.2} />} />
    </g>
  );
}

export function MovingPacket({ path, rest, duration = 4.8 }: { path: string; rest: { x: number; y: number }; duration?: number }) {
  const reduced = usePrefersReducedMotion();
  return (
    <g transform={reduced ? `translate(${rest.x} ${rest.y})` : undefined}>
      <Packet x={0} y={0} />
      {!reduced ? (
        <>
          <animateMotion path={path} dur={`${duration}s`} repeatCount="indefinite" calcMode="spline" keyTimes="0;1" keySplines="0.45 0 0.55 1" />
          <animate attributeName="opacity" values="0;1;1;0" keyTimes="0;0.08;0.92;1" dur={`${duration}s`} repeatCount="indefinite" />
        </>
      ) : null}
    </g>
  );
}

export function Key({ x, y, slashed = false }: { x: number; y: number; slashed?: boolean }) {
  return (
    <g fill="none" stroke={slashed ? INK.muted : INK.ok} strokeWidth={1.6} strokeLinecap="round" strokeLinejoin="round" vectorEffect="non-scaling-stroke">
      <circle cx={x - 10} cy={y} r={6} />
      <circle cx={x - 10} cy={y} r={2} />
      <path d={`M${x - 4} ${y}h23M${x + 12} ${y}v7M${x + 18} ${y}v5`} />
      {slashed ? <path d={`M${x - 21} ${y + 13}l45-26`} stroke={INK.danger} /> : null}
    </g>
  );
}

const RACK = { x: 0, y: 0, w: 66, d: 54 };
const HEAD = { ...RACK, z: 8, h: 10 };
const NODE = { x: RACK.x + 6, y: RACK.y + 1, w: 54, d: 51, h: 12 };
const CAP = { ...RACK, z: 62, h: 4 };
const POST = { x: RACK.x, y: RACK.y, z: 8, w: 6, d: 54, h: 54 };

function RackHoles() {
  return Array.from({ length: 11 }, (_, item) => <circle key={item} className="ik-fill" cx={3} cy={4 + item * 4.4} r={0.6} />);
}

export function ServerRack() {
  return (
    <g transform={`translate(${SERVER_AT.join(" ")})`}>
      <Box {...HEAD} r={3} />
      {[0, 1, 2, 3].map((tier) => (
        <Box key={tier} {...NODE} z={20 + tier * 13.5} r={2} front={
          <>
            <rect className="ik-well" x={4} y={3} width={35} height={6} rx={1} />
            <path className="ik-detail" d="M8 5h22M8 7h14" />
            <circle className="ik-live" cx={46} cy={6} r={1.5} />
          </>
        } />
      ))}
      <Box {...POST} r={1.5} front={<RackHoles />} />
      <Box {...POST} x={RACK.x + RACK.w - 6} r={1.5} front={<RackHoles />} />
      <Box {...CAP} r={3} top={<rect className="ik-detail" x={8} y={8} width={50} height={38} rx={3} />} />
      {[RELAY_LEFT, RELAY_RIGHT].map((port) => {
        const [cx, cy] = project(...port);
        return <circle key={port.join(",")} cx={cx} cy={cy} r={2.4} fill={INK.bg} stroke={INK.fg} strokeWidth={1} />;
      })}
    </g>
  );
}

export const VIEW_BOX = "0 0 900 300";
