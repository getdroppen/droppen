/**
 * The settings tabs, in the order the panel lists them. "access" is this
 * device's identity — its local login and, on the computer's own page, its
 * Droppen link (`access-section.tsx`).
 */
export const ALL_SECTIONS = ["general", "accounts", "access", "plugins", "scheduled tasks"] as const;
export type SectionId = (typeof ALL_SECTIONS)[number];

/**
 * The two old ids, both of which now open `access`: `?section=security` from a
 * page saved before the merge, and `?section=Droppen` from a link made while
 * the two tabs were apart.
 */
const LEGACY_SECTIONS: Record<string, SectionId> = { security: "access", Droppen: "access" };

/** The section a `?section=` asks for — old ids included; anything else is no answer. */
export function askedSection(section: string | undefined): SectionId | undefined {
  if (!section) return undefined;
  return ALL_SECTIONS.find((id) => id === section) ?? LEGACY_SECTIONS[section];
}
