import { forwardRef, type ReactNode, type RefObject, useRef } from "react";
import { BrandMark } from "@/components/brand-mark";
import { cn } from "@/lib/cn";
import { pathD, type Pt, wide } from "./motion";

/*
 * The pieces every landing diagram is drawn with, in the drafting-sheet
 * language: hairline wires, nodes that are a registration symbol and a mono
 * tag, square packets, and colour only for state — green for a live direct
 * path, red for a dropped one or a machine that is off.
 *
 * Nodes are HTML laid out by flexbox at whole pixels (so their 1px borders
 * stay crisp), wires and packets are SVG in the stage's own pixels. A wire's
 * ends run under the nodes, which have the paper colour behind them.
 */

export type WireTone = "ink" | "faint" | "ok" | "danger";

const WIRE_STROKE: Record<WireTone, string> = {
  ink: "var(--color-fg)",
  faint: "var(--color-border-strong)",
  ok: "var(--color-ok)",
  danger: "var(--color-danger)",
};

/**
 * Stroke weights (the diagram exception): hairlines for
 * structure, heavier strokes for data. Each sits on the pixel grid its width
 * needs: odd widths on half pixels, even on whole ones, 1.5px a quarter in
 * (whole device pixels at 2x).
 */
export type WireWeight = "hair" | "data" | "direct" | "fallback";
export const WEIGHT: Record<WireWeight, { width: number; offset: number; dash: string }> = {
  hair: { width: 1, offset: 0.5, dash: "3 3" },
  data: { width: 2, offset: 0, dash: "6 4" },
  direct: { width: 3, offset: 0.5, dash: "7 5" },
  fallback: { width: 1.5, offset: 0.75, dash: "5 4" },
};

/**
 * A wire through `points`, straight or one L. `dashed`: a path standing by
 * (a fallback, a machine that is off, the offline copy). `arrow`: a marker id
 * from {@link Arrows}, drawn at the end, sized by the stroke.
 */
export const Wire = forwardRef<
  SVGPathElement,
  { points: Pt[]; tone?: WireTone; weight?: WireWeight; dashed?: boolean; arrow?: string; className?: string; color?: string }
>(function Wire({ points, tone = "ink", weight = "data", dashed, arrow, className, color }, ref) {
  const w = WEIGHT[weight];
  return (
    <path
      ref={ref}
      d={pathD(points, w.offset)}
      fill="none"
      stroke={color ?? WIRE_STROKE[tone]}
      strokeWidth={w.width}
      strokeDasharray={dashed ? w.dash : undefined}
      markerEnd={arrow ? `url(#${arrow})` : undefined}
      shapeRendering="crispEdges"
      className={className}
    />
  );
});

/** Arrowheads for one diagram, one per tone, scaled by the stroke they end. `id` must be unique on the page. */
export function Arrows({ id }: { id: string }) {
  return (
    <defs>
      {(["ink", "faint", "ok"] as const).map((tone) => (
        <marker key={tone} id={`${id}-${tone}`} viewBox="0 0 8 8" refX="7" refY="4" markerWidth="3.5" markerHeight="3.5" orient="auto" markerUnits="strokeWidth">
          <path d="M0 0L8 4L0 8z" fill={WIRE_STROKE[tone]} shapeRendering="geometricPrecision" />
        </marker>
      ))}
    </defs>
  );
}

export { WIRE_STROKE };

/**
 * A padlock on a 10×12 grid: shut (sealed) or open (just opened). The same
 * glyph in a card row (HTML) and on a wire or a chip (nested in the stage's
 * SVG at `x`, `y`); it takes the colour of the text around it.
 */
export function Lock({ open, size = 10, title, className, x, y }: { open?: boolean; size?: number; title?: string; className?: string; x?: number; y?: number }) {
  return (
    <svg x={x} y={y} width={size} height={size * 1.2} viewBox="0 0 10 12" className={className} role={title ? "img" : undefined} aria-hidden={title ? undefined : true}>
      {title && <title>{title}</title>}
      <path d="M1 6h8v6H1z" fill="currentColor" />
      <path d={open ? "M3 6V3.5a2 2 0 0 1 4 0" : "M3 6V4a2 2 0 0 1 4 0v2"} fill="none" stroke="currentColor" strokeWidth={1.5} />
    </svg>
  );
}

