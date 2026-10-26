import { Select as BaseSelect } from "@base-ui-components/react/select";
import { Check, ChevronDown, ChevronUp } from "lucide-react";
import { Children, memo, useCallback, useLayoutEffect, useMemo, useRef } from "react";
import { useTranslation } from "react-i18next";
import { cn } from "@/lib/cn";

/**
 * shadcn's Base UI Select, drawn on the drafting sheet:
 * the sheet the app's menus use — `bg`, a 1px `border-strong` frame, square
 * corners, no shadow, 32px rows (44 under a finger) — one `hover` fill for
 * pointer and keyboard, the chosen row a `raised` well with an ink check and
 * 500 weight; never the system popup, whose blue highlight and rounded menu
 * belong to another app. The list opens 4px against its trigger
 * (`SelectContent`) and is set in the trigger's font.
 *
 * Root, value and groups are Base UI's own; the parts below only add the look.
 * Keyboard (arrows, Home/End, typeahead, Enter/Space, Escape) and the listbox
 * semantics come from Base UI. The trigger is a `<button role="combobox">`, so
 * a wrapping `<label>` still names it; a trigger with no label of its own needs
 * an `aria-label`.
 */
export const Select = BaseSelect.Root;

/** The chosen item's label. `Select` needs `items` for it to say the label rather than the value. */
export function SelectValue({ className, ...props }: React.ComponentProps<typeof BaseSelect.Value>) {
  return <BaseSelect.Value className={cn("min-w-0 truncate", className)} {...props} />;
}

const TRIGGER = {
  // A form field: 32px with 12px sides and a 13px sans value (model ids and
  // paths may set `font-mono` on the trigger), the same box as `Input`. Phones
  // draw it 40px high and 16px, like the inputs beside it (docs/ui-spec.md
  // §1/§2), so a row of fields keeps one size even though a button never makes
  // iOS zoom. Border states: `border` → hover `border-strong` → open `fg`;
  // keyboard focus is the base layer's 2px ink ring. A disabled field is a
  // fixed grey box, not the whole control faded.
  field:
    "group flex h-8 w-full min-w-0 cursor-pointer items-center justify-between gap-2 border border-border bg-bg px-3 text-left text-[13px] text-fg transition-colors hover:border-border-strong focus:border-fg data-[popup-open]:border-fg disabled:cursor-default disabled:border-border disabled:bg-raised disabled:text-faint max-md:h-10 max-md:text-base",
  // The composer's ghost chip (docs/ui-spec.md §2): 24px, borderless, muted
  // until hovered or open.
  chip: "group relative inline-flex h-6 shrink-0 cursor-pointer items-center gap-0.5 pr-1 pl-1.5 text-left font-mono text-2xs text-muted hover:bg-hover hover:text-fg data-[popup-open]:bg-hover data-[popup-open]:text-fg disabled:cursor-default disabled:opacity-40",
} as const;

export type SelectTriggerVariant = keyof typeof TRIGGER;

export function SelectTrigger({
  variant = "field",
  className,
  iconClassName,
  children,
  ...props
}: React.ComponentProps<typeof BaseSelect.Trigger> & {
  variant?: SelectTriggerVariant;
  /** The chevron's colour and size, for a trigger drawn in a signal colour. */
  iconClassName?: string;
}) {
  return (
    <BaseSelect.Trigger className={cn(TRIGGER[variant], className)} {...props}>
      {children}
      <BaseSelect.Icon
        className={cn(
          // 14px and a 150ms half-turn, tied to the trigger's open state.
          "flex shrink-0 text-faint transition-transform duration-150 ease-out group-data-[popup-open]:rotate-180",
          variant === "chip" ? "[&>svg]:size-2.5" : "[&>svg]:size-3.5",
          iconClassName,
        )}
      >
        <ChevronDown />
      </BaseSelect.Icon>
    </BaseSelect.Trigger>
  );
}

// The scroll arrows sit in the list's flow, not over it: a sticky or absolute
// arrow covers the first/last row it exists to reveal. They scroll with the
// list, 24px tall, and only appear while there is more in that direction.
const ARROW =
  "flex h-6 w-full cursor-default items-center justify-center border-border bg-bg text-faint [&>svg]:size-3.5";

