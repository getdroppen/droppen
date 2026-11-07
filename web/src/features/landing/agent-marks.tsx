/*
 * The agents' marks: the tools whose sessions Droppen follows, drawn from the
 * Lobe Icons set (`@lobehub/icons-static-svg`, MIT, `icons.lobehub.com`) —
 * the library's own files, not paths copied into this module. Each entry
 * names the library's file and the sizes the hero draws it at; the SVG is read
 * raw and parsed into the page's own `<svg>` at build time, so adding an agent
 * is one line here and nothing else.
 *
 * The marks name the tools and belong to their makers; Droppen ships none of
 * them. `tone: "ink"` draws a mark in the page's ink, which turns white in the
 * dark theme — what a mark whose file has no colour version gets. `brand`
 * keeps the colours of the library's `-color.svg` file.
 */

/** A brand's mark in the library: the monochrome file, or the `-color` one. */
export type AgentIcon = {
  /** The library's file name, without `.svg`. */
  slug: string;
  /** `ink` follows the page; `brand` keeps the file's own colours. */
  tone: "ink" | "brand";
};

/** One agent: what it is called, and which of its marks the hero draws. */
export type Agent = {
  /** The tool's name, as the settings and the download page call it. */
  name: string;
  icon: AgentIcon;
};

/**
 * The agents a terminal can run, and that Droppen's ACP settings can be
 * pointed at. Ordered so the two agents the headline names come first.
 */
export const AGENTS: readonly Agent[] = [
  { name: "Claude Code", icon: { slug: "claudecode", tone: "brand" } },
  { name: "Codex", icon: { slug: "codex", tone: "ink" } },
  { name: "Gemini CLI", icon: { slug: "geminicli", tone: "brand" } },
  { name: "Google Antigravity", icon: { slug: "antigravity", tone: "brand" } },
  { name: "Cursor", icon: { slug: "cursor", tone: "ink" } },
  { name: "GitHub Copilot", icon: { slug: "copilot", tone: "brand" } },
  { name: "Windsurf", icon: { slug: "windsurf", tone: "ink" } },
  { name: "Cline", icon: { slug: "cline", tone: "ink" } },
  { name: "Roo Code", icon: { slug: "roocode", tone: "ink" } },
  { name: "Kilo Code", icon: { slug: "kilocode", tone: "brand" } },
  { name: "OpenCode", icon: { slug: "opencode", tone: "ink" } },
  { name: "Goose", icon: { slug: "goose", tone: "ink" } },
  { name: "Junie", icon: { slug: "junie", tone: "brand" } },
  { name: "Kiro", icon: { slug: "kiro", tone: "brand" } },
  { name: "Qwen Code", icon: { slug: "qwen", tone: "brand" } },
  { name: "Kimi CLI", icon: { slug: "kimi", tone: "brand" } },
  { name: "MiniMax Code", icon: { slug: "minimax", tone: "brand" } },
  { name: "Z.ai GLM Agent", icon: { slug: "zhipu", tone: "brand" } },
  { name: "Qoder", icon: { slug: "qoder", tone: "brand" } },
  { name: "Trae", icon: { slug: "trae", tone: "brand" } },
  { name: "OpenHands", icon: { slug: "openhands", tone: "brand" } },
  { name: "CodeBuddy", icon: { slug: "codebuddy", tone: "brand" } },
  { name: "Amp", icon: { slug: "amp", tone: "brand" } },
  { name: "Devin", icon: { slug: "devin", tone: "brand" } },
  { name: "Poolside", icon: { slug: "poolside", tone: "brand" } },
  { name: "Zencoder", icon: { slug: "zencoder", tone: "brand" } },
  { name: "OpenClaw", icon: { slug: "openclaw", tone: "brand" } },
  { name: "Mistral Vibe", icon: { slug: "mistral", tone: "brand" } },
  { name: "Command Code", icon: { slug: "commandcode", tone: "ink" } },
  { name: "CodeFlicker", icon: { slug: "codeflicker", tone: "brand" } },
  { name: "CodeGeeX", icon: { slug: "codegeex", tone: "brand" } },
  { name: "Pi", icon: { slug: "pi", tone: "ink" } },
  { name: "Hermes Agent", icon: { slug: "hermesagent", tone: "ink" } },
  { name: "WorkBuddy", icon: { slug: "workbuddy", tone: "brand" } },
  { name: "Tiangong", icon: { slug: "tiangong", tone: "brand" } },
  { name: "YouMind", icon: { slug: "youmind", tone: "ink" } },
  { name: "Zero One", icon: { slug: "zeroone", tone: "brand" } },
];