/**
 * What travels on a wire: a chip of ciphertext, an ink tag with a shut
 * padlock and the ciphertext's first group (`label`), never plaintext.
 * Without a label, just the padlock (a keystroke).
 */
export const CipherChip = forwardRef<SVGGElement, { label?: string }>(function CipherChip({ label = "" }, ref) {
  const w = label ? 16 + Math.ceil(label.length * 5.5) : 12;
  const x = -Math.round(w / 2);
  return (
    <g ref={ref} opacity={0}>
      <rect x={x} y={-6} width={w} height={12} fill="var(--color-fg)" />
      <Lock x={x + 2.5} y={-4.2} size={7} className="text-bg" />
      {label && (
        <text x={x + 12} y={3} className="fill-bg font-mono text-[9px]">
          {label}
        </text>
      )}
    </g>
  );
});

/**
 * A tag on a wire (the direct path's name), drawn in the stage's SVG so the
 * chips ride over it: 20px high, a 1px green frame, 11px mono. Its frame is
 * at 45% at rest and full while `data-on` is true.
 */
export const WireTag = forwardRef<SVGGElement, { at: Pt; text: string }>(function WireTag({ at, text }, ref) {
  const w = Math.round([...text].reduce((n, ch) => n + (wide(ch) ? 11 : 6.6), 0)) + 12;
  const cx = Math.round(at.x);
  const cy = Math.round(at.y);
  return (
    <g ref={ref} data-on="false" data-signal="live" className="group">
      <rect
        x={Math.round(cx - w / 2) + 0.5}
        y={cy - 9.5}
        width={w - 1}
        height={19}
        fill="var(--color-bg)"
        stroke="var(--color-ok)"
        className="[stroke-opacity:0.45] group-data-[on=true]:[stroke-opacity:1]"
      />
      <text x={cx} y={cy} textAnchor="middle" dominantBaseline="central" className="fill-ok font-mono text-[11px]">
        {text}
      </text>
    </g>
  );
});

/** A padlock on a wire just outside the sender: whatever leaves it is sealed already. */
export function WireLock({ at, title }: { at: Pt; title?: string }) {
  return (
    <g data-wire-lock="" transform={`translate(${Math.round(at.x)} ${Math.round(at.y)})`}>
      <rect x={-6.5} y={-7.5} width={13} height={15} fill="var(--color-bg)" stroke="var(--color-fg)" strokeWidth={1} />
      <Lock x={-4} y={-4.8} size={8} title={title} className="text-fg" />
    </g>
  );
}

/** The refs of one message row, drawn by `drawRow` (story.ts). */
export interface RowRefs {
  row: RefObject<HTMLSpanElement | null>;
  text: RefObject<HTMLSpanElement | null>;
  lock?: RefObject<HTMLSpanElement | null>;
}

export function useRowRefs(): Required<RowRefs> {
  return { row: useRef<HTMLSpanElement>(null), text: useRef<HTMLSpanElement>(null), lock: useRef<HTMLSpanElement>(null) };
}

/** The padlock slot at the start of a message row: always there (so nothing moves), shut or open or empty by `data-lock`. */
const RowLock = forwardRef<HTMLSpanElement, { titles?: { shut: string; open: string } }>(function RowLock({ titles }, ref) {
  return (
    <span ref={ref} data-lock="none" className="grid h-3 w-2.5 shrink-0">
      <Lock title={titles?.shut} className="col-start-1 row-start-1 opacity-0 [[data-lock=shut]>&]:opacity-100" />
      <Lock open title={titles?.open} className="col-start-1 row-start-1 opacity-0 [[data-lock=open]>&]:opacity-100" />
    </span>
  );
});

