import type { components } from "@/api/schema";

type Provider = components["schemas"]["AccountProvider"];

/** The CLIs' product names, untranslated. */
export const PROVIDER_NAME: Record<Provider, string> = { claude: "Claude Code", codex: "Codex" };
