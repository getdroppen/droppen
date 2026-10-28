import { parseSvgGlyph, type Glyph } from "@/components/glyph";
import { useMachine } from "@/lib/machines/context";
import type { MachineKey } from "@/lib/machines/key";
import { useAcpAgents, type AcpAgentList } from "./acp-agent";

/**
 * Every agent mark, by file name, as the build bundles them. Vite replaces
 * the call at build time; outside Vite (the unit tests run the sources
 * directly) there is no such function, and no marks.
 */
function bundledIcons(): Record<string, string> {
  try {
    return import.meta.glob<string>("./presets/icons/*.svg", { query: "?raw", import: "default", eager: true });
  } catch {
    return {};
  }
}

/** The marks in `files` (path → SVG text), by the agent id each file is named for. */
export function glyphsById(files: Record<string, string>): Map<string, Glyph> {
  const glyphs = new Map<string, Glyph>();
  for (const [path, svg] of Object.entries(files)) {
    const id = /^([a-z0-9][a-z0-9-]*)\.svg$/.exec(path.slice(path.lastIndexOf("/") + 1))?.[1];
    const glyph = id ? parseSvgGlyph(`acp-agent:${id}`, svg) : null;
    if (id && glyph) glyphs.set(id, glyph);
  }
  return glyphs;
}

const GLYPHS = glyphsById(bundledIcons());

/** The generic mark of an ACP agent: a terminal prompt, for any agent without one of its own. */
export const ACP_GLYPH: Glyph = {
  id: "acp",
  viewBox: "0 0 24 24",
  paths: ["M4 4h16a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2Zm0 2v12h16V6ZM6.3 9.4 7.7 8l4 4-4 4-1.4-1.4 2.6-2.6ZM12 14.5h5.5v1.8H12Z"],
};

/** The mark of the agent `id`: `presets/icons/<id>.svg`, or the generic one. */
export function acpAgentGlyph(id: string | null | undefined, glyphs: ReadonlyMap<string, Glyph> = GLYPHS): Glyph {
  return (id && glyphs.get(id)) || ACP_GLYPH;
}

/** How one ACP agent is shown wherever a session or a picker names it. */
export interface AcpAgentLook {
  /** Its name: as configured, else its built-in's, else its id; `null` for no agent at all. */
  name: string | null;
  glyph: Glyph;
  /** The agent is not configured (any more): its sessions are shown, not continued. */
  removed: boolean;
}

/**
 * The look of the agent `id` given what the device lists. While the list is
 * still loading nothing is called removed.
 */
export function acpAgentLook(id: string | null | undefined, list: AcpAgentList | undefined, glyphs?: ReadonlyMap<string, Glyph>): AcpAgentLook {
  const glyph = acpAgentGlyph(id, glyphs);
  if (!id) return { name: null, glyph, removed: false };
  const configured = list?.agents.find((agent) => agent.id === id);
  if (configured) return { name: configured.name, glyph, removed: false };
  const preset = list?.presets.find((candidate) => candidate.id === id);
  return { name: preset?.name ?? id, glyph, removed: list !== undefined };
}

/**
 * {@link acpAgentLook} for the agents of the device in view. A session of
 * another device's (`machine`, in a list of several) is not looked up: it is
 * drawn with its agent's mark and named by its agent's id.
 */
export function useAcpAgentLook(id: string | null | undefined, machine?: MachineKey): AcpAgentLook {
  const here = useMachine();
  const ours = machine === undefined || machine === here;
  const { data } = useAcpAgents(ours && !!id);
  return acpAgentLook(id, ours ? data : undefined);
}