/**
 * A row holding a message: the padlock slot (unless `lock` is false), then
 * the text, with a typing caret painted at its end (it grows with the line;
 * nothing moves). `children` is the first frame's text.
 */
export function MessageRow({
  refs,
  children,
  lock = true,
  titles,
  className,
}: {
  refs: RowRefs;
  children: ReactNode;
  lock?: boolean;
  titles?: { shut: string; open: string };
  className?: string;
}) {
  return (
    <span ref={refs.row} data-dim="false" className="flex min-w-0 items-center gap-1.5 data-[dim=true]:opacity-40">
      {lock && <RowLock ref={refs.lock} titles={titles} />}
      <span
        ref={refs.text}
        data-caret="false"
        className={cn("whitespace-pre data-[caret=true]:bg-[linear-gradient(to_left,currentColor_6px,transparent_6px)] data-[caret=true]:pr-[8px]", className ?? "text-xs text-fg")}
      >
        {children}
      </span>
    </span>
  );
}

/**
 * The stage a diagram is drawn on: a hairline grid on the paper, the wires
 * and packets in an SVG the size of the stage, nodes over them. Hidden from
 * assistive tech: every diagram's content is also told in the text beside it.
 */
export const Stage = forwardRef<
  HTMLDivElement,
  {
    /** The stage's measured size (`useSize` on this element). */
    width: number;
    height: number;
    /** The drawing's own height: the stage is at least this tall, and centres it when its cell makes it taller. */
    drawn: number;
    svg: ReactNode;
    /** Drawn over the cards, in the same pixels as `svg`: the chips on the wires, which leave and reach the cards. */
    over?: ReactNode;
    children?: ReactNode;
    /** Pinned to the stage itself rather than the drawing (a legend in a corner). */
    overlay?: ReactNode;
    className?: string;
    grid?: number;
    still?: boolean;
    name: string;
  }
>(function Stage({ width, height, drawn, svg, over, children, overlay, className, grid = 22, still, name }, ref) {
  // The grid fills the whole cell (a taller column beside it leaves no blank
  // band); the drawing sits in its middle, on a whole pixel.
  const dy = Math.max(0, Math.round((height - drawn) / 2));
  return (
    <div
      ref={ref}
      aria-hidden="true"
      data-diagram={name}
      data-still={still ? "" : undefined}
      className={cn("relative w-full overflow-hidden select-none", className)}
      style={{
        minHeight: drawn,
        backgroundImage: "linear-gradient(var(--color-border) 1px, transparent 1px), linear-gradient(90deg, var(--color-border) 1px, transparent 1px)",
        backgroundSize: `${grid}px ${grid}px`,
        backgroundPosition: "-1px -1px",
      }}
    >
      <svg width={width} height={drawn} viewBox={`0 0 ${width} ${drawn}`} className="absolute left-0 block" style={{ top: dy }}>
        {svg}
      </svg>
      <div className="absolute left-0 w-full" style={{ top: dy, height: drawn }}>
        {children}
      </div>
      {over && (
        <svg width={width} height={drawn} viewBox={`0 0 ${width} ${drawn}`} className="pointer-events-none absolute left-0 block" style={{ top: dy }}>
          {over}
        </svg>
      )}
      {overlay}
    </div>
  );
});

/** A step number on the still frame: where on the diagram each step happens. */
export function StepBadge({ n, at }: { n: number; at: Pt }) {
  return (
    <span
      className="absolute flex size-[18px] items-center justify-center bg-fg font-mono text-[10px] font-medium text-bg tabular-nums"
      style={{ left: Math.round(at.x - 9), top: Math.round(at.y - 9) }}
    >
      {n}
    </span>
  );
}

/** The Droppen mark on an ink card: the solid drop-D in paper. */
export function InkMark({ size = 16 }: { size?: number }) {
  return <BrandMark size={size} className="text-bg" />;
}

