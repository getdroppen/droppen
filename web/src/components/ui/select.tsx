import { Select as BaseSelect } from "@base-ui/react/select";
import { CheckIcon, CaretDownIcon, CaretUpIcon } from "@phosphor-icons/react";
import { Children, memo, useCallback, useLayoutEffect, useMemo, useRef } from "react";
import { useTranslation } from "react-i18next";
import { GlyphIcon, type Glyph } from "@/components/glyph";
import { cn } from "@/lib/cn";

export const Select = BaseSelect.Root;

export function SelectValue({ className, ...props }: React.ComponentProps<typeof BaseSelect.Value>) {
  return <BaseSelect.Value className={cn("min-w-0 truncate", className)} {...props} />;
}

const TRIGGER = {
  field:
    "group flex h-8 w-full min-w-0 cursor-pointer items-center justify-between gap-2 border border-border bg-bg px-3 text-left text-[13px] text-fg transition-colors hover:border-border-strong focus:border-fg data-[popup-open]:border-fg disabled:cursor-default disabled:border-border disabled:bg-raised disabled:text-faint max-md:h-10 max-md:text-base",
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
  iconClassName?: string;
}) {
  return (
    <BaseSelect.Trigger className={cn(TRIGGER[variant], className)} {...props}>
      {children}
      <BaseSelect.Icon
        className={cn(
          "flex shrink-0 text-faint transition-transform duration-150 ease-out group-data-[popup-open]:rotate-180",
          variant === "chip" ? "[&>svg]:size-2.5" : "[&>svg]:size-3.5",
          iconClassName,
        )}
      >
        <CaretDownIcon />
      </BaseSelect.Icon>
    </BaseSelect.Trigger>
  );
}

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
  className?: string;
  variant?: SelectTriggerVariant;
  emptyLabel?: string;
}) {
  const { t } = useTranslation();
  const empty = Children.count(children) === 0;
  return (
    <BaseSelect.Portal>
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
            "max-h-[min(var(--available-height),20rem)] min-w-[max(var(--anchor-width),10rem)] max-w-[min(var(--available-width),20rem)]",
            "popup-motion overflow-y-auto overscroll-contain border border-border-strong bg-bg py-1.5 text-fg outline-none",
            variant === "chip" ? "font-mono text-2xs" : "text-[13px]",
            className,
          )}
        >
          <BaseSelect.ScrollUpArrow className={cn(ARROW, "border-b")}>
            <CaretUpIcon />
          </BaseSelect.ScrollUpArrow>
          {empty ? (
            <div className="px-3 py-2 text-[13px] text-muted">{emptyLabel ?? t("no options")}</div>
          ) : (
            children
          )}
          <BaseSelect.ScrollDownArrow className={cn(ARROW, "border-t")}>
            <CaretDownIcon />
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
        <CheckIcon className="size-3.5" />
      </BaseSelect.ItemIndicator>
    </BaseSelect.Item>
  );
}

export type SelectOption<V extends string = string> = { value: V; label: string; disabled?: boolean; glyph?: Glyph };

export function OptionLabel({ label, glyph }: { label: string; glyph?: Glyph }) {
  if (!glyph) return label;
  return (
    <span className="flex min-w-0 items-center gap-2">
      <GlyphIcon glyph={glyph} />
      <span className="min-w-0 truncate">{label}</span>
    </span>
  );
}

export function glyphValue(options: readonly { value: string; label: string; glyph?: Glyph }[]) {
  if (!options.some((o) => o.glyph)) return undefined;
  return (value: string) => {
    const chosen = options.find((o) => o.value === value);
    return chosen ? <OptionLabel label={chosen.label} glyph={chosen.glyph} /> : value;
  };
}

export function useStableOptions<T extends { value: string; label: string; disabled?: boolean; glyph?: Glyph }>(options: readonly T[]): readonly T[] {
  const key = options.map((o) => `${o.value}\u0000${o.label}\u0000${o.disabled ? 1 : 0}\u0000${o.glyph?.id ?? ""}`).join("\u0001");
  return useMemo(() => options, [key]);
}

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
  className?: string;
  popupClassName?: string;
  id?: string;
  title?: string;
  "aria-label"?: string;
  "aria-labelledby"?: string;
};

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
        <SelectValue>{glyphValue(options)}</SelectValue>
      </SelectTrigger>
      <SelectContent className={popupClassName} variant={variant}>
        {options.map((o) => (
          <SelectItem key={o.value} value={o.value} disabled={o.disabled}>
            <OptionLabel label={o.label} glyph={o.glyph} />
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
});
