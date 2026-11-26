/**
 * The settings tabs, in the order the panel lists them. "access" is this
 * device's identity — its local login and, on the computer's own page, its
 * Droppen link (`access-section.tsx`). "desktop app" is the app's own window
 * only (`desktop-app-section.tsx`).
 */
export const ALL_SECTIONS = ["general", "accounts", "access", "plugins", "scheduled tasks", "desktop app"] as const;
export type SectionId = (typeof ALL_SECTIONS)[number];

/**
 * What each tab is called (the translation key). `accounts` is
 * 「智能体与账号」 — agents & accounts — since ACP agents joined the CLIs'
 * accounts there; its id stays, so links to it keep working.
 */
export const SECTION_LABEL: Record<SectionId, string> = {
  general: "general",
  accounts: "agents & accounts",
  access: "access",
  plugins: "plugins",
  "scheduled tasks": "scheduled tasks",
  "desktop app": "Desktop app",
};

/**
 * The tabs a page offers. Plugins belong to the instance the page is served
 * by, and a page a hub serves (cloud mode) has none — no list, no settings
 * block and no such tab (`docs/plugins/runtime.md` §5). Desktop app is the
 * app's own window's, where its bridge is; a browser on the device has it
 * neither.
 */
export function sectionsFor(cloud: boolean, desktopWindow = false): readonly SectionId[] {
  return ALL_SECTIONS.filter((id) => (id !== "plugins" || !cloud) && (id !== "desktop app" || desktopWindow));
}

/**
 * The two old ids, both of which now open `access`: `?section=security` from a
 * page saved before the merge, and `?section=Droppen` from a link made while
 * the two tabs were apart.
 */
const LEGACY_SECTIONS: Record<string, SectionId> = { security: "access", Droppen: "access" };

/** The section a `?section=` asks for — old ids included; `plugins` on a hub's page and `desktop app` outside the app's window are no answer. */
export function askedSection(section: string | undefined, cloud = false, desktopWindow = false): SectionId | undefined {
  if (!section) return undefined;
  const id = ALL_SECTIONS.find((id) => id === section) ?? LEGACY_SECTIONS[section];
  if (!id) return undefined;
  if (id === "plugins" && cloud) return undefined;
  if (id === "desktop app" && !desktopWindow) return undefined;
  return id;
}
