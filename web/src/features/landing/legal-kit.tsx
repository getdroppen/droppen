import type { ReactNode } from "react";
import { CapsLabel } from "@/components/ui/caps-label";
import { Block, Pair, Section, SectionHead, TextLink } from "./page-kit";
import { publicLink } from "./route";

/** One column of a legal section: a heading and the points under it. */
type LegalPanel = { title: string; items: string[] };

/**
 * A section of the privacy notice or the terms: the same document skeleton on
 * both pages, so their insets and column rules cannot drift apart.
 */
export function LegalSection({
  id,
  label,
  title,
  lead,
  left,
  right,
  children,
}: {
  id: string;
  label: string;
  title: string;
  lead?: ReactNode;
  left: LegalPanel;
  right: LegalPanel;
  children?: ReactNode;
}) {
  return (
    <Section id={id}>
      <SectionHead label={label} title={title} lead={lead} />
      <Pair left={<Block title={left.title} items={left.items} />} right={<Block title={right.title} items={right.items} />} />
      {children}
    </Section>
  );
}

/** A link row under a legal section's two columns. */
export function LegalFootLink({ path, testId, children }: { path: string; testId: string; children: ReactNode }) {
  return (
    <p className="border-t border-border px-6 py-4 text-xs leading-5 max-md:px-4">
      <TextLink {...publicLink(path)} data-testid={testId}>
        {children}
      </TextLink>
    </p>
  );
}

/** The contact block that closes both documents. */
export function LegalContact({ label, children }: { label: string; children: ReactNode }) {
  return (
    <Section id="contact">
      <div className="flex flex-col gap-3 px-6 py-8 max-md:px-4">
        <CapsLabel variant="section">{label}</CapsLabel>
        {children}
      </div>
    </Section>
  );
}
