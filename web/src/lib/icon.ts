/**
 * The one place that says how Phosphor icons render, now that the app draws
 * with `@phosphor-icons/react` instead of lucide.
 *
 * Lucide's look was carried by SVG attributes: every icon shipped its own
 * `stroke-width="2"`, and `index.css` narrowed that to the drafting sheet's
 * 1.5 with an attribute selector. Phosphor carries none of it. Its `<svg>` is
 * `fill="currentColor"` with no stroke at all, so `strokeWidth` on a call site
 * is not a thinner line — it is a property nothing reads, left on the DOM to
 * mislead the next person. Weight is a prop, not an attribute: `weight`
 * picks one of the six outlines baked into each icon, and `regular` is the
 * one drawn to sit where lucide's 1.5 stroke sat.
 *
 * So this module holds the convention rather than a wrapper: call sites keep
 * rendering the icon component directly, with the same `className`, size and
 * `aria-*` props they had, and the default weight reaches them through
 * [`ICON_CONTEXT`] on the app root and `IconBase`'s own `regular` default when
 * there is no provider (tests and stories render without one). Wrapping every
 * site in a component instead would add a DOM node under each icon and churn
 * the markup tests assert on, for no gain.
 *
 * What this file deliberately does not do is re-export icons under lucide
 * names: the migration is to Phosphor's names, and aliases would keep every
 * call site reading as lucide long after lucide is gone.
 */
import type { IconProps } from "@phosphor-icons/react";

/**
 * Any Phosphor icon as a value — the replacement for lucide's `LucideIcon`
 * wherever an icon is held in a variable, array or map. A Phosphor icon is an
 * element type rather than a lucide-style function component, so the type has
 * to come from the library's own `Icon` alias.
 */
export type { Icon as IconComponent } from "@phosphor-icons/react";

/**
 * The weight the app's icons are drawn at: thick enough to stay legible at the
 * 12–16px sizes the UI uses, light enough not to shout next to text.
 *
 * Kept as a literal type (not a bare string) so `ICON_CONTEXT`'s `weight`
 * field type-checks as `IconProps["weight"]` without a cast.
 */
export const ICON_WEIGHT = "regular" satisfies NonNullable<IconProps["weight"]>;

/**
 * The provider value for the app root; spread it into `IconContext.Provider`.
 *
 * Only the weight is set. `size` and `color` are left unset on purpose: icons
 * take their size from the `size-*` utility classes already on each call site,
 * and `color` falls back to `currentColor`, which is what lets text colour and
 * `text-faint`/`text-danger`-style utilities tint an icon with no extra prop.
 */
export const ICON_CONTEXT = { weight: ICON_WEIGHT } as const satisfies IconProps;
