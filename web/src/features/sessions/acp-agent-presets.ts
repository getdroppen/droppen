import type { components } from "@/api/schema";

/**
 * A known ACP agent the settings can fill in: how it is started in ACP mode
 * over stdio. Data, not code — the server's built-in list (`acp-agents.json`,
 * served by `GET /api/acp/agents`), and nothing here names an agent. An agent
 * added from one keeps its id, which is what its mark is found by
 * (`presets/icons/<id>.svg`).
 */
export type AcpAgentPreset = components["schemas"]["AcpAgentPreset"];

/** The "add an agent" picker's choice of no preset: whatever is typed. */
export const CUSTOM_PRESET = "custom";

/** The settings form's unsaved edits: `null` is a field left as saved. */
export interface AcpAgentDraft {
  name: string | null;
  command: string | null;
  args: string | null;
}

/**
 * The draft after picking `id`: a preset fills in its name, executable and
 * arguments, which stay editable; the custom choice leaves every field alone.
 * Nothing is saved either way.
 */
export function applyPreset(draft: AcpAgentDraft, id: string, presets: readonly AcpAgentPreset[]): AcpAgentDraft {
  const preset = presets.find((candidate) => candidate.id === id);
  if (!preset) return draft;
  return { name: preset.name, command: preset.command, args: preset.args.join(" ") };
}

/** The longest agent id the server takes. */
const MAX_ID = 64;

/**
 * The id a new agent is saved under: a preset's own, else the name as a slug
 * (lowercase letters, digits and `-`), numbered past the ids already `taken`.
 * The server holds every id to that shape: it names the agent's transcripts.
 */
export function newAgentId(name: string, taken: ReadonlySet<string>, preset?: string): string {
  if (preset && preset !== CUSTOM_PRESET && !taken.has(preset)) return preset;
  const slug = name
    .normalize("NFKD")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, MAX_ID - 4)
    .replace(/-+$/, "");
  const base = slug || "agent";
  if (!taken.has(base)) return base;
  for (let n = 2; ; n++) {
    const candidate = `${base}-${n}`;
    if (!taken.has(candidate)) return candidate;
  }
}
