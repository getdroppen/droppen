import { cn } from "@/lib/cn";

/** A label/value table: the facts a device, a link or the server reports. */
export function DefinitionTable({ className, children, ...props }: React.ComponentProps<"table">) {
  return (
    <table className={cn("w-full max-w-3xl", className)} {...props}>
      <tbody>{children}</tbody>
    </table>
  );
}

export function DefinitionRow({ label, children }: { label: React.ReactNode; children: React.ReactNode }) {
  return (
    <tr className="border-b border-border">
      <td className="py-1 pr-4 align-top font-mono text-2xs whitespace-nowrap text-faint">{label}</td>
      <td className="py-1 font-mono text-xs break-all">{children}</td>
    </tr>
  );
}
