import { Select as BaseSelect } from "@base-ui-components/react/select";
import { Check, ChevronDown, ChevronUp } from "lucide-react";
import { memo, useCallback, useLayoutEffect, useMemo, useRef } from "react";
import { cn } from "@/lib/cn";

/**
 * shadcn's Base UI Select, drawn on the drafting sheet:
 * the sheet the app's menus use — `bg`, a 1px `border-strong` frame, square
 * corners, no shadow, 24px rows — the highlighted row tinted with `hover`, the
 * chosen one marked with an ink check; never the system popup, whose blue
 * highlight and rounded menu belong to another app. The list opens against its
 * trigger (`SelectContent`) and is set in the trigger's font.
 *
 * Root, value and groups are Base UI's own; the parts below only add the look.
 * Keyboard (arrows, Home/End, typeahead, Enter/Space, Escape) and the listbox
 * semantics come from Base UI. The trigger is a `<button role="combobox">`, so
 * a wrapping `<label>` still names it; a trigger with no label of its own needs
 * an `aria-label`.
 */
export const Select = BaseSelect.Root;
export const SelectGroup = BaseSelect.Group;

/** The chosen item's label. `Select` needs `items` for it to say the label rather than the value. */
export function SelectValue({ className, ...props }: React.ComponentProps<typeof BaseSelect.Value>) {
  return <BaseSelect.Value className={cn("min-w-0 truncate", className)} {...props} />;
}

const TRIGGER = {
  // A form field: the same 26px box, border and ink focus as `Input`. Phones
  // draw it at 16px like the inputs beside it (docs/ui-spec.md §1), so a row
  // of fields keeps one size even though a button never makes iOS zoom.
  field:
    "flex h-6.5 w-full min-w-0 cursor-pointer items-center justify-between gap-1.5 border border-border bg-bg px-2 text-left font-mono text-2xs text-fg focus:border-fg data-[popup-open]:border-fg disabled:cursor-default disabled:opacity-40 max-md:text-base",
  // The composer's ghost chip (docs/ui-spec.md §2): 24px, borderless, muted
  // until hovered or open.
  chip: "relative inline-flex h-6 shrink-0 cursor-pointer items-center gap-0.5 pr-1 pl-1.5 text-left font-mono text-2xs text-muted hover:bg-hover hover:text-fg data-[popup-open]:bg-hover data-[popup-open]:text-fg disabled:cursor-default disabled:opacity-40",
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
          "flex shrink-0 text-faint",
          variant === "chip" ? "[&>svg]:size-2.5" : "[&>svg]:size-3",
          iconClassName,
        )}
      >
        <ChevronDown />
      </BaseSelect.Icon>
    </BaseSelect.Trigger>
  );
}

const ARROW =
  "inset-x-px z-10 flex h-4 cursor-default items-center justify-center border-border bg-bg text-faint [&>svg]:size-3";

export function SelectContent({
  className,
  children,
  side = "bottom",
  align = "start",
  sideOffset = 4,
  alignItemWithTrigger = false,
  collisionPadding = 8,
  ...props
}: Omit<React.ComponentProps<typeof BaseSelect.Positioner>, "className"> & {
  /** Classes for the popup sheet (fonts, widths), not the positioner. */
  className?: string;
}) {
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
        <BaseSelect.ScrollUpArrow className={cn(ARROW, "top-px border-b")}>
          <ChevronUp />
        </BaseSelect.ScrollUpArrow>
        <BaseSelect.Popup
          className={cn(
            // Never taller than the room on its side, so a long list scrolls
            // inside the sheet instead of leaving the screen; at least as wide
            // as the trigger and never a sliver under a short chip; never
            // wider than the viewport.
            "max-h-[min(var(--available-height),20rem)] min-w-[max(var(--anchor-width),8rem)] max-w-[min(var(--available-width),24rem)]",
            // The menus' sheet (session actions, language, new…), 4px above
            // the first row and below the last.
            "overflow-y-auto overscroll-contain border border-border-strong bg-bg py-1 font-mono text-2xs text-fg outline-none",
            className,
          )}
        >
          {children}
        </BaseSelect.Popup>
        <BaseSelect.ScrollDownArrow className={cn(ARROW, "bottom-px border-t")}>
          <ChevronDown />
        </BaseSelect.ScrollDownArrow>
      </BaseSelect.Positioner>
    </BaseSelect.Portal>
  );
}

export function SelectItem({ className, children, ...props }: React.ComponentProps<typeof BaseSelect.Item>) {
  return (
    <BaseSelect.Item
      className={cn(
        // 24px rows like the menus that drop from a button; 36px under a
        // finger, the size of the phone nav's buttons, with text that can be
        // read at arm's length. The text starts 8px in, where a form field's
        // own text does, so the list lines up under the field.
        "flex min-h-6 cursor-pointer items-center gap-3 px-2 py-1 outline-none select-none",
        "pointer-coarse:min-h-9 pointer-coarse:text-sm",
        "data-[highlighted]:bg-hover data-[selected]:text-fg",
        "data-[disabled]:cursor-default data-[disabled]:opacity-40",
        className,
      )}
      {...props}
    >
      <BaseSelect.ItemText className="min-w-0 flex-1 truncate">{children}</BaseSelect.ItemText>
      <BaseSelect.ItemIndicator keepMounted className="flex shrink-0 text-fg not-data-[selected]:invisible">
        <Check className="size-3" />
      </BaseSelect.ItemIndicator>
    </BaseSelect.Item>
  );
}

export function SelectGroupLabel({ className, ...props }: React.ComponentProps<typeof BaseSelect.GroupLabel>) {
  return (
    <BaseSelect.GroupLabel
      data-label=""
      className={cn("px-2 pt-1.5 pb-1 font-mono text-[10px] tracking-label text-muted uppercase", className)}
      {...props}
    />
  );
}

export function SelectSeparator({ className, ...props }: React.ComponentProps<typeof BaseSelect.Separator>) {
  return <BaseSelect.Separator className={cn("my-1 h-px bg-border", className)} {...props} />;
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
      <SelectContent className={popupClassName}>
        {options.map((o) => (
          <SelectItem key={o.value} value={o.value} disabled={o.disabled}>
            {o.label}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
});
