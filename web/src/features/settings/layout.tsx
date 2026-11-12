/**
 * The settings page's one skeleton (docs/ui-spec.md §3/§4). Components, not
 * class strings: a section, a row and a field are the same markup with the
 * same measurements wherever they appear, so a section cannot drift from the
 * others by carrying its own copy of the grid.
 *
 * - one content column, 48rem;
 * - from a 36rem container up, a row with a 16rem label, a 14rem control and a
 *   16px gap — every control in a section shares both edges;
 * - 44px rows, 56px when the row carries a hint;
 * - label 13/18 at 500, hint 12/18 muted, value 13/18;
 * - section titles 8px above their content, on the same left edge as the page
 *   title (`settings-view.tsx` puts both at a 12px inset).
 */
import type { ComponentProps, ReactNode } from "react";
import { DefinitionRow, DefinitionTable } from "@/components/ui/definition-table";
import { SectionTitle } from "@/components/ui/section-title";
import { cn } from "@/lib/cn";

/** The column every section's content sits in. */
export function SettingsColumn({ className, children, ...props }: ComponentProps<"div">) {
  return (
    <div className={cn("w-full max-w-3xl", className)} {...props}>
      {children}
    </div>
  );
}

/** A section's title strip; the shared section title, named for this page. */
export { SectionTitle as SettingsHeading };

/** A settings section: the title strip above its own content. */
export function SettingsSection({
  title,
  className,
  children,
  ...props
}: Omit<ComponentProps<"section">, "title"> & { title: ReactNode }) {
  return (
    <section className={cn("border-b border-border p-3", className)} {...props}>
      <SectionTitle>{title}</SectionTitle>
      {children}
    </section>
  );
}

/**
 * One preference row. Below 36rem the label sits above the control; from 36rem
 * the control is a fixed 14rem column, so the page reads as a form. `labelExtra`
 * keeps status lines under the label, where a reader looks after reading it.
 */
export function SettingRow({
  label,
  hint,
  labelExtra,
  tight = false,
  className,
  labelClassName,
  controlClassName,
  children,
  ...props
}: Omit<ComponentProps<"div">, "children"> & {
  label?: ReactNode;
  hint?: ReactNode;
  labelExtra?: ReactNode;
  tight?: boolean;
  labelClassName?: string;
  controlClassName?: string;
  children?: ReactNode;
}) {
  return (
    <div
      className={cn(
        "flex w-full max-w-3xl flex-col items-start gap-2 py-2",
        tight ? "min-h-11" : "min-h-14",
        "@min-[36rem]/settings:grid @min-[36rem]/settings:grid-cols-[16rem_14rem] @min-[36rem]/settings:items-center @min-[36rem]/settings:gap-4",
        className,
      )}
      {...props}
    >
      <div className={cn("min-w-0", labelClassName)}>
        {label && <div className="text-[13px] leading-[18px] font-medium text-fg">{label}</div>}
        {hint && <div className="text-xs leading-[18px] text-muted">{hint}</div>}
        {labelExtra}
      </div>
      {children != null && <div className={cn("w-full min-w-0", controlClassName)}>{children}</div>}
    </div>
  );
}

/**
 * A field in a settings section: the label above the control. `faint` is the
 * wizard's smaller voice for a value the page, not the reader, names.
 */
export function SettingField({
  label,
  hint,
  tone = "muted",
  className,
  labelClassName,
  children,
}: {
  label: ReactNode;
  hint?: ReactNode;
  tone?: "muted" | "faint";
  className?: string;
  labelClassName?: string;
  children: ReactNode;
}) {
  return (
    <label className={cn("flex flex-col gap-1", className)}>
      <span className={cn(tone === "faint" ? "text-2xs text-faint" : "text-xs text-muted", labelClassName)}>
        {label}
        {hint && <> {hint}</>}
      </span>
      {children}
    </label>
  );
}

/** A paragraph of help inside a section, at the hint's 12/18. */
export function SettingsHint({ className, children, ...props }: ComponentProps<"p">) {
  return (
    <p className={cn("text-xs leading-[18px] text-muted", className)} {...props}>
      {children}
    </p>
  );
}

const NOTE_TONE = {
  danger: "text-danger",
  warn: "text-warn-fg",
  ok: "text-ok",
  faint: "text-faint",
  muted: "text-muted",
} as const;

/** A status line: the one way a section says saved, failed, or not yet. */
export function SettingsNote({
  tone = "faint",
  mono = true,
  as: Tag = "span",
  title,
  className,
  children,
}: {
  tone?: keyof typeof NOTE_TONE;
  mono?: boolean;
  as?: "span" | "p" | "div";
  title?: string;
  className?: string;
  children: ReactNode;
}) {
  return (
    <Tag
      data-signal={tone === "danger" ? "failed" : tone === "warn" ? "warn" : tone === "ok" ? "ok" : undefined}
      title={title}
      className={cn(mono && "font-mono", "text-2xs", NOTE_TONE[tone], className)}
    >
      {children}
    </Tag>
  );
}

/** A label/value table, as the device facts and the server's own info draw it. */
export { DefinitionRow as SettingsTableRow, DefinitionTable as SettingsTable };
