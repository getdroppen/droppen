import { cn } from "@/lib/cn";

/**
 * The Droppen mark: a solid drop that is also a D. One contour, a square
 * corner at the top left and a 270° arc, on a 24-unit grid, filled with
 * `currentColor`: solid ink on the light theme, solid paper on the dark.
 * Sizes that are multiples of 12 keep the straight edges on whole device
 * pixels on 2x screens; 24 puts them on whole CSS pixels too.
 *
 * `web/scripts/gen-pwa-assets.mjs` draws the same contour into the favicon
 * (black, transparent ground) and the app icons (paper on an ink tile).
 */
export const BRAND_PATH = "M2 2H12A10 10 0 1 1 2 12Z";

/**
 * The same contour drawn on a `size`-pixel grid: the corner a whole number
 * of pixels in (about 1/12 of the box), the glyph an even number of pixels
 * wide, so the straight edges stay on whole CSS pixels at any size (20, 18,
 * 14…), not only at multiples of 12. At 24 it is exactly {@link BRAND_PATH}.
 */
export function brandPath(size: number): string {
  const c = Math.max(1, Math.round(size / 12));
  const r = Math.floor((size - 2 * c) / 2);
  return `M${c} ${c}H${c + r}A${r} ${r} 0 1 1 ${c} ${c + r}Z`;
}

export function BrandMark({ size = 20, className, title }: { size?: number; className?: string; title?: string }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox={`0 0 ${size} ${size}`}
      role={title ? "img" : undefined}
      aria-label={title}
      aria-hidden={title ? undefined : true}
      data-brand-mark=""
      className={cn("shrink-0 text-fg", className)}
    >
      <path d={brandPath(size)} fill="currentColor" />
    </svg>
  );
}