export function SelectContent({
  className,
  children,
  variant = "field",
  emptyLabel,
  side = "bottom",
  align = "start",
  sideOffset = 4,
  alignItemWithTrigger = false,
  collisionPadding = 8,
  ...props
}: Omit<React.ComponentProps<typeof BaseSelect.Positioner>, "className"> & {
  /** Classes for the popup sheet (fonts, widths), not the positioner. */
  className?: string;
  /** The trigger it hangs from: a field's 13px sans or a chip's 11px mono. */
  variant?: SelectTriggerVariant;
  /** What an empty list says; a sentence in muted 13px. */
  emptyLabel?: string;
}) {
  const { t } = useTranslation();
  const empty = Children.count(children) === 0;
  return (
    <BaseSelect.Portal>
      {/* Against the trigger, 4px off it with the left edges together: below
          it, or above it when there is less room below (a composer chip at the
          bottom of a pane opens upward). Pushed along the edge to stay on
          screen, never onto the trigger: the macOS popup's placement, the
          chosen row laid over the trigger (`alignItemWithTrigger`), is off. */}
      <BaseSelect.Positioner
        side={side}
        align={align}
        sideOffset={sideOffset}
        alignItemWithTrigger={alignItemWithTrigger}
        collisionPadding={collisionPadding}
        className="z-50 outline-none"
        {...props}
      >
        <BaseSelect.Popup
          className={cn(
            // Never taller than the room on its side (20rem at most), so a long
            // list scrolls inside the sheet instead of leaving the screen; at
            // least as wide as the trigger or 10rem, never past 20rem or the
            // viewport.
            "max-h-[min(var(--available-height),20rem)] min-w-[max(var(--anchor-width),10rem)] max-w-[min(var(--available-width),20rem)]",
            // The menus' sheet: 1px `border-strong`, square, no shadow, 6px
            // above the first row and below the last, and the fade/rise in
            // `web/src/index.css`.
            "popup-motion overflow-y-auto overscroll-contain border border-border-strong bg-bg py-1.5 text-fg outline-none",
            variant === "chip" ? "font-mono text-2xs" : "text-[13px]",
            className,
          )}
        >
          <BaseSelect.ScrollUpArrow className={cn(ARROW, "border-b")}>
            <ChevronUp />
          </BaseSelect.ScrollUpArrow>
          {empty ? (
            <div className="px-3 py-2 text-[13px] text-muted">{emptyLabel ?? t("no options")}</div>
          ) : (
            children
          )}
          <BaseSelect.ScrollDownArrow className={cn(ARROW, "border-t")}>
            <ChevronDown />
          </BaseSelect.ScrollDownArrow>
        </BaseSelect.Popup>
      </BaseSelect.Positioner>
    </BaseSelect.Portal>
  );
}

export function SelectItem({ className, children, ...props }: React.ComponentProps<typeof BaseSelect.Item>) {
  return (
    <BaseSelect.Item
      className={cn(
        // 32px rows like the menus that drop from a button; 44px under a
        // finger. The text starts 12px in, where a form field's own text does,
        // so the list lines up under the field. One `hover` fill serves the
        // pointer and the keyboard; the chosen row is a `raised` well with the
        // 14px ink check and 500 weight, not just a tint.
        "flex min-h-8 cursor-pointer items-center gap-3 px-3 py-1 outline-none select-none",
        "pointer-coarse:min-h-11",
        "data-[highlighted]:bg-hover data-[selected]:bg-raised data-[selected]:font-medium data-[selected]:text-fg",
        "data-[disabled]:cursor-default data-[disabled]:opacity-40",
        className,
      )}
      {...props}
    >
      <BaseSelect.ItemText className="min-w-0 flex-1 truncate">{children}</BaseSelect.ItemText>
      <BaseSelect.ItemIndicator keepMounted className="flex shrink-0 text-fg not-data-[selected]:invisible">
        <Check className="size-3.5" />
      </BaseSelect.ItemIndicator>
    </BaseSelect.Item>
  );
}

/** One entry of a flat select: the stored value and what the list says for it. */
export type SelectOption<V extends string = string> = { value: V; label: string; disabled?: boolean };

/**
 * The same array for as long as the options say the same thing. Callers build
 * their option lists inline; handed to `Select` as `items`, a fresh array each
 * render makes Base UI rewrite its store (and redraw what reads it) every time.
 */
export function useStableOptions<T extends { value: string; label: string; disabled?: boolean }>(options: readonly T[]): readonly T[] {
  const key = options.map((o) => `${o.value}\u0000${o.label}\u0000${o.disabled ? 1 : 0}`).join("\u0001");
  // Keyed on what the options say, not on the array's identity.
  return useMemo(() => options, [key]);
}

/**
 * A callback whose identity never changes but which always calls the latest
 * `fn` — so a memoized select is not redrawn because its caller wrote its
 * handler inline.
 */
export function useLatest<A extends unknown[]>(fn: (...args: A) => void): (...args: A) => void {
  const ref = useRef(fn);
  useLayoutEffect(() => {
    ref.current = fn;
  });
  return useCallback((...args: A) => ref.current(...args), []);
}

type SelectFieldProps<V extends string> = {
  value: V;
  options: readonly SelectOption<V>[];
  onValueChange: (value: V) => void;
  variant?: SelectTriggerVariant;
  disabled?: boolean;
  /** The trigger's classes. */
  className?: string;
  popupClassName?: string;
  id?: string;
  title?: string;
  "aria-label"?: string;
  "aria-labelledby"?: string;
};

/**
 * A whole select for a flat list of options, the shape every form here has:
 * trigger showing the chosen label, one row per option. A Base UI select is
 * far heavier than the native one it replaced, so the select itself is only
 * redrawn when what it shows changes, not on every keystroke of its form.
 */
export function SelectField<V extends string>({ options, onValueChange, ...rest }: SelectFieldProps<V>) {
  const items = useStableOptions(options);
  const pick = useLatest(onValueChange);
  return <SelectFieldView {...rest} options={items} onValueChange={pick as (value: string) => void} />;
}

const SelectFieldView = memo(function SelectFieldView({
  value,
  options,
  onValueChange,
  variant = "field",
  disabled,
  className,
  popupClassName,
  ...trigger
}: SelectFieldProps<string>) {
  return (
    <Select
      value={value}
      items={options}
      disabled={disabled}
      onValueChange={(next) => {
        if (next !== null && next !== value) onValueChange(next);
      }}
    >
      <SelectTrigger variant={variant} className={className} {...trigger}>
        <SelectValue />
      </SelectTrigger>
      <SelectContent className={popupClassName} variant={variant}>
        {options.map((o) => (
          <SelectItem key={o.value} value={o.value} disabled={o.disabled}>
            {o.label}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
});