/**
 * A card as the app draws a ledger: a 26px head (icon, name, and a status
 * on the right), then rows of 11px mono metadata split by hairlines. Three
 * or four rows at most. `ink`: the solid card (Droppen), paper on ink.
 */
export function Ledger({
  box,
  head,
  status,
  rows,
  ink,
  dashed,
  className,
  rowRefs,
  tall,
  children,
}: {
  box: { x: number; y: number; w: number; h?: number };
  head: ReactNode;
  status?: ReactNode;
  rows: ReactNode[];
  ink?: boolean;
  dashed?: boolean;
  className?: string;
  rowRefs?: ((el: HTMLDivElement | null) => void)[];
  /** Rows that may wrap onto a second line (the selling point gets room, not an ellipsis). */
  tall?: number[];
  /** Free content under the rows (a terminal's lines), after a hairline. */
  children?: ReactNode;
}) {
  return (
    <div
      data-card=""
      className={cn(
        "pointer-events-none absolute flex flex-col border",
        ink ? "border-fg bg-fg text-bg" : "border-border-strong bg-bg text-fg",
        dashed && "border-dashed",
        className,
      )}
      style={{ left: Math.round(box.x), top: Math.round(box.y), width: Math.round(box.w), height: box.h === undefined ? undefined : Math.round(box.h) }}
    >
      <div className="flex h-[26px] shrink-0 items-center gap-1.5 px-2.5 text-[13px] leading-4 font-semibold whitespace-nowrap">
        <span className="flex min-w-0 items-center gap-1.5 truncate">{head}</span>
        {status && <span className="ml-auto flex shrink-0 items-center gap-1 font-mono text-[11px] font-normal">{status}</span>}
      </div>
      {rows.map((row, i) => (
        <div
          key={i}
          ref={rowRefs?.[i]}
          className={cn(
            "flex shrink-0 items-center gap-1.5 overflow-hidden border-t px-2.5 font-mono text-[11px] leading-[14px] data-[dim=true]:opacity-40",
            tall?.includes(i) ? "h-9 py-1" : "h-[22px] whitespace-nowrap",
            ink ? "border-bg/15" : "border-border",
          )}
        >
          {row}
        </div>
      ))}
      {children && <div className={cn("min-h-0 flex-1 overflow-hidden border-t", ink ? "border-bg/15" : "border-border")}>{children}</div>}
    </div>
  );
}

/** Height of a ledger with `rows` rows, `tall` of them two lines high (head 26, rows 22 or 36, the hairlines and the frame). */
export const ledgerH = (rows: number, tall = 0) => 26 + rows * 23 + tall * 14 + 2;

/** The source tag, as on a session row: `CLAUDE` in a hairline box. */
export function Source({ children = "CLAUDE", ink, tone }: { children?: ReactNode; ink?: boolean; tone?: "ok" }) {
  return (
    <span
      data-signal={tone === "ok" ? "working" : undefined}
      className={cn(
        "shrink-0 border px-1 text-[10px] leading-[14px] tracking-[0.08em] whitespace-nowrap",
        tone === "ok" ? "border-ok/60 text-ok" : ink ? "border-bg/30 opacity-80" : "border-border text-muted",
      )}
    >
      {children}
    </span>
  );
}

/** The path a device is on, as the cloud bar shows it: green under 100 ms, amber under 250 ms. */
export function PathTag({ ms, label }: { ms: number; label: string }) {
  const tone = ms < 100 ? "text-ok" : ms < 250 ? "text-warn-fg" : "text-danger";
  return (
    <span data-signal="latency" className={cn("tabular-nums", tone)}>
      {label} {Math.round(ms)} ms
    </span>
  );
}

/** A green dot: online / working, as in the app. */
export function LiveDot() {
  return <span data-signal="live" data-shape="dot" className="size-1.5 shrink-0 rounded-full bg-ok" />;
}

/** A red square: off / offline, as in the app. */
export function OffSquare() {
  return <span data-signal="offline" className="size-1.5 shrink-0 bg-danger" />;
}
